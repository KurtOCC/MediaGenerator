//! JSON, HTMX and Server-Sent Events endpoints behind `require_auth`.
//!
//! Generation is asynchronous throughout. `POST /api/generate` writes a row and
//! returns immediately with the card for a queued job; the browser then follows
//! `GET /api/jobs/{id}/events` and re-fetches the card when the job finishes.
//!
//! Because the job lives in the database rather than in the page, a reload, a
//! navigation away or even a server restart does not lose it.

use std::{convert::Infallible, time::Duration};

use askama::Template;
use axum::{
    Extension, Json, Router,
    extract::{Form, Path, State},
    response::{
        Redirect, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::{Stream, StreamExt as _};
use mediagenerator_auth::SessionUser;
use mediagenerator_domain::{
    AuditAction, AuditEntry, Job, JobStatus, MediaType, NewJob, i18n::nb, validate_prompt,
};
use mediagenerator_storage::{StorageError, assets, audit, jobs};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tower_sessions::Session;
use uuid::Uuid;

use crate::{
    client_info::ClientInfo, error::AppError, identity::local_user_id, render::Page,
    state::AppState,
};

/// How often a comment is sent on an idle SSE stream.
///
/// Without traffic, a proxy or a laptop going to sleep can drop the connection
/// silently. The browser would reconnect, but a keep-alive is cheaper.
const SSE_KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Returns the API routes that are subject to the request timeout.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(me))
        .route("/api/generate", post(generate))
        .route("/api/jobs/{id}", get(job_status))
        .route("/api/jobs/{id}/card", get(job_card))
        .route("/api/assets/{id}", get(asset))
}

/// Returns the streaming routes, which must not be subject to the timeout.
pub fn stream_router() -> Router<AppState> {
    Router::new().route("/api/jobs/{id}/events", get(job_events))
}

/// The signed-in user, as returned by `GET /api/me`.
///
/// A projection rather than the session type itself: the session value may grow
/// fields that have no business reaching the browser.
#[derive(Debug, Serialize)]
struct MeBody {
    oid: String,
    display_name: String,
    initials: String,
    email: Option<String>,
    roles: Vec<String>,
}

/// Returns the signed-in user as JSON.
async fn me(Extension(user): Extension<SessionUser>) -> Json<MeBody> {
    Json(MeBody {
        oid: user.oid.clone(),
        display_name: user.display_name.clone(),
        initials: user.initials(),
        email: user.email.clone(),
        roles: user.roles.clone(),
    })
}

/// Form body posted by the generator.
#[derive(Debug, Deserialize)]
struct GenerateForm {
    /// What the user wants generated.
    prompt: String,
    /// Which kind of media to produce.
    media_type: MediaType,
}

/// Accepts a generation request and returns the card for the queued job.
async fn generate(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    client: ClientInfo,
    Form(form): Form<GenerateForm>,
) -> Result<Page<JobCardTemplate>, AppError> {
    let prompt = validate_prompt(&form.prompt, state.config.max_prompt_chars)?;
    let user_id = local_user_id(&state.db, &session, &user).await?;

    let job = jobs::create(
        &state.db,
        &NewJob {
            user_id,
            media_type: form.media_type,
            // Provider parameters (size, quality, voice) are added in phase 5.
            parameters: serde_json::json!({}),
            prompt,
        },
    )
    .await
    .map_err(storage_error)?;

    // Recorded before the work starts, so an attempt is in the trail even if
    // the process dies mid-generation. The prompt itself is deliberately not
    // copied here: it lives on the job row, under the retention policy.
    let entry = AuditEntry {
        user_id: Some(user_id),
        action: AuditAction::GenerationRequested,
        entity: "job",
        entity_id: Some(job.id.to_string()),
        ip: client.ip.clone(),
        user_agent: client.user_agent.clone(),
    };
    if let Err(error) = audit::record(&state.db, &entry).await {
        // A missing audit line must not cost the user their generation.
        tracing::error!(%error, job_id = %job.id, "could not write the audit entry");
    }

    if state.jobs.enqueue(job.id).is_err() {
        // The queue is saturated. Fail the row now rather than leaving it
        // queued with nothing coming to pick it up.
        let status = JobStatus::Failed {
            code: mediagenerator_domain::ErrorCode::RateLimited,
            message: nb::ERR_QUEUE_FULL.to_owned(),
        };
        let failed = jobs::finish(&state.db, job.id, &status)
            .await
            .map_err(storage_error)?;
        return Ok(Page(JobCardTemplate::from_job(&failed, None)));
    }

    tracing::info!(
        %user_id,
        job_id = %job.id,
        media_type = %job.media_type,
        prompt_chars = job.prompt.chars().count(),
        "generation queued"
    );

    Ok(Page(JobCardTemplate::from_job(&job, None)))
}

