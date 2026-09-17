//! Server-rendered pages.
//!
//! Every template pulls its visible text from `mediagenerator_domain::i18n::nb`,
//! imported here as `nb` so the generated rendering code can reach it. No
//! Norwegian prose appears in this file or in the templates themselves.
//!
//! Each page is assembled from the partials in `templates/partials/`, one file
//! per visual component and no logic inside them. That is what keeps the
//! documented Leptos option open: the components would port over, the markup
//! would not need rewriting.

use askama::Template;
use axum::{Extension, Router, extract::State, routing::get};
use mediagenerator_auth::SessionUser;
use mediagenerator_domain::{ListFilter, i18n::nb};
use tower_sessions::Session;

use mediagenerator_storage::jobs;

/// Show the remaining-generations hint from this many left and below.
const HINT_THRESHOLD: u32 = 5;

use crate::{
    error::AppError, identity::local_user_id, middleware::csrf, ratelimit, render::Page,
    state::AppState,
};

/// Returns the page routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/", get(index))
}

/// One card in the "Generert" grid.
///
/// Built from the user's own finished work. There is no placeholder content:
/// an empty account shows an empty section, not invented examples.
#[derive(Debug, Clone)]
pub struct Recent {
    /// `image`, `audio` or `video`, for the icon and the preview element.
    pub media_type: &'static str,
    /// Norwegian label for the media type.
    pub label: &'static str,
    /// The prompt, shown in guillemets.
    pub prompt: String,
    /// Link to the asset.
    pub asset_url: String,
}

/// How many finished generations the front page shows.
const RECENT_COUNT: i64 = 3;

/// The front page.
#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    /// Per-session CSRF token, echoed by HTMX on every request.
    csrf_token: String,
    /// Initials shown in the avatar.
    initials: String,
    /// Display name shown next to the avatar.
    display_name: String,
    /// Prompt length limit, shown in the counter and enforced on submit.
    max_prompt_chars: usize,
    /// Pre-rendered card for a job the user still has running.
    ///
    /// Rendered on load so that a reload or a navigation back picks the
    /// generation up again, rather than losing sight of it.
    active_job: Option<String>,
    /// Hint about how many generations are left this hour.
    ///
    /// Empty unless the user is close to the limit: a counter that is always
    /// there is noise, and one that appears only as a refusal is a surprise.
    rate_limit_hint: String,
    /// The user's three most recent finished generations. Empty when there
    /// are none, and the section is then not rendered at all.
    recent: Vec<Recent>,
}

/// Renders the generator.
async fn index(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
) -> Result<Page<IndexTemplate>, AppError> {
    let csrf_token = csrf_token(&session).await?;
    let active_job = active_job_card(&state, &session, &user).await;
    let rate_limit_hint = rate_limit_hint(&state, &session, &user).await;
    let recent = recent_generations(&state, &session, &user).await;

    Ok(Page(IndexTemplate {
        csrf_token,
        initials: user.initials(),
        display_name: user.display_name.clone(),
        max_prompt_chars: state.config.max_prompt_chars,
        rate_limit_hint,
        active_job,
        recent,
    }))
}

/// Returns the remaining-generations hint, or an empty string.
///
/// Only shown once the user is within [`HINT_THRESHOLD`] of the limit. A
/// failure to read the count is swallowed: the hint is a courtesy, and the
/// limit is enforced on submission regardless.
async fn rate_limit_hint(state: &AppState, session: &Session, user: &SessionUser) -> String {
    let Ok(user_id) = local_user_id(&state.db, session, user).await else {
        return String::new();
    };

    match ratelimit::remaining(&state.db, user_id, state.config.rate_limit_per_hour).await {
        Ok(Some(left)) if left <= HINT_THRESHOLD => nb::generations_remaining(left),
        _ => String::new(),
    }
}

/// Returns the user's most recent finished generations, newest first.
///
/// A failure here degrades to an empty section rather than an error page: not
/// being able to show past work is no reason to withhold the generator.
async fn recent_generations(
    state: &AppState,
    session: &Session,
    user: &SessionUser,
) -> Vec<Recent> {
    let Ok(user_id) = local_user_id(&state.db, session, user).await else {
        return Vec::new();
    };

    let filter = ListFilter {
        user_id: Some(user_id),
        media_type: None,
        only_succeeded: true,
        // Your own work, hidden or not: hiding is about colleagues.
        exclude_hidden: false,
        newest_first: true,
        limit: RECENT_COUNT,
        offset: 0,
    };

    let items = jobs::list(&state.db, &filter, user_id)
        .await
        .inspect_err(|error| tracing::error!(%error, "could not read recent generations"))
        .unwrap_or_default();

    items
        .into_iter()
        .filter_map(|item| {
            let asset_id = item.asset_id?;
            Some(Recent {
                media_type: item.media_type.as_str(),
                label: item.media_type.label(),
                prompt: item.prompt,
                asset_url: format!("/api/assets/{asset_id}"),
            })
        })
        .collect()
}

