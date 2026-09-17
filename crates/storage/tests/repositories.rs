//! Repository tests against a real PostgreSQL database.
//!
//! The queries use the runtime `sqlx` API, which is not checked at compile
//! time, so these tests are what catches a column name or a cast that does not
//! match the schema. They are also what proves the ownership scoping actually
//! filters.
//!
//! They run only when `MEDIAGENERATOR_TEST_DATABASE_URL` is set, and they
//! migrate the database they are pointed at. Point it at a throwaway database,
//! never at one holding real generations.

use mediagenerator_domain::{
    AuditAction, AuditEntry, ErrorCode, JobStatus, ListFilter, MediaType, NewAsset, NewJob,
};
use mediagenerator_storage::{Database, StorageError, assets, audit, connect, jobs, users};
use std::sync::LazyLock;

use tokio::sync::RwLock;
use uuid::Uuid;

/// Environment variable holding the throwaway database to test against.
const TEST_DATABASE_URL: &str = "MEDIAGENERATOR_TEST_DATABASE_URL";

/// Opens and migrates the test database, or returns `None` when unconfigured.
///
/// Skipping rather than failing keeps `cargo test` green on a machine with no
/// database, which is also the situation in CI.
async fn database() -> Option<Database> {
    let url = std::env::var(TEST_DATABASE_URL).ok()?;
    match connect(&url, 5).await {
        Ok(db) => Some(db),
        Err(error) => panic!("{TEST_DATABASE_URL} is set but unusable: {error}"),
    }
}

/// Guards the shared test database.
///
/// Most tests only touch their own rows and can run together, so they take a
/// read guard. The retention tests delete *every* job older than zero days,
/// which is every job, so they take the write guard and run alone. Without
/// this they would delete rows out from under whatever else was running.
static DATABASE_ACCESS: LazyLock<RwLock<()>> = LazyLock::new(|| RwLock::new(()));

/// Runs `body` against the database, or does nothing when unconfigured.
macro_rules! with_database {
    (|$db:ident| $body:block) => {
        let _shared = DATABASE_ACCESS.read().await;
        let Some($db) = database().await else {
            eprintln!("skipped: {TEST_DATABASE_URL} is not set");
            return;
        };
        $body
    };
}

/// Runs `body` with the database to itself.
///
/// For tests that delete rows they do not own.
macro_rules! with_exclusive_database {
    (|$db:ident| $body:block) => {
        let _exclusive = DATABASE_ACCESS.write().await;
        let Some($db) = database().await else {
            eprintln!("skipped: {TEST_DATABASE_URL} is not set");
            return;
        };
        $body
    };
}

/// Creates a user with a unique Entra object id.
async fn a_user(db: &Database) -> mediagenerator_domain::User {
    let oid = format!("test-{}", Uuid::new_v4());
    users::upsert_on_login(db, &oid, Some("test@example.com"), "Test Testesen")
        .await
        .expect("the user should be created")
}

/// Creates a queued job for `user_id`.
async fn a_job(db: &Database, user_id: Uuid, prompt: &str) -> mediagenerator_domain::Job {
    jobs::create(
        db,
        &NewJob {
            user_id,
            media_type: MediaType::Image,
            prompt: prompt.to_owned(),
            parameters: serde_json::json!({"size": "1024x1024"}),
        },
    )
    .await
    .expect("the job should be created")
}

#[tokio::test]
async fn signing_in_twice_updates_the_profile_rather_than_duplicating_it() {
    with_database!(|db| {
        let first = a_user(&db).await;

        // The directory is the source of truth, so a changed name must win.
        let second = users::upsert_on_login(
            &db,
            &first.entra_oid,
            Some("nytt@example.com"),
            "Test Testesen-Hansen",
        )
        .await
        .expect("the second sign-in should succeed");

        assert_eq!(first.id, second.id, "the same person must keep one row");
        assert_eq!(second.display_name, "Test Testesen-Hansen");
        assert_eq!(second.email.as_deref(), Some("nytt@example.com"));
        assert!(second.last_login_at >= first.last_login_at);
    });
}

