//! Port of `csr_content_validation_test.go` (Describe "CSR Validation").

use k8s_openapi::ByteString;
use k8s_openapi::api::certificates::v1::{
    CertificateSigningRequest, CertificateSigningRequestSpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::csr_content_validation::*;
use crate::hostname::*;
use crate::test_util::*;

fn csr(
    name: &str,
    signer: &str,
    username: &str,
    groups: &[&str],
    usages: &[&str],
    request: Vec<u8>,
) -> CertificateSigningRequest {
    CertificateSigningRequest {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: CertificateSigningRequestSpec {
            signer_name: signer.to_string(),
            username: Some(username.to_string()),
            groups: Some(groups.iter().map(|s| s.to_string()).collect()),
            usages: Some(usages.iter().map(|s| s.to_string()).collect()),
            request: ByteString(request),
            ..Default::default()
        },
        status: None,
    }
}

const BOOTSTRAP_GROUPS: &[&str] = &[
    GROUP_SERVICE_ACCOUNTS,
    GROUP_SERVICE_ACCOUNTS_MACHINE_CONFIG,
    GROUP_AUTHENTICATED,
];
const BOOTSTRAP_USAGES: &[&str] = &[USAGE_DIGITAL_SIGNATURE, USAGE_CLIENT_AUTH];
const SERVING_GROUPS: &[&str] = &[GROUP_NODES, GROUP_AUTHENTICATED];
const SERVING_USAGES: &[&str] = &[USAGE_DIGITAL_SIGNATURE, USAGE_SERVER_AUTH];

fn node_user(hostname: &str) -> String {
    format!("system:node:{hostname}")
}

mod csr_validation {
    use super::*;

    mod validate_bootstrap_csr_ {
        use super::*;

        mod when_csr_has_valid_bootstrap_configuration {
            use super::*;

            #[test]
            fn should_pass_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes =
                    create_test_csr_with_organization(&node_user(hostname), "system:nodes");
                let csr = csr(
                    "test-bootstrap-csr",
                    SIGNER_BOOTSTRAP,
                    BOOTSTRAPPER_USERNAME,
                    BOOTSTRAP_GROUPS,
                    BOOTSTRAP_USAGES,
                    csr_bytes,
                );

                validate_bootstrap_csr(&csr, hostname).unwrap();
            }
        }

        mod when_csr_has_invalid_username {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes =
                    create_test_csr_with_organization(&node_user(hostname), "system:nodes");
                let csr = csr(
                    "test-invalid-username",
                    SIGNER_BOOTSTRAP,
                    "wrong-username",
                    BOOTSTRAP_GROUPS,
                    BOOTSTRAP_USAGES,
                    csr_bytes,
                );

                let err = validate_bootstrap_csr(&csr, hostname).unwrap_err();
                assert!(err.to_string().contains("invalid username"), "{err}");
            }
        }

        mod when_csr_is_missing_required_groups {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes =
                    create_test_csr_with_organization(&node_user(hostname), "system:nodes");
                let csr = csr(
                    "test-missing-groups",
                    SIGNER_BOOTSTRAP,
                    BOOTSTRAPPER_USERNAME,
                    &[GROUP_AUTHENTICATED], // Missing other groups
                    BOOTSTRAP_USAGES,
                    csr_bytes,
                );

                let err = validate_bootstrap_csr(&csr, hostname).unwrap_err();
                assert!(err.to_string().contains("missing required group"), "{err}");
            }
        }

        mod when_csr_is_missing_required_usages {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes =
                    create_test_csr_with_organization(&node_user(hostname), "system:nodes");
                let csr = csr(
                    "test-missing-usages",
                    SIGNER_BOOTSTRAP,
                    BOOTSTRAPPER_USERNAME,
                    BOOTSTRAP_GROUPS,
                    &[USAGE_DIGITAL_SIGNATURE], // Missing client auth
                    csr_bytes,
                );

                let err = validate_bootstrap_csr(&csr, hostname).unwrap_err();
                assert!(err.to_string().contains("missing required usage"), "{err}");
            }
        }

        mod when_csr_has_invalid_organization {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes =
                    create_test_csr_with_organization(&node_user(hostname), "wrong-org");
                let csr = csr(
                    "test-invalid-org",
                    SIGNER_BOOTSTRAP,
                    BOOTSTRAPPER_USERNAME,
                    BOOTSTRAP_GROUPS,
                    BOOTSTRAP_USAGES,
                    csr_bytes,
                );

                let err = validate_bootstrap_csr(&csr, hostname).unwrap_err();
                assert!(err.to_string().contains("invalid organization"), "{err}");
            }
        }
    }

    mod validate_serving_csr_ {
        use super::*;

        mod when_csr_has_valid_serving_configuration {
            use super::*;

            #[test]
            fn should_pass_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes = create_serving_csr_with_dns(&node_user(hostname), hostname);
                let csr = csr(
                    "test-serving-csr",
                    SIGNER_SERVING,
                    &node_user(hostname),
                    SERVING_GROUPS,
                    SERVING_USAGES,
                    csr_bytes,
                );

                validate_serving_csr(&csr, hostname).unwrap();
            }
        }

        mod when_csr_has_invalid_username {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes = create_serving_csr_with_dns(&node_user(hostname), hostname);
                let csr = csr(
                    "test-invalid-username",
                    SIGNER_SERVING,
                    "wrong-username",
                    SERVING_GROUPS,
                    SERVING_USAGES,
                    csr_bytes,
                );

                let err = validate_serving_csr(&csr, hostname).unwrap_err();
                assert!(err.to_string().contains("invalid username"), "{err}");
            }
        }

        mod when_csr_is_missing_system_nodes_group {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes = create_serving_csr_with_dns(&node_user(hostname), hostname);
                let csr = csr(
                    "test-missing-nodes-group",
                    SIGNER_SERVING,
                    &node_user(hostname),
                    &[GROUP_AUTHENTICATED], // Missing system:nodes
                    SERVING_USAGES,
                    csr_bytes,
                );

                let err = validate_serving_csr(&csr, hostname).unwrap_err();
                assert!(
                    err.to_string()
                        .contains("missing required group: system:nodes"),
                    "{err}"
                );
            }
        }

        mod when_csr_is_missing_required_usages {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes = create_serving_csr_with_dns(&node_user(hostname), hostname);
                let csr = csr(
                    "test-missing-usages",
                    SIGNER_SERVING,
                    &node_user(hostname),
                    SERVING_GROUPS,
                    &[USAGE_DIGITAL_SIGNATURE], // Missing server auth
                    csr_bytes,
                );

                let err = validate_serving_csr(&csr, hostname).unwrap_err();
                assert!(err.to_string().contains("missing required usage"), "{err}");
            }
        }

        mod when_csr_has_wrong_hostname_in_dns_san {
            use super::*;

            #[test]
            fn should_fail_validation() {
                let hostname = TEST_HOSTNAME;
                let csr_bytes = create_serving_csr_with_dns(&node_user(hostname), "wrong-hostname");
                let csr = csr(
                    "test-wrong-dns-name",
                    SIGNER_SERVING,
                    &node_user(hostname),
                    SERVING_GROUPS,
                    SERVING_USAGES,
                    csr_bytes,
                );

                let err = validate_serving_csr(&csr, hostname).unwrap_err();
                assert!(err.to_string().contains("does not match hostname"), "{err}");
            }
        }
    }
}
