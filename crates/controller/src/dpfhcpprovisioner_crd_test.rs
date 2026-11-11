//! Port of `internal/controller/dpfhcpprovisioner_crd_test.go`.
//!
//! CRD Schema Validation Tests
//! These tests verify API-level validation (CEL rules, regex patterns, etc.)
//! They run with the controller but test CRD validation which happens at the API server level

use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;

use crate::suite_test::*;

/// testVirtualIP is the virtual IP used in tests for HighlyAvailable control plane
const TEST_VIRTUAL_IP: &str = "192.168.1.100";

const OCP_RELEASE_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi";

mod dpfhcpprovisioner_crd_schema_validation_tests {
    use super::*;

    async fn after_each(s: &Suite) {
        // Clean up any resources created during tests
        let provisioners: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            s.k8s_client.list_all().await.unwrap_or_default();
        for provisioner in &provisioners {
            let _ = s.k8s_client.delete(provisioner).await;
        }
    }

    fn it<F>(body: F)
    where
        F: AsyncFnOnce(&'static Suite),
    {
        run(async move |s| spec(body(s), after_each(s)).await);
    }

    /// The spec shared by most specs (`DPUClusterRef{test-dpu, default}`, `test-ssh-key`, ...).
    fn spec_base() -> provisioningv1alpha1::DPFHCPProvisionerSpec {
        provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: dpu_cluster_ref("test-dpu", "default"),
            ocp_release_image: OCP_RELEASE_IMAGE.into(),
            ssh_key_secret_ref: local_ref("test-ssh-key"),
            pull_secret_ref: local_ref("test-pull-secret"),
            ..Default::default()
        }
    }

    fn provisioner(
        name: &str,
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec,
    ) -> provisioningv1alpha1::DPFHCPProvisioner {
        provisioningv1alpha1::DPFHCPProvisioner {
            metadata: object_meta(name, "default"),
            spec,
            ..Default::default()
        }
    }

    mod base_domain_validation {
        use super::*;

        #[test]
        fn should_accept_valid_base_domain_patterns() {
            it(async |s| {
                let valid_domains = [
                    "clusters.example.com",
                    "test.example.com",
                    "sub.domain.example.com",
                    "a-b.example.com",
                    "test.co",
                    "long-subdomain-name.example.org",
                ];

                for (i, domain) in valid_domains.iter().enumerate() {
                    let mut p = provisioner(
                        &format!("valid-domain-test-{i}"),
                        provisioningv1alpha1::DPFHCPProvisionerSpec {
                            dpu_deployment_ref: dpu_deployment_ref(
                                "test-dpu-deployment",
                                "default",
                            ),
                            base_domain: domain.to_string(),
                            etcd_storage_class: "test-storage-class".into(),
                            control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                            ..spec_base()
                        },
                    );
                    let res = s.k8s_client.create(&mut p).await;
                    assert!(res.is_ok(), "Domain {domain:?} should be valid: {res:?}");
                    let _ = s.k8s_client.delete(&p).await;
                }
            });
        }

        #[test]
        fn should_reject_invalid_base_domain_patterns() {
            it(async |s| {
                let invalid_domains = [
                    "UPPERCASE.example.com", // Uppercase not allowed
                    "example",               // Missing TLD
                    "example.",              // Trailing dot
                    ".example.com",          // Leading dot
                    "example..com",          // Consecutive dots
                    "example .com",          // Space not allowed
                    "-example.com",          // Leading hyphen
                    "example-.com",          // Trailing hyphen
                    "192.168.1.1",           // IP address not allowed
                    "example.com-",          // Trailing hyphen in label
                    "test_domain.com",       // Underscore not allowed
                    "ex",                    // Too short (less than 4 chars)
                ];

                for domain in invalid_domains {
                    let mut p = provisioner(
                        "invalid-domain-test",
                        provisioningv1alpha1::DPFHCPProvisionerSpec {
                            base_domain: domain.to_string(),
                            etcd_storage_class: "test-storage-class".into(),
                            control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                            ..spec_base()
                        },
                    );
                    let res = s.k8s_client.create(&mut p).await;
                    assert!(res.is_err(), "Domain {domain:?} should be invalid");
                    let _ = s.k8s_client.delete(&p).await;
                }
            });
        }

