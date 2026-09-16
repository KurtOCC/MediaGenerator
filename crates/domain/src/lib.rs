//! Core domain model for Mediagenerator.
//!
//! This crate is deliberately free of I/O and framework dependencies so that the
//! same types can be reused by the web layer, the storage layer and the provider
//! clients.

#![forbid(unsafe_code)]

pub mod asset;
pub mod audit;
pub mod error;
pub mod i18n;
pub mod job;
pub mod media;
pub mod user;

pub use asset::{Asset, NewAsset};
pub use audit::{AuditAction, AuditEntry};
pub use error::{DomainError, ErrorCode};
pub use job::{Job, JobStatus, NewJob};
pub use media::{MediaType, validate_prompt};
pub use user::User;
