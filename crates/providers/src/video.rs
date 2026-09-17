//! Video generation with Sora.
//!
//! The only genuinely asynchronous provider, and the one the `submit`/`poll`
//! split in [`MediaProvider`] exists for:
//!
//! 1. `POST {endpoint}/openai/v1/videos` creates the job,
//! 2. `GET  {endpoint}/openai/v1/videos/{id}` reports on it,
//! 3. `GET  {endpoint}/openai/v1/videos/{id}/content` fetches the finished file.
//!
//! # The documented path was wrong
//!
//! Microsoft Learn documents this as `/openai/v1/video/generations/jobs`, with
//! `width`, `height` and `n_seconds`. That route answers `404 Not Found` on a
//! live resource. The API that actually exists is the OpenAI-compatible one
//! above, taking `seconds` as a string and `size` as `WIDTHxHEIGHT`, and
//! returning `queued` / `in_progress` / `completed` / `failed` with a
//! `progress` percentage.
//!
//! Established by probing the resource on 2026-09-17 and confirmed end to end:
//! a four-second clip came back as a 4 MB MP4.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::{
    Credentials, GeneratedMedia, GenerationRequest, JobStatus, MediaProvider, ProviderJob,
    ReferenceImage, client::send_with_retry, config::ProviderConfig, error::ProviderError,
};

/// Default clip length, in seconds.
const DEFAULT_SECONDS: u32 = 4;

/// Clip lengths Sora 2 accepts.
const SUPPORTED_SECONDS: [u32; 3] = [4, 8, 12];

/// Default frame size: landscape, the most useful for an intranet clip.
const DEFAULT_SIZE: &str = "1280x720";

/// Frame sizes the API accepts.
const SUPPORTED_SIZES: [&str; 4] = ["1280x720", "720x1280", "1792x1024", "1024x1792"];

/// How long to wait between polls.
///
/// A four-second clip takes around a minute, so polling faster than this only
/// adds requests.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);

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

    /// Downloads a finished clip.
    async fn fetch_content(
        &self,
        video_id: &str,
        size: Option<&str>,
        seconds: Option<u32>,
    ) -> Result<GeneratedMedia, ProviderError> {
        let headers = self.credentials.headers().await?;
        let url = self.config.url(&format!("videos/{video_id}/content"));

        let response = send_with_retry(
            || self.http.get(&url).headers(headers.clone()),
            video_id,
            "videos/content",
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

        let (width, height) = size.map_or((None, None), split_size);

        Ok(GeneratedMedia {
            bytes,
            content_type: "video/mp4".to_owned(),
            extension: "mp4",
            width,
            height,
            duration_ms: seconds.map(|value| (value * 1000) as i32),
        })
    }
}

/// A video, as the API reports it.
#[derive(Debug, Deserialize)]
struct Video {
    id: String,
    status: String,
    #[serde(default)]
    error: Option<VideoError>,
    #[serde(default)]
    size: Option<String>,
    /// Sent as a string: `"4"`, `"8"` or `"12"`.
    #[serde(default)]
    seconds: Option<String>,
}

/// Why a video failed.
#[derive(Debug, Deserialize)]
struct VideoError {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[async_trait]
impl MediaProvider for VideoProvider {
    async fn submit(&self, req: GenerationRequest) -> Result<ProviderJob, ProviderError> {
        let size = size(&req.parameters);
        let seconds = seconds(&req.parameters);

        let headers = self.credentials.headers().await?;
        let url = self.config.url("videos");

        tracing::info!(
            correlation_id = %req.correlation_id,
            deployment = %self.config.video_deployment,
            size,
            seconds,
            reference = req.reference.is_some(),
            "submitting video generation"
        );

        // With a reference image the request becomes multipart and the image
        // is used as the clip's first frame — Sora continues from it rather
        // than treating it as a style reference.
        let response = match req.reference {
            Some(reference) => {
                self.submit_with_reference(
                    &req.prompt,
                    &req.correlation_id,
                    reference,
                    size,
                    seconds,
                    &url,
                    &headers,
                )
                .await?
            }
            None => {
                let body = serde_json::json!({
                    "model": self.config.video_deployment,
                    "prompt": req.prompt,
                    "seconds": seconds.to_string(),
                    "size": size,
                });

                send_with_retry(
                    || self.http.post(&url).headers(headers.clone()).json(&body),
                    &req.correlation_id,
                    "videos",
                )
                .await?
            }
        };

        let video: Video = response
            .json()
            .await
            .map_err(|error| ProviderError::Malformed(error.to_string()))?;

        tracing::info!(
            correlation_id = %req.correlation_id,
            provider_job_id = %video.id,
            "video job accepted"
        );

        Ok(ProviderJob::pending(video.id))
    }