#[tokio::test]
async fn a_new_job_starts_queued_and_keeps_its_parameters() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "Solnedgang over Oslofjorden").await;

        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.media_type, MediaType::Image);
        assert_eq!(job.prompt, "Solnedgang over Oslofjorden");
        assert_eq!(job.parameters["size"], "1024x1024");
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());
    });
}

#[tokio::test]
async fn a_job_belonging_to_someone_else_is_reported_as_missing() {
    with_database!(|db| {
        let owner = a_user(&db).await;
        let stranger = a_user(&db).await;
        let job = a_job(&db, owner.id, "hemmelig").await;

        // Not "forbidden": the existence of the row is itself information.
        let result = jobs::by_id_for_user(&db, job.id, stranger.id).await;
        assert!(matches!(result, Err(StorageError::NotFound)));

        // And the owner can still read it.
        assert!(jobs::by_id_for_user(&db, job.id, owner.id).await.is_ok());
    });
}

#[tokio::test]
async fn a_job_runs_through_its_lifecycle() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "livssyklus").await;

        let running = jobs::mark_running(&db, job.id)
            .await
            .expect("the job should start");
        assert_eq!(running.status, JobStatus::Running);
        assert!(running.started_at.is_some());

        let done = jobs::finish(&db, job.id, &JobStatus::Succeeded)
            .await
            .expect("the job should finish");
        assert_eq!(done.status, JobStatus::Succeeded);
        assert!(done.completed_at.is_some());
        assert!(done.duration().is_some());
    });
}

#[tokio::test]
async fn only_one_worker_can_start_the_same_job() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "kappløp").await;

        assert!(jobs::mark_running(&db, job.id).await.is_ok());

        // The second attempt matches no row, which is how a duplicate delivery
        // is prevented from being worked twice.
        let second = jobs::mark_running(&db, job.id).await;
        assert!(matches!(second, Err(StorageError::NotFound)));
    });
}

#[tokio::test]
async fn a_finished_job_cannot_be_finished_again() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "ferdig").await;

        jobs::mark_running(&db, job.id).await.expect("should start");
        jobs::finish(&db, job.id, &JobStatus::Succeeded)
            .await
            .expect("should finish");

        let again = jobs::finish(&db, job.id, &JobStatus::Cancelled).await;
        assert!(matches!(again, Err(StorageError::NotFound)));
    });
}

#[tokio::test]
async fn a_failure_keeps_its_code_and_message() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "feiler").await;
        jobs::mark_running(&db, job.id).await.expect("should start");

        let status = JobStatus::Failed {
            code: ErrorCode::ContentFilter,
            message: "Beskrivelsen ble avvist av innholdsfilteret.".to_owned(),
        };
        let failed = jobs::finish(&db, job.id, &status)
            .await
            .expect("should finish");

        assert_eq!(failed.status, status);

        // And it survives a round trip through the database.
        let reloaded = jobs::by_id_for_user(&db, job.id, user.id)
            .await
            .expect("should reload");
        assert_eq!(reloaded.status.error_code(), Some(ErrorCode::ContentFilter));
    });
}

#[tokio::test]
async fn active_jobs_are_what_a_reload_picks_up() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let queued = a_job(&db, user.id, "i kø").await;
        let finished = a_job(&db, user.id, "ferdig").await;

        jobs::mark_running(&db, finished.id)
            .await
            .expect("should start");
        jobs::finish(&db, finished.id, &JobStatus::Succeeded)
            .await
            .expect("should finish");

        let active = jobs::active_for_user(&db, user.id)
            .await
            .expect("should list active jobs");

        let ids: Vec<_> = active.iter().map(|job| job.id).collect();
        assert!(ids.contains(&queued.id));
        assert!(!ids.contains(&finished.id));
    });
}