/// Status of one job, as JSON.
#[derive(Debug, Serialize)]
struct JobBody {
    id: Uuid,
    status: &'static str,
    media_type: &'static str,
    error_code: Option<&'static str>,
    error_message: Option<String>,
    /// Elapsed time in milliseconds, once the job has finished.
    duration_ms: Option<i64>,
}

impl JobBody {
    /// Builds the JSON view of a job.
    fn from_job(job: &Job) -> Self {
        Self {
            id: job.id,
            status: job.status.as_str(),
            media_type: job.media_type.as_str(),
            error_code: job.status.error_code().map(|code| code.as_str()),
            error_message: job.status.error_message().map(ToOwned::to_owned),
            duration_ms: job
                .duration()
                .map(|duration| duration.whole_milliseconds() as i64),
        }
    }
}

/// Returns the status of one of the caller's jobs.
async fn job_status(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    Path(id): Path<Uuid>,
) -> Result<Json<JobBody>, AppError> {
    let job = load_own_job(&state, &session, &user, id).await?;
    Ok(Json(JobBody::from_job(&job)))
}

/// The rendered card for a job, in whatever state it is in.
#[derive(Template)]
#[template(path = "partials/job_card.html")]
pub struct JobCardTemplate {
    /// Job identifier, used by the browser to open the event stream.
    pub id: String,
    /// The prompt, echoed back under the preview.
    pub prompt: String,
    /// `image`, `audio` or `video`.
    pub media_type: &'static str,
    /// Norwegian label for the media type.
    pub media_label: &'static str,
    /// `queued`, `running`, `succeeded`, `failed` or `cancelled`.
    pub status: &'static str,
    /// True while the job may still change, which is when the browser listens.
    pub pending: bool,
    /// True when the job failed.
    pub failed: bool,
    /// Norwegian failure message, when it failed.
    pub error_message: String,
    /// Human-readable time spent, e.g. "2,5 s". Empty while unfinished.
    pub elapsed: String,
    /// Where the finished asset lives. Empty until phase 5 stores one.
    pub asset_url: String,
}

impl JobCardTemplate {
    /// Builds the card for a job, given the asset it produced, if any.
    ///
    /// The card links to `/api/assets/{id}`, not to a SAS URL. That route
    /// mints a fresh link on each request, so a page left open overnight
    /// still works and no expiring URL is ever baked into the HTML.
    pub fn from_job(job: &Job, asset_id: Option<Uuid>) -> Self {
        let failed = matches!(job.status, JobStatus::Failed { .. });

        Self {
            id: job.id.to_string(),
            prompt: job.prompt.clone(),
            media_type: job.media_type.as_str(),
            media_label: job.media_type.label(),
            status: job.status.as_str(),
            pending: !job.status.is_terminal(),
            failed,
            error_message: job
                .status
                .error_message()
                .unwrap_or(nb::ERR_INTERNAL)
                .to_owned(),
            elapsed: job.duration().map(format_elapsed).unwrap_or_default(),
            asset_url: asset_id
                .map(|id| format!("/api/assets/{id}"))
                .unwrap_or_default(),
        }
    }
}

/// Returns the card for one of the caller's jobs.
async fn job_card(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    Path(id): Path<Uuid>,
) -> Result<Page<JobCardTemplate>, AppError> {
    let job = load_own_job(&state, &session, &user, id).await?;
    Ok(Page(card_for(&state, &job).await))
}

/// Streams status changes for one of the caller's jobs.
///
/// The current status is sent first, so a browser that connects after the job
/// already finished is told immediately rather than waiting for an event that
/// will never come. The stream then ends on a terminal status.
async fn job_events(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    Path(id): Path<Uuid>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    // Ownership is checked once, here. The stream that follows is filtered by
    // this id, so nothing else can reach it.
    let job = load_own_job(&state, &session, &user, id).await?;

    let media_type = job.media_type;
    let initial = futures_util::stream::once(async move { JobBody::from_job(&job) });

    let updates = BroadcastStream::new(state.jobs.subscribe())
        .filter_map(move |event| {
            let matched = event
                .ok()
                .filter(|event| event.job_id == id)
                .map(|event| event.status);
            async move { matched }
        })
        .map(move |status| JobBody {
            id,
            status: status.as_str(),
            media_type: media_type.as_str(),
            error_code: status.error_code().map(|code| code.as_str()),
            error_message: status.error_message().map(ToOwned::to_owned),
            duration_ms: None,
        });

    // Emit the terminal status, then stop. Closing the stream is what tells the
    // browser to stop listening; an SSE connection left open would be
    // reconnected forever.
    let mut terminal_seen = false;
    let stream = initial
        .chain(updates)
        .take_while(move |body| {
            let emit = !terminal_seen;
            terminal_seen = terminal_seen || is_terminal(body.status);
            async move { emit }
        })
        .map(|body| to_event(&body));

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(SSE_KEEP_ALIVE)))
}

