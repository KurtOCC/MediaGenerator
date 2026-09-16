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
use mediagenerator_domain::i18n::nb;
use tower_sessions::Session;

use crate::{error::AppError, middleware::csrf, render::Page, state::AppState};

/// Returns the page routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/historikk", get(history))
        .route("/eksempler", get(archive))
}

/// One card in the "Generert" grid.
#[derive(Debug, Clone)]
pub struct Example {
    /// `image`, `audio` or `video`; selects the icon and the preview.
    pub media_type: &'static str,
    /// Norwegian label shown above the prompt.
    pub label: &'static str,
    /// The prompt that produced the example, shown in guillemets.
    pub prompt: &'static str,
}

/// Placeholder cards until phase 6 reads the user's own recent output.
fn examples() -> Vec<Example> {
    vec![
        Example {
            media_type: "image",
            label: nb::MEDIA_IMAGE,
            prompt: "Solnedgang over Oslofjorden",
        },
        Example {
            media_type: "audio",
            label: nb::MEDIA_AUDIO,
            prompt: "En vennlig telefonsvarer på norsk",
        },
        Example {
            media_type: "video",
            label: nb::MEDIA_VIDEO,
            prompt: "En kort velkomstvideo for nye ansatte",
        },
    ]
}

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
    /// Cards in the "Generert" grid.
    examples: Vec<Example>,
}

/// A page with a heading, an ingress and an empty state.
#[derive(Template)]
#[template(path = "enkel_side.html")]
struct SimplePageTemplate {
    csrf_token: String,
    initials: String,
    display_name: String,
    /// Page heading, also used in the browser tab.
    title: &'static str,
    /// Ingress under the heading.
    lead: &'static str,
}

/// Renders the generator.
async fn index(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
) -> Result<Page<IndexTemplate>, AppError> {
    let csrf_token = csrf_token(&session).await?;

    Ok(Page(IndexTemplate {
        csrf_token,
        initials: user.initials(),
        display_name: user.display_name.clone(),
        max_prompt_chars: state.config.max_prompt_chars,
        examples: examples(),
    }))
}

/// Renders the user's own history. Filled in phase 6.
async fn history(
    session: Session,
    Extension(user): Extension<SessionUser>,
) -> Result<Page<SimplePageTemplate>, AppError> {
    simple_page(
        &session,
        &user,
        nb::PAGE_HISTORY_TITLE,
        nb::PAGE_HISTORY_LEAD,
    )
    .await
}

/// Renders the shared archive. Filled in phase 6.
async fn archive(
    session: Session,
    Extension(user): Extension<SessionUser>,
) -> Result<Page<SimplePageTemplate>, AppError> {
    simple_page(
        &session,
        &user,
        nb::PAGE_ARCHIVE_TITLE,
        nb::PAGE_ARCHIVE_LEAD,
    )
    .await
}

/// Builds a placeholder page.
async fn simple_page(
    session: &Session,
    user: &SessionUser,
    title: &'static str,
    lead: &'static str,
) -> Result<Page<SimplePageTemplate>, AppError> {
    Ok(Page(SimplePageTemplate {
        csrf_token: csrf_token(session).await?,
        initials: user.initials(),
        display_name: user.display_name.clone(),
        title,
        lead,
    }))
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
            examples: examples(),
        }
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
        assert!(html.contains("0/4000"));
        assert!(html.contains(nb::GENERATE));
        assert!(html.contains(nb::GENERATING));

        // All three media types, with their subtitles
        for (title, subtitle) in [
            (nb::MEDIA_IMAGE, nb::MEDIA_IMAGE_SUB),
            (nb::MEDIA_AUDIO, nb::MEDIA_AUDIO_SUB),
            (nb::MEDIA_VIDEO, nb::MEDIA_VIDEO_SUB),
        ] {
            assert!(html.contains(title), "missing {title}");
            assert!(html.contains(subtitle), "missing {subtitle}");
        }

        // Suggestions, generated section and footer
        assert!(html.contains(nb::SUGGESTIONS_LABEL));
        assert!(html.contains(nb::SUGGESTIONS_IMAGE[0]));
        assert!(html.contains(nb::SUGGESTIONS_VIDEO[0]));
        assert!(html.contains(nb::GENERATED_HEADING));
        assert!(html.contains(nb::GENERATED_LINK));
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

    #[test]
    fn the_placeholder_pages_render() {
        let html = SimplePageTemplate {
            csrf_token: "t".to_owned(),
            initials: "HK".to_owned(),
            display_name: "Hans Kristiansen".to_owned(),
            title: nb::PAGE_ARCHIVE_TITLE,
            lead: nb::PAGE_ARCHIVE_LEAD,
        }
        .render()
        .expect("the placeholder page should render");

        assert!(html.contains(nb::PAGE_ARCHIVE_TITLE));
        assert!(html.contains(nb::PAGE_ARCHIVE_LEAD));
        assert!(html.contains(nb::BACK_TO_GENERATOR));
    }
}
