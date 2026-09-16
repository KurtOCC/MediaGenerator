//! Connection pool and migrations.

use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::error::StorageError;

/// The connection pool, shared by every repository.
pub type Database = PgPool;

/// Migrations compiled into the binary.
///
/// Embedding them means a container image can migrate itself on start-up with
/// no separate artefact to keep in step with the code.
static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// How long to wait for a free connection before giving up.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// Opens a pool and applies any outstanding migrations.
///
/// # Errors
///
/// Returns an error when the database cannot be reached or a migration fails.
pub async fn connect(url: &str, max_connections: u32) -> Result<Database, StorageError> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect(url)
        .await?;

    MIGRATIONS.run(&pool).await?;
    tracing::info!("database migrations are up to date");

    Ok(pool)
}
