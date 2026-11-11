//! Port of `internal/controller/ignition_configmap_watch_test.go`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::{CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::reconcile::Predicate;
use ctrlrt::{Ctx, FakeRecorder, ObjectKey, Recorder};
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj};
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta};

use crate::DPFHCPProvisionerReconciler;
use crate::dpfhcpprovisioner_controller::ignition_config_map_predicate;
use crate::ignition;
use crate::suite_test::*;

mod config_map_name {
    use super::*;

    #[test]
    fn should_return_the_expected_name_format() {
        assert_eq!(
            ignition::config_map_name("my-cluster"),
            "bfcfg-my-cluster.cfg"
        );
    }

    #[test]
    fn should_handle_cluster_names_with_special_characters() {
        assert_eq!(
            ignition::config_map_name("cluster-with-dashes"),
            "bfcfg-cluster-with-dashes.cfg"
        );
    }
}

fn scheme() -> Scheme {
    Scheme::new()
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .with_client_go()
}

fn cm(
    name: &str,
    namespace: &str,
    labels: Option<&[(&str, &str)]>,
    annotations: Option<&[(&str, &str)]>,
) -> ConfigMap {
    let map = |kv: &[(&str, &str)]| {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    ConfigMap {
        metadata: ObjectMeta {
            labels: labels.map(map),
            annotations: annotations.map(map),
            ..object_meta(name, namespace)
        },
        ..Default::default()
    }
}

mod ignition_config_map_watch {
    use super::*;

    mod ignition_config_map_predicate {
        use super::*;

        const BFCFG_TRUE: &[(&str, &str)] = &[(ignition::BFCFG_TEMPLATE_LABEL, "true")];

        #[test]
        fn should_accept_delete_events_for_bfcfg_template_config_maps() {
            let pred = ignition_config_map_predicate();
            let c = cm(
                "bfcfg-test-cluster.cfg",
                "dpu-system",
                Some(BFCFG_TRUE),
                None,
            );
            assert!(pred.delete(&c));
        }

        #[test]
        fn should_reject_delete_events_for_config_maps_with_nil_labels() {
            let pred = ignition_config_map_predicate();
            let c = cm("bfcfg-test-cluster.cfg", "dpu-system", None, None);
            assert!(!pred.delete(&c));
        }

        #[test]
        fn should_reject_delete_events_for_non_bfcfg_config_maps() {
            let pred = ignition_config_map_predicate();
            let c = cm("some-other-configmap", "dpu-system", None, None);
            assert!(!pred.delete(&c));
        }

        #[test]
        fn should_reject_delete_events_when_label_value_is_not_true() {
            let pred = ignition_config_map_predicate();
            let c = cm(
                "bfcfg-test-cluster.cfg",
                "dpu-system",
                Some(&[(ignition::BFCFG_TEMPLATE_LABEL, "false")]),
                None,
            );
            assert!(!pred.delete(&c));
        }

        #[test]
        fn should_reject_create_events() {
            let pred = ignition_config_map_predicate();
            let c = cm("bfcfg-test-cluster.cfg", "", Some(BFCFG_TRUE), None);
            assert!(!pred.create(&c));
        }

        #[test]
        fn should_reject_update_events() {
            let pred = ignition_config_map_predicate();
            let c = cm("bfcfg-test-cluster.cfg", "", Some(BFCFG_TRUE), None);
            // Go: event.UpdateEvent{ObjectNew: cm} (ObjectOld nil; the predicate ignores both)
            assert!(!pred.update(&ConfigMap::default(), &c));
        }

        #[test]
        fn should_reject_generic_events() {
            let pred = ignition_config_map_predicate();
            let c = cm("bfcfg-test-cluster.cfg", "", Some(BFCFG_TRUE), None);
            assert!(!pred.generic(&c));
        }
    }

    mod ignition_config_map_to_requests {
        use super::*;

        fn provisioner(
            name: &str,
            namespace: &str,
            cluster: &str,
            cluster_ns: &str,
        ) -> provisioningv1alpha1::DPFHCPProvisioner {
            provisioningv1alpha1::DPFHCPProvisioner {
                metadata: object_meta(name, namespace),
                spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                    dpu_cluster_ref: dpu_cluster_ref(cluster, cluster_ns),
                    ..Default::default()
                },
                ..Default::default()
            }
        }

        fn reconciler(objs: Vec<ctrlrt_fake::InitObject>) -> DPFHCPProvisionerReconciler {
            DPFHCPProvisionerReconciler {
                client: FakeClientBuilder::new()
                    .with_scheme(scheme())
                    .with_objects(objs)
                    .build(),
                ..Default::default()
            }
        }

        #[tokio::test]
        async fn should_map_to_the_dpfhcp_provisioner_referencing_the_dpu_cluster() {
            let p = provisioner(
                "test-provisioner",
                "default",
                "my-dpu-cluster",
                "dpu-system",
            );
            let c = cm(
                "bfcfg-my-dpu-cluster.cfg",
                "dpu-system",
                Some(&[(ignition::BFCFG_TEMPLATE_LABEL, "true")]),
                Some(&[
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
                        "my-dpu-cluster",
                    ),
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
                        "dpu-system",
                    ),
                ]),
            );

            let r = reconciler(vec![obj(&p)]);
            let requests = r
                .ignition_config_map_to_requests(&Ctx::background(), &c)
                .await;

            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0], ObjectKey::new("default", "test-provisioner"));
        }

        #[tokio::test]
        async fn should_return_empty_when_no_matching_provisioner_exists() {
            let c = cm(
                "bfcfg-orphan-cluster.cfg",
                "dpu-system",
                None,
                Some(&[
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
                        "orphan-cluster",
                    ),
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
                        "dpu-system",
                    ),
                ]),
            );

            let r = reconciler(vec![]);
            let requests = r
                .ignition_config_map_to_requests(&Ctx::background(), &c)
                .await;

            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_return_empty_when_annotations_are_missing() {
            let c = cm("bfcfg-no-annotations.cfg", "dpu-system", None, None);

            let r = reconciler(vec![]);
            let requests = r
                .ignition_config_map_to_requests(&Ctx::background(), &c)
                .await;

            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_return_empty_when_only_cluster_name_annotation_is_present() {
            let c = cm(
                "bfcfg-partial.cfg",
                "dpu-system",
                None,
                Some(&[(
                    ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
                    "my-cluster",
                )]),
            );

            let r = reconciler(vec![]);
            let requests = r
                .ignition_config_map_to_requests(&Ctx::background(), &c)
                .await;

            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_only_match_the_provisioner_with_the_correct_dpu_cluster_ref() {
            let matching = provisioner("matching", "ns-a", "target-cluster", "dpu-system");
            let other = provisioner("other", "ns-b", "different-cluster", "dpu-system");

            let c = cm(
                "bfcfg-target-cluster.cfg",
                "dpu-system",
                None,
                Some(&[
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
                        "target-cluster",
                    ),
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
                        "dpu-system",
                    ),
                ]),
            );

            let r = reconciler(vec![obj(&matching), obj(&other)]);
            let requests = r
                .ignition_config_map_to_requests(&Ctx::background(), &c)
                .await;

            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].name, "matching");
            assert_eq!(requests[0].namespace, "ns-a");
        }

        #[tokio::test]
        async fn should_not_match_when_cluster_namespace_differs() {
            let p = provisioner("provisioner", "default", "my-cluster", "dpu-system-a");

            let c = cm(
                "bfcfg-my-cluster.cfg",
                "dpu-system-b",
                None,
                Some(&[
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION,
                        "my-cluster",
                    ),
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION,
                        "dpu-system-b",
                    ),
                ]),
            );

            let r = reconciler(vec![obj(&p)]);
            let requests = r
                .ignition_config_map_to_requests(&Ctx::background(), &c)
                .await;

            assert!(requests.is_empty());
        }
    }

    mod verify_ignition_config_map {
        use super::*;

        fn provisioner(
            generation: i64,
            conditions: Vec<Condition>,
        ) -> provisioningv1alpha1::DPFHCPProvisioner {
            provisioningv1alpha1::DPFHCPProvisioner {
                metadata: ObjectMeta {
                    generation: Some(generation).filter(|g| *g != 0),
                    ..object_meta("test", "default")
                },
                spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
                    dpu_cluster_ref: dpu_cluster_ref("cluster", "dpu-system"),
                    ..Default::default()
                },
                status: provisioningv1alpha1::DPFHCPProvisionerStatus {
                    conditions,
                    ..Default::default()
                },
                ..Default::default()
            }
        }

        fn ign_cond(status: &str, reason: &str, observed_generation: i64) -> Condition {
            let mut c = cond_now(
                provisioningv1alpha1::IGNITION_CONFIGURED,
                status,
                reason,
                "",
            );
            c.observed_generation = Some(observed_generation).filter(|g| *g != 0);
            c
        }

        fn reconciler(
            objs: Vec<ctrlrt_fake::InitObject>,
            recorder: Recorder,
        ) -> DPFHCPProvisionerReconciler {
            DPFHCPProvisionerReconciler {
                client: FakeClientBuilder::new()
                    .with_scheme(scheme())
                    .with_objects(objs)
                    .build(),
                recorder,
                ..Default::default()
            }
        }

        fn recorder() -> Recorder {
            Arc::new(FakeRecorder::new(10))
        }

        #[tokio::test]
        async fn should_return_false_when_no_conditions_exist() {
            let mut p = provisioner(0, vec![]);

            let r = reconciler(vec![], recorder());
            let deleted = r
                .verify_ignition_config_map(&Ctx::background(), &mut p)
                .await;
            assert!(!deleted);

            assert!(find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).is_none());
        }

        #[tokio::test]
        async fn should_return_false_when_ignition_configured_is_false() {
            let mut p = provisioner(
                0,
                vec![ign_cond(CONDITION_FALSE, "IgnitionGenerationFailed", 0)],
            );

            let r = reconciler(vec![], recorder());
            let deleted = r
                .verify_ignition_config_map(&Ctx::background(), &mut p)
                .await;
            assert!(!deleted);

            let cond = find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(cond.reason, "IgnitionGenerationFailed");
        }

        #[tokio::test]
        async fn should_return_false_when_config_map_exists_and_ignition_already_reconciled() {
            let mut p = provisioner(0, vec![ign_cond(CONDITION_TRUE, "IgnitionGenerated", 0)]);

            let c = cm(
                &ignition::config_map_name("cluster"),
                "dpu-system",
                Some(&[(ignition::BFCFG_TEMPLATE_LABEL, "true")]),
                Some(&[(ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION, "cluster")]),
            );

            let r = DPFHCPProvisionerReconciler {
                state: crate::ReconcilerState {
                    ignition_reconciled: AtomicBool::new(true),
                },
                ..reconciler(vec![obj(&c)], recorder())
            };
            let deleted = r
                .verify_ignition_config_map(&Ctx::background(), &mut p)
                .await;
            assert!(!deleted);

            let cond = find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
            assert_eq!(cond.status, CONDITION_TRUE);
        }

        #[tokio::test]
        async fn should_clear_ignition_configured_and_return_true_when_config_map_is_missing() {
            let mut p = provisioner(2, vec![ign_cond(CONDITION_TRUE, "IgnitionGenerated", 2)]);

            let recorder = Arc::new(FakeRecorder::new(10));
            let r = reconciler(vec![], recorder.clone());
            let deleted = r
                .verify_ignition_config_map(&Ctx::background(), &mut p)
                .await;
            assert!(deleted);

            let cond = find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(cond.reason, "ConfigMapDeleted");
            assert_eq!(cond.observed_generation, Some(2));

            // Verify an event was recorded
            let event = recorder.try_recv().expect("an event");
            assert!(event.contains("IgnitionConfigMapDeleted"), "{event}");

            // Verify condition message indicates ConfigMap was deleted
            assert!(cond.message.contains("deleted"), "{}", cond.message);
        }

        #[tokio::test]
        async fn should_preserve_observed_generation_from_the_cr_when_clearing_condition() {
            let mut p = provisioner(5, vec![ign_cond(CONDITION_TRUE, "IgnitionGenerated", 3)]);

            let r = reconciler(vec![], recorder());
            let deleted = r
                .verify_ignition_config_map(&Ctx::background(), &mut p)
                .await;
            assert!(deleted);

            let cond = find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(
                cond.observed_generation,
                Some(5),
                "ObservedGeneration should match the current CR generation, not the old condition's"
            );
        }
    }
}
