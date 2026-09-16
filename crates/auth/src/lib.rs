//! Authentication for Mediagenerator: OpenID Connect against Microsoft Entra
//! ID, server-side session state and the route guards built on top of them.
//!
//! The flow is Authorization Code with PKCE. Everything secret — the PKCE
//! verifier, the CSRF state, the nonce and the ID token — stays in the
//! server-side session; the browser only ever holds a signed cookie carrying an
//! opaque session ID.
//!
//! The web crate owns the HTTP routes (`/auth/login`, `/auth/callback`,
//! `/auth/logout`); this crate owns the protocol and the guards.

#![forbid(unsafe_code)]

pub mod claims;
pub mod config;
pub mod error;
pub mod middleware;
pub mod oidc;
pub mod session;

pub use claims::{EntraClaims, EntraIdTokenClaims};
pub use config::OidcConfig;
pub use error::AuthError;
pub use middleware::{RolePolicy, require_auth, require_role};
pub use oidc::OidcProvider;
pub use session::{LoginFlow, SessionUser};
