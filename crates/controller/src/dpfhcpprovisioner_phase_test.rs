//! Port of `internal/controller/dpfhcpprovisioner_phase_test.go`.

use std::sync::Arc;

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::openshift::config::v1 as configv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::{self, CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::{Ctx, FakeRecorder, Recorder};
use ctrlrt_fake::{FakeClientBuilder, InitObject, Scheme, obj, obj_mut};
use k8s_openapi::api::core::v1::{ConfigMap, Secret};

use crate::DPFHCPProvisionerReconciler;
use crate::dpfhcpprovisioner_controller::is_upgrading;
use crate::ignition;
use crate::suite_test::*;

fn hc_history(state: &str, version: &str, image: &str) -> hyperv1::ControlPlaneUpdateHistory {
    hyperv1::ControlPlaneUpdateHistory {
        state: state.into(),
        version: version.into(),
        image: image.into(),
        ..Default::default()
    }
}

// Phase Transition Tests
// These tests observe controller behavior and verify that the controller
// correctly computes phases based on conditions (NOT manual status manipulation)
mod dpfhcpprovisioner_phase_transitions {
    use super::*;

    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    const INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

    const TEST_NAMESPACE: &str = "default";
    const DPU_CLUSTER_NAME: &str = "test-dpucluster-phase";
    const PULL_SECRET_NAME: &str = "test-pull-secret-phase";
    const SSH_KEY_SECRET_NAME: &str = "test-ssh-key-phase";
    const OCP_RELEASE_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64";

    async fn before_each(s: &Suite) {
        let c = &s.k8s_client;

        // Create DPUCluster
        let mut dpu_cluster = dpuprovisioningv1alpha1::DPUCluster {
            metadata: object_meta(DPU_CLUSTER_NAME, TEST_NAMESPACE),
            spec: dpuprovisioningv1alpha1::DPUClusterSpec {
                type_: dpuprovisioningv1alpha1::STATIC_CLUSTER.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        c.create(&mut dpu_cluster).await.expect("create DPUCluster");

        // Set DPUCluster phase to Ready
        dpu_cluster.status.phase = dpuprovisioningv1alpha1::PHASE_READY.into();
        c.update_status(&mut dpu_cluster)
            .await
            .expect("update DPUCluster status");

        // Create pull-secret
        c.create(&mut test_pull_secret(PULL_SECRET_NAME, TEST_NAMESPACE))
            .await
            .expect("create pull secret");

        // Create ssh-key - must contain "id_rsa.pub" key as expected by validator
        c.create(&mut test_ssh_key(SSH_KEY_SECRET_NAME, TEST_NAMESPACE))
            .await
            .expect("create ssh key");
    }

    async fn after_each(s: &Suite) {
        let c = &s.k8s_client;

        // Clean up all provisioner resources
        let provisioners: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            c.list_all().await.unwrap_or_default();
        for provisioner in &provisioners {
            let _ = c.delete(provisioner).await;
        }

        // Clean up HostedClusters
        let hcs: Vec<hyperv1::HostedCluster> = c.list_all().await.unwrap_or_default();
        for hc in &hcs {
            let _ = c.delete(hc).await;
        }

        // Clean up DPUCluster
        delete_ignore::<dpuprovisioningv1alpha1::DPUCluster>(c, DPU_CLUSTER_NAME, TEST_NAMESPACE)
            .await;

        // Clean up secrets
        delete_ignore::<Secret>(c, PULL_SECRET_NAME, TEST_NAMESPACE).await;
        delete_ignore::<Secret>(c, SSH_KEY_SECRET_NAME, TEST_NAMESPACE).await;
        delete_ignore::<Secret>(c, "phase-test-ready-admin-kubeconfig", TEST_NAMESPACE).await;

        // Clean up copied secrets in clusters namespace
        delete_ignore::<Secret>(c, "phase-test-pull-secret", "clusters").await;
        delete_ignore::<Secret>(c, "phase-test-ssh-key", "clusters").await;
        delete_ignore::<Secret>(c, "phase-test-etcd-encryption-key", "clusters").await;
    }

    /// Run an `It` of this `Describe` with its `BeforeEach`/`AfterEach`.
    fn it<F>(body: F)
    where
        F: AsyncFnOnce(&'static Suite),
    {
        run(async move |s| {
            before_each(s).await;
            spec(body(s), after_each(s)).await;
        });
    }

    fn valid_provisioner(name: &str, dpu_cluster: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
        provisioningv1alpha1::DPFHCPProvisioner {
            metadata: object_meta(name, TEST_NAMESPACE),
            spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                dpu_cluster_ref: dpu_cluster_ref(dpu_cluster, TEST_NAMESPACE),
                dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", TEST_NAMESPACE),
                base_domain: "test-cluster.example.com".into(),
                ocp_release_image: OCP_RELEASE_IMAGE.into(),
                ssh_key_secret_ref: local_ref(SSH_KEY_SECRET_NAME),
                pull_secret_ref: local_ref(PULL_SECRET_NAME),
                etcd_storage_class: "standard".into(),
                control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    async fn phase_of(s: &Suite, name: &str) -> Result<String, String> {
        Ok(
            get::<provisioningv1alpha1::DPFHCPProvisioner>(&s.k8s_client, name, TEST_NAMESPACE)
                .await
                .map(|p| p.status.phase)
                .unwrap_or_default(),
        )
    }

    async fn eventually_phase(s: &Suite, name: &str, want: &str) {
        eventually(TIMEOUT, INTERVAL, async || {
            equal(phase_of(s, name).await?, want.to_string())
        })
        .await;
    }

    /// `Eventually(func() error { Get; mutate status; Status().Update })`
    async fn eventually_update_status<F>(s: &Suite, name: &str, mut mutate: F)
    where
        F: FnMut(&mut provisioningv1alpha1::DPFHCPProvisioner),
    {
        eventually(TIMEOUT, INTERVAL, async || {
            let mut p: provisioningv1alpha1::DPFHCPProvisioner =
                get(&s.k8s_client, name, TEST_NAMESPACE).await?;
            mutate(&mut p);
            s.k8s_client
                .update_status(&mut p)
                .await
                .map_err(|e| e.to_string())
        })
        .await;
    }

    mod controller_driven_phase_computation {
        use super::*;

        #[test]
        fn should_transition_to_pending_when_all_validations_pass() {
            it(async |s| {
                // Create DPFHCPProvisioner with valid configuration
                let mut provisioner = valid_provisioner("phase-test", DPU_CLUSTER_NAME);
                s.k8s_client.create(&mut provisioner).await.expect("create");

                // Controller should set phase to Pending after all validations pass
                eventually_phase(s, "phase-test", provisioningv1alpha1::PHASE_PENDING).await;

                // Verify validation conditions are set correctly
                eventually(TIMEOUT, INTERVAL, async || {
                    let p: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, "phase-test", TEST_NAMESPACE).await?;

                    // All validation conditions should be successful for Pending phase
                    // 1. DPUCluster should exist (DPUClusterMissing should be False or nil)
                    if find_cond(&p, provisioningv1alpha1::DPU_CLUSTER_MISSING)
                        .is_some_and(|c| c.status == CONDITION_TRUE)
                    {
                        // DPUCluster is missing - validation failed
                        return Err("DPUClusterMissing".into());
                    }

                    // 2. DPUCluster type should be valid (ClusterTypeValid should be True)
                    if !find_cond(&p, provisioningv1alpha1::CLUSTER_TYPE_VALID)
                        .is_some_and(|c| c.status == CONDITION_TRUE)
                    {
                        return Err("ClusterTypeValid".into());
                    }

                    // 3. DPUCluster should not be in use (DPUClusterInUse should be False or nil)
                    if find_cond(&p, provisioningv1alpha1::DPU_CLUSTER_IN_USE)
                        .is_some_and(|c| c.status == CONDITION_TRUE)
                    {
                        // DPUCluster is already in use - validation failed
                        return Err("DPUClusterInUse".into());
                    }

                    // 4. Secrets should be valid (SecretsValid should be True)
                    if !find_cond(&p, provisioningv1alpha1::SECRETS_VALID)
                        .is_some_and(|c| c.status == CONDITION_TRUE)
                    {
                        return Err("SecretsValid".into());
                    }

                    // 5. BlueField OCP layer image should be found (BlueFieldOCPLayerImageFound should be True)
                    if !find_cond(&p, provisioningv1alpha1::BLUE_FIELD_OCP_LAYER_IMAGE_FOUND)
                        .is_some_and(|c| c.status == CONDITION_TRUE)
                    {
                        return Err("BlueFieldOCPLayerImageFound".into());
                    }

                    Ok(())
                })
                .await;
            });
        }

        #[test]
        fn should_transition_to_failed_when_dpu_cluster_is_missing() {
            it(async |s| {
                // Create DPFHCPProvisioner referencing non-existent DPUCluster
                let mut provisioner =
                    valid_provisioner("phase-test-missing-dpu", "non-existent-dpu");
                s.k8s_client.create(&mut provisioner).await.expect("create");

                // Controller should set phase to Failed
                eventually_phase(
                    s,
                    "phase-test-missing-dpu",
                    provisioningv1alpha1::PHASE_FAILED,
                )
                .await;

                // Verify DPUClusterMissing condition is set
                eventually(TIMEOUT, INTERVAL, async || {
                    let p: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, "phase-test-missing-dpu", TEST_NAMESPACE).await?;
                    be_true(
                        find_cond(&p, "DPUClusterMissing")
                            .is_some_and(|c| c.status == CONDITION_TRUE),
                        "DPUClusterMissing=True",
                    )
                })
                .await;
            });
        }

        #[test]
        fn should_transition_to_deleting_when_cr_is_being_deleted() {
            it(async |s| {
                // Create DPFHCPProvisioner
                let mut provisioner = valid_provisioner("phase-test-deleting", DPU_CLUSTER_NAME);
                s.k8s_client.create(&mut provisioner).await.expect("create");

                // Wait for initial phase
                eventually_phase(
                    s,
                    "phase-test-deleting",
                    provisioningv1alpha1::PHASE_PENDING,
                )
                .await;

                // Delete the CR
                s.k8s_client.delete(&provisioner).await.expect("delete");

                // Controller should set phase to Deleting (if CR still exists during deletion)
                // Note: CR might be deleted very quickly, so we check if either:
                // 1. Phase is Deleting (CR still exists)
                // 2. CR is deleted (which is also acceptable)
                eventually(TIMEOUT, INTERVAL, async || {
                    match s
                        .k8s_client
                        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ctrlrt::ObjectKey::new(
                            TEST_NAMESPACE,
                            "phase-test-deleting",
                        ))
                        .await
                    {
                        // CR deleted successfully
                        Err(e) if ctrlrt::error::is_not_found(&e) => Ok(()),
                        // If CR still exists, phase should be Deleting
                        Ok(p) => be_true(
                            p.status.phase == provisioningv1alpha1::PHASE_DELETING,
                            "phase Deleting",
                        ),
                        Err(e) => Err(e.to_string()),
                    }
                })
                .await;
            });
        }

        #[test]
        fn should_transition_to_provisioning_when_hosted_cluster_is_created() {
            it(async |s| {
                // Create DPFHCPProvisioner
                let mut provisioner =
                    valid_provisioner("phase-test-provisioning", DPU_CLUSTER_NAME);
                s.k8s_client.create(&mut provisioner).await.expect("create");

                // Wait for Pending phase
                eventually_phase(
                    s,
                    "phase-test-provisioning",
                    provisioningv1alpha1::PHASE_PENDING,
                )
                .await;

                // Set HostedClusterRef to simulate HostedCluster creation
                eventually_update_status(s, "phase-test-provisioning", |p| {
                    p.status.hosted_cluster_ref =
                        object_ref("phase-test-provisioning", TEST_NAMESPACE);
                })
                .await;

                // Controller should transition to Provisioning
                eventually_phase(
                    s,
                    "phase-test-provisioning",
                    provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE,
                )
                .await;
            });
        }

        #[test]
        fn should_stay_in_provisioning_when_hosted_cluster_is_available_but_kubeconfig_is_not_injected()
         {
            it(async |s| {
                // Create DPFHCPProvisioner
                let mut provisioner = valid_provisioner("phase-test-not-ready", DPU_CLUSTER_NAME);
                s.k8s_client.create(&mut provisioner).await.expect("create");

                // Wait for Pending phase
                eventually_phase(
                    s,
                    "phase-test-not-ready",
                    provisioningv1alpha1::PHASE_PENDING,
                )
                .await;

                // Mock: Simulate controller setting HostedCluster created (move to Provisioning)
                eventually_update_status(s, "phase-test-not-ready", |p| {
                    p.status.hosted_cluster_ref =
                        object_ref("phase-test-not-ready", TEST_NAMESPACE);
                })
                .await;

                // Wait for Provisioning phase
                eventually_phase(
                    s,
                    "phase-test-not-ready",
                    provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE,
                )
                .await;

                // Mock: Simulate HC available but kubeconfig NOT injected
                eventually_update_status(s, "phase-test-not-ready", |p| {
                    // Set HC available = True, kubeconfig injected = False
                    meta::set_status_condition(
                        &mut p.status.conditions,
                        cond_now(
                            provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                            CONDITION_TRUE,
                            "Available",
                            "HostedCluster is available",
                        ),
                    );
                    meta::set_status_condition(
                        &mut p.status.conditions,
                        cond_now(
                            provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                            CONDITION_FALSE,
                            provisioningv1alpha1::REASON_KUBE_CONFIG_PENDING,
                            "Waiting for kubeconfig secret",
                        ),
                    );
                })
                .await;

                // Controller should stay in Provisioning (not Ready) since kubeconfig not injected
                consistently(secs(3), INTERVAL, async || {
                    equal(
                        phase_of(s, "phase-test-not-ready").await?,
                        provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE.to_string(),
                    )
                })
                .await;

                // Verify Ready condition is False or not set
                eventually(TIMEOUT, INTERVAL, async || {
                    let p: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, "phase-test-not-ready", TEST_NAMESPACE).await?;
                    // Ready should either be False or not set yet
                    be_true(
                        find_cond(&p, provisioningv1alpha1::READY)
                            .is_none_or(|c| c.status == CONDITION_FALSE),
                        "Ready False or unset",
                    )
                })
                .await;
            });
        }

        #[test]
        fn should_show_waiting_for_control_plane_when_upgrading_and_control_plane_not_ready() {
            it(async |_s| {
                let scheme = Scheme::new().register::<hyperv1::HostedCluster>();
                let hc = hyperv1::HostedCluster {
                    metadata: object_meta("phase-unit-upgrading", TEST_NAMESPACE),
                    ..Default::default()
                };
                let fake_client = FakeClientBuilder::new()
                    .with_scheme(scheme)
                    .with_objects([obj(&hc)])
                    .build();
                let reconciler = DPFHCPProvisionerReconciler {
                    client: fake_client,
                    ..Default::default()
                };

                let mut provisioner = provisioningv1alpha1::DPFHCPProvisioner {
                    metadata: ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        generation: Some(2),
                        ..object_meta("phase-unit-upgrading", TEST_NAMESPACE)
                    },
                    spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                        ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi"
                            .into(),
                        ..Default::default()
                    },
                    status: provisioningv1alpha1::DPFHCPProvisionerStatus {
                        hosted_cluster_ref: object_ref("phase-unit-upgrading", TEST_NAMESPACE),
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let conds = &mut provisioner.status.conditions;
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_TRUE,
                        "Available",
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_UPGRADE_IN_PROGRESS,
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::IGNITION_CONFIGURED,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_RELEASE_IMAGE_UPDATED,
                        2,
                    ),
                );

                reconciler
                    .update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                    .await;
                assert_eq!(
                    provisioner.status.phase,
                    provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE,
                    "should be WaitingForControlPlane when upgrading and control plane has not rolled out target version"
                );
            });
        }

        #[test]
        fn should_transition_to_generating_ignition_when_hosted_cluster_upgrading_is_false() {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let mut provisioner = provisioningv1alpha1::DPFHCPProvisioner {
                    metadata: ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        generation: Some(2),
                        ..object_meta("phase-unit-upgrade-done", TEST_NAMESPACE)
                    },
                    status: provisioningv1alpha1::DPFHCPProvisionerStatus {
                        hosted_cluster_ref: object_ref("phase-unit-upgrade-done", TEST_NAMESPACE),
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let conds = &mut provisioner.status.conditions;
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_TRUE,
                        "Available",
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_UPGRADE_COMPLETE,
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::IGNITION_CONFIGURED,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_RELEASE_IMAGE_UPDATED,
                        2,
                    ),
                );

                reconciler
                    .update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                    .await;
                assert_eq!(
                    provisioner.status.phase,
                    provisioningv1alpha1::PHASE_GENERATING_IGNITION,
                    "should transition to GeneratingIgnition when upgrade is complete"
                );
            });
        }

        #[test]
        fn should_show_generating_ignition_when_upgrading_and_control_plane_is_ready() {
            it(async |_s| {
                let scheme = Scheme::new().register::<hyperv1::HostedCluster>();
                let hc = hyperv1::HostedCluster {
                    metadata: object_meta("phase-unit-upgrading-phase", TEST_NAMESPACE),
                    status: hyperv1::HostedClusterStatus {
                        control_plane_version: hyperv1::ControlPlaneVersionStatus {
                            history: vec![hc_history(
                                configv1::COMPLETED_UPDATE,
                                "4.19.0",
                                "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
                            )],
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let fake_client = FakeClientBuilder::new()
                    .with_scheme(scheme)
                    .with_objects([obj(&hc)])
                    .build();
                let reconciler = DPFHCPProvisionerReconciler {
                    client: fake_client,
                    ..Default::default()
                };

                let mut provisioner = provisioningv1alpha1::DPFHCPProvisioner {
                    metadata: ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                        generation: Some(3),
                        ..object_meta("phase-unit-upgrading-phase", TEST_NAMESPACE)
                    },
                    spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                        ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi"
                            .into(),
                        ..Default::default()
                    },
                    status: provisioningv1alpha1::DPFHCPProvisionerStatus {
                        hosted_cluster_ref: object_ref(
                            "phase-unit-upgrading-phase",
                            TEST_NAMESPACE,
                        ),
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let conds = &mut provisioner.status.conditions;
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_TRUE,
                        "Available",
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_UPGRADE_IN_PROGRESS,
                        0,
                    ),
                );
                meta::set_status_condition(
                    conds,
                    cond_gen(
                        provisioningv1alpha1::IGNITION_CONFIGURED,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_RELEASE_IMAGE_UPDATED,
                        3,
                    ),
                );

                reconciler
                    .update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                    .await;
                assert_eq!(
                    provisioner.status.phase,
                    provisioningv1alpha1::PHASE_GENERATING_IGNITION,
                    "should be GeneratingIgnition when upgrading and control plane has rolled out target version"
                );
            });
        }

        #[test]
        fn is_upgrading_should_return_true_when_hosted_cluster_upgrading_true() {
            it(async |_s| {
                let mut provisioner = provisioningv1alpha1::DPFHCPProvisioner::default();
                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_UPGRADE_IN_PROGRESS,
                        0,
                    ),
                );
                assert!(is_upgrading(&provisioner));
            });
        }

        #[test]
        fn is_upgrading_should_return_false_when_hosted_cluster_upgrading_false() {
            it(async |_s| {
                let mut provisioner = provisioningv1alpha1::DPFHCPProvisioner::default();
                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    cond_gen(
                        provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_UPGRADE_COMPLETE,
                        0,
                    ),
                );
                assert!(!is_upgrading(&provisioner));
            });
        }

        #[test]
        fn is_upgrading_should_return_false_when_condition_is_not_set() {
            it(async |_s| {
                let provisioner = provisioningv1alpha1::DPFHCPProvisioner::default();
                assert!(!is_upgrading(&provisioner));
            });
        }

        fn hc_with(
            conditions: Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>,
            history: Vec<hyperv1::ControlPlaneUpdateHistory>,
        ) -> hyperv1::HostedCluster {
            hyperv1::HostedCluster {
                status: hyperv1::HostedClusterStatus {
                    conditions,
                    control_plane_version: hyperv1::ControlPlaneVersionStatus {
                        history,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }
        }

        const TARGET_4_22_1: &str = "quay.io/openshift-release-dev/ocp-release:4.22.1-multi";

        #[test]
        fn is_hosted_cluster_version_ready_should_return_false_when_cluster_version_progressing_is_true()
         {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let hc = hc_with(
                    vec![ctrlrt::meta::condition(
                        hyperv1::CLUSTER_VERSION_PROGRESSING,
                        CONDITION_TRUE,
                        "",
                        "",
                    )],
                    vec![hc_history(configv1::COMPLETED_UPDATE, "4.22.1", "")],
                );
                assert!(!reconciler.is_hosted_cluster_version_ready(&hc, TARGET_4_22_1));
            });
        }

        #[test]
        fn is_hosted_cluster_version_ready_should_return_true_when_version_matches_and_state_is_completed()
         {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let hc = hc_with(
                    vec![],
                    vec![hc_history(configv1::COMPLETED_UPDATE, "4.22.1", "")],
                );
                assert!(reconciler.is_hosted_cluster_version_ready(&hc, TARGET_4_22_1));
            });
        }

        #[test]
        fn is_hosted_cluster_version_ready_should_return_false_when_version_matches_but_state_is_partial()
         {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let hc = hc_with(
                    vec![],
                    vec![hc_history(configv1::PARTIAL_UPDATE, "4.22.1", "")],
                );
                assert!(!reconciler.is_hosted_cluster_version_ready(&hc, TARGET_4_22_1));
            });
        }

        #[test]
        fn is_hosted_cluster_version_ready_should_return_false_when_version_does_not_match() {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let hc = hc_with(
                    vec![],
                    vec![hc_history(configv1::COMPLETED_UPDATE, "4.22.0", "")],
                );
                assert!(!reconciler.is_hosted_cluster_version_ready(&hc, TARGET_4_22_1));
            });
        }

        #[test]
        fn is_hosted_cluster_version_ready_should_return_false_when_version_history_is_empty() {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let hc = hc_with(vec![], vec![]);
                assert!(!reconciler.is_hosted_cluster_version_ready(&hc, TARGET_4_22_1));
            });
        }

        #[test]
        fn is_hosted_cluster_version_ready_should_return_false_when_control_plane_version_has_no_history()
         {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let hc = hyperv1::HostedCluster::default();
                assert!(!reconciler.is_hosted_cluster_version_ready(&hc, TARGET_4_22_1));
            });
        }

        #[test]
        fn is_hosted_cluster_version_ready_should_return_true_for_digest_images_when_image_matches_and_completed()
         {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let digest_image = "registry.ci.openshift.org/ci-op/release@sha256:abc123def456";
                let hc = hc_with(
                    vec![],
                    vec![hc_history(
                        configv1::COMPLETED_UPDATE,
                        "4.22.0",
                        digest_image,
                    )],
                );
                assert!(reconciler.is_hosted_cluster_version_ready(&hc, digest_image));
            });
        }

        #[test]
        fn is_hosted_cluster_version_ready_should_return_false_for_digest_images_when_image_does_not_match()
         {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let hc = hc_with(
                    vec![],
                    vec![hc_history(
                        configv1::COMPLETED_UPDATE,
                        "4.22.0",
                        "registry.ci/release@sha256:old",
                    )],
                );
                assert!(
                    !reconciler
                        .is_hosted_cluster_version_ready(&hc, "registry.ci/release@sha256:new")
                );
            });
        }

        #[test]
        fn should_transition_to_failed_when_ignition_generation_fails_ignition_configured_false() {
            it(async |s| {
                // Create DPFHCPProvisioner
                let mut provisioner = valid_provisioner("phase-test-ign-fail", DPU_CLUSTER_NAME);
                s.k8s_client.create(&mut provisioner).await.expect("create");

                // Wait for Pending phase
                eventually_phase(
                    s,
                    "phase-test-ign-fail",
                    provisioningv1alpha1::PHASE_PENDING,
                )
                .await;

                // Simulate: set HostedClusterRef + HC available + kubeconfig injected + IgnitionConfigured=False with failure
                eventually_update_status(s, "phase-test-ign-fail", |p| {
                    p.status.hosted_cluster_ref = object_ref("phase-test-ign-fail", TEST_NAMESPACE);
                    meta::set_status_condition(
                        &mut p.status.conditions,
                        cond_now(
                            provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                            CONDITION_TRUE,
                            "Available",
                            "HostedCluster is available",
                        ),
                    );
                    meta::set_status_condition(
                        &mut p.status.conditions,
                        cond_now(
                            provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                            CONDITION_TRUE,
                            provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                            "Kubeconfig injected",
                        ),
                    );
                    let mut c = cond_now(
                        provisioningv1alpha1::IGNITION_CONFIGURED,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_IGNITION_GENERATION_FAILED,
                        "Failed to generate ignition: some error",
                    );
                    c.observed_generation = p.metadata.generation.filter(|g| *g != 0);
                    meta::set_status_condition(&mut p.status.conditions, c);
                })
                .await;

                // Controller should transition to Failed
                eventually_phase(s, "phase-test-ign-fail", provisioningv1alpha1::PHASE_FAILED)
                    .await;

                // Verify IgnitionConfigured condition shows failure
                eventually(TIMEOUT, INTERVAL, async || {
                    let p: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, "phase-test-ign-fail", TEST_NAMESPACE).await?;
                    be_true(
                        find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).is_some_and(|c| {
                            c.status == CONDITION_FALSE
                                && c.reason
                                    == provisioningv1alpha1::REASON_IGNITION_GENERATION_FAILED
                        }),
                        "IgnitionConfigured=False/IgnitionGenerationFailed",
                    )
                })
                .await;
            });
        }

        fn unit_provisioner(name: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
            provisioningv1alpha1::DPFHCPProvisioner {
                metadata: ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                    generation: Some(1),
                    ..object_meta(name, TEST_NAMESPACE)
                },
                status: provisioningv1alpha1::DPFHCPProvisionerStatus {
                    hosted_cluster_ref: object_ref(name, TEST_NAMESPACE),
                    ..Default::default()
                },
                ..Default::default()
            }
        }

        #[test]
        fn should_transition_to_error_when_hosted_cluster_is_blocked_progressing_false_reason_blocked()
         {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let mut provisioner = unit_provisioner("phase-unit-hc-blocked");

                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    ctrlrt::meta::condition(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_FALSE,
                        "WaitingForAvailable",
                        "failed to extract release metadata: manifest unknown",
                    ),
                );
                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    ctrlrt::meta::condition(
                        provisioningv1alpha1::HOSTED_CLUSTER_PROGRESSING,
                        CONDITION_FALSE,
                        hyperv1::BLOCKED_REASON,
                        "failed to extract release metadata: manifest unknown",
                    ),
                );

                reconciler
                    .update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                    .await;
                assert_eq!(
                    provisioner.status.phase,
                    provisioningv1alpha1::PHASE_ERROR,
                    "should be Error when HostedCluster is blocked"
                );
            });
        }

        #[test]
        fn should_transition_to_error_when_hosted_cluster_is_degraded() {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let mut provisioner = unit_provisioner("phase-unit-hc-degraded");

                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    ctrlrt::meta::condition(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_FALSE,
                        "WaitingForAvailable",
                        "HostedCluster is degraded",
                    ),
                );
                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    ctrlrt::meta::condition(
                        provisioningv1alpha1::HOSTED_CLUSTER_DEGRADED,
                        CONDITION_TRUE,
                        "DegradedError",
                        "encountering errors requiring intervention",
                    ),
                );

                reconciler
                    .update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                    .await;
                assert_eq!(
                    provisioner.status.phase,
                    provisioningv1alpha1::PHASE_ERROR,
                    "should be Error when HostedCluster is degraded"
                );
            });
        }

        #[test]
        fn should_stay_in_provisioning_when_hosted_cluster_is_progressing_normally() {
            it(async |_s| {
                let reconciler = DPFHCPProvisionerReconciler::default();
                let mut provisioner = unit_provisioner("phase-unit-hc-progressing");

                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    ctrlrt::meta::condition(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_FALSE,
                        "WaitingForAvailable",
                        "Components are starting up",
                    ),
                );
                meta::set_status_condition(
                    &mut provisioner.status.conditions,
                    ctrlrt::meta::condition(
                        provisioningv1alpha1::HOSTED_CLUSTER_PROGRESSING,
                        CONDITION_TRUE,
                        "Provisioning",
                        "HostedCluster is being provisioned",
                    ),
                );

                reconciler
                    .update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                    .await;
                assert_eq!(
                    provisioner.status.phase,
                    provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE,
                    "should stay in WaitingForControlPlane when HC is progressing normally"
                );
            });
        }
    }
}

