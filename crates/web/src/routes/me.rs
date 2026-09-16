//! Temporary endpoints behind `require_auth`.
//!
//! Phase 2 has no user interface yet, so these two routes exist to make the
//! guard observable: reaching either of them proves that a session was
//! established from a verified ID token. Phase 3 replaces `/` with the real
//! page and keeps `/api/me` as the endpoint the top bar reads the user from.

use axum::{Extension, Json, Router, response::Html, routing::get};
use mediagenerator_auth::SessionUser;
use serde::Serialize;

use crate::state::AppState;

/// Returns the temporary authenticated routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/api/me", get(me))
}

/// The signed-in user, as returned by `GET /api/me`.
///
/// A projection rather than the session type itself: the raw session value may
/// grow fields that have no business being exposed to the browser.
#[derive(Debug, Serialize)]
struct MeBody {
    /// Entra ID object ID.
    oid: String,
    /// Display name for the top bar.
    display_name: String,
    /// Initials for the avatar.
    initials: String,
    /// Work e-mail address, when known.
    email: Option<String>,
    /// App roles held by the user.
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

/// Placeholder front page, replaced by the real UI in phase 3.
async fn index(Extension(user): Extension<SessionUser>) -> Html<String> {
    // Escaped because the display name comes from the directory, not from us.
    let name = html_escape(&user.display_name);
    Html(format!(
        "<!doctype html><html lang=\"nb\"><head><meta charset=\"utf-8\">\
         <title>Mediagenerator</title></head><body>\
         <p>Innlogget som {name}.</p>\
         <form method=\"post\" action=\"/auth/logout\"><button>Logg ut</button></form>\
         </body></html>"
    ))
}

/// Escapes the five characters that are unsafe in HTML text and attributes.
fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escaping_covers_the_dangerous_characters() {
        assert_eq!(
            html_escape("<script>alert('x' & \"y\")</script>"),
            "&lt;script&gt;alert(&#39;x&#39; &amp; &quot;y&quot;)&lt;/script&gt;"
        );
    }

    #[test]
    fn ordinary_names_pass_through_unchanged() {
        assert_eq!(html_escape("Hans Kristiansen"), "Hans Kristiansen");
        assert_eq!(html_escape("Åse Øvrebø"), "Åse Øvrebø");
    }
}
