//! Audit trail entries.
//!
//! What is recorded is who did what, to which entity, when, and from where —
//! never the prompt itself. The prompt lives on the job row, which is subject
//! to the retention policy; the audit trail is kept for accountability and
//! should not quietly become a second copy of the content.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An action worth recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// A user signed in.
    SignIn,
    /// A user signed out.
    SignOut,
    /// A generation was requested.
    GenerationRequested,
    /// A generation finished successfully.
    GenerationSucceeded,
    /// A generation failed.
    GenerationFailed,
    /// A stored asset was handed out as a time-limited link.
    AssetAccessed,
}

impl AuditAction {
    /// Returns the string stored in `audit_log.action`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SignIn => "sign_in",
            Self::SignOut => "sign_out",
            Self::GenerationRequested => "generation_requested",
            Self::GenerationSucceeded => "generation_succeeded",
            Self::GenerationFailed => "generation_failed",
            Self::AssetAccessed => "asset_accessed",
        }
    }
}

/// One entry to append to the audit trail.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Who acted. `None` for actions taken before sign-in completed.
    pub user_id: Option<Uuid>,
    /// What they did.
    pub action: AuditAction,
    /// Which kind of entity was acted on, e.g. `job` or `asset`.
    pub entity: &'static str,
    /// Identifier of that entity.
    pub entity_id: Option<String>,
    /// Client IP, as seen by the application.
    pub ip: Option<String>,
    /// Client user agent, truncated by the caller if unreasonably long.
    pub user_agent: Option<String>,
}
