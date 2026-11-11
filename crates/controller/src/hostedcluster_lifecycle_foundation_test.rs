//! Port of `internal/controller/hostedcluster_lifecycle_foundation_test.go`.

use std::time::Duration;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{ListOptions, Object};
use k8s_openapi::api::core::v1::Secret;

use crate::FINALIZER_NAME;
use crate::suite_test::*;

mod hosted_cluster_lifecycle_foundation_secret_management {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(30);
    const INTERVAL: Duration = Duration::from_secs(1);

    const TEST_NAMESPACE: &str = "default";
    const DPU_CLUSTER_NAME: &str = "test-dpucluster-foundation";
    const PULL_SECRET_NAME: &str = "test-pull-secret-foundation";
    const SSH_KEY_SECRET_NAME: &str = "test-ssh-key-foundation";
    const OCP_RELEASE_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64";
    const BASE_DOMAIN: &str = "test-cluster.example.com";
    const ETCD_STORAGE_CLASS: &str = "standard";
    const CLUSTER_TYPE: &str = "static";

    /// `"test-provisioner-foundation-" + time.Now().Format("20060102150405")`
    fn new_provisioner_name() -> String {
        let now = k8s_openapi::jiff::Zoned::now();
        format!(
            "test-provisioner-foundation-{}",
            now.strftime("%Y%m%d%H%M%S")
        )
    }

