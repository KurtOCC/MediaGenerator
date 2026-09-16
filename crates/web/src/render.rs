//! Rendering askama templates into HTTP responses.
//!
//! askama does not depend on axum, so the bridge lives here: one wrapper that
//! turns any [`Template`] into a `text/html` response and maps a rendering
//! failure to the standard error response instead of panicking.

use askama::Template;
use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

/// Wraps a template so it can be returned directly from a handler.
#[derive(Debug, Clone, Copy)]
pub struct Page<T>(pub T);

impl<T> IntoResponse for Page<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(body) => (
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                )],
                body,
            )
                .into_response(),
            Err(error) => {
                // A template that fails to render is a bug in this codebase,
                // never something the caller did.
                tracing::error!(%error, "template rendering failed");
                crate::error::AppError::Internal(anyhow::Error::new(error)).into_response()
            }
        }
    }
}

/// Renders a template to a string, for callers that need the body itself.
///
/// # Errors
///
/// Returns the askama error when rendering fails.
pub fn render<T: Template>(template: &T) -> Result<String, askama::Error> {
    template.render()
}

/// Status code used when a handler returns a rendered page.
pub const OK: StatusCode = StatusCode::OK;
