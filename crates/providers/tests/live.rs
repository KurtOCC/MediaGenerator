//! Live tests against Azure AI Foundry.
//!
//! These cost real quota, so they run only when
//! `MEDIAGENERATOR_TEST_OPENAI_ENDPOINT` is set. They are what proves the
//! request shapes match the v1 API rather than the legacy deployment-scoped
//! one, and that the response really is base64 rather than a URL.
//!
//! Authentication follows the same chain as production: an Entra token from
//! `az login` locally, or the API key when one is configured.

use mediagenerator_domain::MediaType;
use mediagenerator_providers::{GenerationRequest, MediaProvider as _, ProviderConfig, Providers};

/// Environment variable naming the Azure OpenAI endpoint to test against.
const ENDPOINT_VAR: &str = "MEDIAGENERATOR_TEST_OPENAI_ENDPOINT";

/// Builds the providers, or returns `None` when unconfigured.
fn providers() -> Option<Providers> {
    let endpoint = std::env::var(ENDPOINT_VAR).ok()?;

    let config = ProviderConfig {
        endpoint,
        api_version: std::env::var("MEDIAGENERATOR_TEST_API_VERSION")
            .unwrap_or_else(|_| "preview".to_owned()),
        image_deployment: std::env::var("MEDIAGENERATOR_TEST_IMAGE_DEPLOYMENT")
            .unwrap_or_else(|_| "bilde".to_owned()),
        audio_deployment: std::env::var("MEDIAGENERATOR_TEST_AUDIO_DEPLOYMENT")
            .unwrap_or_else(|_| "tale".to_owned()),
        video_deployment: std::env::var("MEDIAGENERATOR_TEST_VIDEO_DEPLOYMENT")
            .unwrap_or_else(|_| "video".to_owned()),
        tenant_id: std::env::var("AZURE_TENANT_ID").unwrap_or_default(),
        client_id: std::env::var("AZURE_CLIENT_ID").unwrap_or_default(),
        // Deliberately not read from AZURE_CLIENT_SECRET: the app registration
        // is for signing users in, and has no data-plane access here.
        client_secret: None,
        api_key: std::env::var("MEDIAGENERATOR_TEST_OPENAI_KEY").ok(),
    };

    match Providers::new(config) {
        Ok(providers) => Some(providers),
        Err(error) => panic!("{ENDPOINT_VAR} is set but the providers could not be built: {error}"),
    }
}

/// Builds a request with the given prompt.
fn request(
    media_type: MediaType,
    prompt: &str,
    parameters: serde_json::Value,
) -> GenerationRequest {
    GenerationRequest {
        media_type,
        prompt: prompt.to_owned(),
        parameters,
        correlation_id: "live-test".to_owned(),
        reference: None,
    }
}

#[tokio::test]
async fn an_image_comes_back_as_png_bytes() {
    let Some(providers) = providers() else {
        eprintln!("skipped: {ENDPOINT_VAR} is not set");
        return;
    };

    let job = providers
        .image
        .submit(request(
            MediaType::Image,
            "Et enkelt blått bølgemotiv på mørk bakgrunn",
            // The cheapest and quickest combination: this test is about the
            // request shape, not about image quality.
            serde_json::json!({"size": "1024x1024", "quality": "low"}),
        ))
        .await
        .expect("image generation should succeed");

    // Synchronous: nothing left to poll for.
    assert!(job.is_finished());

    let media = job.media.expect("the image should be attached");
    assert_eq!(media.content_type, "image/png");
    assert_eq!(media.extension, "png");
    assert_eq!(media.width, Some(1024));
    assert_eq!(media.height, Some(1024));

    // A real PNG, not a URL or an error page that happened to decode.
    assert_eq!(
        &media.bytes[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        "expected a PNG signature"
    );
}

#[tokio::test]
async fn norwegian_speech_comes_back_as_mp3_bytes() {
    let Some(providers) = providers() else {
        eprintln!("skipped: {ENDPOINT_VAR} is not set");
        return;
    };

    let job = providers
        .audio
        .submit(request(
            MediaType::Audio,
            "Hei, og velkommen til Oslofjord. Dette er en test av norsk tale.",
            serde_json::json!({"voice": "nova"}),
        ))
        .await
        .expect("speech synthesis should succeed");

    let media = job.media.expect("the audio should be attached");
    assert_eq!(media.content_type, "audio/mpeg");
    assert_eq!(media.extension, "mp3");
    assert!(media.bytes.len() > 1000, "suspiciously short audio");

    // MP3 starts with an ID3 tag or a frame sync.
    let starts_with_id3 = media.bytes.starts_with(b"ID3");
    let starts_with_frame = media.bytes.first() == Some(&0xff);
    assert!(
        starts_with_id3 || starts_with_frame,
        "expected MP3 bytes, got {:?}",
        &media.bytes[..4.min(media.bytes.len())]
    );
}

#[tokio::test]
async fn an_unknown_voice_is_corrected_rather_than_rejected() {
    let Some(providers) = providers() else {
        eprintln!("skipped: {ENDPOINT_VAR} is not set");
        return;
    };

    // The API would answer 400 for this voice; the provider substitutes the
    // default instead, because the voice is a preference and not the request.
    let job = providers
        .audio
        .submit(request(
            MediaType::Audio,
            "Kort test.",
            serde_json::json!({"voice": "finn-neural"}),
        ))
        .await
        .expect("an unknown voice should not fail the request");

    assert!(job.media.is_some_and(|media| !media.bytes.is_empty()));
}

#[tokio::test]
async fn a_prompt_over_the_speech_limit_is_refused_without_calling_azure() {
    let Some(providers) = providers() else {
        eprintln!("skipped: {ENDPOINT_VAR} is not set");
        return;
    };

    // The application allows a longer prompt than the speech endpoint does.
    // Catching it here turns a provider 400 into a message the user can act on.
    let long = "a".repeat(5000);
    let error = providers
        .audio
        .submit(request(MediaType::Audio, &long, serde_json::json!({})))
        .await
        .expect_err("an over-long prompt should be refused");

    assert!(
        matches!(
            error,
            mediagenerator_providers::ProviderError::Validation(_)
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_reference_image_turns_the_call_into_an_edit() {
    let Some(providers) = providers() else {
        eprintln!("skipped: {ENDPOINT_VAR} is not set");
        return;
    };

    // A 1x1 PNG is enough: this test is about the multipart request shape and
    // the edits endpoint, not about what comes out the other end.
    const TINY_PNG: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    let mut request = request(
        MediaType::Image,
        "Gjør bakgrunnen mørkeblå",
        serde_json::json!({"size": "1024x1024", "quality": "low"}),
    );
    request.reference = Some(mediagenerator_providers::ReferenceImage {
        bytes: TINY_PNG.to_vec(),
        content_type: "image/png".to_owned(),
        file_name: "referanse.png".to_owned(),
    });

    let job = providers
        .image
        .submit(request)
        .await
        .expect("an image edit should succeed");

    let media = job.media.expect("the edited image should be attached");
    assert_eq!(media.content_type, "image/png");
    assert_eq!(
        &media.bytes[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        "expected a PNG signature"
    );
    // Far larger than the 67-byte input, so it is genuinely a new image.
    assert!(
        media.bytes.len() > 10_000,
        "got {} bytes",
        media.bytes.len()
    );
}
