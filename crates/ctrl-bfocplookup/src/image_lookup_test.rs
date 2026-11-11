//! Port of internal/controller/bfocplookup/image_lookup_test.go
//! ("BlueField OCP Layer Image Lookup" suite, internal package tests).

use std::sync::Arc;

use async_trait::async_trait;
use ctrlrt::{Ctx, FakeRecorder, Recorder};
use ctrlrt_fake::FakeClientBuilder;
use registry::authn::{Keychain, MultiKeychain};
use registry::name::Reference;

use crate::*;

/// fakeImageChecker implements ImageChecker for testing
struct FakeImageChecker {
    err: Option<String>,
}

#[async_trait]
impl ImageChecker for FakeImageChecker {
    async fn check_tag(
        &self,
        _ctx: &Ctx,
        _r: &Reference,
        _keychain: Arc<dyn Keychain>,
    ) -> ctrlrt::Result<()> {
        match &self.err {
            None => Ok(()),
            Some(e) => Err(anyhow::anyhow!(e.clone())),
        }
    }
}

const TEST_REPOSITORY: &str = "quay.io/test-org/bluefield-os";

/// Go's `authn.DefaultKeychain` (unused by the fake checker): an anonymous keychain.
fn default_keychain() -> Arc<dyn Keychain> {
    Arc::new(MultiKeychain::new(vec![]))
}

fn errorf(s: &str) -> anyhow::Error {
    anyhow::anyhow!(s.to_string())
}

// ---------------------------------------------------------------------------------------------
// Describe("OCP Version Extraction from Tag")
// ---------------------------------------------------------------------------------------------

#[test]
fn ocp_version_extraction_from_tag_should_strip_multi_suffix() {
    assert_eq!(
        extract_version_from_tag("quay.io/ocp-release:4.19.0-ec.5-multi"),
        "4.19.0-ec.5"
    );
}

#[test]
fn ocp_version_extraction_from_tag_should_strip_x86_64_suffix() {
    assert_eq!(
        extract_version_from_tag("quay.io/ocp-release:4.18.2-rc.1-x86_64"),
        "4.18.2-rc.1"
    );
}

#[test]
fn ocp_version_extraction_from_tag_should_strip_amd64_suffix() {
    assert_eq!(
        extract_version_from_tag("quay.io/ocp-release:4.19.0-ec.5-amd64"),
        "4.19.0-ec.5"
    );
}

#[test]
fn ocp_version_extraction_from_tag_should_strip_arm64_suffix() {
    assert_eq!(
        extract_version_from_tag("quay.io/ocp-release:4.19.0-ec.5-arm64"),
        "4.19.0-ec.5"
    );
}

#[test]
fn ocp_version_extraction_from_tag_should_strip_ppc64le_suffix() {
    assert_eq!(
        extract_version_from_tag("quay.io/ocp-release:4.19.0-ec.5-ppc64le"),
        "4.19.0-ec.5"
    );
}

#[test]
fn ocp_version_extraction_from_tag_should_strip_s390x_suffix() {
    assert_eq!(
        extract_version_from_tag("quay.io/ocp-release:4.19.0-ec.5-s390x"),
        "4.19.0-ec.5"
    );
}

#[test]
fn ocp_version_extraction_from_tag_should_return_version_as_is_when_no_arch_suffix() {
    assert_eq!(
        extract_version_from_tag("quay.io/ocp-release:4.19.0-ec.5"),
        "4.19.0-ec.5"
    );
}

#[test]
fn ocp_version_extraction_from_tag_should_return_empty_for_digest_images() {
    assert!(extract_version_from_tag("registry.ci/release@sha256:abc123").is_empty());
}

#[test]
fn ocp_version_extraction_from_tag_should_return_empty_for_missing_tag() {
    assert!(extract_version_from_tag("quay.io/ocp-release").is_empty());
}

#[test]
fn ocp_version_extraction_from_tag_should_return_empty_for_empty_tag() {
    assert!(extract_version_from_tag("quay.io/ocp-release:").is_empty());
}

// ---------------------------------------------------------------------------------------------
// Describe("Registry Tag Lookup")
// ---------------------------------------------------------------------------------------------

