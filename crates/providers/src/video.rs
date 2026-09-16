//! Video generation with Sora.
//!
//! The only genuinely asynchronous provider, and the one the `submit`/`poll`
//! split in [`MediaProvider`] exists for:
//!
//! 1. `POST {endpoint}/openai/v1/video/generations/jobs` creates the job,
//! 2. `GET  {endpoint}/openai/v1/video/generations/jobs/{job-id}` reports on it,
//! 3. `GET  {endpoint}/openai/v1/video/generations/{generation-id}/content/video`
//!    fetches the finished file.
//!
//! # Verification status
//!
//! Written against the Microsoft Learn reference (checked 2026-09-16) but **not
//! exercised against a live deployment**: the `sora-2` quota in this
//! subscription is fully assigned to another resource, so no Sora deployment
//! could be created. The image and speech providers were both run for real.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::{
    Credentials, GeneratedMedia, GenerationRequest, JobStatus, MediaProvider, ProviderJob,
    client::send_with_retry, config::ProviderConfig, error::ProviderError,
};

/// Default clip length, in seconds.
const DEFAULT_SECONDS: u32 = 5;

/// Default frame size. The smallest supported, which is also the quickest.
const DEFAULT_WIDTH: u32 = 854;
/// Default frame height, paired with [`DEFAULT_WIDTH`].
const DEFAULT_HEIGHT: u32 = 480;

/// Frame sizes the API documents, in both orientations.
const SUPPORTED_SIZES: [(u32, u32); 6] = [
    (480, 480),
    (854, 480),
    (720, 720),
    (1280, 720),
    (1080, 1080),
    (1920, 1080),
];

/// Longest clip the API accepts, in seconds.
const MAX_SECONDS: u32 = 20;

/// How long to wait between polls.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Generates short video clips.
#[derive(Debug, Clone)]
pub struct VideoProvider {
    config: ProviderConfig,
    http: Client,
    credentials: Credentials,
}

impl VideoProvider {
    /// Builds the provider.
    pub fn new(config: ProviderConfig, http: Client, credentials: Credentials) -> Self {
        Self {
            config,
            http,
            credentials,
        }
    }

    /// Downloads a finished generation.
    async fn fetch_content(
        &self,
        generation_id: &str,
        width: Option<i32>,
        height: Option<i32>,
        seconds: Option<i32>,
    ) -> Result<GeneratedMedia, ProviderError> {
        let headers = self.credentials.headers().await?;
        let url = self
            .config
            .url(&format!("video/generations/{generation_id}/content/video"));

        let response = send_with_retry(
            || self.http.get(&url).headers(headers.clone()),
            generation_id,
            "video/content",
        )
        .await?;

        let bytes = response
            .bytes()
            .await
            .map_err(|error| ProviderError::Malformed(error.to_string()))?
            .to_vec();

        if bytes.is_empty() {
            return Err(ProviderError::Malformed("empty video response".to_owned()));
        }

        Ok(GeneratedMedia {
            bytes,
            content_type: "video/mp4".to_owned(),
            extension: "mp4",
            width,
            height,
            duration_ms: seconds.map(|s| s.saturating_mul(1000)),
        })
    }
}

/// A video generation job, as the API reports it.
#[derive(Debug, Deserialize)]
struct VideoJob {
    id: String,
    status: String,
    #[serde(default)]
    failure_reason: Option<String>,
    #[serde(default)]
    generations: Vec<VideoGeneration>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
    #[serde(default)]
    n_seconds: Option<i32>,
}

/// One produced video within a job.
#[derive(Debug, Deserialize)]
struct VideoGeneration {
    id: String,
}

#[async_trait]
impl MediaProvider for VideoProvider {
    async fn submit(&self, req: GenerationRequest) -> Result<ProviderJob, ProviderError> {
        let (width, height) = size(&req.parameters);
        let seconds = seconds(&req.parameters);

        let body = serde_json::json!({
            "model": self.config.video_deployment,
            "prompt": req.prompt,
            "width": width,
            "height": height,
            "n_seconds": seconds,
            "n_variants": 1,
        });

        let headers = self.credentials.headers().await?;
        let url = self.config.url("video/generations/jobs");

        tracing::info!(
            correlation_id = %req.correlation_id,
            deployment = %self.config.video_deployment,
            width, height, seconds,
            "submitting video generation"
        );

        let response = send_with_retry(
            || self.http.post(&url).headers(headers.clone()).json(&body),
            &req.correlation_id,
            "video/generations/jobs",
        )
        .await?;

        let job: VideoJob = response
            .json()
            .await
            .map_err(|error| ProviderError::Malformed(error.to_string()))?;

        tracing::info!(
            correlation_id = %req.correlation_id,
            provider_job_id = %job.id,
            "video job accepted"
        );

        Ok(ProviderJob::pending(job.id))
    }

