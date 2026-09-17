//! Generation jobs and their lifecycle.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{error::ErrorCode, media::MediaType};

/// Where a job is in its lifecycle.
///
/// The variant names are serialised in lowercase and stored in `jobs.status`,
/// which carries a check constraint listing exactly these five values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum JobStatus {
    /// Accepted and waiting for a worker.
    Queued,
    /// A worker is talking to the provider.
    Running,
    /// Finished; the asset is stored.
    Succeeded,
    /// Finished unsuccessfully.
    Failed {
        /// Stable, machine-readable cause.
        code: ErrorCode,
        /// Norwegian message for the user.
        message: String,
    },
    /// Stopped before completion, at the user's request or on shutdown.
    Cancelled,
}

impl JobStatus {
    /// Returns the string stored in `jobs.status`.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed { .. } => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Returns `true` when the job will not change state again.
    ///
    /// The SSE stream closes on a terminal status, and the worker stops
    /// touching the row.
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed { .. } | Self::Cancelled
        )
    }

    /// Rebuilds a status from the three columns that store it.
    ///
    /// An unknown string, or a `failed` row without a code, means the database
    /// contains something this build did not write. That is treated as a failed
    /// job rather than a panic: a bad row must not take down a request.
    pub fn from_columns(status: &str, code: Option<&str>, message: Option<&str>) -> Self {
        match status {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed {
                code: code.map_or(ErrorCode::Internal, ErrorCode::from_str_or_internal),
                message: message.unwrap_or(crate::i18n::nb::ERR_INTERNAL).to_owned(),
            },
            other => {
                tracing_unknown_status(other);
                Self::Failed {
                    code: ErrorCode::Internal,
                    message: crate::i18n::nb::ERR_INTERNAL.to_owned(),
                }
            }
        }
    }

    /// Returns the error code, when this status carries one.
    pub const fn error_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Failed { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// Returns the error message, when this status carries one.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Failed { message, .. } => Some(message),
            _ => None,
        }
    }
}

/// Logs an unexpected status value without pulling `tracing` into the signature.
fn tracing_unknown_status(value: &str) {
    tracing::error!(status = %value, "unknown job status in the database");
}

/// A generation job as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// Primary key, also the identifier used in URLs.
    pub id: Uuid,
    /// Owner. Every read is scoped by this.
    pub user_id: Uuid,
    /// What kind of media was asked for.
    pub media_type: MediaType,
    /// The prompt, already validated and normalised.
    pub prompt: String,
    /// Provider parameters as submitted.
    pub parameters: serde_json::Value,
    /// Current lifecycle state.
    pub status: JobStatus,
    /// Identifier at the provider, for asynchronous video jobs.
    pub provider_job_id: Option<String>,
    /// When the job was accepted.
    pub created_at: OffsetDateTime,
    /// When a worker picked it up.
    pub started_at: Option<OffsetDateTime>,
    /// When it reached a terminal state.
    pub completed_at: Option<OffsetDateTime>,
}

impl Job {
    /// Returns how long the job took, once it has finished.
    pub fn duration(&self) -> Option<time::Duration> {
        let started = self.started_at?;
        let completed = self.completed_at?;
        Some(completed - started)
    }
}

/// The fields needed to enqueue a new job.
#[derive(Debug, Clone)]
pub struct NewJob {
    /// Owner of the job.
    pub user_id: Uuid,
    /// What kind of media to produce.
    pub media_type: MediaType,
    /// Validated prompt.
    pub prompt: String,
    /// Provider parameters.
    pub parameters: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip_through_their_columns() {
        for status in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Succeeded,
            JobStatus::Cancelled,
        ] {
            let rebuilt = JobStatus::from_columns(status.as_str(), None, None);
            assert_eq!(rebuilt, status);
        }
    }

    #[test]
    fn a_failed_status_keeps_its_code_and_message() {
        let failed = JobStatus::Failed {
            code: ErrorCode::ContentFilter,
            message: "avvist".to_owned(),
        };
        let rebuilt = JobStatus::from_columns("failed", Some("content_filter"), Some("avvist"));
        assert_eq!(rebuilt, failed);
        assert_eq!(rebuilt.error_code(), Some(ErrorCode::ContentFilter));
    }

    #[test]
    fn only_finished_statuses_are_terminal() {
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
        assert!(JobStatus::Succeeded.is_terminal());
        assert!(JobStatus::Cancelled.is_terminal());
        assert!(
            JobStatus::Failed {
                code: ErrorCode::Upstream,
                message: String::new()
            }
            .is_terminal()
        );
    }

    #[test]
    fn an_unrecognised_status_degrades_to_a_failure() {
        let rebuilt = JobStatus::from_columns("noe-helt-annet", None, None);
        assert!(matches!(rebuilt, JobStatus::Failed { .. }));
    }
}

/// A job as the history and archive pages show it.
///
/// A projection rather than a [`Job`]: the listings need the asset and the
/// owner's name, and do not need the prompt parameters or the provider id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobListItem {
    /// Job identifier.
    pub id: Uuid,
    /// What kind of media it produced.
    pub media_type: MediaType,
    /// The prompt, shown in quotes on the card.
    pub prompt: String,
    /// Lifecycle state, so a failed job can be shown as failed.
    pub status: JobStatus,
    /// When the job was accepted.
    pub created_at: OffsetDateTime,
    /// The asset it produced, when it produced one.
    pub asset_id: Option<Uuid>,
    /// Display name of whoever generated it.
    pub owner_name: String,
    /// True when the owner has hidden this from the shared archive.
    pub hidden: bool,
    /// True when the signed-in user owns this job.
    pub owned_by_viewer: bool,
}

/// Which jobs a listing should return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListFilter {
    /// Restrict to one user's jobs. `None` lists everyone's.
    pub user_id: Option<Uuid>,
    /// Restrict to one media type.
    pub media_type: Option<MediaType>,
    /// Exclude jobs the owner has hidden from the shared archive.
    ///
    /// Off for the owner's own views: hiding something from colleagues
    /// should not hide it from yourself.
    pub exclude_hidden: bool,
    /// Exclude anything that did not produce media.
    pub only_succeeded: bool,
    /// Newest first when true, oldest first when false.
    pub newest_first: bool,
    /// Page size.
    pub limit: i64,
    /// Rows to skip.
    pub offset: i64,
}
