# internal/common test mapping

| Go test | Rust test |
|---|---|
| ocp_test.go TestExtractOCPVersion (9 cases) | tests/ocp_test.rs::test_extract_ocp_version |
| dpfversion_test.go TestDPFMajorMinorFromVersion (6 cases) | tests/ocp_test.rs::test_dpf_major_minor_from_version |
| keychain_test.go TestKeychainPrefersRepositoryScopedAuth (2 cases) | tests/keychain_test.rs::test_keychain_prefers_repository_scoped_auth |
| keychain_test.go TestKeychainFallsBackToRegistryAuth | tests/keychain_test.rs::test_keychain_falls_back_to_registry_auth |
| keychain_test.go TestKeychainAnonymousWhenNoMatch | tests/keychain_test.rs::test_keychain_anonymous_when_no_match |
| (extra) Go error strings | tests/keychain_test.rs::test_keychain_error_messages_match_go |
| (extra) config.go semantics | tests/config_test.rs |
| (extra) release.go semantics | tests/release_test.rs |
| (extra) label key derivation | src/constants.rs tests |