    async fn before_each(s: &Suite) {
        let c = &s.k8s_client;

        // Note: Using default namespace for tests, no need to create

        // Create DPUCluster
        let mut dpu_cluster = dpuprovisioningv1alpha1::DPUCluster {
            metadata: object_meta(DPU_CLUSTER_NAME, TEST_NAMESPACE),
            spec: dpuprovisioningv1alpha1::DPUClusterSpec {
                type_: CLUSTER_TYPE.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        c.create(&mut dpu_cluster).await.expect("create DPUCluster");

        // Clean up any leftover DPUServiceTemplates from previous test runs
        let templates: Vec<dpuservicev1alpha1::DPUServiceTemplate> = c
            .list(&ListOptions::new().in_namespace(TEST_NAMESPACE))
            .await
            .expect("list DPUServiceTemplates");
        for t in &templates {
            if let Err(e) = c.delete(t).await {
                assert!(e.to_string().contains("not found"), "{e}");
            }
        }

        // Set DPUCluster phase to Ready
        dpu_cluster.status.phase = dpuprovisioningv1alpha1::PHASE_READY.into();
        c.update_status(&mut dpu_cluster)
            .await
            .expect("update DPUCluster status");

        // Create pull-secret
        c.create(&mut test_pull_secret(PULL_SECRET_NAME, TEST_NAMESPACE))
            .await
            .expect("create pull secret");

        // Create ssh-key
        c.create(&mut test_ssh_key(SSH_KEY_SECRET_NAME, TEST_NAMESPACE))
            .await
            .expect("create ssh key");
    }

    async fn after_each(s: &Suite, provisioner_name: &str) {
        let c = &s.k8s_client;

        // Clean up DPFHCPProvisioner
        delete_ignore::<provisioningv1alpha1::DPFHCPProvisioner>(
            c,
            provisioner_name,
            TEST_NAMESPACE,
        )
        .await;

        // Clean up DPUCluster
        delete_ignore::<dpuprovisioningv1alpha1::DPUCluster>(c, DPU_CLUSTER_NAME, TEST_NAMESPACE)
            .await;

        // Clean up user secrets
        delete_ignore::<Secret>(c, PULL_SECRET_NAME, TEST_NAMESPACE).await;
        delete_ignore::<Secret>(c, SSH_KEY_SECRET_NAME, TEST_NAMESPACE).await;

        // Clean up copied secrets
        delete_ignore::<Secret>(
            c,
            &format!("{provisioner_name}-pull-secret"),
            TEST_NAMESPACE,
        )
        .await;
        delete_ignore::<Secret>(c, &format!("{provisioner_name}-ssh-key"), TEST_NAMESPACE).await;
        delete_ignore::<Secret>(
            c,
            &format!("{provisioner_name}-etcd-encryption-key"),
            TEST_NAMESPACE,
        )
        .await;
    }

    fn it<F>(body: F)
    where
        F: AsyncFnOnce(&'static Suite, String),
    {
        run(async move |s| {
            let provisioner_name = new_provisioner_name();
            before_each(s).await;
            spec(
                body(s, provisioner_name.clone()),
                after_each(s, &provisioner_name),
            )
            .await;
        });
    }

    fn provisioner(
        name: &str,
        dpu_deployment_name: &str,
        pull_secret: &str,
        policy: &str,
        vip: &str,
    ) -> provisioningv1alpha1::DPFHCPProvisioner {
        provisioningv1alpha1::DPFHCPProvisioner {
            metadata: object_meta(name, TEST_NAMESPACE),
            spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                dpu_cluster_ref: dpu_cluster_ref(DPU_CLUSTER_NAME, TEST_NAMESPACE),
                dpu_deployment_ref: dpu_deployment_ref(dpu_deployment_name, TEST_NAMESPACE),
                base_domain: BASE_DOMAIN.into(),
                ocp_release_image: OCP_RELEASE_IMAGE.into(),
                ssh_key_secret_ref: local_ref(SSH_KEY_SECRET_NAME),
                pull_secret_ref: local_ref(pull_secret),
                etcd_storage_class: ETCD_STORAGE_CLASS.into(),
                control_plane_availability_policy: policy.into(),
                virtual_ip: vip.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// `Eventually(func() error { return k8sClient.Get(ctx, key, &secret) }).Should(Succeed())`
    async fn eventually_secret(s: &Suite, name: &str) -> Secret {
        eventually(TIMEOUT, INTERVAL, async || {
            get::<Secret>(&s.k8s_client, name, TEST_NAMESPACE).await
        })
        .await
    }

    /// OwnerReference checks shared by the secret specs.
    fn assert_owned(secret: &Secret, provisioner_name: &str) {
        let refs = secret.owner_references();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, provisioner_name);
        assert_eq!(refs[0].kind, "DPFHCPProvisioner");
        assert_eq!(refs[0].controller, Some(true));
    }

    mod finalizer_management {
        use super::*;

        #[test]
        fn should_add_finalizer_to_dpfhcp_provisioner_on_creation() {
            it(async |s, provisioner_name| {
                // Create DPFHCPProvisioner
                let mut p = provisioner(
                    &provisioner_name,
                    "test-dpu-deployment",
                    PULL_SECRET_NAME,
                    hyperv1::HIGHLY_AVAILABLE,
                    "192.168.1.100",
                );
                s.k8s_client.create(&mut p).await.expect("create");

                // Verify finalizer is added
                eventually(TIMEOUT, INTERVAL, async || {
                    let p: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, &provisioner_name, TEST_NAMESPACE).await?;
                    be_true(
                        ctrlrt::meta::contains_finalizer(&p, FINALIZER_NAME),
                        "finalizer present",
                    )
                })
                .await;
            });
        }
    }

    mod secret_copying_to_same_namespace {
        use super::*;

        #[test]
        fn should_copy_pull_secret_with_correct_type_and_owner_reference() {
            it(async |s, provisioner_name| {
                // Create DPFHCPProvisioner
                let mut p = provisioner(
                    &provisioner_name,
                    "test-dpu-deployment",
                    PULL_SECRET_NAME,
                    hyperv1::SINGLE_REPLICA,
                    "",
                );
                s.k8s_client.create(&mut p).await.expect("create");

                // Verify pull-secret is copied to same namespace (name encodes source ref)
                let pull_secret_target =
                    eventually_secret(s, &ctrl_hostedcluster::pull_secret_copy_name(&p)).await;

                // Verify secret type
                assert_eq!(
                    pull_secret_target.type_.as_deref(),
                    Some("kubernetes.io/dockerconfigjson")
                );

                // Verify OwnerReference is set
                assert_owned(&pull_secret_target, &provisioner_name);

                // Verify data is copied
                assert!(
                    pull_secret_target
                        .data
                        .as_ref()
                        .is_some_and(|d| d.contains_key(".dockerconfigjson"))
                );
            });
        }

        #[test]
        fn should_copy_ssh_key_with_correct_type_and_owner_reference() {
            it(async |s, provisioner_name| {
                // Create DPFHCPProvisioner
                let mut p = provisioner(
                    &provisioner_name,
                    DPU_CLUSTER_NAME,
                    PULL_SECRET_NAME,
                    hyperv1::SINGLE_REPLICA,
                    "",
                );
                s.k8s_client.create(&mut p).await.expect("create");

                // Verify ssh-key is copied to same namespace (name encodes source ref)
                let ssh_key_target =
                    eventually_secret(s, &ctrl_hostedcluster::ssh_key_copy_name(&p)).await;

                // Verify secret type
                assert_eq!(ssh_key_target.type_.as_deref(), Some("Opaque"));

                // Verify OwnerReference is set
                assert_owned(&ssh_key_target, &provisioner_name);

                // Verify data is copied
                assert!(
                    ssh_key_target
                        .data
                        .as_ref()
                        .is_some_and(|d| d.contains_key("id_rsa.pub"))
                );
            });
        }
    }

    mod etcd_encryption_key_generation {
        use super::*;

        #[test]
        fn should_generate_etcd_encryption_key_with_correct_format() {
            it(async |s, provisioner_name| {
                // Create DPFHCPProvisioner
                let mut p = provisioner(
                    &provisioner_name,
                    DPU_CLUSTER_NAME,
                    PULL_SECRET_NAME,
                    hyperv1::SINGLE_REPLICA,
                    "",
                );
                s.k8s_client.create(&mut p).await.expect("create");

                // Verify ETCD key is generated in same namespace
                let etcd_key_secret =
                    eventually_secret(s, &format!("{provisioner_name}-etcd-encryption-key")).await;

                // Verify secret type
                assert_eq!(etcd_key_secret.type_.as_deref(), Some("Opaque"));

                // Verify OwnerReference is set
                assert_owned(&etcd_key_secret, &provisioner_name);

                // Verify key length (32 bytes)
                let data = etcd_key_secret.data.clone().unwrap_or_default();
                let key = data
                    .get(hyperv1::AESCBC_KEY_SECRET_KEY)
                    .expect("AESCBCKeySecretKey present");
                assert_eq!(key.0.len(), 32);
            });
        }
    }

    mod idempotency {
        use super::*;

        #[test]
        fn should_not_create_duplicate_secrets_on_multiple_reconciliations() {
            it(async |s, provisioner_name| {
                let c = &s.k8s_client;

                // Create DPFHCPProvisioner
                let mut p = provisioner(
                    &provisioner_name,
                    DPU_CLUSTER_NAME,
                    PULL_SECRET_NAME,
                    hyperv1::SINGLE_REPLICA,
                    "",
                );
                c.create(&mut p).await.expect("create");

                let etcd_key_name = format!("{provisioner_name}-etcd-encryption-key");

                // Wait for initial reconciliation
                eventually(TIMEOUT, INTERVAL, async || {
                    get::<Secret>(c, &etcd_key_name, TEST_NAMESPACE)
                        .await
                        .map(|_| ())
                })
                .await;

                // Get initial ETCD key for comparison
                let initial_etcd_key: Secret = get(c, &etcd_key_name, TEST_NAMESPACE)
                    .await
                    .expect("get etcd key");
                let initial_key_bytes = initial_etcd_key
                    .data
                    .clone()
                    .unwrap_or_default()
                    .get(hyperv1::AESCBC_KEY_SECRET_KEY)
                    .cloned();

                // Trigger another reconciliation by updating a label
                let mut p: provisioningv1alpha1::DPFHCPProvisioner =
                    get(c, &provisioner_name, TEST_NAMESPACE)
                        .await
                        .expect("get");
                p.metadata.labels =
                    Some([("test-trigger".to_string(), "reconcile".to_string())].into());
                c.update(&mut p).await.expect("update");

                // Wait a bit for potential reconciliation
                tokio::time::sleep(secs(2)).await;

                // Verify ETCD key hasn't changed (idempotency)
                let current_etcd_key: Secret = get(c, &etcd_key_name, TEST_NAMESPACE)
                    .await
                    .expect("get etcd key");
                assert_eq!(
                    current_etcd_key
                        .data
                        .clone()
                        .unwrap_or_default()
                        .get(hyperv1::AESCBC_KEY_SECRET_KEY)
                        .cloned(),
                    initial_key_bytes
                );

                // Verify only one of each secret exists
                let secret_list: Vec<Secret> = c
                    .list(&ListOptions::new().in_namespace(TEST_NAMESPACE))
                    .await
                    .expect("list secrets");

                let mut pull_secret_count = 0;
                let mut ssh_key_count = 0;
                let mut etcd_key_count = 0;

                for secret in &secret_list {
                    // Check via OwnerReference instead of labels
                    if ctrlrt::meta::is_controlled_by(secret, &p) {
                        if secret.name() == ctrl_hostedcluster::pull_secret_copy_name(&p) {
                            pull_secret_count += 1;
                        }
                        if secret.name() == ctrl_hostedcluster::ssh_key_copy_name(&p) {
                            ssh_key_count += 1;
                        }
                        if secret.name() == etcd_key_name {
                            etcd_key_count += 1;
                        }
                    }
                }

                assert_eq!(pull_secret_count, 1);
                assert_eq!(ssh_key_count, 1);
                assert_eq!(etcd_key_count, 1);
            });
        }
    }

    mod error_handling {
        use super::*;

        #[test]
        fn should_handle_missing_user_secret_gracefully() {
            it(async |s, provisioner_name| {
                let c = &s.k8s_client;

                // Create DPFHCPProvisioner with non-existent pull-secret
                let mut p = provisioner(
                    &provisioner_name,
                    DPU_CLUSTER_NAME,
                    "non-existent-secret",
                    hyperv1::SINGLE_REPLICA,
                    "",
                );
                c.create(&mut p).await.expect("create");

                // Verify secret is NOT created due to missing source
                let copy_name = ctrl_hostedcluster::pull_secret_copy_name(&p);
                consistently(secs(5), INTERVAL, async || {
                    match c
                        .get::<Secret>(&ctrlrt::ObjectKey::new(TEST_NAMESPACE, &copy_name))
                        .await
                    {
                        Err(e) if ctrlrt::error::is_not_found(&e) => Ok(()),
                        Err(e) => Err(e.to_string()),
                        Ok(_) => Err("copied pull secret exists".into()),
                    }
                })
                .await;

                // The reconciliation should fail but not crash
                // CR should still exist
                eventually(TIMEOUT, INTERVAL, async || {
                    get::<provisioningv1alpha1::DPFHCPProvisioner>(
                        c,
                        &provisioner_name,
                        TEST_NAMESPACE,
                    )
                    .await
                    .map(|_| ())
                })
                .await;
            });
        }
    }
}
