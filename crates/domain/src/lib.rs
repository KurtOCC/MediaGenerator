//! Core domain model for Mediagenerator.
//!
//! This crate is deliberately free of I/O and framework dependencies so that the
//! same types can be reused by the web layer, the storage layer and the provider
//! clients. Phase 1 only establishes the crate, the shared error type and the
//! Norwegian user-facing text catalogue; the job and asset models arrive in
//! phase 4.

#![forbid(unsafe_code)]

pub mod error;
pub mod i18n;

pub use error::{DomainError, ErrorCode};
