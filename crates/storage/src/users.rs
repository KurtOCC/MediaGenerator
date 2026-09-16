//! The `users` table.

use mediagenerator_domain::User;
use sqlx::Row as _;
use uuid::Uuid;

use crate::{
    db::Database,
    error::{StorageError, map_sqlx},
};

/// Inserts the user if new, refreshes the profile and stamps the sign-in if not.
///
/// Called on every successful sign-in. The directory is the source of truth for
/// the name and the address, so both are overwritten each time rather than kept
/// at whatever they were on first sign-in.
///
/// # Errors
///
/// Returns an error when the statement fails.
pub async fn upsert_on_login(
    db: &Database,
    entra_oid: &str,
    email: Option<&str>,
    display_name: &str,
) -> Result<User, StorageError> {
    let row = sqlx::query(
        "insert into users (entra_oid, email, display_name)
         values ($1, $2, $3)
         on conflict (entra_oid) do update
            set email         = excluded.email,
                display_name  = excluded.display_name,
                last_login_at = now()
         returning id, entra_oid, email, display_name, created_at, last_login_at",
    )
    .bind(entra_oid)
    .bind(email)
    .bind(display_name)
    .fetch_one(db)
    .await
    .map_err(map_sqlx)?;

    Ok(User {
        id: row.try_get("id").map_err(map_sqlx)?,
        entra_oid: row.try_get("entra_oid").map_err(map_sqlx)?,
        email: row.try_get("email").map_err(map_sqlx)?,
        display_name: row.try_get("display_name").map_err(map_sqlx)?,
        created_at: row.try_get("created_at").map_err(map_sqlx)?,
        last_login_at: row.try_get("last_login_at").map_err(map_sqlx)?,
    })
}

/// Looks up a user by primary key.
///
/// # Errors
///
/// Returns [`StorageError::NotFound`] when there is no such user.
pub async fn by_id(db: &Database, id: Uuid) -> Result<User, StorageError> {
    let row = sqlx::query(
        "select id, entra_oid, email, display_name, created_at, last_login_at
         from users where id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await
    .map_err(map_sqlx)?;

    Ok(User {
        id: row.try_get("id").map_err(map_sqlx)?,
        entra_oid: row.try_get("entra_oid").map_err(map_sqlx)?,
        email: row.try_get("email").map_err(map_sqlx)?,
        display_name: row.try_get("display_name").map_err(map_sqlx)?,
        created_at: row.try_get("created_at").map_err(map_sqlx)?,
        last_login_at: row.try_get("last_login_at").map_err(map_sqlx)?,
    })
}
