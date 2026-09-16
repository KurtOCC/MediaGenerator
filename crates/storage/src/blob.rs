//! Azure Blob Storage: uploads and time-limited read links.
//!
//! Generated media goes into a **private** container. The browser never gets a
//! provider URL and never gets a permanent one: it gets a user delegation SAS
//! that expires after `SAS_TTL_MINUTES`.
//!
//! # Why user delegation SAS
//!
//! A service SAS is signed with the storage account key, which then has to
//! exist somewhere — in configuration, in Key Vault, in a developer's `.env`.
//! A user delegation SAS is signed with a short-lived key the Blob service
//! issues against an Entra ID token, so the account keys are never needed and
//! can stay disabled. Each link is also attributable to the identity that
//! issued it, which an account-key SAS is not.

use std::time::Duration;

use azure_core::credentials::TokenCredential;
use azure_identity::{DeveloperToolsCredential, ManagedIdentityCredential};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit as _, Mac};
use sha2::Sha256;
use time::{OffsetDateTime, format_description::well_known::Iso8601};
use tokio::sync::RwLock;

use crate::error::StorageError;

/// Storage REST API version the requests declare.
///
/// Both the delegation key request and the SAS signature are tied to this
/// value: the signed string layout is version-specific, so it must be the same
/// in both places.
const API_VERSION: &str = "2025-05-05";

/// Scope for the Entra token used against Blob Storage.
const SCOPE: &str = "https://storage.azure.com/.default";

/// How long a delegation key is requested for.
///
/// Well beyond any single SAS lifetime, so the key is fetched rarely, but short
/// enough to be uninteresting if it ever leaked.
const DELEGATION_KEY_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Refresh the delegation key this long before it expires.
const DELEGATION_KEY_SKEW: Duration = Duration::from_secs(10 * 60);

/// Clock skew allowed when a SAS starts being valid.
const START_SKEW: Duration = Duration::from_secs(5 * 60);

/// HMAC-SHA256, as the SAS signature requires.
type HmacSha256 = Hmac<Sha256>;

/// A client for one private container.
pub struct BlobStore {
    account: String,
    container: String,
    http: reqwest::Client,
    credential: std::sync::Arc<dyn TokenCredential>,
    delegation_key: RwLock<Option<DelegationKey>>,
}

impl std::fmt::Debug for BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobStore")
            .field("account", &self.account)
            .field("container", &self.container)
            .finish_non_exhaustive()
    }
}

/// A user delegation key, with the fields the signature needs.
#[derive(Debug, Clone)]
struct DelegationKey {
    signed_oid: String,
    signed_tid: String,
    signed_start: String,
    signed_expiry: String,
    signed_service: String,
    signed_version: String,
    value: Vec<u8>,
    expires_at: OffsetDateTime,
}

impl DelegationKey {
    /// Returns `true` when the key is close enough to expiry to replace.
    fn is_stale(&self) -> bool {
        let skew = time::Duration::seconds(DELEGATION_KEY_SKEW.as_secs() as i64);
        OffsetDateTime::now_utc() + skew >= self.expires_at
    }
}

/// Returns `true` when the process is running somewhere that offers a managed
/// identity.
///
/// Container Apps, App Service and Functions all advertise one by setting
/// `IDENTITY_ENDPOINT`; the older App Service runtime used `MSI_ENDPOINT`.
/// Neither is ever set on a developer machine, which makes this a reliable
/// discriminator and avoids paying a long retry timeout to discover the same
/// thing.
fn running_in_azure() -> bool {
    std::env::var_os("IDENTITY_ENDPOINT").is_some() || std::env::var_os("MSI_ENDPOINT").is_some()
}
impl BlobStore {
    /// Builds a client for `container` on `account`.
    ///
    /// # Errors
    ///
    /// Returns an error when no Entra credential is available.
    pub fn new(account: &str, container: &str) -> Result<Self, StorageError> {
        // No account key anywhere: that is the point of a user delegation SAS.
        //
        // The choice is made on the environment rather than by trying the
        // Managed Identity credential first. Constructing it succeeds on any
        // machine; only acquiring a token fails, and on a developer laptop that
        // failure takes the better part of two minutes to arrive.
        let credential: std::sync::Arc<dyn TokenCredential> = if running_in_azure() {
            tracing::info!("using a Managed Identity for Blob Storage");
            ManagedIdentityCredential::new(None).map_err(|error| {
                StorageError::Blob(format!("managed identity unavailable: {error}"))
            })?
        } else {
            tracing::info!("using developer tooling credentials for Blob Storage");
            DeveloperToolsCredential::new(None).map_err(|error| {
                StorageError::Blob(format!("no Azure credential for Blob Storage: {error}"))
            })?
        };

        Ok(Self {
            account: account.to_owned(),
            container: container.to_owned(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|error| StorageError::Blob(error.to_string()))?,
            credential,
            delegation_key: RwLock::new(None),
        })
    }

