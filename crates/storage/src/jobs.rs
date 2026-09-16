//! The `jobs` table.
//!
//! Every read that can reach a user is scoped by `user_id`. Ownership is a
//! `where` clause, not a check the caller is trusted to remember: a job that
//! belongs to someone else must be indistinguishable from one that does not
//! exist.

use mediagenerator_domain::{Job, JobStatus, MediaType, NewJob};
use sqlx::{Row as _, postgres::PgRow};
use uuid::Uuid;

use crate::{
    db::Database,
    error::{StorageError, map_sqlx},
};

/// Columns selected for a [`Job`], in one place so every query agrees.
const JOB_COLUMNS: &str = "id, user_id, media_type::text as media_type, prompt, parameters, \
                           status, provider_job_id, error_code, error_message, \
                           created_at, started_at, completed_at";

/// Builds a [`Job`] from a row.
fn to_job(row: &PgRow) -> Result<Job, StorageError> {
    let media_type: String = row.try_get("media_type").map_err(map_sqlx)?;
    let status: String = row.try_get("status").map_err(map_sqlx)?;
    let error_code: Option<String> = row.try_get("error_code").map_err(map_sqlx)?;
    let error_message: Option<String> = row.try_get("error_message").map_err(map_sqlx)?;

    Ok(Job {
        id: row.try_get("id").map_err(map_sqlx)?,
        user_id: row.try_get("user_id").map_err(map_sqlx)?,
        media_type: media_type.parse().unwrap_or(MediaType::Image),
        prompt: row.try_get("prompt").map_err(map_sqlx)?,
        parameters: row.try_get("parameters").map_err(map_sqlx)?,
        status: JobStatus::from_columns(&status, error_code.as_deref(), error_message.as_deref()),
        provider_job_id: row.try_get("provider_job_id").map_err(map_sqlx)?,
        created_at: row.try_get("created_at").map_err(map_sqlx)?,
        started_at: row.try_get("started_at").map_err(map_sqlx)?,
        completed_at: row.try_get("completed_at").map_err(map_sqlx)?,
    })
}

/// Inserts a job in the `queued` state.
///
/// # Errors
///
/// Returns an error when the insert fails.
pub async fn create(db: &Database, new_job: &NewJob) -> Result<Job, StorageError> {
    let sql = format!(
        "insert into jobs (user_id, media_type, prompt, parameters, status)
         values ($1, $2::media_type, $3, $4, 'queued')
         returning {JOB_COLUMNS}"
    );

    let row = sqlx::query(&sql)
        .bind(new_job.user_id)
        .bind(new_job.media_type.as_str())
        .bind(&new_job.prompt)
        .bind(&new_job.parameters)
        .fetch_one(db)
        .await
        .map_err(map_sqlx)?;

    to_job(&row)
}