    async fn poll(&self, job: &ProviderJob) -> Result<JobStatus, ProviderError> {
        let Some(job_id) = job.id.as_deref() else {
            return Err(ProviderError::Internal(
                "cannot poll a video job without an id".to_owned(),
            ));
        };

        let headers = self.credentials.headers().await?;
        let url = self.config.url(&format!("video/generations/jobs/{job_id}"));

        let response = send_with_retry(
            || self.http.get(&url).headers(headers.clone()),
            job_id,
            "video/generations/jobs/get",
        )
        .await?;

        let status: VideoJob = response
            .json()
            .await
            .map_err(|error| ProviderError::Malformed(error.to_string()))?;

        match classify(&status.status) {
            Progress::Running => Ok(JobStatus::Running),

            Progress::Succeeded => {
                let Some(generation) = status.generations.first() else {
                    return Ok(JobStatus::Failed(ProviderError::Malformed(
                        "job succeeded but produced no generation".to_owned(),
                    )));
                };
                let media = self
                    .fetch_content(
                        &generation.id,
                        status.width,
                        status.height,
                        status.n_seconds,
                    )
                    .await?;
                Ok(JobStatus::Succeeded(Box::new(media)))
            }

            Progress::Failed => {
                let reason = status.failure_reason.unwrap_or_default();
                // Sora reports a filtered prompt through failure_reason rather
                // than an HTTP status, so it is matched here too.
                let error = if reason.to_lowercase().contains("content") {
                    ProviderError::ContentFilter
                } else {
                    ProviderError::Upstream(reason)
                };
                Ok(JobStatus::Failed(error))
            }

            Progress::Cancelled => Ok(JobStatus::Failed(ProviderError::Upstream(
                "the provider cancelled the job".to_owned(),
            ))),
        }
    }
}

/// What a reported status means for us.
#[derive(Debug, PartialEq, Eq)]
enum Progress {
    /// Not finished; poll again.
    Running,
    /// Finished with a video.
    Succeeded,
    /// Finished without one.
    Failed,
    /// Stopped by the provider.
    Cancelled,
}

/// Maps a provider status string onto [`Progress`].
///
/// Unknown values are treated as still running. A new intermediate state added
/// by the service should keep the job waiting, not fail it.
fn classify(status: &str) -> Progress {
    match status.to_lowercase().as_str() {
        "succeeded" | "completed" => Progress::Succeeded,
        "failed" => Progress::Failed,
        "cancelled" | "canceled" => Progress::Cancelled,
        _ => Progress::Running,
    }
}

/// Picks a supported frame size from the job parameters.
fn size(parameters: &serde_json::Value) -> (u32, u32) {
    let requested = parameters
        .get("size")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.split_once('x'))
        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)));

    match requested {
        Some((width, height)) if SUPPORTED_SIZES.contains(&(width, height)) => (width, height),
        _ => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
    }
}

/// Picks a clip length within the documented range.
fn seconds(parameters: &serde_json::Value) -> u32 {
    parameters
        .get("n_seconds")
        .and_then(serde_json::Value::as_u64)
        .map_or(DEFAULT_SECONDS, |value| {
            (value as u32).clamp(1, MAX_SECONDS)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_statuses_are_recognised() {
        assert_eq!(classify("succeeded"), Progress::Succeeded);
        assert_eq!(classify("Succeeded"), Progress::Succeeded);
        assert_eq!(classify("failed"), Progress::Failed);
        assert_eq!(classify("cancelled"), Progress::Cancelled);
        assert_eq!(classify("canceled"), Progress::Cancelled);
    }

    #[test]
    fn an_unknown_status_keeps_the_job_waiting() {
        // A new intermediate state should not be read as a failure.
        assert_eq!(classify("queued"), Progress::Running);
        assert_eq!(classify("preprocessing"), Progress::Running);
        assert_eq!(classify("noe-helt-nytt"), Progress::Running);
    }

    #[test]
    fn only_documented_frame_sizes_are_sent() {
        assert_eq!(size(&serde_json::json!({"size": "1280x720"})), (1280, 720));
        assert_eq!(size(&serde_json::json!({"size": "720x1280"})), (854, 480));
        assert_eq!(size(&serde_json::json!({"size": "999x999"})), (854, 480));
        assert_eq!(size(&serde_json::json!({})), (854, 480));
    }

    #[test]
    fn the_clip_length_is_clamped_to_the_documented_range() {
        assert_eq!(seconds(&serde_json::json!({"n_seconds": 10})), 10);
        assert_eq!(seconds(&serde_json::json!({"n_seconds": 0})), 1);
        assert_eq!(seconds(&serde_json::json!({"n_seconds": 99})), MAX_SECONDS);
        assert_eq!(seconds(&serde_json::json!({})), DEFAULT_SECONDS);
    }
}
