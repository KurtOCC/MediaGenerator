//! Errors raised by the persistence layer.

use mediagenerator_domain::ErrorCode;
use thiserror::Error;

/// Something went wrong talking to the database.
#[derive(Debug, Error)]
pub enum StorageError {
    /// The row asked for does not exist, or is not visible to the caller.
    #[error("not found")]
    NotFound,

    /// A uniqueness constraint was violated.
    #[error("conflicting row already exists")]
    Conflict,

    /// Anything else the driver reported.
    #[error("database error")]
    Database(#[from] sqlx::Error),

    /// A Blob Storage operation failed.
    #[error("blob storage error: {0}")]
    Blob(String),

    /// A migration failed to apply.
    #[error("migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
}

impl StorageError {
    /// Returns the stable error code for this error.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound => ErrorCode::NotFound,
            Self::Conflict => ErrorCode::Validation,
            Self::Database(_) | Self::Migration(_) | Self::Blob(_) => ErrorCode::Internal,
        }
    }
}

/// Maps a driver error to [`StorageError`], recognising the cases that are not
/// really failures.
///
/// `RowNotFound` is an ordinary outcome of looking something up, and a unique
/// violation is a caller problem rather than an internal one; neither should be
/// reported as a database fault.
pub fn map_sqlx(error: sqlx::Error) -> StorageError {
    match &error {
        sqlx::Error::RowNotFound => StorageError::NotFound,
        sqlx::Error::Database(db) if db.is_unique_violation() => StorageError::Conflict,
        _ => StorageError::Database(error),
    }
}
