//! The application's view of a signed-in person.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// A user, keyed locally by a surrogate id and externally by the Entra object id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// Local primary key, referenced by jobs and the audit log.
    pub id: Uuid,
    /// Entra ID object id. Unique, and the key used on sign-in.
    pub entra_oid: String,
    /// Work e-mail address, when the ID token carried one.
    pub email: Option<String>,
    /// Display name from the directory.
    pub display_name: String,
    /// When the user first signed in.
    pub created_at: OffsetDateTime,
    /// When the user last signed in.
    pub last_login_at: OffsetDateTime,
}
