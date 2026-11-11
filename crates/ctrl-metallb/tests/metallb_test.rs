//! Port of `internal/controller/metallb/metallb_test.go` (Ginkgo suite "MetalLB Suite",
//! bootstrapped by `suite_test.go`; here every spec is a plain `#[tokio::test]`).
//!
//! Test paths mirror the Ginkgo spec path: `Describe` / `Context` → nested modules,
//! `It` → test function. See `TEST_MAPPING.md`.

use std::collections::BTreeMap;
use std::sync::Arc;

use apis::hypershift::v1beta1 as hyperv1;
use apis::metallb::v1beta1 as metallbv1beta1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use common::OperatorConfig;
use ctrl_metallb::{CleanupHandler, MetalLBManager};
use ctrlrt::meta::{CONDITION_TRUE, find_status_condition};
use ctrlrt::{Client, Ctx, FakeRecorder, Object, ObjectKey, Recorder};
use ctrlrt_fake::{FakeClientBuilder, InitObject, Scheme};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

/// BeforeEach state.
struct Suite {
    ctx: Ctx,
    recorder: Arc<FakeRecorder>,
    scheme: Scheme,
    operator_config: OperatorConfig,
}

/// BeforeEach
fn before_each() -> Suite {
    // Setup scheme
    let scheme = Scheme::new()
        // provisioningv1alpha1.AddToScheme
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .register::<provisioningv1alpha1::DPFHCPProvisionerConfig>()
        // metallbv1beta1.AddToScheme
        .register::<metallbv1beta1::IPAddressPool>()
        .register::<metallbv1beta1::L2Advertisement>()
        // hyperv1.AddToScheme
        .register::<hyperv1::HostedCluster>()
        .register::<hyperv1::NodePool>();

    Suite {
        ctx: Ctx::background(),
        recorder: Arc::new(FakeRecorder::new(100)),
        scheme,
        // Default operator config with MetalLB enabled
        operator_config: OperatorConfig {
            disable_metal_lb: false,
            ..Default::default()
        },
    }
}

impl Suite {
    fn recorder(&self) -> Recorder {
        self.recorder.clone()
    }

    fn client(&self, objs: Vec<InitObject>, status: bool) -> Client {
        let mut b = FakeClientBuilder::new()
            .with_scheme(self.scheme.clone())
            .with_objects(objs);
        if status {
            b = b.with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>();
        }
        b.build()
    }

    /// `Eventually(recorder.Events).Should(Receive(ContainSubstring(s)))`: receive events
    /// until one matches.
    fn eventually_receive_containing(&self, s: &str) {
        loop {
            match self.recorder.try_recv() {
                Some(e) if e.contains(s) => return,
                Some(_) => continue,
                None => panic!("no event containing {s:?} was received"),
            }
        }
    }
}

