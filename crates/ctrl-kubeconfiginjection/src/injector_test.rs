//! Port of `internal/controller/kubeconfiginjection/injector_test.go` (Ginkgo suite
//! "Kubeconfig Injection Suite", bootstrapped by `suite_test.go`; here every spec is a plain
//! `#[tokio::test]`). `Describe` → nested module, `It` → test function.

use std::collections::BTreeMap;
use std::sync::Arc;

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::k8s::core::v1::{LocalObjectReference, ObjectReference};
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::{CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::{Client, Ctx, FakeRecorder, ObjectKey, Recorder};
use ctrlrt_fake::{FakeClientBuilder, InitObject, Scheme};
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta};

use super::*;

/// BeforeEach state.
struct Suite {
    ctx: Ctx,
    recorder: Arc<FakeRecorder>,
    scheme: Scheme,
}

/// BeforeEach
fn before_each() -> Suite {
    // Setup scheme
    let scheme = Scheme::new()
        // provisioningv1alpha1.AddToScheme
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .register::<provisioningv1alpha1::DPFHCPProvisionerConfig>()
        // corev1.AddToScheme (kinds used here)
        .register::<Secret>()
        .register::<ConfigMap>()
        // dpuprovisioningv1alpha1.AddToScheme (kinds used here)
        .register::<dpuprovisioningv1alpha1::DPUCluster>();

    Suite {
        ctx: Ctx::background(),
        recorder: Arc::new(FakeRecorder::new(100)),
        scheme,
    }
}

impl Suite {
    fn recorder(&self) -> Recorder {
        self.recorder.clone()
    }

