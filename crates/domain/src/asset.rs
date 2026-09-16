//! Stored media produced by a finished job.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// A file in the private blob container, produced by a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// Primary key, used in `/api/assets/{id}`.
    pub id: Uuid,
    /// The job that produced this file.
    pub job_id: Uuid,
    /// Path inside the private container.
    ///
    /// Never a full URL: the browser is handed a time-limited SAS built from
    /// this path, so a link cannot outlive its expiry or be shared forever.
    pub blob_path: String,
    /// MIME type, used for the preview element and the download.
    pub content_type: String,
    /// Size in bytes.
    pub size_bytes: i64,
    /// Playing time for audio and video.
    pub duration_ms: Option<i32>,
    /// Pixel width for images and video.
    pub width: Option<i32>,
    /// Pixel height for images and video.
    pub height: Option<i32>,
    /// When the file was stored.
    pub created_at: OffsetDateTime,
}

/// The fields needed to record a newly stored file.
#[derive(Debug, Clone)]
pub struct NewAsset {
    /// The job that produced the file.
    pub job_id: Uuid,
    /// Path inside the private container.
    pub blob_path: String,
    /// MIME type.
    pub content_type: String,
    /// Size in bytes.
    pub size_bytes: i64,
    /// Playing time for audio and video.
    pub duration_ms: Option<i32>,
    /// Pixel width.
    pub width: Option<i32>,
    /// Pixel height.
    pub height: Option<i32>,
}