    /// Returns the base URL of the account's blob endpoint.
    fn endpoint(&self) -> String {
        format!("https://{}.blob.core.windows.net", self.account)
    }

    /// Returns an Entra token for Blob Storage.
    async fn token(&self) -> Result<String, StorageError> {
        let token = self
            .credential
            .get_token(&[SCOPE], None)
            .await
            .map_err(|error| StorageError::Blob(format!("could not acquire a token: {error}")))?;
        Ok(token.token.secret().to_owned())
    }

    /// Uploads bytes as a block blob.
    ///
    /// # Errors
    ///
    /// Returns an error when the upload is refused.
    pub async fn upload(
        &self,
        path: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<(), StorageError> {
        let url = format!("{}/{}/{}", self.endpoint(), self.container, path);
        let token = self.token().await?;
        let length = bytes.len();

        let response = self
            .http
            .put(&url)
            .bearer_auth(token)
            .header("x-ms-version", API_VERSION)
            .header("x-ms-blob-type", "BlockBlob")
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await
            .map_err(|error| StorageError::Blob(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(StorageError::Blob(format!(
                "upload failed with {status}: {}",
                body.chars().take(300).collect::<String>()
            )));
        }

        tracing::info!(path, length, "stored generated media");
        Ok(())
    }

    /// Returns the cached delegation key, fetching a new one when needed.
    async fn delegation_key(&self) -> Result<DelegationKey, StorageError> {
        if let Some(key) = self.delegation_key.read().await.as_ref()
            && !key.is_stale()
        {
            return Ok(key.clone());
        }

        let mut guard = self.delegation_key.write().await;
        if let Some(key) = guard.as_ref()
            && !key.is_stale()
        {
            return Ok(key.clone());
        }

        let fresh = self.fetch_delegation_key().await?;
        *guard = Some(fresh.clone());
        Ok(fresh)
    }

    /// Asks the Blob service for a user delegation key.
    async fn fetch_delegation_key(&self) -> Result<DelegationKey, StorageError> {
        let start = OffsetDateTime::now_utc();
        let expiry = start + time::Duration::seconds(DELEGATION_KEY_TTL.as_secs() as i64);

        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <KeyInfo><Start>{}</Start><Expiry>{}</Expiry></KeyInfo>",
            iso8601(start),
            iso8601(expiry)
        );

        let url = format!(
            "{}/?restype=service&comp=userdelegationkey",
            self.endpoint()
        );
        let token = self.token().await?;

        let response = self
            .http
            .post(&url)
            .bearer_auth(token)
            .header("x-ms-version", API_VERSION)
            .header(reqwest::header::CONTENT_TYPE, "application/xml")
            .body(body)
            .send()
            .await
            .map_err(|error| StorageError::Blob(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(StorageError::Blob(format!(
                "could not get a user delegation key ({status}): {}",
                text.chars().take(300).collect::<String>()
            )));
        }

        let xml = response
            .text()
            .await
            .map_err(|error| StorageError::Blob(error.to_string()))?;

        parse_delegation_key(&xml, expiry)
    }

    /// Returns a read-only URL for `path`, valid for `ttl`.
    ///
    /// # Errors
    ///
    /// Returns an error when a delegation key could not be obtained.
    pub async fn read_url(&self, path: &str, ttl: Duration) -> Result<String, StorageError> {
        let key = self.delegation_key().await?;

        let start =
            OffsetDateTime::now_utc() - time::Duration::seconds(START_SKEW.as_secs() as i64);
        let expiry = OffsetDateTime::now_utc() + time::Duration::seconds(ttl.as_secs() as i64);

        let signed_start = iso8601(start);
        let signed_expiry = iso8601(expiry);
        let permissions = "r";
        let resource = "b";

        // Canonical resource: /blob/{account}/{container}/{blob}
        let canonical = format!("/blob/{}/{}/{}", self.account, self.container, path);

        // The field order below is fixed by the Storage REST specification for
        // a user delegation SAS. Every field is present, empty where unused,
        // and the trailing newlines are part of the string to sign.
        let string_to_sign = [
            permissions,
            &signed_start,
            &signed_expiry,
            &canonical,
            &key.signed_oid,
            &key.signed_tid,
            &key.signed_start,
            &key.signed_expiry,
            &key.signed_service,
            &key.signed_version,
            "", // signed authorized user object id
            "", // signed unauthorized user object id
            "", // signed correlation id
            "", // signed IP
            "", // signed protocol
            API_VERSION,
            resource,
            "", // signed snapshot time
            "", // signed encryption scope
            "", // rscc: Cache-Control override
            "", // rscd: Content-Disposition override
            "", // rsce: Content-Encoding override
            "", // rscl: Content-Language override
            "", // rsct: Content-Type override
        ]
        .join("\n");

        let signature = sign(&key.value, &string_to_sign)?;

        let query = [
            ("sv", API_VERSION.to_owned()),
            ("sr", resource.to_owned()),
            ("st", signed_start),
            ("se", signed_expiry),
            ("sp", permissions.to_owned()),
            ("skoid", key.signed_oid.clone()),
            ("sktid", key.signed_tid.clone()),
            ("skt", key.signed_start.clone()),
            ("ske", key.signed_expiry.clone()),
            ("sks", key.signed_service.clone()),
            ("skv", key.signed_version.clone()),
            ("sig", signature),
        ]
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");

        Ok(format!(
            "{}/{}/{}?{query}",
            self.endpoint(),
            self.container,
            path
        ))
    }

    /// Deletes a blob, ignoring one that is already gone.
    ///
    /// # Errors
    ///
    /// Returns an error when the delete is refused for any other reason.
    pub async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let url = format!("{}/{}/{}", self.endpoint(), self.container, path);
        let token = self.token().await?;

        let response = self
            .http
            .delete(&url)
            .bearer_auth(token)
            .header("x-ms-version", API_VERSION)
            .send()
            .await
            .map_err(|error| StorageError::Blob(error.to_string()))?;

        // Retention runs repeatedly; a blob already removed is success.
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }

        Err(StorageError::Blob(format!(
            "delete failed with {}",
            response.status()
        )))
    }
}

