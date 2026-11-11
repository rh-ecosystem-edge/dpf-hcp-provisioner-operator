//! Port of `csr_owner_validation_test.go`
//! (Describe "CSR Owner Validation - DPU Existence Check").
//!
//! Go shares one fake client / clientset across the suite and removes objects with
//! `DeferCleanup`; here every spec builds fresh fakes (same observable state per spec).

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use ctrlrt::{Client, Ctx};
use hostedclient::HostedClient;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::csr_owner_validation::Validator;
use crate::test_util::*;

/// BeforeEach state.
struct Suite {
    ctx: Ctx,
    mgmt_client: Client,
    dpu_namespace: &'static str,
    test_hostname: &'static str,
}

fn before_each() -> Suite {
    Suite {
        ctx: Ctx::background(),
        mgmt_client: k8s_client(),
        dpu_namespace: "test-dpu-namespace",
        test_hostname: "test-dpu-node",
    }
}

fn dpu(name: &str, namespace: &str, n: u32, phase: &str) -> dpuprovisioningv1alpha1::DPU {
    dpuprovisioningv1alpha1::DPU {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        spec: dpuprovisioningv1alpha1::DPUSpec {
            dpu_node_name: format!("node-{n}"),
            dpu_device_name: format!("device-{n}"),
            bfb: Some(format!("bfb-{n}")),
            serial_number: if n == 1 && name != "dpu-1" {
                "SN123456".to_string()
            } else {
                format!("SN{}", n.to_string().repeat(6))
            },
            ..Default::default()
        },
        status: dpuprovisioningv1alpha1::DPUStatus {
            phase: phase.to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

impl Suite {
    async fn create(&self, dpu: &dpuprovisioningv1alpha1::DPU) {
        let mut d = dpu.clone();
        self.mgmt_client.create(&mut d).await.unwrap();
    }

    /// Fresh `fakeClientset` holding the test node.
    async fn clientset_with_node(&self) -> HostedClient {
        let cs = fake_clientset();
        let mut node = create_fake_node(self.test_hostname);
        cs.create(&mut node).await.unwrap();
        cs
    }
}

mod csr_owner_validation_dpu_existence_check {
    use super::*;

    mod dpu_existence_check {
        use super::*;

        mod when_dpu_exists_with_matching_name {
            use super::*;

            #[tokio::test]
            async fn should_find_dpu_with_exact_name_match() {
                let s = before_each();
                // Note: Implementation matches by DPU object name, not Spec fields
                s.create(&dpu(s.test_hostname, s.dpu_namespace, 1, ""))
                    .await;

                // Create validator (no hcClient since we're only testing DPU lookup)
                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);

                let dpu = validator.get_dpu(&s.ctx, s.test_hostname).await.unwrap();
                assert!(dpu.is_some());
            }
        }

        mod when_dpu_does_not_exist {
            use super::*;

            #[tokio::test]
            async fn should_not_find_dpu_when_none_exists() {
                let s = before_each();
                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);

                let dpu = validator.get_dpu(&s.ctx, s.test_hostname).await.unwrap();
                assert!(dpu.is_none());
            }

            #[tokio::test]
            async fn should_not_find_dpu_in_different_namespace() {
                let s = before_each();
                s.create(&dpu(s.test_hostname, "wrong-namespace", 1, ""))
                    .await;

                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);

                let dpu = validator.get_dpu(&s.ctx, s.test_hostname).await.unwrap();
                assert!(dpu.is_none());
            }

            #[tokio::test]
            async fn should_not_find_dpu_with_different_name() {
                let s = before_each();
                s.create(&dpu("dpu-123", s.dpu_namespace, 1, "")).await;

                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);

                let dpu = validator.get_dpu(&s.ctx, s.test_hostname).await.unwrap();
                assert!(dpu.is_none());
            }
        }

        mod when_multiple_dpus_exist {
            use super::*;

            #[tokio::test]
            async fn should_find_the_correct_dpu_among_multiple() {
                let s = before_each();
                s.create(&dpu("dpu-1", s.dpu_namespace, 1, "")).await;
                // This one matches our search
                s.create(&dpu(s.test_hostname, s.dpu_namespace, 2, ""))
                    .await;
                s.create(&dpu("dpu-3", s.dpu_namespace, 3, "")).await;

                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);

                // Should find dpu-2 by matching object name
                let dpu = validator.get_dpu(&s.ctx, s.test_hostname).await.unwrap();
                assert!(dpu.is_some());
                assert_eq!(dpu.unwrap().spec.serial_number, "SN222222");
            }
        }
    }

    mod bootstrap_csr_phase_validation {
        use super::*;

        mod when_dpu_is_in_dpu_cluster_config_phase {
            use super::*;

            #[tokio::test]
            async fn should_approve_bootstrap_csr() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG,
                ))
                .await;

                // No hostedClient - bootstrap CSRs don't check node existence
                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, true)
                    .await
                    .unwrap();
                assert!(result.valid);
                assert!(
                    result.reason.contains("DPU Cluster Config"),
                    "{}",
                    result.reason
                );
            }
        }

        mod when_dpu_is_in_ready_phase {
            use super::*;

            #[tokio::test]
            async fn should_reject_bootstrap_csr() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_READY,
                ))
                .await;

                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, true)
                    .await
                    .unwrap();
                assert!(!result.valid);
                assert!(
                    result
                        .reason
                        .contains("bootstrap CSRs only approved in phase"),
                    "{}",
                    result.reason
                );
            }
        }

        mod when_dpu_is_in_initializing_phase {
            use super::*;

            #[tokio::test]
            async fn should_reject_bootstrap_csr() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_INITIALIZING,
                ))
                .await;

                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, true)
                    .await
                    .unwrap();
                assert!(!result.valid);
            }
        }
    }

    mod serving_csr_phase_validation {
        use super::*;

        mod when_dpu_is_in_dpu_cluster_config_phase {
            use super::*;

            #[tokio::test]
            async fn should_approve_serving_csr_for_initial_node_join() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG,
                ))
                .await;

                // Create fake node in hosted cluster to pass node existence check
                let cs = s.clientset_with_node().await;

                let validator = Validator::new(s.mgmt_client.clone(), Some(cs), s.dpu_namespace);
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, false)
                    .await
                    .unwrap();
                assert!(result.valid);
                assert!(result.reason.contains("node exists"), "{}", result.reason);
            }
        }

        mod when_dpu_is_in_ready_phase {
            use super::*;

            #[tokio::test]
            async fn should_approve_serving_csr() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_READY,
                ))
                .await;

                let cs = s.clientset_with_node().await;

                let validator = Validator::new(s.mgmt_client.clone(), Some(cs), s.dpu_namespace);
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, false)
                    .await
                    .unwrap();
                assert!(result.valid);
                assert!(result.reason.contains("node exists"), "{}", result.reason);
            }
        }

        mod when_dpu_is_in_node_effect_removal_phase {
            use super::*;

            #[tokio::test]
            async fn should_approve_serving_csr() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_NODE_EFFECT_REMOVAL,
                ))
                .await;

                let cs = s.clientset_with_node().await;

                let validator = Validator::new(s.mgmt_client.clone(), Some(cs), s.dpu_namespace);
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, false)
                    .await
                    .unwrap();
                assert!(result.valid);
                assert!(result.reason.contains("node exists"), "{}", result.reason);
            }
        }

        mod when_dpu_is_in_initializing_phase {
            use super::*;

            #[tokio::test]
            async fn should_reject_serving_csr() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_INITIALIZING,
                ))
                .await;

                let validator = Validator::new(s.mgmt_client.clone(), None, s.dpu_namespace);
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, false)
                    .await
                    .unwrap();
                assert!(!result.valid);
                assert!(
                    result
                        .reason
                        .contains("serving CSRs only approved in phases"),
                    "{}",
                    result.reason
                );
            }
        }

        mod when_dpu_is_in_ready_phase_but_node_does_not_exist {
            use super::*;

            #[tokio::test]
            async fn should_reject_serving_csr() {
                let s = before_each();
                s.create(&dpu(
                    s.test_hostname,
                    s.dpu_namespace,
                    1,
                    dpuprovisioningv1alpha1::DPU_READY,
                ))
                .await;

                // Don't create node - this simulates node doesn't exist yet
                let validator = Validator::new(
                    s.mgmt_client.clone(),
                    Some(fake_clientset()),
                    s.dpu_namespace,
                );
                let result = validator
                    .validate_csr_owner(&s.ctx, s.test_hostname, false)
                    .await
                    .unwrap();
                assert!(!result.valid);
                assert!(
                    result.reason.contains("does not exist yet"),
                    "{}",
                    result.reason
                );
            }
        }
    }
}