    fn client(&self, objs: Vec<InitObject>, provisioner_status: bool, dpu_status: bool) -> Client {
        let mut b = FakeClientBuilder::new()
            .with_scheme(self.scheme.clone())
            .with_objects(objs);
        if provisioner_status {
            b = b.with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>();
        }
        if dpu_status {
            b = b.with_status_subresource::<dpuprovisioningv1alpha1::DPUCluster>();
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

/// Helper function to find KubeConfigInjected condition
fn find_condition(conditions: &[Condition]) -> Option<&Condition> {
    conditions
        .iter()
        .find(|c| c.type_ == provisioningv1alpha1::KUBE_CONFIG_INJECTED)
}

fn provisioner(with_hc: bool) -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioningv1alpha1::DPFHCPProvisioner {
        metadata: ObjectMeta {
            name: Some("test-provisioner".into()),
            namespace: Some("test-ns".into()),
            ..Default::default()
        },
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: provisioningv1alpha1::DPUClusterReference {
                name: "test-dpu".into(),
                namespace: "dpu-ns".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        status: provisioningv1alpha1::DPFHCPProvisionerStatus {
            hosted_cluster_ref: with_hc.then(|| ObjectReference {
                name: "test-provisioner".into(),
                namespace: "test-ns".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn secret(namespace: &str, key: &str, data: &str) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some("test-provisioner-admin-kubeconfig".into()),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            key.to_string(),
            ByteString(data.as_bytes().to_vec()),
        )])),
        ..Default::default()
    }
}

fn dpu_cluster(type_: &str, kubeconfig: &str) -> dpuprovisioningv1alpha1::DPUCluster {
    dpuprovisioningv1alpha1::DPUCluster {
        metadata: ObjectMeta {
            name: Some("test-dpu".into()),
            namespace: Some("dpu-ns".into()),
            ..Default::default()
        },
        spec: dpuprovisioningv1alpha1::DPUClusterSpec {
            type_: type_.into(),
            kubeconfig: kubeconfig.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn secret_data(s: &Secret, key: &str) -> Option<Vec<u8>> {
    s.data.as_ref()?.get(key).map(|b| b.0.clone())
}

async fn get_secret(c: &Client, ns: &str) -> ctrlrt::Result<Secret> {
    c.get(&ObjectKey::new(ns, "test-provisioner-admin-kubeconfig"))
        .await
}

async fn get_dpu(c: &Client) -> ctrlrt::Result<dpuprovisioningv1alpha1::DPUCluster> {
    c.get(&ObjectKey::new("dpu-ns", "test-dpu")).await
}

fn previously_injected(p: &mut provisioningv1alpha1::DPFHCPProvisioner) {
    p.status.kube_config_secret_ref = Some(LocalObjectReference {
        name: "test-provisioner-admin-kubeconfig".into(),
        ..Default::default()
    });
    // Set initial condition to True (was previously injected)
    ctrlrt::meta::set_status_condition(
        &mut p.status.conditions,
        ctrlrt::meta::condition(
            provisioningv1alpha1::KUBE_CONFIG_INJECTED,
            CONDITION_TRUE,
            provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
            "Previously injected",
        ),
    );
}

mod kubeconfig_injection_reconciler {
    use super::*;

    mod happy_path_full_injection_flow {
        use super::*;

        #[tokio::test]
        async fn should_successfully_inject_kubeconfig_when_all_prerequisites_are_met() {
            let s = before_each();
            // Given: DPFHCPProvisioner with HostedCluster created
            let mut provisioner = provisioner(true);

            // HC kubeconfig secret exists (created by Hypershift with "kubeconfig" key)
            let hc_secret = secret(
                "test-ns",
                SOURCE_KUBECONFIG_SECRET_KEY,
                "fake-kubeconfig-data",
            );

            // DPUCluster exists
            let dpu_cluster = dpu_cluster(dpuprovisioningv1alpha1::STATIC_CLUSTER, "");

            let fake_client = s.client(
                vec![
                    ctrlrt_fake::obj_mut(&mut provisioner),
                    ctrlrt_fake::obj(&hc_secret),
                    ctrlrt_fake::obj(&dpu_cluster),
                ],
                true,
                true,
            );

            let injector = KubeconfigInjector::new(fake_client.clone(), s.recorder());

            // Verify BEFORE state - nothing should be set up yet
            let before_cond = find_condition(&provisioner.status.conditions);
            assert!(
                before_cond.is_none(),
                "Condition should not be set initially"
            );

            // When: Reconciliation runs
            let res = injector.inject_kubeconfig(&s.ctx, &mut provisioner).await;

            // Then: Success
            assert!(res.is_ok(), "unexpected error: {:?}", res.err());

            // Verify events were emitted for secret creation and DPUCluster update
            s.eventually_receive_containing("KubeConfigInjected");
            s.eventually_receive_containing("DPUClusterUpdated");

            // Secret created in DPUCluster namespace
            let dest_secret = get_secret(&fake_client, "dpu-ns").await.unwrap();
            // Destination secret should have "super-admin.conf" key (DPF operator expects this)
            assert_eq!(
                secret_data(&dest_secret, DESTINATION_KUBECONFIG_SECRET_KEY),
                Some(b"fake-kubeconfig-data".to_vec())
            );
            let labels = dest_secret.metadata.labels.clone().unwrap_or_default();
            assert_eq!(
                labels[common::LABEL_DPFHCP_PROVISIONER_NAME],
                "test-provisioner"
            );
            assert_eq!(
                labels[common::LABEL_DPFHCP_PROVISIONER_NAMESPACE],
                "test-ns"
            );

            // DPUCluster updated
            let updated_dpu = get_dpu(&fake_client).await.unwrap();
            assert_eq!(
                updated_dpu.spec.kubeconfig,
                "test-provisioner-admin-kubeconfig"
            );

            // Status updated
            let r = provisioner
                .status
                .kube_config_secret_ref
                .as_ref()
                .expect("ref");
            assert_eq!(r.name, "test-provisioner-admin-kubeconfig");

            // Condition set to True
            let after_cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(after_cond.status, CONDITION_TRUE);
            assert_eq!(
                after_cond.reason,
                provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED
            );
            assert!(after_cond.message.contains("successfully"));
        }
    }

    mod secret_not_ready_scenario {
        use super::*;

        #[tokio::test]
        async fn should_set_condition_to_pending_when_hc_kubeconfig_secret_doesnt_exist() {
            let s = before_each();
            // Given: DPFHCPProvisioner but no HC kubeconfig secret yet
            let mut provisioner = provisioner(true);

            let dpu_cluster = dpu_cluster("", "");

            let fake_client = s.client(
                vec![
                    ctrlrt_fake::obj_mut(&mut provisioner),
                    ctrlrt_fake::obj(&dpu_cluster),
                ],
                true,
                false,
            );

            let injector = KubeconfigInjector::new(fake_client, s.recorder());

            // When: Reconciliation runs
            let res = injector.inject_kubeconfig(&s.ctx, &mut provisioner).await;

            // Then: Requeue without error
            assert!(res.is_ok(), "unexpected error: {:?}", res.err());

            // Condition set to pending
            let cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(
                cond.reason,
                provisioningv1alpha1::REASON_KUBE_CONFIG_PENDING
            );
        }
    }

    mod idempotency_scenario_a_drift_detection {
        use super::*;

        #[tokio::test]
        async fn should_detect_and_correct_content_drift_between_source_and_destination_secrets() {
            let s = before_each();
            // Given: Both secrets exist but with different content (drift scenario)
            let mut provisioner = provisioner(true);
            previously_injected(&mut provisioner);

            let hc_secret = secret(
                "test-ns",
                SOURCE_KUBECONFIG_SECRET_KEY,
                "new-rotated-kubeconfig-data",
            );

            let dest_secret = secret(
                "dpu-ns",
                DESTINATION_KUBECONFIG_SECRET_KEY,
                "old-kubeconfig-data", // DRIFT!
            );

            let dpu_cluster = dpu_cluster("", "test-provisioner-admin-kubeconfig");

            let fake_client = s.client(
                vec![
                    ctrlrt_fake::obj_mut(&mut provisioner),
                    ctrlrt_fake::obj(&hc_secret),
                    ctrlrt_fake::obj(&dest_secret),
                    ctrlrt_fake::obj(&dpu_cluster),
                ],
                true,
                true,
            );

            let injector = KubeconfigInjector::new(fake_client.clone(), s.recorder());

            // Verify BEFORE state
            let before_cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(before_cond.status, CONDITION_TRUE);
            assert_eq!(before_cond.message, "Previously injected");

            // Verify drift exists before reconciliation
            let before_secret = get_secret(&fake_client, "dpu-ns").await.unwrap();
            assert_eq!(
                secret_data(&before_secret, DESTINATION_KUBECONFIG_SECRET_KEY),
                Some(b"old-kubeconfig-data".to_vec())
            );

            // When: Reconciliation runs
            let res = injector.inject_kubeconfig(&s.ctx, &mut provisioner).await;

            // Then: Success
            assert!(res.is_ok(), "unexpected error: {:?}", res.err());

            // Verify drift was DETECTED - check for DriftCorrected event
            s.eventually_receive_containing("DriftCorrected");

            // Destination secret updated to match source
            let updated_secret = get_secret(&fake_client, "dpu-ns").await.unwrap();
            assert_eq!(
                secret_data(&updated_secret, DESTINATION_KUBECONFIG_SECRET_KEY),
                Some(b"new-rotated-kubeconfig-data".to_vec())
            );

            // Condition remains True (stayed healthy through drift correction)
            let after_cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(after_cond.status, CONDITION_TRUE);
            assert_eq!(
                after_cond.reason,
                provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED
            );
            assert!(after_cond.message.contains("successfully"));

            // Verify status was updated
            let r = provisioner
                .status
                .kube_config_secret_ref
                .as_ref()
                .expect("ref");
            assert_eq!(r.name, "test-provisioner-admin-kubeconfig");
        }
    }

    mod idempotency_scenario_b_secret_exists_dpu_cluster_not_updated {
        use super::*;

        #[tokio::test]
        async fn should_update_dpu_cluster_without_recreating_secret() {
            let s = before_each();
            // Given: Secret exists but DPUCluster not updated (partial completion scenario)
            let mut provisioner = provisioner(true);

            // No initial condition set (injection was interrupted)

            let hc_secret = secret("test-ns", SOURCE_KUBECONFIG_SECRET_KEY, "kubeconfig-data");
            let dest_secret = secret(
                "dpu-ns",
                DESTINATION_KUBECONFIG_SECRET_KEY,
                "kubeconfig-data",
            );

            // Kubeconfig field is empty - NOT UPDATED YET
            let dpu_cluster = dpu_cluster("", "");

            let fake_client = s.client(
                vec![
                    ctrlrt_fake::obj_mut(&mut provisioner),
                    ctrlrt_fake::obj(&hc_secret),
                    ctrlrt_fake::obj(&dest_secret),
                    ctrlrt_fake::obj(&dpu_cluster),
                ],
                true,
                true,
            );

            let injector = KubeconfigInjector::new(fake_client.clone(), s.recorder());

            // Verify BEFORE state
            let before_cond = find_condition(&provisioner.status.conditions);
            assert!(before_cond.is_none(), "Condition should not be set yet");

            let before_dpu = get_dpu(&fake_client).await.unwrap();
            assert!(
                before_dpu.spec.kubeconfig.is_empty(),
                "DPUCluster should not be updated yet"
            );

            // Secret should already exist
            let existing_secret = get_secret(&fake_client, "dpu-ns")
                .await
                .expect("Secret should already exist");
            let original_resource_version = existing_secret.metadata.resource_version.clone();

            // When: Reconciliation runs
            let res = injector.inject_kubeconfig(&s.ctx, &mut provisioner).await;

            // Then: Success
            assert!(res.is_ok(), "unexpected error: {:?}", res.err());

            // Verify NO secret recreation - secret should not have been recreated
            let after_secret = get_secret(&fake_client, "dpu-ns").await.unwrap();
            assert_eq!(
                after_secret.metadata.resource_version, original_resource_version,
                "Secret should not have been recreated/updated"
            );

            // DPUCluster SHOULD be updated
            let updated_dpu = get_dpu(&fake_client).await.unwrap();
            assert_eq!(
                updated_dpu.spec.kubeconfig,
                "test-provisioner-admin-kubeconfig"
            );

            // Condition set to True (injection completed)
            let after_cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(after_cond.status, CONDITION_TRUE);
            assert_eq!(
                after_cond.reason,
                provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED
            );

            // Status updated
            let r = provisioner
                .status
                .kube_config_secret_ref
                .as_ref()
                .expect("ref");
            assert_eq!(r.name, "test-provisioner-admin-kubeconfig");
        }
    }

    mod idempotency_scenario_c_secret_missing_dpu_cluster_updated {
        use super::*;

        #[tokio::test]
        async fn should_recreate_secret_without_modifying_dpu_cluster() {
            let s = before_each();
            // Given: DPUCluster updated but secret missing (secret was deleted scenario)
            let mut provisioner = provisioner(true);
            // Set initial condition to True (was previously injected but secret got deleted)
            previously_injected(&mut provisioner);

            let hc_secret = secret("test-ns", SOURCE_KUBECONFIG_SECRET_KEY, "kubeconfig-data");

            let dpu_cluster = dpu_cluster("", "test-provisioner-admin-kubeconfig"); // Already updated

            // NOTE: destSecret is NOT created - it's missing!

            let fake_client = s.client(
                vec![
                    ctrlrt_fake::obj_mut(&mut provisioner),
                    ctrlrt_fake::obj(&hc_secret),
                    ctrlrt_fake::obj(&dpu_cluster),
                ], // No destSecret
                true,
                true,
            );

            let injector = KubeconfigInjector::new(fake_client.clone(), s.recorder());

            // Verify BEFORE state
            let before_cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(before_cond.status, CONDITION_TRUE);

            let before_dpu = get_dpu(&fake_client).await.unwrap();
            assert_eq!(
                before_dpu.spec.kubeconfig, "test-provisioner-admin-kubeconfig",
                "DPUCluster should already be updated"
            );
            let original_dpu_resource_version = before_dpu.metadata.resource_version.clone();

            // Verify secret is missing
            let err = get_secret(&fake_client, "dpu-ns")
                .await
                .expect_err("Secret should be missing");
            assert!(
                ctrlrt::error::is_not_found(&err),
                "Secret should be missing"
            );

            // When: Reconciliation runs
            let res = injector.inject_kubeconfig(&s.ctx, &mut provisioner).await;

            // Then: Success
            assert!(res.is_ok(), "unexpected error: {:?}", res.err());

            // Secret SHOULD be recreated with "super-admin.conf" key
            let recreated_secret = get_secret(&fake_client, "dpu-ns")
                .await
                .expect("Secret should be recreated");
            assert_eq!(
                secret_data(&recreated_secret, DESTINATION_KUBECONFIG_SECRET_KEY),
                Some(b"kubeconfig-data".to_vec())
            );
            let labels = recreated_secret.metadata.labels.clone().unwrap_or_default();
            assert_eq!(
                labels[common::LABEL_DPFHCP_PROVISIONER_NAME],
                "test-provisioner"
            );
            assert_eq!(
                labels[common::LABEL_DPFHCP_PROVISIONER_NAMESPACE],
                "test-ns"
            );

            // DPUCluster should NOT be modified (already updated)
            let after_dpu = get_dpu(&fake_client).await.unwrap();
            assert_eq!(
                after_dpu.spec.kubeconfig,
                "test-provisioner-admin-kubeconfig"
            );
            assert_eq!(
                after_dpu.metadata.resource_version, original_dpu_resource_version,
                "DPUCluster should not have been modified"
            );

            // Condition remains True (stayed healthy through secret recreation)
            let after_cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(after_cond.status, CONDITION_TRUE);
            assert_eq!(
                after_cond.reason,
                provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED
            );

            // Status still references the secret
            let r = provisioner
                .status
                .kube_config_secret_ref
                .as_ref()
                .expect("ref");
            assert_eq!(r.name, "test-provisioner-admin-kubeconfig");
        }
    }

    mod skip_when_dpu_cluster_not_found {
        use super::*;

        #[tokio::test]
        async fn should_skip_injection_without_error_when_dpu_cluster_is_missing() {
            let s = before_each();
            let mut provisioner = provisioner(true);

            let hc_secret = secret(
                "test-ns",
                SOURCE_KUBECONFIG_SECRET_KEY,
                "fake-kubeconfig-data",
            );

            let fake_client = s.client(
                vec![
                    ctrlrt_fake::obj_mut(&mut provisioner),
                    ctrlrt_fake::obj(&hc_secret),
                ],
                true,
                false,
            );

            let injector = KubeconfigInjector::new(fake_client, s.recorder());

            let res = injector.inject_kubeconfig(&s.ctx, &mut provisioner).await;
            assert!(res.is_ok(), "unexpected error: {:?}", res.err());

            let cond = find_condition(&provisioner.status.conditions).expect("condition");
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(
                cond.reason,
                provisioningv1alpha1::REASON_KUBE_CONFIG_PENDING
            );
            assert!(cond.message.contains("dpu-ns/test-dpu"));
        }
    }

    mod skip_when_hosted_cluster_not_created {
        use super::*;

        #[tokio::test]
        async fn should_skip_injection_when_hosted_cluster_ref_is_nil() {
            let s = before_each();
            // Given: DPFHCPProvisioner without HostedCluster created yet
            // HostedClusterRef is nil
            let mut provisioner = provisioner(false);

            let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)], false, false);

            let injector = KubeconfigInjector::new(fake_client, s.recorder());

            // When: Reconciliation runs
            let res = injector.inject_kubeconfig(&s.ctx, &mut provisioner).await;

            // Then: Skip without error
            assert!(res.is_ok(), "unexpected error: {:?}", res.err());

            // No condition set
            let cond = find_condition(&provisioner.status.conditions);
            assert!(cond.is_none());
        }
    }
}