mod handle_upgrade {
    use super::*;
    use std::time::Duration;

    const OLD_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.18.0-multi";
    const NEW_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi";
    const TEST_NAMESPACE: &str = "default";
    const DPU_CLUSTER_NAME: &str = "test-dpucluster";
    const DPU_CLUSTER_NS: &str = "test-dpucluster-ns";

    fn scheme() -> Scheme {
        Scheme::new()
            .register::<provisioningv1alpha1::DPFHCPProvisioner>()
            .register::<hyperv1::HostedCluster>()
            .register::<hyperv1::NodePool>()
            .with_client_go()
    }

    fn new_cr(image: &str, phase: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
        provisioningv1alpha1::DPFHCPProvisioner {
            metadata: ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                uid: Some("test-uid-123".into()),
                ..object_meta("test-provisioner", TEST_NAMESPACE)
            },
            spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                ocp_release_image: image.into(),
                dpu_cluster_ref: dpu_cluster_ref(DPU_CLUSTER_NAME, DPU_CLUSTER_NS),
                ..Default::default()
            },
            status: provisioningv1alpha1::DPFHCPProvisionerStatus {
                phase: phase.into(),
                hosted_cluster_ref: object_ref("test-provisioner", TEST_NAMESPACE),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn new_hc(image: &str) -> hyperv1::HostedCluster {
        hyperv1::HostedCluster {
            metadata: object_meta("test-provisioner", TEST_NAMESPACE),
            spec: hyperv1::HostedClusterSpec {
                release: hyperv1::Release {
                    image: image.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // newHCWithPriorVersion creates an HC with a Completed version history entry for
    // a prior image, proving a previous version was active (used in recovery tests).
    fn new_hc_with_prior_version(
        spec_image: &str,
        prior_completed_image: &str,
    ) -> hyperv1::HostedCluster {
        let mut hc = new_hc(spec_image);
        hc.status.control_plane_version.history = vec![hc_history(
            configv1::COMPLETED_UPDATE,
            "4.18.0",
            prior_completed_image,
        )];
        hc
    }

    fn new_np(image: &str) -> hyperv1::NodePool {
        hyperv1::NodePool {
            metadata: object_meta("test-provisioner", TEST_NAMESPACE),
            spec: hyperv1::NodePoolSpec {
                release: hyperv1::Release {
                    image: image.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn new_ignition_cm() -> ConfigMap {
        ConfigMap {
            metadata: object_meta(&ignition::config_map_name(DPU_CLUSTER_NAME), DPU_CLUSTER_NS),
            data: Some(
                [(
                    "BF_CFG_TEMPLATE".to_string(),
                    "some-ignition-data".to_string(),
                )]
                .into(),
            ),
            ..Default::default()
        }
    }

    fn build_reconciler(
        cr: &mut provisioningv1alpha1::DPFHCPProvisioner,
        objs: Vec<InitObject>,
    ) -> DPFHCPProvisionerReconciler {
        let mut all_objs = vec![obj_mut(cr)];
        all_objs.extend(objs);
        let fake_client = FakeClientBuilder::new()
            .with_scheme(scheme())
            .with_objects(all_objs)
            .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
            .build();

        DPFHCPProvisionerReconciler {
            client: fake_client,
            recorder: Arc::new(FakeRecorder::new(10)) as Recorder,
            ..Default::default()
        }
    }

    fn ctx() -> Ctx {
        Ctx::background()
    }

    async fn get_hc(r: &DPFHCPProvisionerReconciler) -> hyperv1::HostedCluster {
        get(&r.client, "test-provisioner", TEST_NAMESPACE)
            .await
            .unwrap()
    }

    async fn get_np(r: &DPFHCPProvisionerReconciler) -> hyperv1::NodePool {
        get(&r.client, "test-provisioner", TEST_NAMESPACE)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn should_recover_when_hc_was_updated_but_np_was_not_partial_failure() {
        let mut cr = new_cr(NEW_IMAGE, provisioningv1alpha1::PHASE_READY);
        meta::set_status_condition(
            &mut cr.status.conditions,
            cond_gen(
                provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                CONDITION_TRUE,
                "Available",
                0,
            ),
        );
        meta::set_status_condition(
            &mut cr.status.conditions,
            cond_gen(
                provisioningv1alpha1::IGNITION_CONFIGURED,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_IGNITION_GENERATED,
                0,
            ),
        );

        let hc = new_hc_with_prior_version(NEW_IMAGE, OLD_IMAGE); // HC updated, history proves prior version
        let np = new_np(OLD_IMAGE); // NP NOT updated due to crash

        let r = build_reconciler(&mut cr, vec![obj(&hc), obj(&np)]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert_eq!(
            result.requeue_after,
            Duration::from_secs(1),
            "should requeue to continue monitoring upgrade"
        );

        // NP should be updated to new image
        assert_eq!(
            get_np(&r).await.spec.release.image,
            NEW_IMAGE,
            "NodePool should be updated to new image during recovery"
        );

        // HC should remain unchanged (already matched)
        assert_eq!(
            get_hc(&r).await.spec.release.image,
            NEW_IMAGE,
            "HostedCluster should still have new image"
        );

        // HostedClusterUpgrading should be True
        let upgrading_cond =
            find_cond(&cr, provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING).unwrap();
        assert_eq!(upgrading_cond.status, CONDITION_TRUE);

        // IgnitionConfigured should be False (invalidated)
        let ign_cond = find_cond(&cr, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
        assert_eq!(ign_cond.status, CONDITION_FALSE);
        assert_eq!(
            ign_cond.reason,
            provisioningv1alpha1::REASON_RELEASE_IMAGE_UPDATED
        );
    }

    #[tokio::test]
    async fn should_not_trigger_upgrade_recovery_when_in_cluster_version_progressing_phase_initial_install()
     {
        let mut cr = new_cr(
            NEW_IMAGE,
            provisioningv1alpha1::PHASE_CLUSTER_VERSION_PROGRESSING,
        );
        meta::set_status_condition(
            &mut cr.status.conditions,
            cond_gen(
                provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                CONDITION_TRUE,
                "Available",
                0,
            ),
        );
        meta::set_status_condition(
            &mut cr.status.conditions,
            cond_gen(
                provisioningv1alpha1::IGNITION_CONFIGURED,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_IGNITION_GENERATED,
                0,
            ),
        );

        let hc = new_hc(NEW_IMAGE); // HC image matches CR (initial install, not an upgrade)
        let np = new_np(OLD_IMAGE); // NP still has old image (hasn't been synced yet)

        let r = build_reconciler(&mut cr, vec![obj(&hc), obj(&np)]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert!(
            result.requeue_after.is_zero(),
            "should return early without entering upgrade flow"
        );

        // NP should NOT be updated (no upgrade detected)
        assert_eq!(
            get_np(&r).await.spec.release.image,
            OLD_IMAGE,
            "NodePool should not be modified during initial install"
        );

        // HostedClusterUpgrading should NOT be set
        assert!(find_cond(&cr, provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING).is_none());
    }

    #[tokio::test]
    async fn should_recover_and_delete_stale_ignition_config_map_during_partial_failure() {
        let mut cr = new_cr(NEW_IMAGE, provisioningv1alpha1::PHASE_READY);
        meta::set_status_condition(
            &mut cr.status.conditions,
            cond_gen(
                provisioningv1alpha1::IGNITION_CONFIGURED,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_IGNITION_GENERATED,
                0,
            ),
        );

        let hc = new_hc_with_prior_version(NEW_IMAGE, OLD_IMAGE); // HC updated, history proves prior version
        let np = new_np(OLD_IMAGE); // NP not updated
        let cm = new_ignition_cm(); // Stale CM still present (crash happened before CM deletion)

        let r = build_reconciler(&mut cr, vec![obj(&hc), obj(&np), obj(&cm)]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert_eq!(
            result.requeue_after,
            Duration::from_secs(1),
            "should requeue to continue monitoring upgrade"
        );

        // Ignition ConfigMap should be deleted
        let err = r
            .client
            .get::<ConfigMap>(&ctrlrt::ObjectKey::new(
                DPU_CLUSTER_NS,
                ignition::config_map_name(DPU_CLUSTER_NAME),
            ))
            .await
            .unwrap_err();
        assert!(
            ctrlrt::error::is_not_found(&err),
            "stale ignition ConfigMap should be deleted during recovery"
        );
    }

    #[tokio::test]
    async fn should_detect_upgrade_and_update_both_hc_and_np() {
        let mut cr = new_cr(NEW_IMAGE, provisioningv1alpha1::PHASE_READY);
        let hc = new_hc(OLD_IMAGE);
        let np = new_np(OLD_IMAGE);

        let r = build_reconciler(&mut cr, vec![obj(&hc), obj(&np)]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert_eq!(result.requeue_after, Duration::from_secs(1));

        // HC should be updated
        assert_eq!(get_hc(&r).await.spec.release.image, NEW_IMAGE);

        // NP should be updated
        assert_eq!(get_np(&r).await.spec.release.image, NEW_IMAGE);

        // HostedClusterUpgrading should be True
        let upgrading_cond =
            find_cond(&cr, provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING).unwrap();
        assert_eq!(upgrading_cond.status, CONDITION_TRUE);
        assert_eq!(
            upgrading_cond.reason,
            provisioningv1alpha1::REASON_UPGRADE_IN_PROGRESS
        );
    }

    #[tokio::test]
    async fn should_complete_upgrade_when_version_is_ready() {
        let mut cr = new_cr(
            NEW_IMAGE,
            provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE,
        );
        meta::set_status_condition(
            &mut cr.status.conditions,
            cond_gen(
                provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_UPGRADE_IN_PROGRESS,
                0,
            ),
        );

        let mut hc = new_hc(NEW_IMAGE);
        hc.status.control_plane_version = hyperv1::ControlPlaneVersionStatus {
            history: vec![hc_history(configv1::COMPLETED_UPDATE, "4.19.0", NEW_IMAGE)],
            ..Default::default()
        };
        let np = new_np(NEW_IMAGE);

        let r = build_reconciler(&mut cr, vec![obj(&hc), obj(&np)]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert!(result.requeue_after.is_zero());

        // HostedClusterUpgrading should be set to False
        let upgrading_cond =
            find_cond(&cr, provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING).unwrap();
        assert_eq!(upgrading_cond.status, CONDITION_FALSE);
        assert_eq!(
            upgrading_cond.reason,
            provisioningv1alpha1::REASON_UPGRADE_COMPLETE
        );
    }

    #[tokio::test]
    async fn should_skip_upgrade_during_pending_phase() {
        let mut cr = new_cr(NEW_IMAGE, provisioningv1alpha1::PHASE_PENDING);
        let hc = new_hc(OLD_IMAGE);
        let np = new_np(OLD_IMAGE);

        let r = build_reconciler(&mut cr, vec![obj(&hc), obj(&np)]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert!(result.requeue_after.is_zero());

        assert_eq!(
            get_hc(&r).await.spec.release.image,
            OLD_IMAGE,
            "HC should not be updated during Pending phase"
        );
        assert_eq!(
            get_np(&r).await.spec.release.image,
            OLD_IMAGE,
            "NP should not be updated during Pending phase"
        );
    }

    #[tokio::test]
    async fn should_no_op_when_images_match_and_version_is_ready() {
        let mut cr = new_cr(NEW_IMAGE, provisioningv1alpha1::PHASE_READY);
        let mut hc = new_hc(NEW_IMAGE);
        hc.status.control_plane_version = hyperv1::ControlPlaneVersionStatus {
            history: vec![hc_history(configv1::COMPLETED_UPDATE, "4.19.0", NEW_IMAGE)],
            ..Default::default()
        };
        let np = new_np(NEW_IMAGE);

        let r = build_reconciler(&mut cr, vec![obj(&hc), obj(&np)]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert!(result.requeue_after.is_zero());

        assert!(
            find_cond(&cr, provisioningv1alpha1::HOSTED_CLUSTER_UPGRADING).is_none(),
            "HostedClusterUpgrading should not be set when no upgrade is happening"
        );
    }

    #[tokio::test]
    async fn should_skip_when_hosted_cluster_ref_is_nil() {
        let mut cr = new_cr(NEW_IMAGE, provisioningv1alpha1::PHASE_READY);
        cr.status.hosted_cluster_ref = None;

        let r = build_reconciler(&mut cr, vec![]);

        let result = r.handle_upgrade(&ctx(), &mut cr).await.unwrap();
        assert!(result.requeue_after.is_zero());
    }
}