#[tokio::test]
async fn the_history_page_is_newest_first() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let first = a_job(&db, user.id, "eldst").await;
        let second = a_job(&db, user.id, "nyest").await;

        let page = jobs::page_for_user(&db, user.id, 10, 0)
            .await
            .expect("should list jobs");

        assert_eq!(page.len(), 2);
        assert_eq!(page[0].id, second.id);
        assert_eq!(page[1].id, first.id);

        // And paging does not repeat a row.
        let second_page = jobs::page_for_user(&db, user.id, 1, 1)
            .await
            .expect("should list jobs");
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].id, first.id);
    });
}

#[tokio::test]
async fn an_asset_is_only_reachable_by_the_owner_of_its_job() {
    with_database!(|db| {
        let owner = a_user(&db).await;
        let stranger = a_user(&db).await;
        let job = a_job(&db, owner.id, "med fil").await;

        let asset = assets::create(
            &db,
            &NewAsset {
                job_id: job.id,
                blob_path: "2026/09/test.png".to_owned(),
                content_type: "image/png".to_owned(),
                size_bytes: 1234,
                duration_ms: None,
                width: Some(1024),
                height: Some(1024),
            },
        )
        .await
        .expect("the asset should be created");

        assert!(
            assets::by_id_for_user(&db, asset.id, owner.id)
                .await
                .is_ok()
        );

        // The join is the access check; without it an id would be enough.
        let denied = assets::by_id_for_user(&db, asset.id, stranger.id).await;
        assert!(matches!(denied, Err(StorageError::NotFound)));

        let found = assets::for_job(&db, job.id)
            .await
            .expect("should look up by job");
        assert_eq!(found.map(|a| a.id), Some(asset.id));
    });
}

#[tokio::test]
async fn deleting_a_job_takes_its_assets_with_it() {
    with_exclusive_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "slettes").await;

        assets::create(
            &db,
            &NewAsset {
                job_id: job.id,
                blob_path: "2026/09/slettes.png".to_owned(),
                content_type: "image/png".to_owned(),
                size_bytes: 10,
                duration_ms: None,
                width: None,
                height: None,
            },
        )
        .await
        .expect("the asset should be created");

        // Retention deletes jobs; the cascade is what stops assets being
        // orphaned behind them.
        jobs::delete_older_than(&db, 0)
            .await
            .expect("the sweep should run");

        assert!(jobs::by_id_for_user(&db, job.id, user.id).await.is_err());
        assert_eq!(
            assets::for_job(&db, job.id)
                .await
                .expect("should look up by job"),
            None
        );
    });
}

#[tokio::test]
async fn the_rate_limit_counter_sees_jobs_from_the_last_hour() {
    with_database!(|db| {
        let user = a_user(&db).await;
        assert_eq!(
            jobs::count_last_hour(&db, user.id)
                .await
                .expect("should count"),
            0
        );

        a_job(&db, user.id, "en").await;
        a_job(&db, user.id, "to").await;

        assert_eq!(
            jobs::count_last_hour(&db, user.id)
                .await
                .expect("should count"),
            2
        );
    });
}

#[tokio::test]
async fn the_startup_sweep_fails_whatever_was_left_running() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "avbrutt").await;
        jobs::mark_running(&db, job.id).await.expect("should start");

        jobs::fail_orphans(&db, ErrorCode::Internal.as_str(), "avbrutt")
            .await
            .expect("the sweep should run");

        let reloaded = jobs::by_id_for_user(&db, job.id, user.id)
            .await
            .expect("should reload");
        assert!(matches!(reloaded.status, JobStatus::Failed { .. }));
        assert!(reloaded.completed_at.is_some());
    });
}