fn provisioner(policy: &str, vip: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioningv1alpha1::DPFHCPProvisioner {
        metadata: ObjectMeta {
            name: Some("test-provisioner".into()),
            namespace: Some("test-ns".into()),
            ..Default::default()
        },
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            control_plane_availability_policy: policy.into(),
            virtual_ip: vip.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn bare_provisioner() -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioner("", "")
}

fn owner_labels() -> Option<BTreeMap<String, String>> {
    Some(BTreeMap::from([
        (
            common::LABEL_DPFHCP_PROVISIONER_NAME.to_string(),
            "test-provisioner".to_string(),
        ),
        (
            common::LABEL_DPFHCP_PROVISIONER_NAMESPACE.to_string(),
            "test-ns".to_string(),
        ),
    ]))
}

fn pool(
    labels: Option<BTreeMap<String, String>>,
    spec: metallbv1beta1::IPAddressPoolSpec,
) -> metallbv1beta1::IPAddressPool {
    metallbv1beta1::IPAddressPool {
        metadata: ObjectMeta {
            name: Some("test-provisioner".into()),
            namespace: Some(common::OPENSHIFT_OPERATORS_NAMESPACE.into()),
            labels,
            ..Default::default()
        },
        spec,
        ..Default::default()
    }
}

fn full_pool_spec(address: &str) -> metallbv1beta1::IPAddressPoolSpec {
    metallbv1beta1::IPAddressPoolSpec {
        addresses: Some(vec![address.into()]),
        service_allocation: Some(metallbv1beta1::ServiceAllocation {
            namespaces: vec!["clusters-test-provisioner".into()],
            ..Default::default()
        }),
        auto_assign: Some(true),
        ..Default::default()
    }
}

fn advert(
    labels: Option<BTreeMap<String, String>>,
    pools: Vec<String>,
) -> metallbv1beta1::L2Advertisement {
    metallbv1beta1::L2Advertisement {
        metadata: ObjectMeta {
            name: Some("advertise-test-provisioner".into()),
            namespace: Some(common::OPENSHIFT_OPERATORS_NAMESPACE.into()),
            labels,
            ..Default::default()
        },
        spec: metallbv1beta1::L2AdvertisementSpec {
            ip_address_pools: pools,
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn get_pool(c: &Client) -> ctrlrt::Result<metallbv1beta1::IPAddressPool> {
    c.get(&ObjectKey::new(
        common::OPENSHIFT_OPERATORS_NAMESPACE,
        "test-provisioner",
    ))
    .await
}

async fn get_advert(c: &Client) -> ctrlrt::Result<metallbv1beta1::L2Advertisement> {
    c.get(&ObjectKey::new(
        common::OPENSHIFT_OPERATORS_NAMESPACE,
        "advertise-test-provisioner",
    ))
    .await
}

mod metal_lb_manager {
    use super::*;

    mod configure_metal_lb {
        use super::*;

        mod when_should_expose_through_load_balancer_is_false {
            use super::*;

            #[tokio::test]
            async fn should_skip_metal_lb_configuration() {
                let s = before_each();
                // Given: DPFHCPProvisioner with SingleReplica and no VirtualIP
                let mut provisioner = provisioner(hyperv1::SINGLE_REPLICA, ""); // No VIP provided

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB is called
                let res = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await;

                // Then: No error, no requeue
                assert!(res.is_ok(), "unexpected error: {:?}", res.err());

                // No MetalLB resources should be created
                let pool_list = fake_client
                    .list_all::<metallbv1beta1::IPAddressPool>()
                    .await
                    .unwrap();
                assert!(pool_list.is_empty());
            }
        }

        mod when_metal_lb_resources_do_not_exist {
            use super::*;

            #[tokio::test]
            async fn should_create_ip_address_pool_and_l2_advertisement() {
                let s = before_each();
                // Given: DPFHCPProvisioner with HighlyAvailable and VirtualIP
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB is called
                let res = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await;

                // Then: Success
                assert!(res.is_ok(), "unexpected error: {:?}", res.err());

                // IPAddressPool created
                let pool = get_pool(&fake_client).await.unwrap();
                assert_eq!(
                    pool.spec.addresses,
                    Some(vec!["192.168.1.100/32".to_string()])
                );
                assert_eq!(
                    pool.spec.service_allocation.as_ref().unwrap().namespaces,
                    vec!["test-ns-test-provisioner".to_string()]
                );
                assert!(pool.spec.auto_assign.unwrap());
                assert_eq!(
                    pool.labels()[common::LABEL_DPFHCP_PROVISIONER_NAME],
                    "test-provisioner"
                );
                assert_eq!(
                    pool.labels()[common::LABEL_DPFHCP_PROVISIONER_NAMESPACE],
                    "test-ns"
                );

                // L2Advertisement created
                let advert = get_advert(&fake_client).await.unwrap();
                assert_eq!(
                    advert.spec.ip_address_pools,
                    vec!["test-provisioner".to_string()]
                );
                assert_eq!(
                    advert.labels()[common::LABEL_DPFHCP_PROVISIONER_NAME],
                    "test-provisioner"
                );
                assert_eq!(
                    advert.labels()[common::LABEL_DPFHCP_PROVISIONER_NAMESPACE],
                    "test-ns"
                );

                // Condition set to True
                let cond = find_status_condition(
                    &provisioner.status.conditions,
                    provisioningv1alpha1::METAL_LB_CONFIGURED,
                )
                .expect("condition");
                assert_eq!(cond.status, CONDITION_TRUE);
                assert_eq!(cond.reason, "MetalLBReady");

                // Event emitted
                s.eventually_receive_containing("MetalLBConfigured");
            }
        }

        mod when_metal_lb_resources_exist_with_correct_spec {
            use super::*;

            #[tokio::test]
            async fn should_not_update_resources() {
                let s = before_each();
                // Given: DPFHCPProvisioner with existing MetalLB resources
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let existing_pool = pool(owner_labels(), full_pool_spec("192.168.1.100/32"));
                let existing_advert = advert(owner_labels(), vec!["test-provisioner".into()]);

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&existing_pool),
                        ctrlrt_fake::obj(&existing_advert),
                    ],
                    true,
                );

                let manager = MetalLBManager::new(fake_client, s.recorder());

                // When: ConfigureMetalLB is called
                let res = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await;

                // Then: Success, no updates
                assert!(res.is_ok(), "unexpected error: {:?}", res.err());

                // Condition set to True
                let cond = find_status_condition(
                    &provisioner.status.conditions,
                    provisioningv1alpha1::METAL_LB_CONFIGURED,
                )
                .expect("condition");
                assert_eq!(cond.status, CONDITION_TRUE);
            }
        }

        mod when_metal_lb_resources_exist_with_incorrect_spec_drift {
            use super::*;

            #[tokio::test]
            async fn should_update_ip_address_pool_to_correct_drift() {
                let s = before_each();
                // Given: DPFHCPProvisioner with drifted IPAddressPool
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let drifted_pool = pool(owner_labels(), full_pool_spec("192.168.1.200/32")); // Wrong IP!
                let existing_advert = advert(owner_labels(), vec!["test-provisioner".into()]);

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&drifted_pool),
                        ctrlrt_fake::obj(&existing_advert),
                    ],
                    true,
                );

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB is called
                let res = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await;

                // Then: Success
                assert!(res.is_ok(), "unexpected error: {:?}", res.err());

                // IPAddressPool corrected
                let pool = get_pool(&fake_client).await.unwrap();
                assert_eq!(
                    pool.spec.addresses,
                    Some(vec!["192.168.1.100/32".to_string()])
                );

                // Drift correction event emitted
                s.eventually_receive_containing("MetalLBDriftCorrected");
            }

            #[tokio::test]
            async fn should_update_l2_advertisement_to_correct_drift() {
                let s = before_each();
                // Given: DPFHCPProvisioner with drifted L2Advertisement
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let existing_pool = pool(owner_labels(), full_pool_spec("192.168.1.100/32"));
                let drifted_advert = advert(owner_labels(), vec!["wrong-pool".into()]); // Wrong pool reference!

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&existing_pool),
                        ctrlrt_fake::obj(&drifted_advert),
                    ],
                    true,
                );

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB is called
                let res = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await;

                // Then: Success
                assert!(res.is_ok(), "unexpected error: {:?}", res.err());

                // L2Advertisement corrected
                let advert = get_advert(&fake_client).await.unwrap();
                assert_eq!(
                    advert.spec.ip_address_pools,
                    vec!["test-provisioner".to_string()]
                );

                // Drift correction event emitted
                s.eventually_receive_containing("MetalLBDriftCorrected");
            }
        }

        mod when_disable_metal_lb_is_true_in_operator_config {
            use super::*;

            #[tokio::test]
            async fn should_skip_metal_lb_configuration_and_set_condition() {
                let s = before_each();
                // Given: DPFHCPProvisioner with VirtualIP but MetalLB disabled in config
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // Override operator config to disable MetalLB
                let config_with_disabled = OperatorConfig {
                    disable_metal_lb: true,
                    ..Default::default()
                };

                // When: ConfigureMetalLB is called
                let res = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &config_with_disabled)
                    .await;

                // Then: No error
                assert!(res.is_ok(), "unexpected error: {:?}", res.err());

                // No MetalLB resources should be created
                let pool_list = fake_client
                    .list_all::<metallbv1beta1::IPAddressPool>()
                    .await
                    .unwrap();
                assert!(pool_list.is_empty());

                let advert_list = fake_client
                    .list_all::<metallbv1beta1::L2Advertisement>()
                    .await
                    .unwrap();
                assert!(advert_list.is_empty());

                // Condition set to True with MetalLBDisabled reason
                let updated_provisioner: provisioningv1alpha1::DPFHCPProvisioner = fake_client
                    .get(&ObjectKey::new("test-ns", "test-provisioner"))
                    .await
                    .unwrap();

                let cond = find_status_condition(
                    &updated_provisioner.status.conditions,
                    provisioningv1alpha1::METAL_LB_CONFIGURED,
                )
                .expect("condition");
                assert_eq!(cond.status, CONDITION_TRUE);
                assert_eq!(cond.reason, "MetalLBDisabled");
                assert_eq!(cond.message, "MetalLB disabled in operator configuration");
            }
        }
    }

    mod cleanup_handler {
        use super::*;

        mod when_both_metal_lb_resources_exist {
            use super::*;

            #[tokio::test]
            async fn should_delete_ip_address_pool_and_l2_advertisement() {
                let s = before_each();
                // Given: DPFHCPProvisioner with existing MetalLB resources
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let existing_pool = pool(
                    owner_labels(),
                    metallbv1beta1::IPAddressPoolSpec {
                        addresses: Some(vec!["192.168.1.100/32".into()]),
                        ..Default::default()
                    },
                );
                let existing_advert = advert(owner_labels(), vec!["test-provisioner".into()]);

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&existing_pool),
                        ctrlrt_fake::obj(&existing_advert),
                    ],
                    false,
                );

                let handler = CleanupHandler::new(fake_client, s.recorder());

                // When: Cleanup is called first time
                let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: Requeues because it initiated IPAddressPool deletion
                assert!(result.requeue_after > std::time::Duration::ZERO);

                // When: Cleanup is called second time (IPAddressPool deleted, now deletes L2Advertisement)
                let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: Requeues because it initiated L2Advertisement deletion
                assert!(result.requeue_after > std::time::Duration::ZERO);

                // When: Cleanup is called third time (all resources deleted)
                let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: Success - all resources gone
                assert!(result.requeue_after.is_zero());

                // Event emitted
                s.eventually_receive_containing("MetalLBCleanupComplete");
            }
        }

        mod when_metal_lb_resources_dont_exist {
            use super::*;

            #[tokio::test]
            async fn should_return_nil_idempotent() {
                let s = before_each();
                // Given: DPFHCPProvisioner with no MetalLB resources
                let mut provisioner = bare_provisioner();

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], false);

                let handler = CleanupHandler::new(fake_client, s.recorder());

                // When: Cleanup is called
                let result = handler.cleanup(&s.ctx, &provisioner).await;

                // Then: Success (idempotent)
                let result = result.unwrap();
                assert!(result.requeue_after.is_zero());
            }
        }

        mod when_only_ip_address_pool_exists {
            use super::*;

            #[tokio::test]
            async fn should_delete_ip_address_pool_and_return_success() {
                let s = before_each();
                // Given: DPFHCPProvisioner with only IPAddressPool
                let mut provisioner = bare_provisioner();

                let existing_pool = pool(owner_labels(), Default::default());

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&existing_pool),
                    ],
                    false,
                );

                let handler = CleanupHandler::new(fake_client, s.recorder());

                // When: Cleanup is called first time
                let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: Requeues (waiting for deletion)
                assert!(result.requeue_after > std::time::Duration::ZERO);

                // When: Called again (fake client already deleted the resource)
                let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: Success
                assert!(result.requeue_after.is_zero());
            }
        }

        mod when_only_l2_advertisement_exists {
            use super::*;

            #[tokio::test]
            async fn should_delete_l2_advertisement_and_return_success() {
                let s = before_each();
                // Given: DPFHCPProvisioner with only L2Advertisement
                let mut provisioner = bare_provisioner();

                let existing_advert = advert(owner_labels(), vec![]);

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&existing_advert),
                    ],
                    false,
                );

                let handler = CleanupHandler::new(fake_client, s.recorder());

                // When: Cleanup is called first time
                let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: Requeues (waiting for deletion)
                assert!(result.requeue_after > std::time::Duration::ZERO);

                // When: Called again (fake client already deleted the resource)
                let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: Success
                assert!(result.requeue_after.is_zero());
            }
        }

        mod when_cleanup_is_called_multiple_times_idempotency {
            use super::*;

            #[tokio::test]
            async fn should_succeed_on_subsequent_calls() {
                let s = before_each();
                // Given: DPFHCPProvisioner with no resources
                let mut provisioner = bare_provisioner();

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], false);

                let handler = CleanupHandler::new(fake_client, s.recorder());

                // When: Cleanup is called multiple times
                handler.cleanup(&s.ctx, &provisioner).await.unwrap();
                handler.cleanup(&s.ctx, &provisioner).await.unwrap();
                handler.cleanup(&s.ctx, &provisioner).await.unwrap();

                // Then: All succeed (idempotent)
            }
        }
    }

    mod helper_functions {
        use super::*;

        mod build_ip_address_pool {
            use super::*;

            #[tokio::test]
            async fn should_build_correct_ip_address_pool_spec() {
                let s = before_each();
                // Given: DPFHCPProvisioner
                let mut provisioner = provisioner("", "10.0.0.100");

                let fake_client = s.client(vec![], false);

                let _manager = MetalLBManager::new(fake_client, s.recorder());

                // When: We would call buildIPAddressPool (tested indirectly through ConfigureMetalLB)
                // Create the resource and verify
                provisioner.spec.control_plane_availability_policy =
                    hyperv1::HIGHLY_AVAILABLE.into();
                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());
                manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await
                    .unwrap();

                // Then: Verify the created pool matches expected spec
                let pool = get_pool(&fake_client).await.unwrap();
                assert_eq!(pool.spec.addresses, Some(vec!["10.0.0.100/32".to_string()]));
                assert_eq!(
                    pool.spec.service_allocation.as_ref().unwrap().namespaces,
                    vec!["test-ns-test-provisioner".to_string()]
                );
                assert!(pool.spec.auto_assign.unwrap());
            }
        }

        mod build_l2_advertisement {
            use super::*;

            #[tokio::test]
            async fn should_build_correct_l2_advertisement_spec() {
                let s = before_each();
                // Given: DPFHCPProvisioner
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "10.0.0.100");

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB creates the advertisement
                manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await
                    .unwrap();

                // Then: Verify the created advertisement matches expected spec
                let advert = get_advert(&fake_client).await.unwrap();
                assert_eq!(
                    advert.spec.ip_address_pools,
                    vec!["test-provisioner".to_string()]
                );
            }
        }
    }

    mod edge_cases {
        use super::*;

        mod when_dpfhcp_provisioner_has_single_replica_with_virtual_ip {
            use super::*;

            #[tokio::test]
            async fn should_configure_metal_lb() {
                let s = before_each();
                // Given: SingleReplica with VirtualIP provided
                let mut provisioner = provisioner(hyperv1::SINGLE_REPLICA, "192.168.1.100");

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB is called
                let res = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await;

                // Then: MetalLB resources created
                assert!(res.is_ok(), "unexpected error: {:?}", res.err());

                get_pool(&fake_client).await.unwrap();
            }
        }

        mod when_ip_address_pool_exists_without_ownership_labels_ownership_conflict {
            use super::*;

            #[tokio::test]
            async fn should_return_error_and_not_take_ownership() {
                let s = before_each();
                // Given: IPAddressPool exists but lacks ownership labels (created by user/other operator)
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let unowned_pool = pool(
                    Some(BTreeMap::new()), // Missing ownership labels!
                    metallbv1beta1::IPAddressPoolSpec {
                        addresses: Some(vec!["192.168.1.100/32".into()]),
                        ..Default::default()
                    },
                );

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&unowned_pool),
                    ],
                    true,
                );

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB is called
                let err = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await
                    .expect_err("expected error");

                // Then: Error returned - won't take ownership
                assert!(err.to_string().contains("not owned by DPFHCPProvisioner"));
                assert!(err.to_string().contains("missing ownership labels"));

                // Pool remains unchanged (labels not added)
                let pool = get_pool(&fake_client).await.unwrap();
                assert!(pool.labels().is_empty()); // Labels still empty - didn't claim ownership
            }
        }

        mod when_l2_advertisement_exists_without_ownership_labels_ownership_conflict {
            use super::*;

            #[tokio::test]
            async fn should_return_error_and_not_take_ownership() {
                let s = before_each();
                // Given: L2Advertisement exists but lacks ownership labels
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                // IPAddressPool with correct ownership (so we get past that check)
                let owned_pool = pool(owner_labels(), full_pool_spec("192.168.1.100/32"));

                // L2Advertisement WITHOUT ownership labels
                let unowned_advert = advert(
                    Some(BTreeMap::new()), // Missing ownership labels!
                    vec!["test-provisioner".into()],
                );

                let fake_client = s.client(
                    vec![
                        ctrlrt_fake::obj_mut(&mut provisioner),
                        ctrlrt_fake::obj(&owned_pool),
                        ctrlrt_fake::obj(&unowned_advert),
                    ],
                    true,
                );

                let manager = MetalLBManager::new(fake_client.clone(), s.recorder());

                // When: ConfigureMetalLB is called
                let err = manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await
                    .expect_err("expected error");

                // Then: Error returned - won't take ownership
                assert!(err.to_string().contains("not owned by DPFHCPProvisioner"));
                assert!(err.to_string().contains("missing ownership labels"));

                // Advertisement remains unchanged
                let advert = get_advert(&fake_client).await.unwrap();
                assert!(advert.labels().is_empty()); // Labels still empty
            }
        }
    }

    mod status_condition_management {
        use super::*;

        mod when_metal_lb_configuration_succeeds {
            use super::*;

            #[tokio::test]
            async fn should_set_metal_lb_configured_condition_to_true_with_correct_reason() {
                let s = before_each();
                // Given: Successful MetalLB configuration
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client, s.recorder());

                // When: ConfigureMetalLB succeeds
                manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await
                    .unwrap();

                // Then: Condition is True with MetalLBReady reason
                let cond = find_status_condition(
                    &provisioner.status.conditions,
                    provisioningv1alpha1::METAL_LB_CONFIGURED,
                )
                .expect("condition");
                assert_eq!(cond.status, CONDITION_TRUE);
                assert_eq!(cond.reason, "MetalLBReady");
                assert_eq!(cond.message, "MetalLB configured successfully");
                assert_eq!(
                    cond.observed_generation.unwrap_or(0),
                    provisioner.generation()
                );
            }
        }

        mod when_condition_changes {
            use super::*;

            #[tokio::test]
            async fn should_emit_event_only_when_condition_actually_changes() {
                let s = before_each();
                // Given: DPFHCPProvisioner
                let mut provisioner = provisioner(hyperv1::HIGHLY_AVAILABLE, "192.168.1.100");

                let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], true);

                let manager = MetalLBManager::new(fake_client, s.recorder());

                // When: ConfigureMetalLB is called first time
                manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await
                    .unwrap();

                // Then: Event emitted
                s.eventually_receive_containing("MetalLBConfigured");

                // When: ConfigureMetalLB is called again (no change)
                manager
                    .configure_metal_lb(&s.ctx, &mut provisioner, &s.operator_config)
                    .await
                    .unwrap();

                // Then: No additional event (condition didn't change)
                assert_eq!(s.recorder.try_recv(), None);
            }
        }
    }
}

/// Not a Go spec: checks the `ctrl_finalizer::CleanupHandler` impl delegates to the
/// inherent methods (Go: `var _ finalizer.CleanupHandler = &CleanupHandler{}` usage).
#[tokio::test]
async fn finalizer_cleanup_handler_trait_impl() {
    let s = before_each();
    let mut provisioner = bare_provisioner();
    let existing_pool = pool(owner_labels(), Default::default());
    let fake_client = s.client(
        vec![
            ctrlrt_fake::obj_mut(&mut provisioner),
            ctrlrt_fake::obj(&existing_pool),
        ],
        false,
    );
    let handler: Box<dyn ctrl_finalizer::CleanupHandler> =
        Box::new(CleanupHandler::new(fake_client, s.recorder()));
    assert_eq!(handler.name(), "metallb");
    let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();
    assert_eq!(result.requeue_after, std::time::Duration::from_secs(10));
    let result = handler.cleanup(&s.ctx, &provisioner).await.unwrap();
    assert!(result.is_zero());
    s.eventually_receive_containing(
        "Normal MetalLBCleanupComplete MetalLB resources cleaned up successfully",
    );
}
