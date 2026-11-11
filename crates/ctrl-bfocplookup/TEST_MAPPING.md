# internal/controller/bfocplookup test mapping

> **Golden fixtures are frozen.** The fixtures referenced below were generated from the former Go
> implementation by the generators under `hack/golden/`, which were removed together with the Go
> code (still in the git history, commit `fead352`). They can no longer be regenerated; see
> `docs/rust-port/CONVENTIONS.md` (rule 6).

Go suite: `image_lookup_test.go` (package `bfocplookup`, Ginkgo suite "BlueField OCP Layer Image Lookup").
`suite_test.go` only bootstraps Ginkgo (`TestBFOCPLookup` → `RunSpecs`); it has no Rust counterpart —
each spec is a plain `#[test]` / `#[tokio::test]` in `src/image_lookup_test.rs` (internal tests, since the Go
specs exercise unexported functions).

All 32 Go specs are ported.

| Go spec (`BlueField OCP Layer Image Lookup / ...`) | Rust test (`src/image_lookup_test.rs`) |
|---|---|
| OCP Version Extraction from Tag / should strip -multi suffix | `image_lookup_test::ocp_version_extraction_from_tag_should_strip_multi_suffix` |
| OCP Version Extraction from Tag / should strip -x86_64 suffix | `image_lookup_test::ocp_version_extraction_from_tag_should_strip_x86_64_suffix` |
| OCP Version Extraction from Tag / should strip -amd64 suffix | `image_lookup_test::ocp_version_extraction_from_tag_should_strip_amd64_suffix` |
| OCP Version Extraction from Tag / should strip -arm64 suffix | `image_lookup_test::ocp_version_extraction_from_tag_should_strip_arm64_suffix` |
| OCP Version Extraction from Tag / should strip -ppc64le suffix | `image_lookup_test::ocp_version_extraction_from_tag_should_strip_ppc64le_suffix` |
| OCP Version Extraction from Tag / should strip -s390x suffix | `image_lookup_test::ocp_version_extraction_from_tag_should_strip_s390x_suffix` |
| OCP Version Extraction from Tag / should return version as-is when no arch suffix | `image_lookup_test::ocp_version_extraction_from_tag_should_return_version_as_is_when_no_arch_suffix` |
| OCP Version Extraction from Tag / should return empty for digest images | `image_lookup_test::ocp_version_extraction_from_tag_should_return_empty_for_digest_images` |
| OCP Version Extraction from Tag / should return empty for missing tag | `image_lookup_test::ocp_version_extraction_from_tag_should_return_empty_for_missing_tag` |
| OCP Version Extraction from Tag / should return empty for empty tag | `image_lookup_test::ocp_version_extraction_from_tag_should_return_empty_for_empty_tag` |
| Registry Tag Lookup / When version tag exists in registry / should return the full image reference | `image_lookup_test::registry_tag_lookup_when_version_tag_exists_in_registry_should_return_the_full_image_reference` |
| Registry Tag Lookup / When version tag does not exist in registry / should return VersionNotFoundError | `image_lookup_test::registry_tag_lookup_when_version_tag_does_not_exist_in_registry_should_return_version_not_found_error` |
| Registry Tag Lookup / When registry returns a 404 not found error / should return VersionNotFoundError | `image_lookup_test::registry_tag_lookup_when_registry_returns_a_404_not_found_error_should_return_version_not_found_error` |
| Registry Tag Lookup / When registry returns an auth error / should return RegistryAuthError | `image_lookup_test::registry_tag_lookup_when_registry_returns_an_auth_error_should_return_registry_auth_error` |
| Registry Tag Lookup / When registry returns a transient error / should return RegistryAccessError | `image_lookup_test::registry_tag_lookup_when_registry_returns_a_transient_error_should_return_registry_access_error` |
| BlueField OCP Layer Image URL Validation / When URL is valid / should not return an error | `image_lookup_test::blue_field_ocp_layer_image_url_validation_when_url_is_valid_should_not_return_an_error` |
| BlueField OCP Layer Image URL Validation / When URL is valid with port / should not return an error | `image_lookup_test::blue_field_ocp_layer_image_url_validation_when_url_is_valid_with_port_should_not_return_an_error` |
| BlueField OCP Layer Image URL Validation / When URL is empty / should return InvalidBlueFieldOCPLayerImageURLError | `image_lookup_test::blue_field_ocp_layer_image_url_validation_when_url_is_empty_should_return_invalid_blue_field_ocp_layer_image_url_error` |
| BlueField OCP Layer Image URL Validation / When URL is missing colon separator / should return InvalidBlueFieldOCPLayerImageURLError | `image_lookup_test::blue_field_ocp_layer_image_url_validation_when_url_is_missing_colon_separator_should_return_invalid_blue_field_ocp_layer_image_url_error` |
| Error Types / RegistryAccessError / should have a descriptive error message | `image_lookup_test::error_types_registry_access_error_should_have_a_descriptive_error_message` |
| Error Types / RegistryAuthError / should have a descriptive error message | `image_lookup_test::error_types_registry_auth_error_should_have_a_descriptive_error_message` |
| Error Types / VersionNotFoundError / should include version and repository in error message | `image_lookup_test::error_types_version_not_found_error_should_include_version_and_repository_in_error_message` |
| Error Types / InvalidImageFormatError / should include the message and URL in error message | `image_lookup_test::error_types_invalid_image_format_error_should_include_the_message_and_url_in_error_message` |
| Error Types / InvalidBlueFieldOCPLayerImageURLError / should include version and message in error message | `image_lookup_test::error_types_invalid_blue_field_ocp_layer_image_url_error_should_include_version_and_message_in_error_message` |
| isAuthError / should detect UNAUTHORIZED errors | `image_lookup_test::is_auth_error_should_detect_unauthorized_errors` |
| isAuthError / should detect denied errors | `image_lookup_test::is_auth_error_should_detect_denied_errors` |
| isAuthError / should detect 403 errors | `image_lookup_test::is_auth_error_should_detect_403_errors` |
| isAuthError / should not match non-auth errors | `image_lookup_test::is_auth_error_should_not_match_non_auth_errors` |
| isNotFoundError / should detect MANIFEST_UNKNOWN errors | `image_lookup_test::is_not_found_error_should_detect_manifest_unknown_errors` |
| isNotFoundError / should detect NOT_FOUND errors | `image_lookup_test::is_not_found_error_should_detect_not_found_errors` |
| isNotFoundError / should detect 404 errors | `image_lookup_test::is_not_found_error_should_detect_404_errors` |
| isNotFoundError / should not match non-not-found errors | `image_lookup_test::is_not_found_error_should_not_match_non_not_found_errors` |