#[tokio::test]
async fn an_audit_entry_is_recorded_with_its_client_details() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_job(&db, user.id, "revisjon").await;

        audit::record(
            &db,
            &AuditEntry {
                user_id: Some(user.id),
                action: AuditAction::GenerationRequested,
                entity: "job",
                entity_id: Some(job.id.to_string()),
                ip: Some("213.160.245.172".to_owned()),
                // Longer than the column keeps, and ending mid-character to
                // make sure truncation does not split a code point.
                user_agent: Some("æ".repeat(600)),
            },
        )
        .await
        .expect("the audit entry should be written");
    });
}

#[tokio::test]
async fn an_audit_entry_tolerates_a_missing_ip() {
    with_database!(|db| {
        let user = a_user(&db).await;

        audit::record(
            &db,
            &AuditEntry {
                user_id: Some(user.id),
                action: AuditAction::SignIn,
                entity: "user",
                entity_id: Some(user.id.to_string()),
                ip: None,
                user_agent: None,
            },
        )
        .await
        .expect("the audit entry should be written");
    });
}

/// Creates a queued job of a given media type.
async fn a_typed_job(
    db: &Database,
    user_id: Uuid,
    media_type: MediaType,
    prompt: &str,
) -> mediagenerator_domain::Job {
    jobs::create(
        db,
        &NewJob {
            user_id,
            media_type,
            prompt: prompt.to_owned(),
            parameters: serde_json::json!({}),
        },
    )
    .await
    .expect("the job should be created")
}

/// Marks a job succeeded and gives it an asset.
async fn succeed_with_asset(db: &Database, job_id: Uuid) -> Uuid {
    jobs::mark_running(db, job_id).await.expect("should start");
    jobs::finish(db, job_id, &JobStatus::Succeeded)
        .await
        .expect("should finish");

    assets::create(
        db,
        &NewAsset {
            job_id,
            blob_path: format!("2026/09/test/{job_id}.png"),
            content_type: "image/png".to_owned(),
            size_bytes: 1,
            duration_ms: None,
            width: None,
            height: None,
        },
    )
    .await
    .expect("the asset should be created")
    .id
}

/// A filter with sane defaults for the tests.
fn filter(user_id: Option<Uuid>) -> ListFilter {
    ListFilter {
        user_id,
        media_type: None,
        only_succeeded: false,
        newest_first: true,
        limit: 50,
        offset: 0,
    }
}

#[tokio::test]
async fn a_listing_carries_the_asset_and_the_owner_in_one_query() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let job = a_typed_job(&db, user.id, MediaType::Image, "med fil").await;
        let asset_id = succeed_with_asset(&db, job.id).await;

        let listed = jobs::list(&db, &filter(Some(user.id)), user.id)
            .await
            .expect("the listing should run");

        let found = listed
            .iter()
            .find(|item| item.id == job.id)
            .expect("the job should be listed");

        assert_eq!(found.asset_id, Some(asset_id));
        assert_eq!(found.owner_name, user.display_name);
        assert!(found.owned_by_viewer);
    });
}

#[tokio::test]
async fn the_media_filter_narrows_the_listing() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let image = a_typed_job(&db, user.id, MediaType::Image, "bilde").await;
        let audio = a_typed_job(&db, user.id, MediaType::Audio, "lyd").await;

        let mut only_audio = filter(Some(user.id));
        only_audio.media_type = Some(MediaType::Audio);

        let listed = jobs::list(&db, &only_audio, user.id)
            .await
            .expect("the listing should run");

        let ids: Vec<_> = listed.iter().map(|item| item.id).collect();
        assert!(ids.contains(&audio.id));
        assert!(!ids.contains(&image.id));

        // And the count agrees with the listing, or pagination would lie.
        assert_eq!(
            jobs::count(&db, &only_audio)
                .await
                .expect("count should run"),
            listed.len() as i64
        );
    });
}

