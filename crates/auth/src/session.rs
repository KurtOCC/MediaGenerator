//! What the authentication layer keeps in the server-side session.
//!
//! Two things are stored, under separate keys with separate lifetimes:
//!
//! * [`LoginFlow`] — short-lived state for a sign-in in progress. Removed as
//!   soon as the callback is handled, successfully or not.
//! * [`SessionUser`] — the signed-in identity, kept for the life of the session.
//!
//! Neither is ever sent to the browser. The only thing the client holds is the
//! signed session cookie, which carries an opaque session ID.

use mediagenerator_domain::i18n::nb;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tower_sessions::Session;

use crate::{claims::EntraIdTokenClaims, error::AuthError};

/// Session key holding the in-progress [`LoginFlow`].
pub const LOGIN_FLOW_KEY: &str = "auth.flow";

/// Session key holding the authenticated [`SessionUser`].
pub const USER_KEY: &str = "auth.user";

/// Session key holding the raw ID token, used as `id_token_hint` on sign-out.
pub const ID_TOKEN_KEY: &str = "auth.id_token";

/// How long a sign-in may take from `/auth/login` to `/auth/callback`.
///
/// Long enough for a password plus MFA prompt, short enough that an abandoned
/// flow cannot be resumed much later.
pub const LOGIN_FLOW_TTL_SECONDS: i64 = 15 * 60;

/// Server-side state for a sign-in that is in progress.
///
/// Held in the session between the redirect to Entra ID and the callback. The
/// PKCE verifier and the nonce are secrets: if either reached the browser, the
/// protection they provide would be gone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginFlow {
    /// PKCE code verifier matching the challenge sent to the provider.
    pub pkce_verifier: String,
    /// CSRF `state` value echoed back by the provider.
    pub state: String,
    /// Nonce bound into the ID token, to detect token replay.
    pub nonce: String,
    /// Local path to return to once sign-in completes.
    pub return_to: String,
}

/// An entry in the session, stamped with when it was created.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stamped<T> {
    value: T,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

/// The authenticated user, as resolved from a verified ID token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUser {
    /// Entra ID object ID: stable across applications in the tenant.
    pub oid: String,
    /// Work e-mail address, when the token carried one.
    pub email: Option<String>,
    /// User principal name, when the token carried one.
    pub upn: Option<String>,
    /// Display name shown in the top bar.
    pub display_name: String,
    /// App roles assigned to this user for this application.
    pub roles: Vec<String>,
    /// Group object IDs, when the app registration emits a groups claim.
    pub groups: Vec<String>,
    /// When this user signed in.
    #[serde(with = "time::serde::rfc3339")]
    pub authenticated_at: OffsetDateTime,
}

impl SessionUser {
    /// Builds a session user from verified ID token claims.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::MissingIdentity`] when the token carries neither an
    /// `oid` claim nor a subject.
    pub fn from_claims(claims: &EntraIdTokenClaims) -> Result<Self, AuthError> {
        let extra = claims.additional_claims();

        // `oid` is preferred over `sub`: `sub` is pairwise per application and
        // would change if the app registration were ever recreated.
        let oid = extra
            .oid
            .clone()
            .unwrap_or_else(|| claims.subject().as_str().to_owned());
        if oid.is_empty() {
            return Err(AuthError::MissingIdentity);
        }

        let email = claims
            .email()
            .map(|email| email.as_str().to_owned())
            .or_else(|| extra.upn.clone());

        let display_name = claims
            .name()
            .and_then(|name| name.get(None))
            .map(|name| name.as_str().to_owned())
            .or_else(|| claims.preferred_username().map(|n| n.as_str().to_owned()))
            .or_else(|| email.clone())
            .unwrap_or_else(|| nb::UNKNOWN_USER.to_owned());

        Ok(Self {
            oid,
            email,
            upn: extra.upn.clone(),
            display_name,
            roles: extra.roles.clone(),
            groups: extra.groups.clone(),
            authenticated_at: OffsetDateTime::now_utc(),
        })
    }