        #[test]
        fn should_reject_base_domain_exceeding_max_length_253_chars() {
            it(async |s| {
                // Create a domain name with >253 characters
                let long_domain = "very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.example.com";

                let mut p = provisioner(
                    "long-domain-test",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        base_domain: long_domain.into(),
                        etcd_storage_class: "test-storage-class".into(),
                        ..spec_base()
                    },
                );
                assert!(s.k8s_client.create(&mut p).await.is_err());
            });
        }

        #[test]
        fn should_reject_cr_missing_required_base_domain_field() {
            it(async |s| {
                let mut p = provisioner(
                    "missing-required",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        dpu_cluster_ref: dpu_cluster_ref("test-dpu", "dpu-system"),
                        // BaseDomain missing (required field)
                        etcd_storage_class: "test-storage-class".into(),
                        control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                        ..spec_base()
                    },
                );

                assert!(
                    s.k8s_client.create(&mut p).await.is_err(),
                    "Missing required field should be rejected by CRD validation"
                );
            });
        }
    }

    mod enum_validation {
        use super::*;

        #[test]
        fn should_accept_valid_control_plane_availability_policy_values() {
            it(async |s| {
                let valid_policies = [hyperv1::SINGLE_REPLICA, hyperv1::HIGHLY_AVAILABLE];

                for (i, policy) in valid_policies.iter().enumerate() {
                    let mut p = provisioner(
                        &format!("valid-cp-policy-{i}"),
                        provisioningv1alpha1::DPFHCPProvisionerSpec {
                            dpu_deployment_ref: dpu_deployment_ref(
                                "test-dpu-deployment",
                                "default",
                            ),
                            base_domain: "test.example.com".into(),
                            etcd_storage_class: "test-storage-class".into(),
                            control_plane_availability_policy: policy.to_string(),
                            ..spec_base()
                        },
                    );
                    // Add VIP if HighlyAvailable
                    if *policy == hyperv1::HIGHLY_AVAILABLE {
                        p.spec.virtual_ip = TEST_VIRTUAL_IP.into();
                    }
                    let res = s.k8s_client.create(&mut p).await;
                    assert!(res.is_ok(), "Policy {policy:?} should be valid: {res:?}");
                    let _ = s.k8s_client.delete(&p).await;
                }
            });
        }

        #[test]
        fn should_reject_invalid_control_plane_availability_policy_values() {
            it(async |s| {
                let mut p = provisioner(
                    "invalid-cp-policy",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        base_domain: "test.example.com".into(),
                        etcd_storage_class: "test-storage-class".into(),
                        control_plane_availability_policy: "InvalidValue".into(),
                        ..spec_base()
                    },
                );
                assert!(s.k8s_client.create(&mut p).await.is_err());
            });
        }
    }

    mod default_values_application {
        use super::*;

        #[test]
        fn should_apply_default_values_when_fields_are_omitted() {
            it(async |s| {
                let mut p = provisioner(
                    "test-defaults",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", "default"),
                        base_domain: "test.example.com".into(),
                        // Optional fields omitted - should get defaults
                        ..spec_base()
                    },
                );

                assert!(
                    s.k8s_client.create(&mut p).await.is_err(),
                    "Should reject HighlyAvailable (default) without VIP"
                );

                // Try again with VIP for HighlyAvailable default
                p.metadata.name = Some("test-defaults-with-vip".into());
                p.spec.virtual_ip = TEST_VIRTUAL_IP.into();
                s.k8s_client.create(&mut p).await.expect("create with VIP");

                // Retrieve the created resource to verify defaults were applied
                let created: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, "test-defaults-with-vip", "default")
                        .await
                        .expect("get");

                // Verify defaults were applied by the API server
                assert_eq!(
                    created.spec.control_plane_availability_policy,
                    hyperv1::HIGHLY_AVAILABLE
                );

                let _ = s.k8s_client.delete(&p).await;
            });
        }
    }

    mod vip_requirement_based_on_control_plane_availability_policy {
        use super::*;

        #[test]
        fn should_reject_highly_available_without_vip() {
            it(async |s| {
                let mut p = provisioner(
                    "ha-without-vip",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", "default"),
                        base_domain: "test.example.com".into(),
                        control_plane_availability_policy: hyperv1::HIGHLY_AVAILABLE.into(),
                        // VirtualIP not set - should be rejected
                        ..spec_base()
                    },
                );

                let err = s
                    .k8s_client
                    .create(&mut p)
                    .await
                    .expect_err("Should reject HighlyAvailable without VIP");
                assert!(err.to_string().contains("virtualIP is required"), "{err}");
            });
        }

        #[test]
        fn should_accept_highly_available_with_vip() {
            it(async |s| {
                let mut p = provisioner(
                    "ha-with-vip",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", "default"),
                        base_domain: "test.example.com".into(),
                        control_plane_availability_policy: hyperv1::HIGHLY_AVAILABLE.into(),
                        virtual_ip: TEST_VIRTUAL_IP.into(),
                        ..spec_base()
                    },
                );

                let res = s.k8s_client.create(&mut p).await;
                assert!(
                    res.is_ok(),
                    "Should accept HighlyAvailable with VIP: {res:?}"
                );
                let _ = s.k8s_client.delete(&p).await;
            });
        }

        #[test]
        fn should_accept_single_replica_without_vip() {
            it(async |s| {
                let mut p = provisioner(
                    "single-without-vip",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", "default"),
                        base_domain: "test.example.com".into(),
                        control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                        // VirtualIP not set - should be allowed for SingleReplica
                        ..spec_base()
                    },
                );

                let res = s.k8s_client.create(&mut p).await;
                assert!(
                    res.is_ok(),
                    "Should accept SingleReplica without VIP: {res:?}"
                );
                let _ = s.k8s_client.delete(&p).await;
            });
        }

        #[test]
        fn should_accept_single_replica_with_vip() {
            it(async |s| {
                let mut p = provisioner(
                    "single-with-vip",
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", "default"),
                        base_domain: "test.example.com".into(),
                        control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                        virtual_ip: TEST_VIRTUAL_IP.into(),
                        ..spec_base()
                    },
                );

                let res = s.k8s_client.create(&mut p).await;
                assert!(res.is_ok(), "Should accept SingleReplica with VIP: {res:?}");
                let _ = s.k8s_client.delete(&p).await;
            });
        }
    }

    mod field_immutability_validation {
        use super::*;

        async fn before_each(s: &Suite) {
            let mut p = provisioner(
                "immutability-test",
                provisioningv1alpha1::DPFHCPProvisionerSpec {
                    dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", "default"),
                    base_domain: "test.example.com".into(),
                    etcd_storage_class: "test-storage-class".into(),
                    control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                    ..spec_base()
                },
            );
            s.k8s_client.create(&mut p).await.expect("create");
        }

        async fn after_each_inner(s: &Suite) {
            // Clean up the provisioner created in BeforeEach
            // Must delete and wait for finalizer cleanup before next test
            let mut p = provisioningv1alpha1::DPFHCPProvisioner {
                metadata: object_meta("immutability-test", "default"),
                ..Default::default()
            };
            let _ = s.k8s_client.delete(&p).await;
            // Wait for deletion to complete (finalizer processing)
            eventually(secs(10), millis(500), async || {
                match s
                    .k8s_client
                    .get_into(
                        &ctrlrt::ObjectKey::new("default", "immutability-test"),
                        &mut p,
                    )
                    .await
                {
                    Err(_) => Ok(()),
                    Ok(()) => Err("provisioner still exists".to_string()),
                }
            })
            .await;
        }

        fn it_immutability<F>(body: F)
        where
            F: AsyncFnOnce(&'static Suite),
        {
            run(async move |s| {
                spec(
                    async {
                        before_each(s).await;
                        spec(body(s), after_each_inner(s)).await;
                    },
                    after_each(s),
                )
                .await
            });
        }

        /// `Eventually(func() error { Get; DeepCopy; mutate; Update }, 5s, 100ms)`
        async fn update_eventually<F>(s: &Suite, mutate: F) -> ctrlrt::Result<()>
        where
            F: Fn(&mut provisioningv1alpha1::DPFHCPProvisioner),
        {
            let mut fresh: provisioningv1alpha1::DPFHCPProvisioner = s
                .k8s_client
                .get(&ctrlrt::ObjectKey::new("default", "immutability-test"))
                .await?;
            mutate(&mut fresh);
            s.k8s_client.update(&mut fresh).await
        }

        /// `.Should(MatchError(ContainSubstring(msg)))`
        async fn eventually_rejected<F>(s: &Suite, msg: &str, mutate: F)
        where
            F: Fn(&mut provisioningv1alpha1::DPFHCPProvisioner),
        {
            eventually(secs(5), millis(100), async || {
                match update_eventually(s, &mutate).await {
                    Err(e) if e.to_string().contains(msg) => Ok(()),
                    Err(e) => Err(format!("expected error containing {msg:?}, got {e}")),
                    Ok(()) => Err(format!("expected error containing {msg:?}, got success")),
                }
            })
            .await;
        }

        /// `.Should(Succeed())`
        async fn eventually_accepted<F>(s: &Suite, mutate: F)
        where
            F: Fn(&mut provisioningv1alpha1::DPFHCPProvisioner),
        {
            eventually(secs(5), millis(100), async || {
                update_eventually(s, &mutate)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
        }

        #[test]
        fn should_reject_updates_to_dpu_cluster_ref() {
            it_immutability(async |s| {
                // Wrap in Eventually to handle race with controller
                eventually_rejected(s, "dpuClusterRef is immutable", |p| {
                    p.spec.dpu_cluster_ref.name = "different-dpu".into();
                })
                .await;
            });
        }

        #[test]
        fn should_reject_updates_to_base_domain() {
            it_immutability(async |s| {
                eventually_rejected(s, "baseDomain is immutable", |p| {
                    p.spec.base_domain = "updated.example.com".into();
                })
                .await;
            });
        }

        #[test]
        fn should_allow_updates_to_ssh_key_secret_ref_mutable_for_credential_rotation() {
            it_immutability(async |s| {
                // sshKeySecretRef is intentionally mutable so users can rotate SSH keys
                eventually_accepted(s, |p| {
                    p.spec.ssh_key_secret_ref.name = "different-ssh-key".into();
                })
                .await;
            });
        }

        #[test]
        fn should_allow_updates_to_pull_secret_ref_mutable_for_credential_rotation() {
            it_immutability(async |s| {
                // pullSecretRef is intentionally mutable so users can rotate pull secrets
                eventually_accepted(s, |p| {
                    p.spec.pull_secret_ref.name = "different-pull-secret".into();
                })
                .await;
            });
        }

        #[test]
        fn should_reject_updates_to_etcd_storage_class() {
            it_immutability(async |s| {
                eventually_rejected(s, "etcdStorageClass is immutable", |p| {
                    p.spec.etcd_storage_class = "different-storage-class".into();
                })
                .await;
            });
        }

        #[test]
        fn should_reject_updates_to_control_plane_availability_policy() {
            it_immutability(async |s| {
                eventually_rejected(s, "controlPlaneAvailabilityPolicy is immutable", |p| {
                    p.spec.control_plane_availability_policy = hyperv1::HIGHLY_AVAILABLE.into();
                    p.spec.virtual_ip = TEST_VIRTUAL_IP.into(); // Required for HighlyAvailable
                })
                .await;
            });
        }

        #[test]
        fn should_reject_updates_to_virtual_ip() {
            it_immutability(async |s| {
                // First set the VIP - wrap in Eventually to handle race with controller
                eventually_accepted(s, |p| {
                    p.spec.virtual_ip = TEST_VIRTUAL_IP.into();
                })
                .await;

                // Now try to change it - should fail with immutability error
                eventually_rejected(s, "virtualIP is immutable", |p| {
                    p.spec.virtual_ip = "192.168.1.101".into();
                })
                .await;
            });
        }

        #[test]
        fn should_allow_updates_to_ocp_release_image_mutable() {
            it_immutability(async |s| {
                eventually_accepted(s, |p| {
                    p.spec.ocp_release_image =
                        "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi".into();
                })
                .await;
            });
        }
    }
}
