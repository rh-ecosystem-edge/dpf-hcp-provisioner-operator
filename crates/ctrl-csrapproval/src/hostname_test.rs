//! Port of `hostname_test.go` (Describe "Hostname Extraction").

use k8s_openapi::ByteString;
use k8s_openapi::api::certificates::v1::{
    CertificateSigningRequest, CertificateSigningRequestCondition, CertificateSigningRequestSpec,
    CertificateSigningRequestStatus,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::hostname::*;
use crate::test_util::*;

fn csr(name: &str, signer: &str, username: Option<&str>, request: Vec<u8>) -> CertificateSigningRequest {
    CertificateSigningRequest {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: CertificateSigningRequestSpec {
            signer_name: signer.to_string(),
            username: username.map(str::to_string),
            request: ByteString(request),
            ..Default::default()
        },
        status: None,
    }
}

fn with_conditions(name: &str, conditions: Vec<(&str, &str)>) -> CertificateSigningRequest {
    CertificateSigningRequest {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        status: Some(CertificateSigningRequestStatus {
            conditions: Some(
                conditions
                    .into_iter()
                    .map(|(t, s)| CertificateSigningRequestCondition {
                        type_: t.to_string(),
                        status: s.to_string(),
                        ..Default::default()
                    })
                    .collect(),
            ),
            certificate: None,
        }),
        ..Default::default()
    }
}

fn signer_only(name: &str, signer: &str) -> CertificateSigningRequest {
    csr(name, signer, None, vec![])
}

mod hostname_extraction {
    use super::*;

    mod extract_hostname_from_bootstrap_csr {
        use super::*;

        mod when_csr_has_valid_cn_with_system_node_prefix {
            use super::*;

            #[test]
            fn should_extract_hostname_correctly() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes = create_test_csr(&format!("system:node:{hostname}"));
                let csr = csr("test-bootstrap-csr", SIGNER_BOOTSTRAP, None, csr_bytes);

                let extracted = extract_hostname(Some(&csr)).unwrap();
                assert_eq!(extracted, hostname);
            }
        }

        mod when_csr_has_invalid_cn_format {
            use super::*;

            #[test]
            fn should_return_error() {
                let csr_bytes = create_test_csr("invalid-format");
                let csr = csr("test-invalid-csr", SIGNER_BOOTSTRAP, None, csr_bytes);

                let err = extract_hostname(Some(&csr)).unwrap_err();
                assert!(err.to_string().contains("invalid CN format"), "{err}");
            }
        }

        mod when_csr_has_empty_cn_after_prefix_removal {
            use super::*;

            #[test]
            fn should_return_error() {
                let csr_bytes = create_test_csr("system:node:");
                let csr = csr("test-empty-hostname-csr", SIGNER_BOOTSTRAP, None, csr_bytes);

                let err = extract_hostname(Some(&csr)).unwrap_err();
                assert!(err.to_string().contains("invalid CN format"), "{err}");
            }
        }
    }

    mod extract_hostname_from_serving_csr {
        use super::*;

        mod when_csr_has_valid_username_with_system_node_prefix {
            use super::*;

            #[test]
            fn should_extract_hostname_correctly() {
                let hostname = TEST_HOSTNAME;
                let username = format!("system:node:{hostname}");
                let csr = csr("test-serving-csr", SIGNER_SERVING, Some(&username), vec![]);

                let extracted = extract_hostname(Some(&csr)).unwrap();
                assert_eq!(extracted, hostname);
            }
        }

        mod when_csr_has_invalid_username_format {
            use super::*;

            #[test]
            fn should_return_error() {
                let csr = csr(
                    "test-invalid-serving-csr",
                    SIGNER_SERVING,
                    Some("invalid-format"),
                    vec![],
                );

                let err = extract_hostname(Some(&csr)).unwrap_err();
                assert!(err.to_string().contains("invalid username format"), "{err}");
            }
        }

        mod when_csr_has_empty_username_after_prefix_removal {
            use super::*;

            #[test]
            fn should_return_error() {
                let csr = csr(
                    "test-empty-username-csr",
                    SIGNER_SERVING,
                    Some("system:node:"),
                    vec![],
                );

                let err = extract_hostname(Some(&csr)).unwrap_err();
                assert!(err.to_string().contains("invalid username format"), "{err}");
            }
        }
    }

    mod extract_hostname_with_unsupported_signer {
        use super::*;

        mod when_csr_has_unsupported_signer_name {
            use super::*;

            #[test]
            fn should_return_error() {
                let csr = csr(
                    "test-unsupported-csr",
                    "unsupported.signer.io/test",
                    Some("system:node:test-host"),
                    vec![],
                );

                let err = extract_hostname(Some(&csr)).unwrap_err();
                assert!(err.to_string().contains("unsupported signer name"), "{err}");
            }
        }

        mod when_csr_is_nil {
            use super::*;

            #[test]
            fn should_return_error() {
                let err = extract_hostname(None).unwrap_err();
                assert!(err.to_string().contains("CSR is nil"), "{err}");
            }
        }
    }

    mod is_pending_ {
        use super::*;

        mod when_csr_has_no_conditions {
            use super::*;

            #[test]
            fn should_return_true() {
                let csr = with_conditions("test-pending-csr", vec![]);
                assert!(is_pending(Some(&csr)));
            }
        }

        mod when_csr_has_approved_condition {
            use super::*;

            #[test]
            fn should_return_false() {
                let csr = with_conditions("test-approved-csr", vec![("Approved", "True")]);
                assert!(!is_pending(Some(&csr)));
            }
        }

        mod when_csr_has_denied_condition {
            use super::*;

            #[test]
            fn should_return_false() {
                let csr = with_conditions("test-denied-csr", vec![("Denied", "True")]);
                assert!(!is_pending(Some(&csr)));
            }
        }

        mod when_csr_is_nil {
            use super::*;

            #[test]
            fn should_return_false() {
                assert!(!is_pending(None));
            }
        }
    }

    mod filter_by_signer_name_ {
        use super::*;

        mod when_list_contains_mixed_signer_types {
            use super::*;

            #[test]
            fn should_filter_to_only_bootstrap_and_serving_csrs() {
                let csrs = vec![
                    signer_only("bootstrap-csr", SIGNER_BOOTSTRAP),
                    signer_only("serving-csr", SIGNER_SERVING),
                    signer_only("other-csr", "other.signer.io/test"),
                ];

                let filtered = filter_by_signer_name(&csrs);
                assert_eq!(filtered.len(), 2);
                assert_eq!(filtered[0].metadata.name.as_deref(), Some("bootstrap-csr"));
                assert_eq!(filtered[1].metadata.name.as_deref(), Some("serving-csr"));
            }
        }

        mod when_list_contains_no_matching_signers {
            use super::*;

            #[test]
            fn should_return_empty_list() {
                let csrs = vec![signer_only("other-csr", "other.signer.io/test")];
                assert!(filter_by_signer_name(&csrs).is_empty());
            }
        }

        mod when_list_is_empty {
            use super::*;

            #[test]
            fn should_return_empty_list() {
                let csrs: Vec<CertificateSigningRequest> = vec![];
                assert!(filter_by_signer_name(&csrs).is_empty());
            }
        }
    }
}