#[tokio::test]
async fn the_archive_hides_jobs_that_produced_nothing() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let with_file = a_typed_job(&db, user.id, MediaType::Image, "har fil").await;
        let queued = a_typed_job(&db, user.id, MediaType::Image, "ingen fil").await;
        succeed_with_asset(&db, with_file.id).await;

        let mut archive = filter(Some(user.id));
        archive.only_succeeded = true;

        let listed = jobs::list(&db, &archive, user.id)
            .await
            .expect("the listing should run");

        let ids: Vec<_> = listed.iter().map(|item| item.id).collect();
        assert!(ids.contains(&with_file.id));
        assert!(
            !ids.contains(&queued.id),
            "a gallery entry with no file has nothing to show"
        );
    });
}

#[tokio::test]
async fn the_shared_archive_shows_other_peoples_work_but_marks_it_as_theirs() {
    with_database!(|db| {
        let viewer = a_user(&db).await;
        let other = a_user(&db).await;

        let mine = a_typed_job(&db, viewer.id, MediaType::Image, "mitt").await;
        let theirs = a_typed_job(&db, other.id, MediaType::Image, "deres").await;
        succeed_with_asset(&db, mine.id).await;
        succeed_with_asset(&db, theirs.id).await;

        // user_id = None is the "alle" view.
        let listed = jobs::list(&db, &filter(None), viewer.id)
            .await
            .expect("the listing should run");

        let mine_row = listed.iter().find(|item| item.id == mine.id);
        let theirs_row = listed.iter().find(|item| item.id == theirs.id);

        assert!(mine_row.is_some_and(|item| item.owned_by_viewer));
        assert!(theirs_row.is_some_and(|item| !item.owned_by_viewer));
    });
}

#[tokio::test]
async fn sorting_runs_both_ways() {
    with_database!(|db| {
        let user = a_user(&db).await;
        let first = a_typed_job(&db, user.id, MediaType::Image, "eldst").await;
        let second = a_typed_job(&db, user.id, MediaType::Image, "nyest").await;

        let newest = jobs::list(&db, &filter(Some(user.id)), user.id)
            .await
            .expect("the listing should run");
        assert_eq!(newest.first().map(|item| item.id), Some(second.id));

        let mut oldest_first = filter(Some(user.id));
        oldest_first.newest_first = false;
        let oldest = jobs::list(&db, &oldest_first, user.id)
            .await
            .expect("the listing should run");
        assert_eq!(oldest.first().map(|item| item.id), Some(first.id));
    });
}

#[tokio::test]
async fn paging_does_not_repeat_or_skip_a_row() {
    with_database!(|db| {
        let user = a_user(&db).await;
        for index in 0..5 {
            a_typed_job(&db, user.id, MediaType::Image, &format!("nr {index}")).await;
        }

        let mut page = filter(Some(user.id));
        page.limit = 2;

        let mut seen = Vec::new();
        for offset in [0, 2, 4] {
            page.offset = offset;
            let rows = jobs::list(&db, &page, user.id)
                .await
                .expect("the listing should run");
            seen.extend(rows.into_iter().map(|item| item.id));
        }

        assert_eq!(seen.len(), 5);
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), 5, "a row appeared on two pages");
    });
}

#[tokio::test]
async fn retention_finds_the_blob_paths_before_the_rows_are_deleted() {
    with_exclusive_database!(|db| {
        let user = a_user(&db).await;
        let job = a_typed_job(&db, user.id, MediaType::Image, "gammel").await;
        succeed_with_asset(&db, job.id).await;

        // Everything older than zero days, which is everything.
        let paths = assets::paths_older_than(&db, 0)
            .await
            .expect("the lookup should run");
        assert!(
            paths.iter().any(|path| path.contains(&job.id.to_string())),
            "the path must be readable before the cascade removes the row"
        );

        jobs::delete_older_than(&db, 0)
            .await
            .expect("the sweep should run");

        let after = assets::paths_older_than(&db, 0)
            .await
            .expect("the lookup should run");
        assert!(!after.iter().any(|path| path.contains(&job.id.to_string())));
    });
}
