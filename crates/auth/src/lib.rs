//! Authentication: OpenID Connect against Microsoft Entra ID, session handling,
//! `require_auth` middleware and optional role checks.
//!
//! Phase 1 only establishes the crate; the OIDC flow arrives in phase 2.

#![forbid(unsafe_code)]
