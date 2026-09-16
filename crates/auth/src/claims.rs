//! Entra ID specific ID token claims and the concrete OpenID Connect types
//! built on top of them.
//!
//! `openidconnect` is generic over the provider's extra claims. Entra ID adds
//! `oid`, `roles`, `groups`, `upn` and `tid` on top of the standard OIDC claim
//! set, so the generic parameters are pinned here once and reused everywhere
//! else in the crate.

use openidconnect::{
    Client, EmptyExtraTokenFields, EndpointMaybeSet, EndpointNotSet, EndpointSet, IdToken,
    IdTokenClaims, IdTokenFields, StandardErrorResponse, StandardTokenResponse,
    core::{
        CoreAuthDisplay, CoreAuthPrompt, CoreErrorResponseType, CoreGenderClaim, CoreJsonWebKey,
        CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm, CoreRevocableToken,
        CoreRevocationErrorResponse, CoreTokenIntrospectionResponse, CoreTokenType,
    },
};
use serde::{Deserialize, Serialize};

/// Claims Microsoft Entra ID adds beyond the standard OpenID Connect set.
///
/// All fields are optional: which ones Entra ID actually emits depends on the
/// app registration's token configuration and on whether the user has been
/// assigned an app role.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntraClaims {
    /// Immutable object ID of the user in the directory.
    ///
    /// Unlike `sub`, which is pairwise per application, `oid` identifies the
    /// same user across every application in the tenant. This is what the
    /// `users.entra_oid` column stores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oid: Option<String>,

    /// App roles assigned to the user for this application.
    ///
    /// Present only when the user or one of their groups is assigned a role.
    #[serde(default)]
    pub roles: Vec<String>,

    /// Group object IDs, when the app registration emits a groups claim.
    #[serde(default)]
    pub groups: Vec<String>,

    /// User principal name, typically the work e-mail address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upn: Option<String>,

    /// Tenant ID the token was issued for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tid: Option<String>,
}

impl openidconnect::AdditionalClaims for EntraClaims {}

/// ID token as issued by Entra ID.
pub type EntraIdToken = IdToken<
    EntraClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

/// Verified claims of an Entra ID token.
pub type EntraIdTokenClaims = IdTokenClaims<EntraClaims, CoreGenderClaim>;

/// ID token fields carried in the token endpoint response.
pub type EntraIdTokenFields = IdTokenFields<
    EntraClaims,
    EmptyExtraTokenFields,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

/// Token endpoint response from Entra ID.
pub type EntraTokenResponse = StandardTokenResponse<EntraIdTokenFields, CoreTokenType>;

/// A fully configured OpenID Connect client for Entra ID.
///
/// The endpoint type-state parameters match what
/// `Client::from_provider_metadata` returns: the authorization endpoint is
/// always present, the token and user-info endpoints are present if the
/// discovery document advertises them, and the rest are unset.
pub type EntraClient = Client<
    EntraClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    EntraTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;
