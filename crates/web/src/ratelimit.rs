//! Per-user rate limiting.
//!
//! The limit is counted from the `jobs` table rather than from an in-memory
//! counter. That costs one query per submission and buys three things: the
//! limit survives a restart, it holds across replicas, and it cannot drift from
//! what the user can actually see in their own history.
//!
//! It is a fairness control, not a security boundary. The global queue bound in
//! [`crate::jobs`] is what protects the provider from overload.

use mediagenerator_domain::i18n::nb;
use mediagenerator_storage::{Database, jobs};
use uuid::Uuid;

use crate::error::AppError;

/// Refuses the request when the user has generated too much in the last hour.
///
/// A limit of zero disables the check, which is how it is turned off for an
/// environment that does not want one.
///
/// # Errors
///
/// Returns [`AppError::RateLimited`] when the limit is reached, or an internal
/// error when the count could not be read.
pub async fn check(db: &Database, user_id: Uuid, per_hour: u32) -> Result<(), AppError> {
    if per_hour == 0 {
        return Ok(());
    }

    let used = jobs::count_last_hour(db, user_id)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;

    let limit = i64::from(per_hour);
    if used >= limit {
        tracing::info!(
            %user_id,
            used,
            limit,
            "refused a generation: the hourly limit is reached"
        );
        return Err(AppError::RateLimited);
    }

    // Worth seeing in the log before anyone complains they were cut off.
    if used + 1 == limit {
        tracing::info!(%user_id, limit, "user reached the hourly limit with this generation");
    }

    Ok(())
}

/// Returns how many generations the user has left this hour.
///
/// Shown in the interface so the limit is visible before it bites, rather than
/// arriving as a refusal out of nowhere. `None` means no limit is configured.
///
/// # Errors
///
/// Returns an error when the count could not be read.
pub async fn remaining(
    db: &Database,
    user_id: Uuid,
    per_hour: u32,
) -> Result<Option<u32>, AppError> {
    if per_hour == 0 {
        return Ok(None);
    }

    let used = jobs::count_last_hour(db, user_id)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;

    let used = u32::try_from(used.max(0)).unwrap_or(u32::MAX);
    Ok(Some(per_hour.saturating_sub(used)))
}

/// Returns the Norwegian message for a user who has hit the limit.
pub const fn limit_message() -> &'static str {
    nb::ERR_RATE_LIMITED
}
