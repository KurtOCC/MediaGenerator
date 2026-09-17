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
    extract::{Form, Multipart, Path, State},
    http::StatusCode,
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
    client_info::ClientInfo, error::AppError, identity::local_user_id, ratelimit, render::Page,
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
        .route("/api/jobs/{id}/synlighet", post(set_visibility))
        .route("/api/jobs/{id}/slett", post(delete_job))
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

/// Largest reference image accepted, in bytes.
const MAX_REFERENCE_BYTES: usize = 10 * 1024 * 1024;

/// Image types the reference upload accepts.
const REFERENCE_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

/// What the multipart form carried.
///
/// The form is multipart rather than urlencoded because of the optional
/// reference image, so the fields are read by hand instead of by `serde`.
#[derive(Debug, Default)]
struct GenerateFields {
    prompt: String,
    media_type: Option<MediaType>,
    image_size: Option<String>,
    image_quality: Option<String>,
    audio_voice: Option<String>,
    video_seconds: Option<u32>,
    video_size: Option<String>,
    /// File name and bytes of the uploaded reference, when there is one.
    reference: Option<(String, Vec<u8>)>,
}

impl GenerateFields {
    /// Builds the provider parameters for the selected media type.
    ///
    /// Only the fields belonging to the chosen type are read.
    ///
    /// Every control is submitted regardless of which option set is visible:
    /// CSS has no bearing on form submission, and a collapsed `<details>`
    /// submits its contents like anything else. Selecting here rather than
    /// trusting the form is what makes that harmless — and it is also what
    /// stops a hand-crafted POST applying a video length to an image.
    ///
    /// Values are not validated here. Each provider clamps or falls back to its
    /// own default, which keeps the rules next to the API that imposes them.
    fn parameters(&self, media_type: MediaType, reference_path: Option<&str>) -> serde_json::Value {
        match media_type {
            MediaType::Image => serde_json::json!({
                "size": self.image_size,
                "quality": self.image_quality,
                "reference_path": reference_path,
            }),
            MediaType::Audio => serde_json::json!({ "voice": self.audio_voice }),
            MediaType::Video => serde_json::json!({
                "reference_path": reference_path,
                "n_seconds": self.video_seconds,
                "size": self.video_size,
            }),
        }
    }
}

/// Reads the generate form.
///
/// # Errors
///
/// Returns a validation error when a part is malformed, the upload is larger
/// than [`MAX_REFERENCE_BYTES`], or it is not an image type we accept.
async fn read_fields(mut multipart: Multipart) -> Result<GenerateFields, AppError> {
    let mut fields = GenerateFields::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::Validation(error.body_text()))?
    {
        let name = field.name().unwrap_or_default().to_owned();

        if name == "reference" || name == "reference_video" {
            let content_type = field.content_type().unwrap_or_default().to_owned();
            let file_name = field.file_name().unwrap_or("referanse").to_owned();
            let bytes = field
                .bytes()
                .await
                .map_err(|error| AppError::Validation(error.body_text()))?;

            // An untouched file input still submits an empty part.
            if bytes.is_empty() {
                continue;
            }
            if bytes.len() > MAX_REFERENCE_BYTES {
                return Err(AppError::Validation(nb::ERR_REFERENCE_TOO_LARGE.to_owned()));
            }
            // The declared type is checked, not trusted: it decides nothing but
            // whether we accept the file at all, and the provider validates the
            // actual bytes.
            if !REFERENCE_TYPES.contains(&content_type.as_str()) {
                return Err(AppError::Validation(nb::ERR_REFERENCE_TYPE.to_owned()));
            }

            fields.reference = Some((file_name, bytes.to_vec()));
            continue;
        }

        let value = field
            .text()
            .await
            .map_err(|error| AppError::Validation(error.body_text()))?;

        match name.as_str() {
            "prompt" => fields.prompt = value,
            "media_type" => fields.media_type = value.parse().ok(),
            "image_size" => fields.image_size = Some(value),
            "image_quality" => fields.image_quality = Some(value),
            "audio_voice" => fields.audio_voice = Some(value),
            "video_seconds" => fields.video_seconds = value.parse().ok(),
            "video_size" => fields.video_size = Some(value),
            // Unknown fields are ignored rather than refused: a control added
            // to a newer page should not break an instance still running the
            // older code.
            _ => {}
        }
    }

    Ok(fields)
}

