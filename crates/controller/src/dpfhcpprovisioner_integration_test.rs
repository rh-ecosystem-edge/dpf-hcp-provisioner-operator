//! Port of `internal/controller/dpfhcpprovisioner_integration_test.go`.

use std::collections::BTreeMap;
use std::time::Duration;

use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::CONDITION_TRUE;

use crate::suite_test::*;

mod dpfhcpprovisioner_integration_tests_with_envtest {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(30);
    const INTERVAL: Duration = Duration::from_secs(1);

    async fn after_each(s: &Suite) {
        // Clean up all resources
        let provisioners: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
            s.k8s_client.list_all().await.unwrap_or_default();
        for provisioner in &provisioners {
            let _ = s.k8s_client.delete(provisioner).await;
        }

        // Wait for all deletions to complete to prevent race conditions with next test
        eventually(TIMEOUT, INTERVAL, async || {
            let provisioners: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
                s.k8s_client.list_all().await.unwrap_or_default();
            equal(provisioners.len(), 0)
        })
        .await;
    }

    /// Ginkgo runs the specs of this container consecutively, so every spec after the first
    /// starts with the cluster-wide cleanup of the previous spec's `AfterEach` done (the
    /// "List Operations" spec relies on it). Rust specs of different containers interleave,
    /// so that precondition is re-established by running the same cleanup first.
    fn it<F>(body: F)
    where
        F: AsyncFnOnce(&'static Suite),
    {
        run(async move |s| {
            after_each(s).await;
            spec(body(s), after_each(s)).await
        });
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

    fn spec_base(base_domain: &str) -> provisioningv1alpha1::DPFHCPProvisionerSpec {
        provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: dpu_cluster_ref("test-dpu", "dpu-system"),
            dpu_deployment_ref: dpu_deployment_ref("test-dpu-deployment", "default"),
            base_domain: base_domain.into(),
            ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi".into(),
            ssh_key_secret_ref: local_ref("test-ssh-key"),
            pull_secret_ref: local_ref("test-pull-secret"),
            etcd_storage_class: "test-storage-class".into(),
            control_plane_availability_policy: hyperv1::SINGLE_REPLICA.into(),
            ..Default::default()
        }
    }

    mod complete_cr_lifecycle {
        use super::*;

        #[test]
        fn should_successfully_create_read_update_and_delete_a_dpfhcp_provisioner() {
            it(async |s| {
                let provisioner_name = "lifecycle-test";

                // CREATE
                // By("Creating a new DPFHCPProvisioner")
                let mut p = provisioner(
                    provisioner_name,
                    provisioningv1alpha1::DPFHCPProvisionerSpec {
                        control_plane_availability_policy: String::new(),
                        virtual_ip: "192.168.1.100".into(), // Required for HighlyAvailable default
                        ..spec_base("lifecycle.example.com")
                    },
                );
                s.k8s_client.create(&mut p).await.expect("create");

                // READ
                // By("Reading the created DPFHCPProvisioner")
                let created: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, provisioner_name, "default")
                        .await
                        .expect("get");
                assert_eq!(created.spec.base_domain, "lifecycle.example.com");
                assert_eq!(created.spec.dpu_cluster_ref.name, "test-dpu");
                assert_eq!(created.spec.dpu_cluster_ref.namespace, "dpu-system");

                // Verify defaults were applied
                assert_eq!(
                    created.spec.control_plane_availability_policy,
                    hyperv1::HIGHLY_AVAILABLE
                );

                // UPDATE
                // By("Updating the DPFHCPProvisioner spec")
                // Wrap in Eventually to handle race with controller
                let mut initial_generation = 0;
                eventually(secs(5), millis(100), async || {
                    let mut to_update: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, provisioner_name, "default").await?;
                    initial_generation = to_update.metadata.generation.unwrap_or(0);
                    // Update only mutable fields
                    to_update.spec.ocp_release_image =
                        "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi".into();
                    s.k8s_client
                        .update(&mut to_update)
                        .await
                        .map_err(|e| e.to_string())
                })
                .await;

                // Verify update
                let updated: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, provisioner_name, "default")
                        .await
                        .expect("get");
                assert_eq!(
                    updated.spec.ocp_release_image,
                    "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi"
                );
                assert!(updated.metadata.generation.unwrap_or(0) > initial_generation);

                // DELETE
                // By("Deleting the DPFHCPProvisioner")
                s.k8s_client.delete(&updated).await.expect("delete");

                // Verify deletion
                eventually(secs(5), millis(500), async || {
                    match s
                        .k8s_client
                        .get::<provisioningv1alpha1::DPFHCPProvisioner>(&ctrlrt::ObjectKey::new(
                            "default",
                            provisioner_name,
                        ))
                        .await
                    {
                        Err(e) if ctrlrt::error::is_not_found(&e) => Ok(()),
                        Err(e) => Err(e.to_string()),
                        Ok(_) => Err("still exists".into()),
                    }
                })
                .await;
            });
        }

        #[test]
        fn should_handle_status_updates_independently_of_spec() {
            it(async |s| {
                let provisioner_name = "status-update-test";

                // Create resource
                let mut p = provisioner(provisioner_name, spec_base("status.example.com"));
                s.k8s_client.create(&mut p).await.expect("create");

                // Get the created resource
                let created: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, provisioner_name, "default")
                        .await
                        .expect("get");

                let initial_generation = created.metadata.generation;

                // Update status with retry to avoid race with controller
                // By("Updating status fields")
                eventually(secs(5), millis(100), async || {
                    let mut p: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, provisioner_name, "default").await?;
                    p.status.phase = provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE.into();
                    s.k8s_client
                        .update_status(&mut p)
                        .await
                        .map_err(|e| e.to_string())
                })
                .await;

                // Verify status was updated but generation didn't change
                let updated: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, provisioner_name, "default")
                        .await
                        .expect("get");

                assert_eq!(
                    updated.status.phase,
                    provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE
                );
                assert_eq!(
                    updated.metadata.generation, initial_generation,
                    "Status update should not increment generation"
                );
            });
        }

        #[test]
        fn should_support_full_status_population() {
            it(async |s| {
                let provisioner_name = "full-status-test";

                let mut p = provisioner(provisioner_name, spec_base("full-status.example.com"));
                s.k8s_client.create(&mut p).await.expect("create");

                let _created: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, provisioner_name, "default")
                        .await
                        .expect("get");

                // Populate all status fields
                // By("Setting all status fields")
                // Wrap in Eventually to handle race with controller
                eventually(secs(5), millis(100), async || {
                    let mut p: provisioningv1alpha1::DPFHCPProvisioner =
                        get(&s.k8s_client, provisioner_name, "default").await?;
                    p.status = provisioningv1alpha1::DPFHCPProvisionerStatus {
                        phase: provisioningv1alpha1::PHASE_READY.into(),
                        conditions: vec![cond_now(
                            "Ready",
                            CONDITION_TRUE,
                            "ProvisionerReady",
                            "All components operational",
                        )],
                        hosted_cluster_ref: Some(apis::k8s::core::v1::ObjectReference {
                            api_version: "hypershift.openshift.io/v1beta1".into(),
                            kind: "HostedCluster".into(),
                            name: provisioner_name.into(),
                            namespace: "default".into(),
                            ..Default::default()
                        }),
                        kube_config_secret_ref: Some(local_ref(&format!(
                            "{provisioner_name}-kubeconfig"
                        ))),
                        ..Default::default()
                    };
                    s.k8s_client
                        .update_status(&mut p)
                        .await
                        .map_err(|e| e.to_string())
                })
                .await;

                // Verify all status fields
                // Note: the controller runs concurrently and may overwrite the phase
                // (e.g. to Failed due to missing DPUCluster) and add conditions
                // (e.g. DPUClusterMissing), so we verify fields are populated rather
                // than asserting exact values.
                let retrieved: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, provisioner_name, "default")
                        .await
                        .expect("get");

                assert!(!retrieved.status.phase.is_empty());
                assert!(!retrieved.status.conditions.is_empty());
                assert!(retrieved.status.hosted_cluster_ref.is_some());
                assert!(retrieved.status.kube_config_secret_ref.is_some());
            });
        }
    }

    mod list_operations {
        use super::*;

        #[test]
        fn should_list_multiple_dpfhcp_provisioner_resources() {
            it(async |s| {
                // Create multiple provisioners
                let provisioner_names = ["provisioner-1", "provisioner-2", "provisioner-3"];

                for name in provisioner_names {
                    let mut p = provisioner(
                        name,
                        provisioningv1alpha1::DPFHCPProvisionerSpec {
                            dpu_cluster_ref: dpu_cluster_ref(
                                &format!("test-dpu-{name}"),
                                "dpu-system",
                            ),
                            ..spec_base(&format!("{name}.example.com"))
                        },
                    );
                    s.k8s_client.create(&mut p).await.expect("create");
                }

                // List all provisioners
                let provisioner_list: Vec<provisioningv1alpha1::DPFHCPProvisioner> =
                    s.k8s_client.list_all().await.expect("list");
                assert_eq!(provisioner_list.len(), provisioner_names.len());

                // Verify each provisioner exists
                let found_names: BTreeMap<String, bool> = provisioner_list
                    .iter()
                    .map(|p| (p.metadata.name.clone().unwrap_or_default(), true))
                    .collect();

                for name in provisioner_names {
                    assert!(
                        found_names.get(name).copied().unwrap_or(false),
                        "Provisioner {name:?} should be in the list"
                    );
                }
            });
        }
    }

    mod resource_metadata {
        use super::*;

        #[test]
        fn should_preserve_labels_and_annotations() {
            it(async |s| {
                let mut p = provisioner("test-metadata", spec_base("metadata.example.com"));
                p.metadata.labels = Some(
                    [
                        ("environment".to_string(), "test".to_string()),
                        ("team".to_string(), "platform".to_string()),
                    ]
                    .into(),
                );
                p.metadata.annotations = Some(
                    [(
                        "description".to_string(),
                        "Test provisioner for metadata preservation".to_string(),
                    )]
                    .into(),
                );

                s.k8s_client.create(&mut p).await.expect("create");

                let created: provisioningv1alpha1::DPFHCPProvisioner =
                    get(&s.k8s_client, "test-metadata", "default")
                        .await
                        .expect("get");

                // Verify labels and annotations
                let labels = created.metadata.labels.clone().unwrap_or_default();
                assert_eq!(labels.get("environment").map(String::as_str), Some("test"));
                assert_eq!(labels.get("team").map(String::as_str), Some("platform"));
                let annotations = created.metadata.annotations.clone().unwrap_or_default();
                assert_eq!(
                    annotations.get("description").map(String::as_str),
                    Some("Test provisioner for metadata preservation")
                );

                // Clean up
                let _ = s.k8s_client.delete(&p).await;
            });
        }
    }
}
