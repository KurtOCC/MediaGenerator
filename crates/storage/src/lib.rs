//! Persistence for Mediagenerator: PostgreSQL repositories and, from phase 5,
//! Azure Blob Storage.
//!
//! Queries use the runtime `sqlx` API rather than the `query!` macros. The
//! macros check SQL against a live database at compile time, which would mean
//! either a database in CI or a committed offline cache that has to be
//! regenerated on every query change. The trade is deliberate: type errors in
//! SQL surface in the repository tests, which run against a real database, and
//! `cargo build` stays independent of any server.

#![forbid(unsafe_code)]

pub mod assets;
pub mod audit;
pub mod db;
pub mod error;
pub mod jobs;
pub mod users;

pub use db::{Database, connect};
pub use error::StorageError;