/// Renders a job status as an SSE `status` event.
fn to_event(body: &JobBody) -> Result<Event, Infallible> {
    // Serialising a plain struct of owned strings cannot fail; if it somehow
    // did, an empty payload is better than dropping the connection.
    let data = serde_json::to_string(body).unwrap_or_else(|error| {
        tracing::error!(%error, "could not serialise a job event");
        String::from("{}")
    });
    Ok(Event::default().event("status").data(data))
}

/// Returns `true` for a status the job cannot move on from.
fn is_terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "cancelled")
}

/// Loads a job, refusing anything the caller does not own.
async fn load_own_job(
    state: &AppState,
    session: &Session,
    user: &SessionUser,
    id: Uuid,
) -> Result<Job, AppError> {
    let user_id = local_user_id(&state.db, session, user).await?;
    jobs::by_id_for_user(&state.db, id, user_id)
        .await
        .map_err(storage_error)
}

/// Maps a storage failure to the user-facing error.
fn storage_error(error: StorageError) -> AppError {
    match error {
        StorageError::NotFound => AppError::NotFound,
        other => AppError::Internal(anyhow::Error::new(other)),
    }
}

/// Formats a duration the way Norwegian writes it: comma as decimal separator.
fn format_elapsed(duration: time::Duration) -> String {
    let seconds = duration.as_seconds_f64().max(0.0);
    let rendered = format!("{seconds:.1}");
    format!("{} {}", rendered.replace('.', ","), nb::SECONDS_SUFFIX)
}

/// Redirects to a time-limited link for one of the caller's assets.
///
/// The ownership check is the point of this route existing at all: without it,
/// knowing an id would be enough to read anyone's media. The SAS is minted per
/// request and expires after `SAS_TTL_MINUTES`, so a copied link stops working
/// rather than becoming a permanent public URL.
async fn asset(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    client: ClientInfo,
    Path(id): Path<Uuid>,
) -> Result<Redirect, AppError> {
    let user_id = local_user_id(&state.db, &session, &user).await?;

    let asset = assets::by_id_for_user(&state.db, id, user_id)
        .await
        .map_err(storage_error)?;

    let url = state
        .blobs
        .read_url(&asset.blob_path, state.config.sas_ttl())
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;

    // Handing out a readable link is worth recording: it is the moment the
    // media actually leaves the private container.
    let entry = AuditEntry {
        user_id: Some(user_id),
        action: AuditAction::AssetAccessed,
        entity: "asset",
        entity_id: Some(asset.id.to_string()),
        ip: client.ip.clone(),
        user_agent: client.user_agent.clone(),
    };
    if let Err(error) = audit::record(&state.db, &entry).await {
        tracing::error!(%error, asset_id = %asset.id, "could not write the audit entry");
    }

    Ok(Redirect::temporary(&url))
}

/// Builds the card for a job, looking up the asset it produced.
///
/// A failure to read the asset degrades to a card without a preview rather
/// than to an error page: the job itself is unaffected.
pub async fn card_for(state: &AppState, job: &Job) -> JobCardTemplate {
    let asset_id = assets::for_job(&state.db, job.id)
        .await
        .inspect_err(|error| tracing::error!(%error, job_id = %job.id, "could not read the asset"))
        .ok()
        .flatten()
        .map(|asset| asset.id);

    JobCardTemplate::from_job(job, asset_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_uses_a_comma_as_the_decimal_separator() {
        assert_eq!(format_elapsed(time::Duration::milliseconds(1200)), "1,2 s");
        assert_eq!(format_elapsed(time::Duration::milliseconds(450)), "0,5 s");
        assert_eq!(format_elapsed(time::Duration::seconds(12)), "12,0 s");
    }

    #[test]
    fn a_negative_duration_does_not_render_as_negative() {
        // Clock adjustments can make completed_at precede started_at.
        assert_eq!(format_elapsed(time::Duration::milliseconds(-10)), "0,0 s");
    }
}