/// Loads one job belonging to `user_id`.
///
/// # Errors
///
/// Returns [`StorageError::NotFound`] when the job does not exist or belongs to
/// another user.
pub async fn by_id_for_user(db: &Database, id: Uuid, user_id: Uuid) -> Result<Job, StorageError> {
    let sql = format!("select {JOB_COLUMNS} from jobs where id = $1 and user_id = $2");

    let row = sqlx::query(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_one(db)
        .await
        .map_err(map_sqlx)?;

    to_job(&row)
}

/// Loads one job without an ownership check.
///
/// Only the background worker may use this: it acts on behalf of the system,
/// not of a request. Nothing it returns is rendered without a later check.
///
/// # Errors
///
/// Returns [`StorageError::NotFound`] when the job does not exist.
pub async fn by_id_for_worker(db: &Database, id: Uuid) -> Result<Job, StorageError> {
    let sql = format!("select {JOB_COLUMNS} from jobs where id = $1");

    let row = sqlx::query(&sql)
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(map_sqlx)?;

    to_job(&row)
}

/// Returns a page of the user's jobs, newest first.
///
/// # Errors
///
/// Returns an error when the query fails.
pub async fn page_for_user(
    db: &Database,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Job>, StorageError> {
    let sql = format!(
        "select {JOB_COLUMNS} from jobs
         where user_id = $1
         order by created_at desc
         limit $2 offset $3"
    );

    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await
        .map_err(map_sqlx)?;

    rows.iter().map(to_job).collect()
}

/// Returns the user's jobs that have not finished.
///
/// This is what makes a running generation survive a reload: the page asks the
/// database what is still in flight rather than trusting anything the browser
/// kept.
///
/// # Errors
///
/// Returns an error when the query fails.
pub async fn active_for_user(db: &Database, user_id: Uuid) -> Result<Vec<Job>, StorageError> {
    let sql = format!(
        "select {JOB_COLUMNS} from jobs
         where user_id = $1 and status in ('queued', 'running')
         order by created_at desc"
    );

    let rows = sqlx::query(&sql)
        .bind(user_id)
        .fetch_all(db)
        .await
        .map_err(map_sqlx)?;

    rows.iter().map(to_job).collect()
}

/// Moves a job from `queued` to `running`.
///
/// The `where status = 'queued'` makes this a compare-and-set: if two workers
/// ever pick up the same id, only one update matches a row and the other is
/// told the job is gone.
///
/// # Errors
///
/// Returns [`StorageError::NotFound`] when the job was not queued any more.
pub async fn mark_running(db: &Database, id: Uuid) -> Result<Job, StorageError> {
    let sql = format!(
        "update jobs
         set status = 'running', started_at = now()
         where id = $1 and status = 'queued'
         returning {JOB_COLUMNS}"
    );

    let row = sqlx::query(&sql)
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(map_sqlx)?;

    to_job(&row)
}

/// Writes a terminal status.
///
/// # Errors
///
/// Returns an error when the update fails, or [`StorageError::NotFound`] when
/// the job had already finished.
pub async fn finish(db: &Database, id: Uuid, status: &JobStatus) -> Result<Job, StorageError> {
    let sql = format!(
        "update jobs
         set status = $2, error_code = $3, error_message = $4, completed_at = now()
         where id = $1 and status in ('queued', 'running')
         returning {JOB_COLUMNS}"
    );

    let row = sqlx::query(&sql)
        .bind(id)
        .bind(status.as_str())
        .bind(status.error_code().map(|code| code.as_str()))
        .bind(status.error_message())
        .fetch_one(db)
        .await
        .map_err(map_sqlx)?;

    to_job(&row)
}

/// Records the provider's own job identifier, for asynchronous generation.
///
/// # Errors
///
/// Returns an error when the update fails.
pub async fn set_provider_job_id(
    db: &Database,
    id: Uuid,
    provider_job_id: &str,
) -> Result<(), StorageError> {
    sqlx::query("update jobs set provider_job_id = $2 where id = $1")
        .bind(id)
        .bind(provider_job_id)
        .execute(db)
        .await
        .map_err(map_sqlx)?;
    Ok(())
}

/// Fails every job left `queued` or `running` by a previous process.
///
/// Called once at start-up. A worker that was interrupted mid-flight leaves a
/// row that nothing will ever finish, and a job stuck on "Genererer…" forever
/// is worse than one that says it failed and can be retried.
///
/// # Errors
///
/// Returns an error when the update fails.
pub async fn fail_orphans(db: &Database, code: &str, message: &str) -> Result<u64, StorageError> {
    let result = sqlx::query(
        "update jobs
         set status = 'failed', error_code = $1, error_message = $2, completed_at = now()
         where status in ('queued', 'running')",
    )
    .bind(code)
    .bind(message)
    .execute(db)
    .await
    .map_err(map_sqlx)?;

    Ok(result.rows_affected())
}

/// Counts a user's jobs created within the last hour, for rate limiting.
///
/// # Errors
///
/// Returns an error when the query fails.
pub async fn count_last_hour(db: &Database, user_id: Uuid) -> Result<i64, StorageError> {
    let row = sqlx::query(
        "select count(*) as antall from jobs
         where user_id = $1 and created_at > now() - interval '1 hour'",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
    .map_err(map_sqlx)?;

    row.try_get("antall").map_err(map_sqlx)
}

/// Deletes jobs older than `days`, returning how many were removed.
///
/// Assets cascade with the job, so the media rows go too; phase 5 removes the
/// blobs themselves in the same sweep.
///
/// # Errors
///
/// Returns an error when the delete fails.
pub async fn delete_older_than(db: &Database, days: i32) -> Result<u64, StorageError> {
    let result =
        sqlx::query("delete from jobs where created_at < now() - make_interval(days => $1)")
            .bind(days)
            .execute(db)
            .await
            .map_err(map_sqlx)?;

    Ok(result.rows_affected())
}