/// BeforeEach: `lookup = &ImageLookup{}` (the client and recorder are never used here).
fn before_each_lookup(err: Option<&str>) -> ImageLookup {
    let c = FakeClientBuilder::new().build();
    let mut lookup = ImageLookup::new(c, Arc::new(FakeRecorder::discarding()) as Recorder);
    lookup.image_checker = Arc::new(FakeImageChecker {
        err: err.map(str::to_string),
    });
    lookup
}

#[tokio::test]
async fn registry_tag_lookup_when_version_tag_exists_in_registry_should_return_the_full_image_reference()
 {
    let lookup = before_each_lookup(None);
    let image = lookup
        .validate_tag_exists(
            &Ctx::background(),
            TEST_REPOSITORY,
            "4.19.0-ec.5",
            default_keychain(),
        )
        .await
        .expect("no error");
    assert_eq!(image, format!("{TEST_REPOSITORY}:4.19.0-ec.5"));
}

#[tokio::test]
async fn registry_tag_lookup_when_version_tag_does_not_exist_in_registry_should_return_version_not_found_error()
 {
    let lookup = before_each_lookup(Some("MANIFEST_UNKNOWN: manifest unknown"));
    let err = lookup
        .validate_tag_exists(
            &Ctx::background(),
            TEST_REPOSITORY,
            "4.20.0-ec.1",
            default_keychain(),
        )
        .await
        .unwrap_err();
    assert!(err.is::<VersionNotFoundError>());
}

#[tokio::test]
async fn registry_tag_lookup_when_registry_returns_a_404_not_found_error_should_return_version_not_found_error()
 {
    let lookup = before_each_lookup(Some("unexpected status code 404 not found"));
    let err = lookup
        .validate_tag_exists(
            &Ctx::background(),
            TEST_REPOSITORY,
            "4.19.0-ec.5",
            default_keychain(),
        )
        .await
        .unwrap_err();
    assert!(err.is::<VersionNotFoundError>());
}

#[tokio::test]
async fn registry_tag_lookup_when_registry_returns_an_auth_error_should_return_registry_auth_error()
{
    let lookup = before_each_lookup(Some("UNAUTHORIZED: authentication required"));
    let err = lookup
        .validate_tag_exists(
            &Ctx::background(),
            TEST_REPOSITORY,
            "4.19.0-ec.5",
            default_keychain(),
        )
        .await
        .unwrap_err();
    assert!(err.is::<RegistryAuthError>());
}

#[tokio::test]
async fn registry_tag_lookup_when_registry_returns_a_transient_error_should_return_registry_access_error()
 {
    let lookup = before_each_lookup(Some("connection refused"));
    let err = lookup
        .validate_tag_exists(
            &Ctx::background(),
            TEST_REPOSITORY,
            "4.19.0-ec.5",
            default_keychain(),
        )
        .await
        .unwrap_err();
    assert!(err.is::<RegistryAccessError>());
}

// ---------------------------------------------------------------------------------------------
// Describe("BlueField OCP Layer Image URL Validation")
// ---------------------------------------------------------------------------------------------

#[test]
fn blue_field_ocp_layer_image_url_validation_when_url_is_valid_should_not_return_an_error() {
    let url = "quay.io/edge-infrastructure/bluefield-rhcos:4.19.0-ec.5";
    assert!(validate_blue_field_ocp_layer_image_url(url, "4.19.0-ec.5").is_ok());
}

#[test]
fn blue_field_ocp_layer_image_url_validation_when_url_is_valid_with_port_should_not_return_an_error()
 {
    let url = "registry.example.com:5000/bluefield-rhcos:4.19.0-ec.5";
    assert!(validate_blue_field_ocp_layer_image_url(url, "4.19.0-ec.5").is_ok());
}

#[test]
fn blue_field_ocp_layer_image_url_validation_when_url_is_empty_should_return_invalid_blue_field_ocp_layer_image_url_error()
 {
    let err = validate_blue_field_ocp_layer_image_url("", "4.19.0-ec.5").unwrap_err();
    assert!(err.is::<InvalidBlueFieldOCPLayerImageURLError>());
}

