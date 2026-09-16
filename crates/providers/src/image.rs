//! Image generation.
//!
//! `POST {endpoint}/openai/v1/images/generations`. Synchronous: the call
//! returns the finished image, so [`MediaProvider::submit`] does the work and
//! [`MediaProvider::poll`] has nothing left to do.
//!
//! The `gpt-image-1` series always answers with base64 and ignores
//! `response_format`, so no URL is ever handled here. That is also what we
//! want: the bytes go straight to our own private container, and the browser
//! never sees a provider URL.

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::Client;
use serde::Deserialize;

use crate::{
    Credentials, GeneratedMedia, GenerationRequest, JobStatus, MediaProvider, ProviderJob,
    client::send_with_retry, config::ProviderConfig, error::ProviderError,
};

/// Default image size when the job carries no preference.
const DEFAULT_SIZE: &str = "1024x1024";

/// Default quality. `medium` is the compromise between wait and detail.
const DEFAULT_QUALITY: &str = "medium";

/// Generates still images.
#[derive(Debug, Clone)]
pub struct ImageProvider {
    config: ProviderConfig,
    http: Client,
    credentials: Credentials,
}

impl ImageProvider {
    /// Builds the provider.
    pub fn new(config: ProviderConfig, http: Client, credentials: Credentials) -> Self {
        Self {
            config,
            http,
            credentials,
        }
    }
}

/// The subset of the response we read.
#[derive(Debug, Deserialize)]
struct ImagesResponse {
    data: Vec<ImageData>,
}

/// One generated image.
#[derive(Debug, Deserialize)]
struct ImageData {
    /// Base64-encoded bytes. Always present for the `gpt-image-1` series.
    #[serde(default)]
    b64_json: Option<String>,
    /// Present for `dall-e-*`, which returns a short-lived URL instead.
    #[serde(default)]
    url: Option<String>,
}

#[async_trait]
impl MediaProvider for ImageProvider {
    async fn submit(&self, req: GenerationRequest) -> Result<ProviderJob, ProviderError> {
        let size = string_param(&req.parameters, "size", DEFAULT_SIZE);
        let quality = string_param(&req.parameters, "quality", DEFAULT_QUALITY);

        let body = serde_json::json!({
            "model": self.config.image_deployment,
            "prompt": req.prompt,
            "n": 1,
            "size": size,
            "quality": quality,
            "output_format": "png",
        });

        let headers = self.credentials.headers().await?;
        let url = self.config.url("images/generations");

        // The prompt is not logged: it is user content, and the correlation id
        // is enough to tie this to the job row that does hold it.
        tracing::info!(
            correlation_id = %req.correlation_id,
            deployment = %self.config.image_deployment,
            size, quality,
            "submitting image generation"
        );

        let response = send_with_retry(
            || self.http.post(&url).headers(headers.clone()).json(&body),
            &req.correlation_id,
            "images/generations",
        )
        .await?;

        let parsed: ImagesResponse = response
            .json()
            .await
            .map_err(|error| ProviderError::Malformed(error.to_string()))?;

        let first = parsed
            .data
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Malformed("no image in the response".to_owned()))?;

        let Some(encoded) = first.b64_json else {
            // A deployment configured to return URLs would need a second fetch
            // before the bytes could be stored. Refusing loudly beats silently
            // handing the browser a provider URL.
            let hint = if first.url.is_some() {
                "the deployment returned a URL; expected base64"
            } else {
                "the response carried neither b64_json nor url"
            };
            return Err(ProviderError::Malformed(hint.to_owned()));
        };

        let bytes = STANDARD.decode(encoded).map_err(|error| {
            ProviderError::Malformed(format!("image was not valid base64: {error}"))
        })?;

        let (width, height) = parse_size(&size);

        tracing::info!(
            correlation_id = %req.correlation_id,
            bytes = bytes.len(),
            "image generated"
        );

        Ok(ProviderJob::immediate(GeneratedMedia {
            bytes,
            content_type: "image/png".to_owned(),
            extension: "png",
            width,
            height,
            duration_ms: None,
        }))
    }

    /// Never called: [`ProviderJob::is_finished`] is already true after
    /// `submit`, so the worker has nothing to poll for.
    async fn poll(&self, _job: &ProviderJob) -> Result<JobStatus, ProviderError> {
        Err(ProviderError::Internal(
            "image generation is synchronous and has nothing to poll".to_owned(),
        ))
    }
}

/// Reads a string parameter from the job's stored parameters.
fn string_param(parameters: &serde_json::Value, key: &str, default: &str) -> String {
    parameters
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_owned()
}

/// Splits a `WIDTHxHEIGHT` size into its parts.
///
/// Returns `(None, None)` for `auto`, where the model picks and the actual
/// dimensions are not known from the request.
fn parse_size(size: &str) -> (Option<i32>, Option<i32>) {
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
    fn a_size_is_split_into_width_and_height() {
        assert_eq!(parse_size("1024x1024"), (Some(1024), Some(1024)));
        assert_eq!(parse_size("1536x1024"), (Some(1536), Some(1024)));
    }

    #[test]
    fn an_unparseable_size_yields_no_dimensions() {
        assert_eq!(parse_size("auto"), (None, None));
        assert_eq!(parse_size("stor x liten"), (None, None));
    }

    #[test]
    fn parameters_fall_back_to_the_default() {
        let empty = serde_json::json!({});
        assert_eq!(string_param(&empty, "size", DEFAULT_SIZE), DEFAULT_SIZE);

        let given = serde_json::json!({"size": "1536x1024"});
        assert_eq!(string_param(&given, "size", DEFAULT_SIZE), "1536x1024");

        // An empty string is a missing value, not a valid one.
        let blank = serde_json::json!({"size": ""});
        assert_eq!(string_param(&blank, "size", DEFAULT_SIZE), DEFAULT_SIZE);
    }
}