/// Exact reason strings (Rust-only; pins the Go `fmt.Sprintf` formats).
mod reason_strings {
    use super::*;

    #[tokio::test]
    async fn all_reasons_match_go_format() {
        let s = before_each();
        let v = Validator::new(
            s.mgmt_client.clone(),
            Some(fake_clientset()),
            s.dpu_namespace,
        );
        let r = v.validate_csr_owner(&s.ctx, "h", true).await.unwrap();
        assert_eq!(
            r.reason,
            "no matching DPU found for hostname h in namespace test-dpu-namespace"
        );

        s.create(&dpu(
            "h",
            s.dpu_namespace,
            1,
            dpuprovisioningv1alpha1::DPU_READY,
        ))
        .await;
        let r = v.validate_csr_owner(&s.ctx, "h", true).await.unwrap();
        assert_eq!(
            r.reason,
            "DPU h is in phase Ready, bootstrap CSRs only approved in phase DPU Cluster Config"
        );
        let r = v.validate_csr_owner(&s.ctx, "h", false).await.unwrap();
        assert_eq!(r.reason, "node h does not exist yet in hosted cluster");

        s.create(&dpu(
            "i",
            s.dpu_namespace,
            1,
            dpuprovisioningv1alpha1::DPU_INITIALIZING,
        ))
        .await;
        let r = v.validate_csr_owner(&s.ctx, "i", false).await.unwrap();
        assert_eq!(
            r.reason,
            "DPU i is in phase Initializing, serving CSRs only approved in phases DPU Cluster Config, Ready, or Node Effect Removal"
        );

        s.create(&dpu(
            "c",
            s.dpu_namespace,
            1,
            dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG,
        ))
        .await;
        let r = v.validate_csr_owner(&s.ctx, "c", true).await.unwrap();
        assert_eq!(
            r.reason,
            "DPU in DPU Cluster Config phase, bootstrap CSR approved"
        );
    }
}