/// Renders the card for the user's most recent unfinished job, if any.
///
/// A failure here is swallowed on purpose: not being able to show a running
/// job is a degraded front page, not a broken one, and the job itself is
/// unaffected.
async fn active_job_card(
    state: &AppState,
    session: &Session,
    user: &SessionUser,
) -> Option<String> {
    let user_id = local_user_id(&state.db, session, user).await.ok()?;
    let active = jobs::active_for_user(&state.db, user_id)
        .await
        .inspect_err(|error| tracing::error!(%error, "could not read active jobs"))
        .ok()?;

    let newest = active.first()?;
    crate::routes::api::card_for(state, newest)
        .await
        .render()
        .inspect_err(|error| tracing::error!(%error, "could not render the active job card"))
        .ok()
}

/// Reads or creates this session's CSRF token.
async fn csrf_token(session: &Session) -> Result<String, AppError> {
    csrf::token(session)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_template(display_name: &str) -> IndexTemplate {
        IndexTemplate {
            csrf_token: "token-123".to_owned(),
            initials: "HK".to_owned(),
            display_name: display_name.to_owned(),
            max_prompt_chars: 4000,
            rate_limit_hint: String::new(),
            active_job: None,
            recent: Vec::new(),
        }
    }

    #[test]
    fn the_generated_section_appears_only_with_real_work_in_it() {
        let mut template = index_template("Hans Kristiansen");
        template.recent = vec![Recent {
            media_type: "audio",
            label: nb::MEDIA_AUDIO,
            prompt: "En vennlig norsk stemme".to_owned(),
            asset_url: "/api/assets/abc".to_owned(),
        }];

        let html = template.render().expect("the front page should render");

        assert!(html.contains(nb::GENERATED_HEADING));
        assert!(html.contains(nb::GENERATED_LINK));
        assert!(html.contains("En vennlig norsk stemme"));
        // Real media, from our own asset route — never a provider URL.
        assert!(html.contains("/api/assets/abc"));
        // No invented examples survive anywhere.
        assert!(!html.contains("Solnedgang over Oslofjorden"));
    }

    #[test]
    fn the_front_page_renders_the_documented_layout() {
        let html = index_template("Hans Kristiansen")
            .render()
            .expect("the front page should render");

        // Shell
        assert!(html.contains("lang=\"nb\""));
        assert!(html.contains("/assets/css/app.css"));
        assert!(html.contains("/assets/js/htmx.min.js"));

        // Hero
        assert!(html.contains(nb::HERO_EYEBROW));
        assert!(html.contains(nb::HERO_TITLE_LEAD));
        assert!(html.contains(nb::HERO_TITLE_ACCENT));
        assert!(html.contains(nb::HERO_SUBTITLE_1));

        // Generator
        assert!(html.contains(nb::PROMPT_PLACEHOLDER));
        assert!(html.contains("0 / 4000"));
        assert!(html.contains(nb::GENERATE));
        assert!(html.contains(nb::GENERATING));

        // The three media type buttons. The subtitles moved into the options
        // panel, which names the selected type instead.
        for title in [nb::MEDIA_IMAGE, nb::MEDIA_AUDIO, nb::MEDIA_VIDEO] {
            assert!(html.contains(title), "missing {title}");
        }

        // The options panel is collapsed: a <details> with no `open`.
        assert!(html.contains("<details"));
        assert!(!html.contains("<details open"));

        // Suggestions, generated section and footer
        // The suggestion chips are gone; the per-media-type options replaced
        // them, and all three sets are rendered with CSS choosing one.
        assert!(html.contains(nb::OPTIONS_LABEL));
        assert!(html.contains(nb::IMAGE_QUALITY));
        assert!(html.contains(nb::AUDIO_VOICE));
        assert!(html.contains(nb::VIDEO_LENGTH));
        assert!(html.contains(nb::VIDEO_4S));
        // The "Generert" section is absent with nothing to show: a gallery of
        // work you did not make is worse than no gallery.
        assert!(!html.contains(nb::GENERATED_HEADING));
        assert!(html.contains(nb::FOOTER));
    }

    #[test]
    fn exactly_one_media_type_is_preselected() {
        let html = index_template("Hans Kristiansen")
            .render()
            .expect("the front page should render");
        // `checked="checked"` rather than the bare attribute, so the assertion
        // cannot be confused by the group-has-[:checked] utility classes.
        assert_eq!(html.matches(r#"checked="checked""#).count(), 1);

        // Image is the default, as in the layout.
        let image_at = html
            .find(r#"id="medietype-bilde""#)
            .expect("the image radio should be present");
        let audio_at = html
            .find(r#"id="medietype-lyd""#)
            .expect("the audio radio should be present");
        assert!(html[image_at..audio_at].contains(r#"checked="checked""#));
    }

    #[test]
    fn the_csrf_token_reaches_both_htmx_and_the_sign_out_form() {
        let html = index_template("Hans Kristiansen")
            .render()
            .expect("the front page should render");
        assert!(html.contains(r#"hx-headers='{"x-csrf-token": "token-123"}'"#));
        assert!(html.contains(r#"name="csrf_token" value="token-123""#));
    }

    #[test]
    fn a_display_name_from_the_directory_is_escaped() {
        // The name comes from Entra ID, not from us, so it is untrusted.
        let html = index_template("<script>alert(1)</script>")
            .render()
            .expect("the front page should render");
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&#60;script&#62;"));
    }
}