## Additional tests (no Go counterpart)

Golden differential tests, `tests/golden.rs`, fixtures from `hack/golden/bfocplookup`.
Each runs `LookupBlueFieldOCPLayerImage` (Go) / `lookup_blue_field_ocp_layer_image` (Rust) against the fake client with a
fake `ImageChecker` (digest release images go through an in-process rules registry, `tests/support/mod.rs`) and compares
results, errors, events (`FakeRecorder` strings), checked references and the encoded in-memory + stored DPFHCPProvisioner
(conditions, `status.blueFieldOCPLayerImage`, observedGeneration).

| Fixture | Rust test |
|---|---|
| `tests/fixtures/empty_release_image.json` | `golden::golden_empty_release_image` |
| `tests/fixtures/missing_pull_secret.json` | `golden::golden_missing_pull_secret` |
| `tests/fixtures/pull_secret_missing_key.json` | `golden::golden_pull_secret_missing_key` |
| `tests/fixtures/pull_secret_invalid_json.json` | `golden::golden_pull_secret_invalid_json` |
| `tests/fixtures/found.json` | `golden::golden_found` |
| `tests/fixtures/found_then_not_found.json` | `golden::golden_found_then_not_found` |
| `tests/fixtures/version_not_found.json` | `golden::golden_version_not_found` |
| `tests/fixtures/auth_error.json` | `golden::golden_auth_error` |
| `tests/fixtures/transient_error.json` | `golden::golden_transient_error` |
| `tests/fixtures/invalid_repository.json` | `golden::golden_invalid_repository` |
| `tests/fixtures/empty_repository.json` | `golden::golden_empty_repository` |
| `tests/fixtures/status_update_fails_on_success.json` | `golden::golden_status_update_fails_on_success` |
| `tests/fixtures/status_update_fails_on_permanent_error.json` | `golden::golden_status_update_fails_on_permanent_error` |
| `tests/fixtures/digest_with_label.json` | `golden::golden_digest_with_label` |
| `tests/fixtures/digest_without_label.json` | `golden::golden_digest_without_label` |
| `tests/fixtures/digest_empty_label.json` | `golden::golden_digest_empty_label` |
| `tests/fixtures/digest_manifest_not_found.json` | `golden::golden_digest_manifest_not_found` |
| `tests/fixtures/digest_unauthorized.json` | `golden::golden_digest_unauthorized` |
| `tests/fixtures/digest_config_not_found.json` | `golden::golden_digest_config_not_found` |
| `tests/fixtures/digest_bad_reference.json` | `golden::golden_digest_bad_reference` |