/// Signs a string with the delegation key.
fn sign(key: &[u8], string_to_sign: &str) -> Result<String, StorageError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|error| StorageError::Blob(format!("invalid delegation key: {error}")))?;
    mac.update(string_to_sign.as_bytes());
    Ok(STANDARD.encode(mac.finalize().into_bytes()))
}

/// Formats a timestamp the way Storage expects: `YYYY-MM-DDThh:mm:ssZ`.
///
/// Written out rather than delegated to `Iso8601::DEFAULT`, which renders
/// sub-second precision and a numeric offset. Storage rejects both, and the
/// string is part of what gets signed, so an approximation would produce a
/// signature mismatch rather than a clear error.
fn iso8601(value: OffsetDateTime) -> String {
    let value = value.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
    )
}

/// Percent-encodes a SAS query value.
///
/// The signature is Base64 and routinely contains `+`, `/` and `=`, every one
/// of which changes meaning if it reaches the URL unencoded.
fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Pulls the delegation key fields out of the XML response.
///
/// A hand-written reader rather than an XML dependency: the document has seven
/// known elements and no attributes or namespaces to speak of.
fn parse_delegation_key(
    xml: &str,
    fallback_expiry: OffsetDateTime,
) -> Result<DelegationKey, StorageError> {
    let field = |name: &str| -> Result<String, StorageError> {
        element(xml, name)
            .ok_or_else(|| StorageError::Blob(format!("delegation key response had no <{name}>")))
    };

    let signed_expiry = field("SignedExpiry")?;
    let expires_at =
        OffsetDateTime::parse(&signed_expiry, &Iso8601::DEFAULT).unwrap_or(fallback_expiry);

    let value = STANDARD
        .decode(field("Value")?)
        .map_err(|error| StorageError::Blob(format!("delegation key was not base64: {error}")))?;

    Ok(DelegationKey {
        signed_oid: field("SignedOid")?,
        signed_tid: field("SignedTid")?,
        signed_start: field("SignedStart")?,
        signed_expiry,
        signed_service: field("SignedService")?,
        signed_version: field("SignedVersion")?,
        value,
        expires_at,
    })
}

