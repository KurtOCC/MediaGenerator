//! Persistence layer: PostgreSQL repositories (sqlx) and Azure Blob Storage.
//!
//! Phase 1 only establishes the crate. Repositories and migrations arrive in
//! phase 4, Blob Storage and SAS links in phase 5.

#![forbid(unsafe_code)]
