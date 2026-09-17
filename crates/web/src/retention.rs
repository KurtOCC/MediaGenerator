//! Retention cleanup.
//!
//! Prompts and generated media are personal data: a prompt says what someone
//! asked for, and the audit trail says when. `RETENTION_DAYS` bounds how long
//! that is kept, and this task is what makes the setting real rather than
//! aspirational.
//!
//! The order matters. Blob paths are read first, then the blobs are deleted,
//! then the job rows — deleting the rows first would cascade the asset records
//! away and leave the files orphaned in the container with nothing pointing at
//! them.
//!
//! The audit trail is deliberately **not** swept: it records who did what and
//! when, never the prompt itself, and an accountability record that deletes
//! itself on the same schedule as the content is not much of a record. Removing
//! it is a policy decision, not a technical one.

use std::{sync::Arc, time::Duration};

use mediagenerator_storage::{BlobStore, Database, assets, jobs};

/// How often the sweep runs.
///
/// Daily is ample for a 90-day window and keeps the load off any particular
/// hour; the first run happens shortly after start-up so a fresh deployment
/// does not wait a day to catch up.
const SWEEP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after start-up the first sweep runs.
///
/// Long enough to stay out of the way while the process is still warming up.
const FIRST_SWEEP_DELAY: Duration = Duration::from_secs(5 * 60);

/// Starts the retention task.
///
/// Runs until the process exits. A failed sweep is logged and retried at the
/// next interval rather than taking anything down: falling behind on deletion
/// by a day is not an outage.
pub fn spawn(db: Database, blobs: Arc<BlobStore>, retention_days: u32) {
    if retention_days == 0 {
        tracing::warn!("RETENTION_DAYS is 0; retention cleanup is disabled");
        return;
    }

    tokio::spawn(async move {
        tokio::time::sleep(FIRST_SWEEP_DELAY).await;
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);

        loop {
            ticker.tick().await;
            if let Err(error) = sweep(&db, &blobs, retention_days).await {
                tracing::error!(%error, "retention sweep failed; will retry tomorrow");
            }
        }
    });

    tracing::info!(retention_days, "retention cleanup scheduled");
}

/// Deletes everything older than `retention_days`.
///
/// # Errors
///
/// Returns an error when the database could not be read or the rows could not
/// be deleted. A blob that refuses to delete is logged and skipped: leaving one
/// file behind is better than abandoning the rest of the sweep.
pub async fn sweep(
    db: &Database,
    blobs: &BlobStore,
    retention_days: u32,
) -> Result<SweepReport, mediagenerator_storage::StorageError> {
    let days = i32::try_from(retention_days).unwrap_or(i32::MAX);

    let paths = assets::paths_older_than(db, days).await?;
    let mut blobs_deleted = 0_u64;
    let mut blobs_failed = 0_u64;

    for path in &paths {
        match blobs.delete(path).await {
            Ok(()) => blobs_deleted += 1,
            Err(error) => {
                blobs_failed += 1;
                tracing::error!(path, %error, "could not delete an expired blob");
            }
        }
    }

    // Only now: the cascade takes the asset rows with the jobs.
    let jobs_deleted = jobs::delete_older_than(db, days).await?;

    let report = SweepReport {
        jobs_deleted,
        blobs_deleted,
        blobs_failed,
    };

    if report.touched_anything() {
        tracing::info!(
            retention_days,
            jobs_deleted = report.jobs_deleted,
            blobs_deleted = report.blobs_deleted,
            blobs_failed = report.blobs_failed,
            "retention sweep completed"
        );
    }

    Ok(report)
}

/// What one sweep did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    /// Job rows removed, with their assets cascading.
    pub jobs_deleted: u64,
    /// Blobs removed from the container.
    pub blobs_deleted: u64,
    /// Blobs that could not be removed and were left behind.
    pub blobs_failed: u64,
}

impl SweepReport {
    /// Returns `true` when the sweep found anything at all.
    ///
    /// Used to keep a quiet daily sweep out of the log.
    pub const fn touched_anything(&self) -> bool {
        self.jobs_deleted > 0 || self.blobs_deleted > 0 || self.blobs_failed > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_sweep_is_not_worth_logging() {
        let quiet = SweepReport {
            jobs_deleted: 0,
            blobs_deleted: 0,
            blobs_failed: 0,
        };
        assert!(!quiet.touched_anything());
    }

    #[test]
    fn a_sweep_that_only_failed_is_still_worth_logging() {
        let failed = SweepReport {
            jobs_deleted: 0,
            blobs_deleted: 0,
            blobs_failed: 3,
        };
        assert!(failed.touched_anything());
    }
}
