//! Settings for the Azure AI Foundry clients.

/// Everything the three providers need to reach Azure.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Azure OpenAI endpoint, without a trailing slash.
    ///
    /// The `*.openai.azure.com` form, not the Foundry project endpoint: the
    /// image, speech and video operations live on the Azure OpenAI data plane.
    pub endpoint: String,
    /// Value of the `api-version` query parameter.
    pub api_version: String,
    /// Deployment name for image generation.
    pub image_deployment: String,
    /// Deployment name for text-to-speech.
    pub audio_deployment: String,
    /// Deployment name for video generation.
    pub video_deployment: String,
    /// Entra ID tenant, for the client-credentials flow.
    pub tenant_id: String,
    /// Application id, for the client-credentials flow.
    pub client_id: String,
    /// Client secret. Absent when running on a Managed Identity.
    pub client_secret: Option<String>,
    /// API key, accepted only outside production.
    pub api_key: Option<String>,
}

impl ProviderConfig {
    /// Builds the URL for a v1 data-plane operation.
    ///
    /// `api-version` is optional on the v1 API, so an empty setting means "send
    /// none" rather than "send an empty one".
    pub fn url(&self, path: &str) -> String {
        let base = format!(
            "{}/openai/v1/{}",
            self.endpoint,
            path.trim_start_matches('/')
        );
        if self.api_version.trim().is_empty() {
            base
        } else {
            format!("{base}?api-version={}", self.api_version)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(api_version: &str) -> ProviderConfig {
        ProviderConfig {
            endpoint: "https://example.openai.azure.com".to_owned(),
            api_version: api_version.to_owned(),
            image_deployment: "bilde".to_owned(),
            audio_deployment: "tale".to_owned(),
            video_deployment: "video".to_owned(),
            tenant_id: "t".to_owned(),
            client_id: "c".to_owned(),
            client_secret: None,
            api_key: None,
        }
    }

    #[test]
    fn urls_follow_the_v1_data_plane_shape() {
        assert_eq!(
            config("preview").url("images/generations"),
            "https://example.openai.azure.com/openai/v1/images/generations?api-version=preview"
        );
    }

    #[test]
    fn a_leading_slash_on_the_path_is_tolerated() {
        assert_eq!(
            config("preview").url("/audio/speech"),
            "https://example.openai.azure.com/openai/v1/audio/speech?api-version=preview"
        );
    }

    #[test]
    fn an_empty_api_version_sends_none() {
        assert_eq!(
            config("  ").url("images/generations"),
            "https://example.openai.azure.com/openai/v1/images/generations"
        );
    }
}
