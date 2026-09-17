//! The `jobs` table.
//!
//! Every read that can reach a user is scoped by `user_id`. Ownership is a
//! `where` clause, not a check the caller is trusted to remember: a job that
//! belongs to someone else must be indistinguishable from one that does not
//! exist.

use mediagenerator_domain::{Job, JobListItem, JobStatus, ListFilter, MediaType, NewJob};
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

/// Columns selected for a [`JobListItem`].
///
/// The asset is joined in rather than fetched per row: a page of twenty jobs
/// would otherwise be twenty-one queries.
const LIST_COLUMNS: &str = "j.id, j.user_id, j.media_type::text as media_type, j.prompt, \
                            j.status, j.error_code, j.error_message, j.created_at, \
                            j.hidden, a.id as asset_id, u.display_name as owner_name";

/// Builds the `where` clause and records which parameter each filter took.
///
/// The fragments are assembled from fixed strings only; every value the caller
/// supplies travels as a bind parameter, so nothing here can be injected into.
fn list_predicates(filter: &ListFilter) -> (String, bool, bool) {
    let mut clauses = vec!["true".to_owned()];
    let mut next = 1;

    let by_user = filter.user_id.is_some();
    if by_user {
        clauses.push(format!("j.user_id = ${next}"));
        next += 1;
    }

    let by_media = filter.media_type.is_some();
    if by_media {
        clauses.push(format!("j.media_type = ${next}::media_type"));
    }

    if filter.exclude_hidden {
        clauses.push("not j.hidden".to_owned());
    }

    if filter.only_succeeded {
        // A job with no asset has nothing to show in a gallery, even if the
        // row itself says it succeeded.
        clauses.push("j.status = 'succeeded' and a.id is not null".to_owned());
    }

    (clauses.join(" and "), by_user, by_media)
}

/// Builds a [`JobListItem`] from a row.
fn to_list_item(row: &PgRow, viewer: Uuid) -> Result<JobListItem, StorageError> {
    let media_type: String = row.try_get("media_type").map_err(map_sqlx)?;
    let status: String = row.try_get("status").map_err(map_sqlx)?;
    let error_code: Option<String> = row.try_get("error_code").map_err(map_sqlx)?;
    let error_message: Option<String> = row.try_get("error_message").map_err(map_sqlx)?;
    let user_id: Uuid = row.try_get("user_id").map_err(map_sqlx)?;

    Ok(JobListItem {
        id: row.try_get("id").map_err(map_sqlx)?,
        media_type: media_type.parse().unwrap_or(MediaType::Image),
        prompt: row.try_get("prompt").map_err(map_sqlx)?,
        status: JobStatus::from_columns(&status, error_code.as_deref(), error_message.as_deref()),
        created_at: row.try_get("created_at").map_err(map_sqlx)?,
        asset_id: row.try_get("asset_id").map_err(map_sqlx)?,
        owner_name: row.try_get("owner_name").map_err(map_sqlx)?,
        hidden: row.try_get("hidden").map_err(map_sqlx)?,
        owned_by_viewer: user_id == viewer,
    })
}

/// Returns one page of jobs matching `filter`.
///
/// `viewer` decides only which rows are marked as the viewer's own; what is
/// visible at all is decided by `filter.user_id`.
///
/// # Errors
///
/// Returns an error when the query fails.
pub async fn list(
    db: &Database,
    filter: &ListFilter,
    viewer: Uuid,
) -> Result<Vec<JobListItem>, StorageError> {
    let (predicates, by_user, by_media) = list_predicates(filter);
    let direction = if filter.newest_first { "desc" } else { "asc" };

    // Two more placeholders than the predicates used, for limit and offset.
    let used = usize::from(by_user) + usize::from(by_media);
    let sql = format!(
        "select {LIST_COLUMNS}
         from jobs j
         join users u on u.id = j.user_id
         left join assets a on a.job_id = j.id
         where {predicates}
         order by j.created_at {direction}
         limit ${} offset ${}",
        used + 1,
        used + 2
    );

    let mut query = sqlx::query(&sql);
    if let Some(user_id) = filter.user_id {
        query = query.bind(user_id);
    }
    if let Some(media_type) = filter.media_type {
        query = query.bind(media_type.as_str());
    }
    let rows = query
        .bind(filter.limit)
        .bind(filter.offset)
        .fetch_all(db)
        .await
        .map_err(map_sqlx)?;

    rows.iter().map(|row| to_list_item(row, viewer)).collect()
}

/// Counts the jobs matching `filter`, for pagination.
///
/// # Errors
///
/// Returns an error when the query fails.
pub async fn count(db: &Database, filter: &ListFilter) -> Result<i64, StorageError> {
    let (predicates, by_user, by_media) = list_predicates(filter);

    let sql = format!(
        "select count(*) as antall
         from jobs j
         left join assets a on a.job_id = j.id
         where {predicates}"
    );
    let _ = (by_user, by_media);

    let mut query = sqlx::query(&sql);
    if let Some(user_id) = filter.user_id {
        query = query.bind(user_id);
    }
    if let Some(media_type) = filter.media_type {
        query = query.bind(media_type.as_str());
    }

    let row = query.fetch_one(db).await.map_err(map_sqlx)?;
    row.try_get("antall").map_err(map_sqlx)
}

/// Hides or unhides one of the caller's own jobs.
///
/// The `user_id` in the `where` clause is the access check: hiding is only ever
/// something you do to your own work.
///
/// # Errors
///
/// Returns [`StorageError::NotFound`] when the job does not exist or belongs to
/// someone else.
pub async fn set_hidden(
    db: &Database,
    id: Uuid,
    user_id: Uuid,
    hidden: bool,
) -> Result<(), StorageError> {
    let result = sqlx::query("update jobs set hidden = $3 where id = $1 and user_id = $2")
        .bind(id)
        .bind(user_id)
        .bind(hidden)
        .execute(db)
        .await
        .map_err(map_sqlx)?;

    if result.rows_affected() == 0 {
        return Err(StorageError::NotFound);
    }
    Ok(())
}

/// Deletes one of the caller's own jobs, returning the blob paths it owned.
///
/// The paths come back so the caller can remove the files: the cascade takes
/// the asset rows with the job, and with them the only record of where the
/// media lives.
///
/// # Errors
///
/// Returns [`StorageError::NotFound`] when the job does not exist or belongs to
/// someone else.
pub async fn delete_for_user(
    db: &Database,
    id: Uuid,
    user_id: Uuid,
) -> Result<Vec<String>, StorageError> {
    // Read the paths first, still scoped by owner.
    let rows = sqlx::query(
        "select a.blob_path
         from assets a
         join jobs j on j.id = a.job_id
         where j.id = $1 and j.user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(map_sqlx)?;

    let paths: Vec<String> = rows
        .iter()
        .map(|row| row.try_get("blob_path").map_err(map_sqlx))
        .collect::<Result<_, _>>()?;

    let result = sqlx::query("delete from jobs where id = $1 and user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(map_sqlx)?;

    if result.rows_affected() == 0 {
        return Err(StorageError::NotFound);
    }

    Ok(paths)
}