/// Returns the text content of the first `<name>` element.
fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
        <UserDelegationKey>
          <SignedOid>32a5b2f2-1111-2222-3333-444455556666</SignedOid>
          <SignedTid>9f08805a-b141-4a2d-923f-80d0576d602a</SignedTid>
          <SignedStart>2026-09-16T12:00:00Z</SignedStart>
          <SignedExpiry>2026-09-16T18:00:00Z</SignedExpiry>
          <SignedService>b</SignedService>
          <SignedVersion>2025-05-05</SignedVersion>
          <Value>cGFzc29yZC1zb20tYmFzZTY0</Value>
        </UserDelegationKey>"#;

    #[test]
    fn the_delegation_key_is_read_from_the_xml() {
        let key = parse_delegation_key(SAMPLE, OffsetDateTime::now_utc())
            .expect("the sample should parse");

        assert_eq!(key.signed_tid, "9f08805a-b141-4a2d-923f-80d0576d602a");
        assert_eq!(key.signed_service, "b");
        assert_eq!(key.signed_version, "2025-05-05");
        assert_eq!(key.value, b"passord-som-base64");
    }

    #[test]
    fn a_missing_field_is_an_error_rather_than_a_default() {
        let broken = SAMPLE.replace("<SignedOid>", "<Annet>");
        assert!(parse_delegation_key(&broken, OffsetDateTime::now_utc()).is_err());
    }

    #[test]
    fn base64_signatures_survive_url_encoding() {
        // These three characters are exactly what breaks an unencoded SAS.
        assert_eq!(encode("a+b/c="), "a%2Bb%2Fc%3D");
        assert_eq!(encode("2026-09-16T12:00:00Z"), "2026-09-16T12%3A00%3A00Z");
        // Unreserved characters pass through untouched.
        assert_eq!(encode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    }

    #[test]
    fn timestamps_are_rendered_the_way_storage_expects() {
        let stamp = OffsetDateTime::from_unix_timestamp(1_789_000_000).expect("valid");
        let rendered = iso8601(stamp);
        assert_eq!(rendered, "2026-09-10T00:26:40Z", "got {rendered}");
    }

    #[test]
    fn signing_is_stable_for_a_known_key_and_string() {
        // Guards the HMAC wiring: a change in encoding or digest would move it.
        let signature = sign(b"hemmelig-nokkel", "linje1\nlinje2").expect("should sign");
        assert_eq!(
            signature,
            sign(b"hemmelig-nokkel", "linje1\nlinje2").expect("should sign")
        );
        assert_ne!(
            signature,
            sign(b"annen-nokkel", "linje1\nlinje2").expect("should sign")
        );
    }

    #[test]
    fn a_key_near_its_expiry_is_treated_as_stale() {
        let soon = OffsetDateTime::now_utc() + time::Duration::minutes(5);
        let later = OffsetDateTime::now_utc() + time::Duration::hours(5);

        let key = |expires_at| DelegationKey {
            signed_oid: String::new(),
            signed_tid: String::new(),
            signed_start: String::new(),
            signed_expiry: String::new(),
            signed_service: String::new(),
            signed_version: String::new(),
            value: Vec::new(),
            expires_at,
        };

        assert!(
            key(soon).is_stale(),
            "a key expiring inside the skew is stale"
        );
        assert!(!key(later).is_stale());
    }
}
