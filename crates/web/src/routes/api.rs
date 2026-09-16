//! JSON and HTMX endpoints behind `require_auth`.
//!
//! `POST /api/generate` is a mock in this phase: it validates the input, waits
//! briefly and returns the finished result card. Phase 4 replaces the body of
//! the handler with a queued job and an SSE stream, and phase 5 puts a real
//! provider behind it. The request shape, the response fragment and everything
//! the browser does stay as they are.

use std::time::Duration;

use askama::Template;
use axum::{
    Extension, Json, Router,
    extract::{Form, State},
    routing::{get, post},
};

use mediagenerator_auth::SessionUser;
use mediagenerator_domain::{
    i18n::nb,
    media::{MediaType, validate_prompt},
};
use serde::{Deserialize, Serialize};

use crate::{error::AppError, render::Page, state::AppState};

/// How long the mock pretends to work, so the spinner is actually visible.
const MOCK_DURATION: Duration = Duration::from_millis(1200);

/// Returns the API routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(me))
        .route("/api/generate", post(generate))
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

/// The rendered result card.
#[derive(Template)]
#[template(path = "partials/result_card.html")]
struct ResultCardTemplate {
    /// The prompt, echoed back under the preview.
    prompt: String,
    /// `image`, `audio` or `video`.
    media_type: &'static str,
    /// Human-readable time spent, e.g. "1,2 s".
    elapsed: String,
    /// True while generation is mocked, which disables the download actions.
    is_mock: bool,
    /// Where the finished asset lives. Empty while mocked.
    asset_url: String,
}

/// Accepts a generation request and returns the result card.
///
/// Validation happens before anything else, and its message is the Norwegian
/// one from the domain crate, so the same text appears whether the form was
/// posted by HTMX or by a browser without scripting.
async fn generate(
    State(state): State<AppState>,
    Extension(user): Extension<SessionUser>,
    Form(form): Form<GenerateForm>,
) -> Result<Page<ResultCardTemplate>, AppError> {
    let prompt = validate_prompt(&form.prompt, state.config.max_prompt_chars)?;

    tracing::info!(
        oid = %user.oid,
        media_type = %form.media_type,
        prompt_chars = prompt.chars().count(),
        "generation requested"
    );

    // Stands in for the provider round trip. Phase 4 turns this into a queued
    // job the browser follows over SSE.
    tokio::time::sleep(MOCK_DURATION).await;

    Ok(Page(ResultCardTemplate {
        prompt,
        media_type: form.media_type.as_str(),
        elapsed: format_elapsed(MOCK_DURATION),
        is_mock: true,
        asset_url: String::new(),
    }))
}

/// Formats a duration the way Norwegian writes it: comma as decimal separator.
fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    let rendered = format!("{seconds:.1}");
    format!("{} {}", rendered.replace('.', ","), nb::SECONDS_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_uses_a_comma_as_the_decimal_separator() {
        assert_eq!(format_elapsed(Duration::from_millis(1200)), "1,2 s");
        assert_eq!(format_elapsed(Duration::from_millis(450)), "0,5 s");
        assert_eq!(format_elapsed(Duration::from_secs(12)), "12,0 s");
    }
}
