//! Core domain model for Mediagenerator.
//!
//! This crate is deliberately free of I/O and framework dependencies so that the
//! same types can be reused by the web layer, the storage layer and the provider
//! clients.

#![forbid(unsafe_code)]

pub mod error;
pub mod i18n;
pub mod media;

pub use error::{DomainError, ErrorCode};
pub use media::{MediaType, validate_prompt};