    async fn poll(&self, job: &ProviderJob) -> Result<JobStatus, ProviderError> {
        let Some(video_id) = job.id.as_deref() else {
            return Err(ProviderError::Internal(
                "cannot poll a video without an id".to_owned(),
            ));
        };

        let headers = self.credentials.headers().await?;
        let url = self.config.url(&format!("videos/{video_id}"));

        let response = send_with_retry(
            || self.http.get(&url).headers(headers.clone()),
            video_id,
            "videos/get",
        )
        .await?;

        let video: Video = response
            .json()
            .await
            .map_err(|error| ProviderError::Malformed(error.to_string()))?;

        match classify(&video.status) {
            Progress::Running => Ok(JobStatus::Running),

            Progress::Succeeded => {
                let seconds = video
                    .seconds
                    .as_deref()
                    .and_then(|value| value.parse::<u32>().ok());
                let media = self
                    .fetch_content(video_id, video.size.as_deref(), seconds)
                    .await?;
                Ok(JobStatus::Succeeded(Box::new(media)))
            }

            Progress::Failed => {
                let detail = video.error.unwrap_or(VideoError {
                    code: None,
                    message: None,
                });
                let code = detail.code.unwrap_or_default();
                let message = detail.message.unwrap_or_default();

                // Sora reports a filtered prompt in the error body rather than
                // through an HTTP status, so it is matched here too.
                let combined = format!("{code} {message}").to_lowercase();
                let error = if combined.contains("content") || combined.contains("moderation") {
                    ProviderError::ContentFilter
                } else {
                    ProviderError::Upstream(message)
                };
                Ok(JobStatus::Failed(error))
            }
        }
    }
}

impl VideoProvider {
    /// Submits a clip that continues from an uploaded first frame.
    ///
    /// # Verification status
    ///
    /// The multipart shape follows the API that the JSON path was established
    /// against, but it has **not** been run: the plain path was verified end to
    /// end, this one was not.
    #[allow(clippy::too_many_arguments)]
    async fn submit_with_reference(
        &self,
        prompt: &str,
        correlation_id: &str,
        reference: ReferenceImage,
        size: &'static str,
        seconds: u32,
        url: &str,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<reqwest::Response, ProviderError> {
        send_with_retry(
            || {
                // Rebuilt per attempt: a multipart body cannot be cloned, and
                // send_with_retry may call this more than once.
                let part = reqwest::multipart::Part::bytes(reference.bytes.clone())
                    .file_name(reference.file_name.clone())
                    .mime_str(&reference.content_type)
                    .unwrap_or_else(|_| reqwest::multipart::Part::bytes(reference.bytes.clone()));

                let form = reqwest::multipart::Form::new()
                    .text("model", self.config.video_deployment.clone())
                    .text("prompt", prompt.to_owned())
                    .text("seconds", seconds.to_string())
                    .text("size", size)
                    .part("input_reference", part);

                self.http.post(url).headers(headers.clone()).multipart(form)
            },
            correlation_id,
            "videos/with-reference",
        )
        .await
    }
}

/// What a reported status means for us.
#[derive(Debug, PartialEq, Eq)]
enum Progress {
    /// Not finished; poll again.
    Running,
    /// Finished with a clip.
    Succeeded,
    /// Finished without one.
    Failed,
}

/// Maps a reported status onto [`Progress`].
///
/// Unknown values are treated as still running. A new intermediate state added
/// by the service should keep the job waiting, not fail it.
fn classify(status: &str) -> Progress {
    match status.to_lowercase().as_str() {
        "completed" | "succeeded" => Progress::Succeeded,
        "failed" | "cancelled" | "canceled" => Progress::Failed,
        _ => Progress::Running,
    }
}

/// Picks a supported frame size from the job parameters.
fn size(parameters: &serde_json::Value) -> &'static str {
    parameters
        .get("size")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| SUPPORTED_SIZES.iter().find(|size| **size == value).copied())
        .unwrap_or(DEFAULT_SIZE)
}