#[test]
fn blue_field_ocp_layer_image_url_validation_when_url_is_missing_colon_separator_should_return_invalid_blue_field_ocp_layer_image_url_error()
 {
    let url = "quay.io/edge-infrastructure/bluefield-rhcos";
    let err = validate_blue_field_ocp_layer_image_url(url, "4.19.0-ec.5").unwrap_err();
    assert!(err.is::<InvalidBlueFieldOCPLayerImageURLError>());
}

// ---------------------------------------------------------------------------------------------
// Describe("Error Types")
// ---------------------------------------------------------------------------------------------

#[test]
fn error_types_registry_access_error_should_have_a_descriptive_error_message() {
    let err = RegistryAccessError {
        repository: TEST_REPOSITORY.into(),
        err: errorf("connection refused"),
    };
    assert!(err.to_string().contains("failed to access registry"));
    assert!(err.to_string().contains(TEST_REPOSITORY));
}

#[test]
fn error_types_registry_auth_error_should_have_a_descriptive_error_message() {
    let err = RegistryAuthError {
        repository: TEST_REPOSITORY.into(),
        err: errorf("UNAUTHORIZED"),
    };
    assert!(err.to_string().contains("authentication failed"));
    assert!(err.to_string().contains(TEST_REPOSITORY));
}

#[test]
fn error_types_version_not_found_error_should_include_version_and_repository_in_error_message() {
    let err = VersionNotFoundError {
        version: "4.19.0-ec.5".into(),
        repository: TEST_REPOSITORY.into(),
    };
    assert!(err.to_string().contains("4.19.0-ec.5"));
    assert!(err.to_string().contains(TEST_REPOSITORY));
}

#[test]
fn error_types_invalid_image_format_error_should_include_the_message_and_url_in_error_message() {
    let err = InvalidImageFormatError {
        message: "missing tag separator".into(),
        url: "quay.io/test/image".into(),
    };
    assert!(err.to_string().contains("missing tag separator"));
    assert!(err.to_string().contains("quay.io/test/image"));
}

#[test]
fn error_types_invalid_blue_field_ocp_layer_image_url_error_should_include_version_and_message_in_error_message()
 {
    let err = InvalidBlueFieldOCPLayerImageURLError {
        version: "4.19.0-ec.5".into(),
        url: String::new(),
        message: "BlueField OCP layer image URL is empty".into(),
    };
    assert!(err.to_string().contains("4.19.0-ec.5"));
    assert!(err.to_string().contains("empty"));
}

// ---------------------------------------------------------------------------------------------
// Describe("isAuthError")
// ---------------------------------------------------------------------------------------------

#[test]
fn is_auth_error_should_detect_unauthorized_errors() {
    assert!(is_auth_error(&errorf(
        "UNAUTHORIZED: authentication required"
    )));
}

#[test]
fn is_auth_error_should_detect_denied_errors() {
    assert!(is_auth_error(&errorf("denied: access forbidden")));
}

#[test]
fn is_auth_error_should_detect_403_errors() {
    assert!(is_auth_error(&errorf("unexpected status code 403")));
}

#[test]
fn is_auth_error_should_not_match_non_auth_errors() {
    assert!(!is_auth_error(&errorf("connection refused")));
}

// ---------------------------------------------------------------------------------------------
// Describe("isNotFoundError")
// ---------------------------------------------------------------------------------------------

#[test]
fn is_not_found_error_should_detect_manifest_unknown_errors() {
    assert!(is_not_found_error(&errorf(
        "MANIFEST_UNKNOWN: manifest unknown"
    )));
}

#[test]
fn is_not_found_error_should_detect_not_found_errors() {
    assert!(is_not_found_error(&errorf("NOT_FOUND: resource not found")));
}

#[test]
fn is_not_found_error_should_detect_404_errors() {
    assert!(is_not_found_error(&errorf("unexpected status code 404")));
}

#[test]
fn is_not_found_error_should_not_match_non_not_found_errors() {
    assert!(!is_not_found_error(&errorf("connection refused")));
}