/// Returns the file extension of an uploaded name, defaulting to `png`.
fn reference_extension(file_name: &str) -> String {
    let candidate = file_name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_lowercase();

    if candidate.is_empty()
        || candidate.len() > 5
        || !candidate.chars().all(|c| c.is_ascii_alphanumeric())
    {
        // The name comes from the client, and it ends up in a blob path.
        return "png".to_owned();
    }
    candidate
}

/// Accepts a generation request and returns the card for the queued job.
async fn generate(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    client: ClientInfo,
    multipart: Multipart,
) -> Result<Page<JobCardTemplate>, AppError> {
    let fields = read_fields(multipart).await?;
    let media_type = fields
        .media_type
        .ok_or_else(|| AppError::Validation(nb::ERR_UNKNOWN_MEDIA_TYPE.to_owned()))?;

    let prompt = validate_prompt(&fields.prompt, state.config.max_prompt_chars)?;
    let user_id = local_user_id(&state.db, &session, &user).await?;

    // Checked before the row is written, so a refused generation leaves no
    // trace in the history and does not itself count towards the limit.
    ratelimit::check(&state.db, user_id, state.config.rate_limit_per_hour).await?;

    // The reference is stored rather than held in memory: the worker that picks
    // the job up may be a different one, minutes later, after a restart.
    let reference_path = match (media_type, fields.reference.as_ref()) {
        // Both image and video accept one; audio does not.
        (MediaType::Image | MediaType::Video, Some((file_name, bytes))) => {
            let path = format!(
                "referanser/{}.{}",
                Uuid::new_v4(),
                reference_extension(file_name)
            );
            state
                .blobs
                .upload(&path, "application/octet-stream", bytes.clone())
                .await
                .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;
            Some(path)
        }
        _ => None,
    };

    let job = jobs::create(
        &state.db,
        &NewJob {
            user_id,
            media_type,
            parameters: fields.parameters(media_type, reference_path.as_deref()),
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
        reference = reference_path.is_some(),
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

/// Form body for the archive actions.
#[derive(Debug, Deserialize)]
struct VisibilityForm {
    /// `true` to hide from the shared archive, `false` to show it again.
    hidden: bool,
}

/// Hides or shows one of the caller's own generations in the shared archive.
///
/// Hidden is not private: the owner still sees it in their own history. It only
/// means "do not show this to colleagues".
async fn set_visibility(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    Path(id): Path<Uuid>,
    Form(form): Form<VisibilityForm>,
) -> Result<StatusCode, AppError> {
    let user_id = local_user_id(&state.db, &session, &user).await?;

    jobs::set_hidden(&state.db, id, user_id, form.hidden)
        .await
        .map_err(storage_error)?;

    tracing::info!(%user_id, job_id = %id, hidden = form.hidden, "archive visibility changed");
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes one of the caller's own generations, media and all.
///
/// Irreversible, and deliberately separate from hiding. The blobs go first:
/// deleting the row cascades the asset records away, and with them the only
/// record of where the files live.
async fn delete_job(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    client: ClientInfo,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let user_id = local_user_id(&state.db, &session, &user).await?;

    let paths = jobs::delete_for_user(&state.db, id, user_id)
        .await
        .map_err(storage_error)?;

    for path in &paths {
        if let Err(error) = state.blobs.delete(path).await {
            // The row is gone either way. Leaving one orphaned blob behind is
            // better than reporting a failure for work that did happen; the
            // retention sweep will not find it, so it is logged loudly.
            tracing::error!(%error, path, job_id = %id, "could not delete the blob");
        }
    }

    let entry = AuditEntry {
        user_id: Some(user_id),
        action: AuditAction::GenerationDeleted,
        entity: "job",
        entity_id: Some(id.to_string()),
        ip: client.ip.clone(),
        user_agent: client.user_agent.clone(),
    };
    if let Err(error) = audit::record(&state.db, &entry).await {
        tracing::error!(%error, job_id = %id, "could not write the audit entry");
    }

    tracing::info!(%user_id, job_id = %id, blobs = paths.len(), "generation deleted");
    Ok(StatusCode::NO_CONTENT)
}
