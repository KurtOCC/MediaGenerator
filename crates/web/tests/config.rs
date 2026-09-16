//! Tests for configuration validation.

mod common;

use mediagenerator_web::config::{Environment, Secret};

#[test]
fn a_complete_configuration_validates() {
    assert!(common::test_config().validate().is_ok());
}

#[test]
fn a_short_session_secret_is_rejected() {
    let mut config = common::test_config();
    config.session_secret = Secret::new("too-short");
    assert!(config.validate().is_err());
}

#[test]
fn trailing_slashes_are_rejected() {
    let mut config = common::test_config();
    config.app_base_url = "http://localhost:8080/".to_owned();
    assert!(config.validate().is_err());

    let mut config = common::test_config();
    config.azure_openai_endpoint = "https://example.openai.azure.com/".to_owned();
    assert!(config.validate().is_err());
}

#[test]
fn an_api_key_is_refused_in_production() {
    let mut config = common::test_config();
    config.app_env = Environment::Production;
    config.azure_openai_api_key = Some(Secret::new("a-key"));
    assert!(config.validate().is_err());
}

#[test]
fn an_empty_required_role_disables_the_role_check() {
    let mut config = common::test_config();
    config.required_app_role = Some("   ".to_owned());
    assert_eq!(config.required_app_role(), None);

    config.required_app_role = Some("Mediagenerator.User".to_owned());
    assert_eq!(config.required_app_role(), Some("Mediagenerator.User"));
}

#[test]
fn a_secret_is_redacted_in_debug_output() {
    let secret = Secret::new("super-hemmelig");
    assert_eq!(format!("{secret:?}"), "Secret(***)");
    assert!(!format!("{:?}", common::test_config()).contains("super-hemmelig"));
}
