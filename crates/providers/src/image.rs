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
    ReferenceImage, client::send_with_retry, config::ProviderConfig, error::ProviderError,
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

        // With a reference image this becomes an *edit* rather than a
        // generation: a different endpoint, and multipart instead of JSON.
        // The prompt then describes what to do with the image rather than
        // what to draw from nothing.
        if let Some(reference) = req.reference {
            return self
                .edit(&req.prompt, &req.correlation_id, reference, &size, &quality)
                .await;
        }

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

        let bytes = decode_first_image(response).await?;

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

impl ImageProvider {
    /// Generates from a prompt *and* a reference image.
    ///
    /// `POST {endpoint}/openai/v1/images/edits`, multipart. The response
    /// shape is the same as a plain generation, so the parsing is shared.
    async fn edit(
        &self,
        prompt: &str,
        correlation_id: &str,
        reference: ReferenceImage,
        size: &str,
        quality: &str,
    ) -> Result<ProviderJob, ProviderError> {
        let headers = self.credentials.headers().await?;
        let url = self.config.url("images/edits");

        tracing::info!(
            correlation_id,
            deployment = %self.config.image_deployment,
            size,
            quality,
            reference_bytes = reference.bytes.len(),
            "submitting image edit from a reference"
        );

        let response = send_with_retry(
            || {
                // Rebuilt per attempt: a multipart body cannot be cloned, and
                // send_with_retry may call this more than once.
                let part = reqwest::multipart::Part::bytes(reference.bytes.clone())
                    .file_name(reference.file_name.clone())
                    .mime_str(&reference.content_type)
                    .unwrap_or_else(|_| reqwest::multipart::Part::bytes(reference.bytes.clone()));

                let form = reqwest::multipart::Form::new()
                    .text("model", self.config.image_deployment.clone())
                    .text("prompt", prompt.to_owned())
                    .text("n", "1")
                    .text("size", size.to_owned())
                    .text("quality", quality.to_owned())
                    .part("image", part);

                self.http
                    .post(&url)
                    .headers(headers.clone())
                    .multipart(form)
            },
            correlation_id,
            "images/edits",
        )
        .await?;

        let bytes = decode_first_image(response).await?;
        let (width, height) = parse_size(size);

        tracing::info!(correlation_id, bytes = bytes.len(), "image edited");

        Ok(ProviderJob::immediate(GeneratedMedia {
            bytes,
            content_type: "image/png".to_owned(),
            extension: "png",
            width,
            height,
            duration_ms: None,
        }))
    }
}

/// Reads the first image out of an images response and decodes it.
///
/// Shared by generation and editing: both answer with the same shape.
async fn decode_first_image(response: reqwest::Response) -> Result<Vec<u8>, ProviderError> {
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

    STANDARD
        .decode(encoded)
        .map_err(|error| ProviderError::Malformed(format!("image was not valid base64: {error}")))
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
