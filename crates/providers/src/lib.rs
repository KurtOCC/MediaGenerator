//! Azure AI Foundry clients for image, audio and video generation.
//!
//! # Which API this targets
//!
//! Azure OpenAI now exposes a **v1 API** where the path is
//! `{endpoint}/openai/v1/...` and the deployment name travels in the request
//! body as `model`. The older shape —
//! `{endpoint}/openai/deployments/{deployment}/images/generations` with a dated
//! `api-version` — is the legacy one.
//!
//! Verified against Microsoft Learn on 2026-09-16 (*Azure OpenAI image, audio,
//! and video REST API reference (v1 preview)*, doc revision 2026-06-24), and
//! exercised for real against `mediagenerator-it-resource` in `swedencentral`:
//!
//! | Operation | Path |
//! | --- | --- |
//! | Image | `POST {endpoint}/openai/v1/images/generations` |
//! | Speech | `POST {endpoint}/openai/v1/audio/speech` |
//! | Video, create | `POST {endpoint}/openai/v1/video/generations/jobs` |
//! | Video, poll | `GET {endpoint}/openai/v1/video/generations/jobs/{id}` |
//! | Video, fetch | `GET {endpoint}/openai/v1/video/generations/{id}/content/video` |
//!
//! `api-version` is optional on the v1 API and defaults to `v1`; `preview` opts
//! into preview features. It stays configurable through
//! `AZURE_OPENAI_API_VERSION` rather than being hard-coded, and the value the
//! clients were tested with is `preview`.
//!
//! # Authentication
//!
//! An Entra ID token for `https://cognitiveservices.azure.com/.default`, from a
//! Managed Identity in Azure or the developer's `az login` session locally. See
//! [`auth::Credentials::new`] for the order. An API key is accepted as a
//! fallback and is refused when `APP_ENV=production`.

#![forbid(unsafe_code)]

pub mod audio;
pub mod auth;
pub mod client;
pub mod config;
pub mod error;
pub mod image;
pub mod video;

use async_trait::async_trait;
use mediagenerator_domain::MediaType;

pub use auth::Credentials;
pub use config::ProviderConfig;
pub use error::ProviderError;

/// What the user asked for.
#[derive(Debug, Clone)]
pub struct GenerationRequest {
    /// Which kind of media to produce.
    pub media_type: MediaType,
    /// The validated prompt.
    pub prompt: String,
    /// Provider parameters as stored on the job.
    pub parameters: serde_json::Value,
    /// Correlation ID of the request that started this, for log stitching.
    pub correlation_id: String,
}

/// Finished media, still in memory and not yet stored.
#[derive(Clone)]
pub struct GeneratedMedia {
    /// The bytes themselves.
    pub bytes: Vec<u8>,
    /// MIME type, used for the blob and the preview element.
    pub content_type: String,
    /// File extension, without the dot.
    pub extension: &'static str,
    /// Pixel width, for images and video.
    pub width: Option<i32>,
    /// Pixel height, for images and video.
    pub height: Option<i32>,
    /// Playing time, for audio and video.
    pub duration_ms: Option<i32>,
}

impl std::fmt::Debug for GeneratedMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bytes are megabytes of media; never print them.
        f.debug_struct("GeneratedMedia")
            .field("content_type", &self.content_type)
            .field("bytes", &format_args!("{} bytes", self.bytes.len()))
            .field("width", &self.width)
            .field("height", &self.height)
            .field("duration_ms", &self.duration_ms)
            .finish()
    }
}

/// A submitted piece of work.
///
/// Image and speech finish inside the request itself, so `media` is already
/// present and `id` is `None`. Video is genuinely asynchronous: `id` is the
/// identifier to poll and `media` stays `None` until it succeeds.
#[derive(Debug)]
pub struct ProviderJob {
    /// The provider's own job id, for asynchronous generation.
    pub id: Option<String>,
    /// The finished media, when generation completed during `submit`.
    pub media: Option<GeneratedMedia>,
}

impl ProviderJob {
    /// A job that finished immediately.
    pub fn immediate(media: GeneratedMedia) -> Self {
        Self {
            id: None,
            media: Some(media),
        }
    }

    /// A job the provider is still working on.
    pub fn pending(id: String) -> Self {
        Self {
            id: Some(id),
            media: None,
        }
    }

    /// Returns `true` when there is nothing left to poll for.
    pub const fn is_finished(&self) -> bool {
        self.media.is_some()
    }
}

/// Where a submitted piece of work has got to.
#[derive(Debug)]
pub enum JobStatus {
    /// Still being generated; poll again after a delay.
    Running,
    /// Finished, with the media attached.
    Succeeded(Box<GeneratedMedia>),
    /// Finished unsuccessfully.
    Failed(ProviderError),
}

/// A backend that can generate one kind of media.
#[async_trait]
pub trait MediaProvider: Send + Sync {
    /// Submits a request.
    ///
    /// # Errors
    ///
    /// Returns an error when the request is rejected or the call fails after
    /// its retries.
    async fn submit(&self, req: GenerationRequest) -> Result<ProviderJob, ProviderError>;

    /// Asks how a submitted job is getting on.
    ///
    /// # Errors
    ///
    /// Returns an error when the status could not be retrieved.
    async fn poll(&self, job: &ProviderJob) -> Result<JobStatus, ProviderError>;
}

/// The three providers, ready to use.
pub struct Providers {
    /// Still images.
    pub image: image::ImageProvider,
    /// Speech.
    pub audio: audio::AudioProvider,
    /// Video.
    pub video: video::VideoProvider,
}

impl std::fmt::Debug for Providers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Providers")
    }
}

impl Providers {
    /// Builds all three from one configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client or the credential cannot be built.
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderError> {
        let http = client::build_http_client()?;
        let credentials = Credentials::new(&config)?;

        Ok(Self {
            image: image::ImageProvider::new(config.clone(), http.clone(), credentials.clone()),
            audio: audio::AudioProvider::new(config.clone(), http.clone(), credentials.clone()),
            video: video::VideoProvider::new(config, http, credentials),
        })
    }

    /// Returns the provider for a media type.
    pub fn for_media(&self, media_type: MediaType) -> &dyn MediaProvider {
        match media_type {
            MediaType::Image => &self.image,
            MediaType::Audio => &self.audio,
            MediaType::Video => &self.video,
        }
    }
}
