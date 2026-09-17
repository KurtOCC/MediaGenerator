//! The `assets` table.

use mediagenerator_domain::{Asset, NewAsset};
use sqlx::{Row as _, postgres::PgRow};
use uuid::Uuid;

use crate::{
    db::Database,
    error::{StorageError, map_sqlx},
};

/// Columns selected for an [`Asset`].
const ASSET_COLUMNS: &str =
    "id, job_id, blob_path, content_type, size_bytes, duration_ms, width, height, created_at";

/// Builds an [`Asset`] from a row.
fn to_asset(row: &PgRow) -> Result<Asset, StorageError> {
    Ok(Asset {
        id: row.try_get("id").map_err(map_sqlx)?,
        job_id: row.try_get("job_id").map_err(map_sqlx)?,
        blob_path: row.try_get("blob_path").map_err(map_sqlx)?,
        content_type: row.try_get("content_type").map_err(map_sqlx)?,
        size_bytes: row.try_get("size_bytes").map_err(map_sqlx)?,
        duration_ms: row.try_get("duration_ms").map_err(map_sqlx)?,
        width: row.try_get("width").map_err(map_sqlx)?,
        height: row.try_get("height").map_err(map_sqlx)?,
        created_at: row.try_get("created_at").map_err(map_sqlx)?,
    })
}

/// Records a stored file.
///
/// # Errors
///
/// Returns an error when the insert fails.
pub async fn create(db: &Database, new_asset: &NewAsset) -> Result<Asset, StorageError> {
    let sql = format!(
        "insert into assets
            (job_id, blob_path, content_type, size_bytes, duration_ms, width, height)
         values ($1, $2, $3, $4, $5, $6, $7)
         returning {ASSET_COLUMNS}"
    );

    let row = sqlx::query(&sql)
        .bind(new_asset.job_id)
        .bind(&new_asset.blob_path)
        .bind(&new_asset.content_type)
        .bind(new_asset.size_bytes)
        .bind(new_asset.duration_ms)
        .bind(new_asset.width)
        .bind(new_asset.height)
        .fetch_one(db)
        .await
        .map_err(map_sqlx)?;

    to_asset(&row)
}

/// Returns the asset produced by a job, if it has one.
///
/// # Errors
///
/// Returns an error when the query fails.
pub async fn for_job(db: &Database, job_id: Uuid) -> Result<Option<Asset>, StorageError> {
    let sql =
        format!("select {ASSET_COLUMNS} from assets where job_id = $1 order by created_at limit 1");

    let row = sqlx::query(&sql)
        .bind(job_id)
        .fetch_optional(db)
        .await
        .map_err(map_sqlx)?;

    row.as_ref().map(to_asset).transpose()
}

/// Loads an asset, but only if the given user owns the job that produced it.
///
/// The join is what enforces ownership: `/api/assets/{id}` hands out a signed
/// URL, so a missing check here would let anyone with an id read anyone's media.
///
/// # Errors
///
/// Returns [`StorageError::NotFound`] when the asset does not exist or belongs
/// to another user.
pub async fn by_id_for_user(db: &Database, id: Uuid, user_id: Uuid) -> Result<Asset, StorageError> {
    let sql = format!(
        "select {} from assets a
         join jobs j on j.id = a.job_id
         where a.id = $1 and j.user_id = $2",
        ASSET_COLUMNS
            .split(", ")
            .map(|column| format!("a.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let row = sqlx::query(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_one(db)
        .await
        .map_err(map_sqlx)?;

    to_asset(&row)
}

/// Returns the blob paths of assets whose job is older than `days`.
///
/// Read before the rows are deleted: the cascade removes the asset row, and
/// with it the only record of where the file lives. Without this the blobs
/// would be orphaned in the container forever.
///
/// # Errors
///
/// Returns an error when the query fails.
pub async fn paths_older_than(db: &Database, days: i32) -> Result<Vec<String>, StorageError> {
    let rows = sqlx::query(
        "select a.blob_path
         from assets a
         join jobs j on j.id = a.job_id
         where j.created_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .fetch_all(db)
    .await
    .map_err(map_sqlx)?;

    rows.iter()
        .map(|row| row.try_get("blob_path").map_err(map_sqlx))
        .collect()
}
