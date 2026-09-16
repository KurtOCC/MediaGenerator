//! The `audit_log` table.

use mediagenerator_domain::AuditEntry;

use crate::{
    db::Database,
    error::{StorageError, map_sqlx},
};

/// Longest user agent kept, in bytes.
///
/// Long enough for any real browser, short enough that a crafted header cannot
/// be used to bloat the table.
const MAX_USER_AGENT: usize = 512;

/// Appends an entry to the audit trail.
///
/// # Errors
///
/// Returns an error when the insert fails.
pub async fn record(db: &Database, entry: &AuditEntry) -> Result<(), StorageError> {
    let user_agent = entry.user_agent.as_deref().map(|value| {
        let mut end = value.len().min(MAX_USER_AGENT);
        // Truncating mid-character would produce invalid UTF-8.
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        &value[..end]
    });

    sqlx::query(
        "insert into audit_log (user_id, action, entity, entity_id, ip, user_agent)
         values ($1, $2, $3, $4, $5::text::inet, $6)",
    )
    .bind(entry.user_id)
    .bind(entry.action.as_str())
    .bind(entry.entity)
    .bind(entry.entity_id.as_deref())
    .bind(entry.ip.as_deref())
    .bind(user_agent)
    .execute(db)
    .await
    .map_err(map_sqlx)?;

    Ok(())
}
