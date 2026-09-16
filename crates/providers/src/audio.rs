//! Text-to-speech.
//!
//! `POST {endpoint}/openai/v1/audio/speech`, which answers with the audio
//! bytes directly rather than JSON. Synchronous, like image generation.
//!
//! # Norwegian
//!
//! The OpenAI voices are multilingual and read Norwegian from the text itself —
//! there is no language parameter to set. The deployed `tts` model does not
//! accept `instructions`, so the voice is chosen by name and nothing else.
//!
//! # Swapping in Azure AI Speech
//!
//! Azure AI Speech would give real Norwegian neural voices (`nb-NO-FinnNeural`
//! and friends) and SSML. It is a different endpoint and a different request
//! format, so it belongs behind its own type implementing [`MediaProvider`],
//! selected by configuration. Nothing outside this module would change: the
//! trait is the seam.

use async_trait::async_trait;
use reqwest::Client;

use crate::{
    Credentials, GeneratedMedia, GenerationRequest, JobStatus, MediaProvider, ProviderJob,
    client::send_with_retry, config::ProviderConfig, error::ProviderError,
};

/// Voice used when the job names none.
const DEFAULT_VOICE: &str = "nova";

/// Voices the API accepts. An unknown name is a 400, so it is checked here.
const VOICES: [&str; 6] = ["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

/// Longest text the speech endpoint accepts, in characters.
const MAX_INPUT_CHARS: usize = 4096;

/// Generates speech.
#[derive(Debug, Clone)]
pub struct AudioProvider {
    config: ProviderConfig,
    http: Client,
    credentials: Credentials,
}

impl AudioProvider {
    /// Builds the provider.
    pub fn new(config: ProviderConfig, http: Client, credentials: Credentials) -> Self {
        Self {
            config,
            http,
            credentials,
        }
    }
}

#[async_trait]
impl MediaProvider for AudioProvider {
    async fn submit(&self, req: GenerationRequest) -> Result<ProviderJob, ProviderError> {
        // The application allows a longer prompt than this endpoint does, so
        // the limit is reported as a validation failure the user can act on
        // rather than being sent and bounced as a 400.
        if req.prompt.chars().count() > MAX_INPUT_CHARS {
            return Err(ProviderError::Validation(format!(
                "speech input is limited to {MAX_INPUT_CHARS} characters"
            )));
        }

        let voice = voice(&req.parameters);
        let body = serde_json::json!({
            "model": self.config.audio_deployment,
            "input": req.prompt,
            "voice": voice,
            "response_format": "mp3",
        });

        let headers = self.credentials.headers().await?;
        let url = self.config.url("audio/speech");

        tracing::info!(
            correlation_id = %req.correlation_id,
            deployment = %self.config.audio_deployment,
            voice,
            "submitting speech synthesis"
        );

        let response = send_with_retry(
            || self.http.post(&url).headers(headers.clone()).json(&body),
            &req.correlation_id,
            "audio/speech",
        )
        .await?;

        let bytes = response
            .bytes()
            .await
            .map_err(|error| ProviderError::Malformed(error.to_string()))?
            .to_vec();

        if bytes.is_empty() {
            return Err(ProviderError::Malformed("empty audio response".to_owned()));
        }

        tracing::info!(
            correlation_id = %req.correlation_id,
            bytes = bytes.len(),
            "speech generated"
        );

        Ok(ProviderJob::immediate(GeneratedMedia {
            bytes,
            content_type: "audio/mpeg".to_owned(),
            extension: "mp3",
            width: None,
            height: None,
            // The endpoint reports no duration and decoding the MP3 to find
            // out would cost more than the number is worth.
            duration_ms: None,
        }))
    }

    /// Never called: speech finishes during `submit`.
    async fn poll(&self, _job: &ProviderJob) -> Result<JobStatus, ProviderError> {
        Err(ProviderError::Internal(
            "speech synthesis is synchronous and has nothing to poll".to_owned(),
        ))
    }
}

/// Picks the voice, falling back when the job names an unknown one.
///
/// Silently correcting beats a 400: the voice is a preference, not the point of
/// the request.
fn voice(parameters: &serde_json::Value) -> &'static str {
    let requested = parameters.get("voice").and_then(serde_json::Value::as_str);

    requested
        .and_then(|name| VOICES.iter().find(|voice| **voice == name).copied())
        .unwrap_or(DEFAULT_VOICE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_voice_is_used() {
        assert_eq!(voice(&serde_json::json!({"voice": "fable"})), "fable");
    }

    #[test]
    fn an_unknown_or_missing_voice_falls_back() {
        assert_eq!(voice(&serde_json::json!({})), DEFAULT_VOICE);
        assert_eq!(voice(&serde_json::json!({"voice": "finn"})), DEFAULT_VOICE);
        assert_eq!(voice(&serde_json::json!({"voice": ""})), DEFAULT_VOICE);
    }

    #[test]
    fn every_listed_voice_is_accepted() {
        for name in VOICES {
            assert_eq!(voice(&serde_json::json!({ "voice": name })), name);
        }
    }
}
