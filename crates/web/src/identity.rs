//! Bridging the session identity to the `users` row.
//!
//! The session carries what Entra ID said: an object id, a name, an address.
//! Jobs and audit entries reference a local `users.id`. This module maps one to
//! the other and caches the result in the session, so the lookup happens once
//! per sign-in rather than once per request.

use mediagenerator_auth::SessionUser;
use mediagenerator_storage::{Database, users};
use tower_sessions::Session;
use uuid::Uuid;

use crate::error::AppError;

/// Session key holding the local `users.id`.
const USER_ID_KEY: &str = "app.user_id";

/// Returns the local user id for the signed-in session.
///
/// Reads the cached value, and falls back to an upsert keyed on the Entra
/// object id. The fallback matters for sessions created before the database
/// existed, and after a sign-in where the database was briefly unavailable:
/// neither should force the user to sign in again.
///
/// # Errors
///
/// Returns an error when the database cannot be reached or the session cannot
/// be written.
pub async fn local_user_id(
    db: &Database,
    session: &Session,
    user: &SessionUser,
) -> Result<Uuid, AppError> {
    if let Some(cached) = session
        .get::<Uuid>(USER_ID_KEY)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?
    {
        return Ok(cached);
    }

    let row = users::upsert_on_login(db, &user.oid, user.email.as_deref(), &user.display_name)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;

    remember(session, row.id).await?;
    Ok(row.id)
}

/// Caches the local user id on the session.
///
/// # Errors
///
/// Returns an error when the session cannot be written.
pub async fn remember(session: &Session, user_id: Uuid) -> Result<(), AppError> {
    session
        .insert(USER_ID_KEY, user_id)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))
}