    /// Returns `true` when the user holds `role`, as an app role or a group.
    ///
    /// Both are checked so that access can be granted either by assigning the
    /// app role directly or by emitting a groups claim, without a code change.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|assigned| assigned == role)
            || self.groups.iter().any(|group| group == role)
    }

    /// Returns the initials shown in the avatar, at most two letters.
    pub fn initials(&self) -> String {
        let letters: String = self
            .display_name
            .split_whitespace()
            .filter_map(|word| word.chars().next())
            .filter(|c| c.is_alphabetic())
            .take(2)
            .collect();

        if letters.is_empty() {
            "?".to_owned()
        } else {
            letters.to_uppercase()
        }
    }
}

/// Stores the in-progress login flow in the session.
///
/// # Errors
///
/// Returns an error when the session store rejects the write.
pub async fn put_login_flow(session: &Session, flow: LoginFlow) -> Result<(), AuthError> {
    let stamped = Stamped {
        value: flow,
        created_at: OffsetDateTime::now_utc(),
    };
    session.insert(LOGIN_FLOW_KEY, stamped).await?;
    Ok(())
}

/// Removes and returns the in-progress login flow.
///
/// The flow is taken, not read: a callback may only be handled once, so a
/// replayed callback finds nothing.
///
/// # Errors
///
/// Returns [`AuthError::NoFlowInProgress`] when the session holds no flow, and
/// [`AuthError::FlowExpired`] when it is older than [`LOGIN_FLOW_TTL_SECONDS`].
pub async fn take_login_flow(session: &Session) -> Result<LoginFlow, AuthError> {
    let stamped: Stamped<LoginFlow> = session
        .remove(LOGIN_FLOW_KEY)
        .await?
        .ok_or(AuthError::NoFlowInProgress)?;

    let age = OffsetDateTime::now_utc() - stamped.created_at;
    if age.whole_seconds() > LOGIN_FLOW_TTL_SECONDS {
        return Err(AuthError::FlowExpired);
    }
    Ok(stamped.value)
}

/// Stores the authenticated user and the raw ID token in the session.
///
/// # Errors
///
/// Returns an error when the session store rejects the write.
pub async fn put_user(
    session: &Session,
    user: &SessionUser,
    id_token: &str,
) -> Result<(), AuthError> {
    session.insert(USER_KEY, user).await?;
    session.insert(ID_TOKEN_KEY, id_token).await?;
    Ok(())
}

/// Returns the authenticated user, if the session holds one.
///
/// # Errors
///
/// Returns an error when the session store cannot be read.
pub async fn current_user(session: &Session) -> Result<Option<SessionUser>, AuthError> {
    Ok(session.get(USER_KEY).await?)
}

/// Returns the raw ID token, if the session holds one.
///
/// # Errors
///
/// Returns an error when the session store cannot be read.
pub async fn id_token(session: &Session) -> Result<Option<String>, AuthError> {
    Ok(session.get(ID_TOKEN_KEY).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(display_name: &str, roles: &[&str], groups: &[&str]) -> SessionUser {
        SessionUser {
            oid: "oid".to_owned(),
            email: None,
            upn: None,
            display_name: display_name.to_owned(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
            authenticated_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn initials_take_the_first_letter_of_the_first_two_words() {
        assert_eq!(user("Hans Kristiansen", &[], &[]).initials(), "HK");
        assert_eq!(user("Hans Petter Kristiansen", &[], &[]).initials(), "HP");
        assert_eq!(user("Hans", &[], &[]).initials(), "H");
    }

    #[test]
    fn initials_fall_back_when_the_name_has_no_letters() {
        assert_eq!(user("  ", &[], &[]).initials(), "?");
        assert_eq!(user("123", &[], &[]).initials(), "?");
    }

    #[test]
    fn a_role_may_come_from_app_roles_or_from_groups() {
        assert!(user("H K", &["Mediagenerator.User"], &[]).has_role("Mediagenerator.User"));
        assert!(user("H K", &[], &["Mediagenerator.User"]).has_role("Mediagenerator.User"));
        assert!(!user("H K", &["Other.Role"], &[]).has_role("Mediagenerator.User"));
    }
}