/// Picks a supported clip length.
///
/// Sora 2 accepts 4, 8 or 12 seconds and nothing between, so an unsupported
/// value falls back rather than being clamped to something it would reject.
fn seconds(parameters: &serde_json::Value) -> u32 {
    parameters
        .get("n_seconds")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32)
        .filter(|value| SUPPORTED_SECONDS.contains(value))
        .unwrap_or(DEFAULT_SECONDS)
}

/// Splits a `WIDTHxHEIGHT` size into its parts.
fn split_size(size: &str) -> (Option<i32>, Option<i32>) {
    let Some((width, height)) = size.split_once('x') else {
        return (None, None);
    };
    match (width.parse(), height.parse()) {
        (Ok(width), Ok(height)) => (Some(width), Some(height)),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_statuses_are_recognised() {
        assert_eq!(classify("completed"), Progress::Succeeded);
        assert_eq!(classify("Completed"), Progress::Succeeded);
        assert_eq!(classify("failed"), Progress::Failed);
        assert_eq!(classify("cancelled"), Progress::Failed);
    }

    #[test]
    fn an_unknown_status_keeps_the_job_waiting() {
        // These are the two the service actually reports while working.
        assert_eq!(classify("queued"), Progress::Running);
        assert_eq!(classify("in_progress"), Progress::Running);
        // A new intermediate state should not be read as a failure.
        assert_eq!(classify("noe-helt-nytt"), Progress::Running);
    }

    #[test]
    fn only_supported_frame_sizes_are_sent() {
        assert_eq!(size(&serde_json::json!({"size": "720x1280"})), "720x1280");
        assert_eq!(size(&serde_json::json!({"size": "1792x1024"})), "1792x1024");
        // 720x720 was offered by an earlier build; the API does not take it.
        assert_eq!(size(&serde_json::json!({"size": "720x720"})), DEFAULT_SIZE);
        assert_eq!(size(&serde_json::json!({})), DEFAULT_SIZE);
    }

    #[test]
    fn only_the_three_supported_lengths_are_sent() {
        assert_eq!(seconds(&serde_json::json!({"n_seconds": 4})), 4);
        assert_eq!(seconds(&serde_json::json!({"n_seconds": 8})), 8);
        assert_eq!(seconds(&serde_json::json!({"n_seconds": 12})), 12);
        // Not clamped: 6 is not a length Sora accepts, so it falls back to one
        // that is rather than being rounded into a rejection.
        assert_eq!(
            seconds(&serde_json::json!({"n_seconds": 6})),
            DEFAULT_SECONDS
        );
        assert_eq!(
            seconds(&serde_json::json!({"n_seconds": 20})),
            DEFAULT_SECONDS
        );
        assert_eq!(seconds(&serde_json::json!({})), DEFAULT_SECONDS);
    }

    #[test]
    fn a_size_splits_into_width_and_height() {
        assert_eq!(split_size("1280x720"), (Some(1280), Some(720)));
        assert_eq!(split_size("noe"), (None, None));
    }
}
