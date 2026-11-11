//! Port of `internal/controller/dpudeployment_watch_test.go`.

use std::sync::Arc;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::{self, CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::{Ctx, FakeRecorder, ObjectKey, Recorder};
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj, obj_mut};
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::DPFHCPProvisionerReconciler;
use crate::ignition;
use crate::suite_test::*;

fn scheme() -> Scheme {
    Scheme::new()
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .with_client_go()
        .register::<dpuservicev1alpha1::DPUDeployment>()
        .register::<dpuprovisioningv1alpha1::DPUFlavor>()
}

fn recorder() -> Recorder {
    Arc::new(FakeRecorder::new(10))
}

fn provisioner_with(
    name: &str,
    namespace: &str,
    dd_ref: Option<(&str, &str)>,
    conditions: Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioningv1alpha1::DPFHCPProvisioner {
        metadata: object_meta(name, namespace),
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: dpu_cluster_ref("cluster", "dpu-system"),
            dpu_deployment_ref: dd_ref.and_then(|(n, ns)| dpu_deployment_ref(n, ns)),
            ..Default::default()
        },
        status: provisioningv1alpha1::DPFHCPProvisionerStatus {
            conditions,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn dpu_deployment(
    name: &str,
    namespace: &str,
    bfb: Option<&str>,
    flavor: Option<&str>,
) -> dpuservicev1alpha1::DPUDeployment {
    dpuservicev1alpha1::DPUDeployment {
        metadata: object_meta(name, namespace),
        spec: dpuservicev1alpha1::DPUDeploymentSpec {
            dpus: dpuservicev1alpha1::DPUs {
                bfb: bfb.map(String::from),
                flavor: flavor.map(String::from),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn dpu_flavor(name: &str, namespace: &str) -> dpuprovisioningv1alpha1::DPUFlavor {
    dpuprovisioningv1alpha1::DPUFlavor {
        metadata: object_meta(name, namespace),
        ..Default::default()
    }
}

fn ignition_cm(annotations: &[(&str, &str)]) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            labels: Some(
                [(
                    ignition::BFCFG_TEMPLATE_LABEL.to_string(),
                    "true".to_string(),
                )]
                .into(),
            ),
            annotations: Some(
                annotations
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            ..object_meta(&ignition::config_map_name("cluster"), "dpu-system")
        },
        ..Default::default()
    }
}

fn ignition_configured_true() -> Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition> {
    vec![cond_now(
        provisioningv1alpha1::IGNITION_CONFIGURED,
        CONDITION_TRUE,
        provisioningv1alpha1::REASON_IGNITION_GENERATED,
        "",
    )]
}

mod dpu_deployment_watch {
    use super::*;

    mod dpu_deployment_to_requests {
        use super::*;

        #[tokio::test]
        async fn should_map_to_the_dpfhcp_provisioner_referencing_the_dpu_deployment() {
            let provisioner = provisioner_with(
                "test-provisioner",
                "default",
                Some(("my-deployment", "dpf-operator-system")),
                vec![],
            );
            let dd = dpu_deployment("my-deployment", "dpf-operator-system", None, None);

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&provisioner)])
                .build();

            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                ..Default::default()
            };
            let requests = r.dpu_deployment_to_requests(&Ctx::background(), &dd).await;

            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0], ObjectKey::new("default", "test-provisioner"));
        }

        #[tokio::test]
        async fn should_return_empty_when_no_provisioner_references_the_dpu_deployment() {
            let provisioner = provisioner_with(
                "test-provisioner",
                "default",
                Some(("other-deployment", "dpf-operator-system")),
                vec![],
            );
            let dd = dpu_deployment("my-deployment", "dpf-operator-system", None, None);

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&provisioner)])
                .build();

            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                ..Default::default()
            };
            let requests = r.dpu_deployment_to_requests(&Ctx::background(), &dd).await;

            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_return_empty_when_provisioner_has_nil_dpu_deployment_ref() {
            let provisioner = provisioner_with("test-provisioner", "default", None, vec![]);
            let dd = dpu_deployment("my-deployment", "dpf-operator-system", None, None);

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&provisioner)])
                .build();

            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                ..Default::default()
            };
            let requests = r.dpu_deployment_to_requests(&Ctx::background(), &dd).await;

            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_not_match_when_dpu_deployment_namespace_differs() {
            let provisioner = provisioner_with(
                "test-provisioner",
                "default",
                Some(("my-deployment", "namespace-a")),
                vec![],
            );
            let dd = dpu_deployment("my-deployment", "namespace-b", None, None);

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&provisioner)])
                .build();

            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                ..Default::default()
            };
            let requests = r.dpu_deployment_to_requests(&Ctx::background(), &dd).await;

            assert!(requests.is_empty());
        }
    }

    mod handle_dependency_changes {
        use super::*;

        #[tokio::test]
        async fn should_return_false_when_ignition_configured_is_not_true() {
            let mut provisioner = provisioner_with(
                "test",
                "default",
                Some(("my-deployment", "dpf-operator-system")),
                vec![],
            );

            let fake_client = FakeClientBuilder::new().with_scheme(scheme()).build();
            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                recorder: recorder(),
                ..Default::default()
            };
            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut provisioner)
                .await
                .expect("no error");
            assert!(!changed);
        }

        #[tokio::test]
        async fn should_return_false_when_dpu_deployment_ref_is_nil() {
            let mut provisioner =
                provisioner_with("test", "default", None, ignition_configured_true());

            let fake_client = FakeClientBuilder::new().with_scheme(scheme()).build();
            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                recorder: recorder(),
                ..Default::default()
            };
            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut provisioner)
                .await
                .expect("no error");
            assert!(!changed);
        }

        #[tokio::test]
        async fn should_return_false_when_flavor_matches_config_map_annotation() {
            let mut provisioner = provisioner_with(
                "test",
                "default",
                Some(("my-deployment", "dpf-operator-system")),
                ignition_configured_true(),
            );
            let dd = dpu_deployment(
                "my-deployment",
                "dpf-operator-system",
                Some("my-bfb"),
                Some("my-flavor"),
            );
            let flavor = dpu_flavor("my-flavor", "dpf-operator-system");
            let cm = ignition_cm(&[
                (ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION, "cluster"),
                (
                    ignition::BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION,
                    "my-flavor",
                ),
                (ignition::BFCFG_TEMPLATE_BFB_NAME_ANNOTATION, "my-bfb"),
            ]);

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&dd), obj(&flavor), obj(&cm)])
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                recorder: recorder(),
                ..Default::default()
            };
            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut provisioner)
                .await
                .expect("no error");
            assert!(!changed);
        }

        #[tokio::test]
        async fn should_return_true_when_flavor_changed() {
            let mut provisioner = provisioner_with(
                "test",
                "default",
                Some(("my-deployment", "dpf-operator-system")),
                ignition_configured_true(),
            );
            let dd = dpu_deployment(
                "my-deployment",
                "dpf-operator-system",
                Some("my-bfb"),
                Some("new-flavor"),
            );
            let flavor = dpu_flavor("new-flavor", "dpf-operator-system");
            let cm = ignition_cm(&[
                (ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION, "cluster"),
                (
                    ignition::BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION,
                    "old-flavor",
                ),
                (ignition::BFCFG_TEMPLATE_BFB_NAME_ANNOTATION, "my-bfb"),
            ]);

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj_mut(&mut provisioner), obj(&dd), obj(&flavor), obj(&cm)])
                .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                recorder: recorder(),
                ..Default::default()
            };
            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut provisioner)
                .await
                .expect("no error");
            assert!(changed);
        }

        #[tokio::test]
        async fn should_return_true_when_config_map_has_nil_annotations() {
            let mut provisioner = provisioner_with(
                "test",
                "default",
                Some(("my-deployment", "dpf-operator-system")),
                ignition_configured_true(),
            );
            let dd = dpu_deployment(
                "my-deployment",
                "dpf-operator-system",
                Some("my-bfb"),
                Some("my-flavor"),
            );
            let flavor = dpu_flavor("my-flavor", "dpf-operator-system");
            let cm = ignition_cm(&[(ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION, "cluster")]);

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj_mut(&mut provisioner), obj(&dd), obj(&flavor), obj(&cm)])
                .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                recorder: recorder(),
                ..Default::default()
            };
            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut provisioner)
                .await
                .expect("no error");
            assert!(changed);
        }

        #[tokio::test]
        async fn should_delete_config_map_and_set_dependency_deleted_when_dpu_deployment_is_not_found()
         {
            let mut provisioner = provisioner_with(
                "test",
                "default",
                Some(("missing-deployment", "dpf-operator-system")),
                ignition_configured_true(),
            );

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj_mut(&mut provisioner)])
                .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                recorder: recorder(),
                ..Default::default()
            };

            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut provisioner)
                .await
                .expect("no error");
            assert!(changed);

            let cond = find_cond(&provisioner, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(cond.reason, provisioningv1alpha1::REASON_DEPENDENCY_DELETED);
        }

        #[tokio::test]
        async fn should_return_false_when_config_map_is_not_found() {
            let mut provisioner = provisioner_with(
                "test",
                "default",
                Some(("my-deployment", "dpf-operator-system")),
                ignition_configured_true(),
            );
            let dd = dpu_deployment(
                "my-deployment",
                "dpf-operator-system",
                Some("my-bfb"),
                Some("my-flavor"),
            );
            let flavor = dpu_flavor("my-flavor", "dpf-operator-system");

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&dd), obj(&flavor)])
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                recorder: recorder(),
                ..Default::default()
            };
            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut provisioner)
                .await
                .expect("no error");
            assert!(!changed);
        }
    }

    mod phase_computation_with_condition_reasons {
        use super::*;

        fn provisioner_with_ign_reason(reason: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
            let mut p = provisioner_with(
                "test",
                "default",
                None,
                vec![
                    cond_now(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_TRUE,
                        "AsExpected",
                        "",
                    ),
                    cond_now(
                        provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                        "",
                    ),
                    cond_now(
                        provisioningv1alpha1::IGNITION_CONFIGURED,
                        CONDITION_FALSE,
                        reason,
                        "",
                    ),
                ],
            );
            p.metadata.generation = Some(1);
            p.status.hosted_cluster_ref = object_ref("test", "default");
            p
        }

        #[tokio::test]
        async fn should_transition_to_ignition_generating_not_failed_when_reason_is_dpu_deployment_changed()
         {
            let mut provisioner =
                provisioner_with_ign_reason(provisioningv1alpha1::REASON_DPU_DEPLOYMENT_CHANGED);

            let r = DPFHCPProvisionerReconciler {
                recorder: recorder(),
                ..Default::default()
            };
            r.update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                .await;

            assert_ne!(provisioner.status.phase, provisioningv1alpha1::PHASE_FAILED);
            assert_eq!(
                provisioner.status.phase,
                provisioningv1alpha1::PHASE_GENERATING_IGNITION
            );
        }

        #[tokio::test]
        async fn should_set_failed_phase_for_ignition_generation_failed_reason() {
            let mut provisioner = provisioner_with_ign_reason(
                provisioningv1alpha1::REASON_IGNITION_GENERATION_FAILED,
            );

            let r = DPFHCPProvisionerReconciler {
                recorder: recorder(),
                ..Default::default()
            };
            r.update_phase_from_conditions(&Ctx::background(), &mut provisioner)
                .await;

            assert_eq!(provisioner.status.phase, provisioningv1alpha1::PHASE_FAILED);
        }
    }
}

mod handle_dependency_changes_dependency_readiness {
    use super::*;

    fn doca_provisioner(
        phase: &str,
        with_ref: bool,
        conditions: Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>,
    ) -> provisioningv1alpha1::DPFHCPProvisioner {
        let mut p = provisioner_with(
            "doca",
            "clusters",
            Some(("dpudeployment", "dpf-operator-system")),
            conditions,
        );
        p.metadata.generation = Some(1);
        p.status.phase = phase.into();
        if with_ref {
            p.status.hosted_cluster_ref = object_ref("doca", "clusters");
        }
        p
    }

    fn available_and_injected() -> Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition> {
        vec![
            cond_now(
                provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                CONDITION_TRUE,
                "AsExpected",
                "",
            ),
            cond_now(
                provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                "",
            ),
        ]
    }

    fn reconciler_for(
        provisioner: &mut provisioningv1alpha1::DPFHCPProvisioner,
    ) -> DPFHCPProvisionerReconciler {
        let fake_client = FakeClientBuilder::new()
            .with_scheme(scheme())
            .with_objects([obj_mut(provisioner)])
            .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
            .build();
        DPFHCPProvisionerReconciler {
            client: fake_client,
            recorder: recorder(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn should_wait_instead_of_failing_when_dpu_deployment_is_not_found() {
        let mut provisioner = doca_provisioner(
            provisioningv1alpha1::PHASE_GENERATING_IGNITION,
            true,
            available_and_injected(),
        );
        let r = reconciler_for(&mut provisioner);

        let (changed, result) = r
            .handle_dependency_changes(&Ctx::background(), &mut provisioner)
            .await
            .expect("no error");
        assert!(changed);
        assert!(result.requeue_after.is_zero());

        let cond = find_cond(&provisioner, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
        assert_eq!(cond.status, CONDITION_FALSE);
        assert_eq!(cond.reason, provisioningv1alpha1::REASON_DEPENDENCY_DELETED);
        assert!(cond.message.contains("dpudeployment"), "{}", cond.message);
        assert_eq!(
            provisioner.status.phase,
            provisioningv1alpha1::PHASE_GENERATING_IGNITION
        );
    }

    #[tokio::test]
    async fn should_wait_instead_of_retrying_when_recovering_from_ignition_generation_failed_due_to_missing_dpu_deployment()
     {
        let mut conditions = available_and_injected();
        conditions.push(cond_now(
            provisioningv1alpha1::IGNITION_CONFIGURED,
            CONDITION_FALSE,
            provisioningv1alpha1::REASON_IGNITION_GENERATION_FAILED,
            "Failed to generate ignition: failed to retrieve DPU Flavor: failed to get DPUDeployment: DPUDeployment.svc.dpu.nvidia.com \"dpudeployment\" not found",
        ));
        let mut provisioner =
            doca_provisioner(provisioningv1alpha1::PHASE_FAILED, true, conditions);
        let r = reconciler_for(&mut provisioner);

        let (changed, result) = r
            .handle_dependency_changes(&Ctx::background(), &mut provisioner)
            .await
            .expect("no error");
        assert!(changed);
        assert!(result.requeue_after.is_zero());

        let cond = find_cond(&provisioner, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
        assert_eq!(cond.reason, provisioningv1alpha1::REASON_DEPENDENCY_DELETED);
        assert_eq!(
            provisioner.status.phase,
            provisioningv1alpha1::PHASE_GENERATING_IGNITION
        );
    }

    #[tokio::test]
    async fn should_not_block_reconciliation_before_ignition_prerequisites_are_met() {
        let mut provisioner = doca_provisioner(
            provisioningv1alpha1::PHASE_WAITING_FOR_CONTROL_PLANE,
            false,
            vec![],
        );
        let r = reconciler_for(&mut provisioner);

        let (changed, _) = r
            .handle_dependency_changes(&Ctx::background(), &mut provisioner)
            .await
            .expect("no error");
        assert!(!changed);
        assert!(find_cond(&provisioner, provisioningv1alpha1::IGNITION_CONFIGURED).is_none());
    }
}

mod compute_ready_condition_with_dpu_deployment_changed {
    use super::*;

    #[tokio::test]
    async fn should_set_ready_false_when_ignition_configured_is_false_due_to_dpu_deployment_changed()
     {
        let mut provisioner = provisioningv1alpha1::DPFHCPProvisioner {
            metadata: ObjectMeta {
                generation: Some(1),
                ..object_meta("test", "default")
            },
            status: provisioningv1alpha1::DPFHCPProvisionerStatus {
                conditions: vec![
                    cond_now(
                        provisioningv1alpha1::HOSTED_CLUSTER_AVAILABLE,
                        CONDITION_TRUE,
                        "AsExpected",
                        "",
                    ),
                    cond_now(
                        provisioningv1alpha1::KUBE_CONFIG_INJECTED,
                        CONDITION_TRUE,
                        provisioningv1alpha1::REASON_KUBE_CONFIG_INJECTED,
                        "",
                    ),
                    cond_now(
                        provisioningv1alpha1::IGNITION_CONFIGURED,
                        CONDITION_FALSE,
                        provisioningv1alpha1::REASON_DPU_DEPLOYMENT_CHANGED,
                        "",
                    ),
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let scheme = Scheme::new().register::<provisioningv1alpha1::DPFHCPProvisioner>();
        let fake_client = FakeClientBuilder::new().with_scheme(scheme).build();
        let r = DPFHCPProvisionerReconciler {
            client: fake_client,
            recorder: recorder(),
            ..Default::default()
        };
        r.compute_ready_condition(&Ctx::background(), &mut provisioner)
            .await;

        let ready_cond = find_cond(&provisioner, provisioningv1alpha1::READY).unwrap();
        assert_eq!(ready_cond.status, CONDITION_FALSE);
        assert_eq!(ready_cond.reason, "IgnitionNotConfigured");
    }
}

/// `DPUDeployment{BFB: test-bfb, Flavor: flavor, NodeEffect: {NoEffect: true},
/// DPUSetStrategy: {Type: RollingUpdate}, Services: {stub-svc}, ServiceChains: {...}}`
fn stub_dpu_deployment(
    name: &str,
    namespace: &str,
    flavor: &str,
) -> dpuservicev1alpha1::DPUDeployment {
    dpuservicev1alpha1::DPUDeployment {
        metadata: object_meta(name, namespace),
        spec: dpuservicev1alpha1::DPUDeploymentSpec {
            dpus: dpuservicev1alpha1::DPUs {
                bfb: Some("test-bfb".into()),
                flavor: Some(flavor.into()),
                node_effect: dpuprovisioningv1alpha1::Action {
                    no_effect: Some(true),
                    ..Default::default()
                },
                dpu_set_strategy: dpuprovisioningv1alpha1::DPUSetStrategy {
                    type_: "RollingUpdate".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            services: Some(
                [(
                    "stub-svc".to_string(),
                    dpuservicev1alpha1::DPUDeploymentServiceConfiguration {
                        service_template: "stub-tpl".into(),
                        service_configuration: "stub-cfg".into(),
                        ..Default::default()
                    },
                )]
                .into(),
            ),
            service_chains: Some(dpuservicev1alpha1::ServiceChains {
                switches: Some(vec![dpuservicev1alpha1::DPUDeploymentSwitch {
                    ports: Some(vec![dpuservicev1alpha1::DPUDeploymentPort {
                        service_interface: Some(dpuservicev1alpha1::ServiceIfc {
                            match_labels: Some([("test".to_string(), "true".to_string())].into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The ignition ConfigMap created by the envtest specs.
fn integ_ignition_cm(cluster_name: &str, cluster_ns: &str, flavor: &str) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            labels: Some(
                [(
                    ignition::BFCFG_TEMPLATE_LABEL.to_string(),
                    "true".to_string(),
                )]
                .into(),
            ),
            annotations: Some(
                [
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION.to_string(),
                        cluster_name.to_string(),
                    ),
                    (
                        ignition::BFCFG_TEMPLATE_CLUSTER_NAMESPACE_ANNOTATION.to_string(),
                        cluster_ns.to_string(),
                    ),
                    (
                        ignition::BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION.to_string(),
                        flavor.to_string(),
                    ),
                ]
                .into(),
            ),
            ..object_meta(&ignition::config_map_name(cluster_name), cluster_ns)
        },
        data: Some(
            [(
                "BF_CFG_TEMPLATE".to_string(),
                r#"{"ignition":{"version":"3.4.0"}}"#.to_string(),
            )]
            .into(),
        ),
        ..Default::default()
    }
}

fn integ_provisioner(
    name: &str,
    namespace: &str,
    cluster_name: &str,
    cluster_ns: &str,
    deployment_name: &str,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioningv1alpha1::DPFHCPProvisioner {
        metadata: object_meta(name, namespace),
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: dpu_cluster_ref(cluster_name, cluster_ns),
            dpu_deployment_ref: dpu_deployment_ref(deployment_name, cluster_ns),
            base_domain: "test.example.com".into(),
            ocp_release_image: "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64".into(),
            ssh_key_secret_ref: local_ref("dummy-ssh"),
            pull_secret_ref: local_ref("dummy-pull"),
            control_plane_availability_policy: "SingleReplica".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `Eventually(func(g Gomega) { Get; SetStatusCondition(IgnitionConfigured=True); Status().Update }, "10s", "1s")`
async fn set_ignition_configured_true(s: &Suite, name: &str, namespace: &str) {
    eventually(secs(10), secs(1), async || {
        let mut fresh: provisioningv1alpha1::DPFHCPProvisioner =
            get(&s.k8s_client, name, namespace).await?;
        meta::set_status_condition(
            &mut fresh.status.conditions,
            cond_now(
                provisioningv1alpha1::IGNITION_CONFIGURED,
                CONDITION_TRUE,
                provisioningv1alpha1::REASON_IGNITION_GENERATED,
                "",
            ),
        );
        s.k8s_client
            .update_status(&mut fresh)
            .await
            .map_err(|e| e.to_string())
    })
    .await;
}

/// `Eventually(func(g Gomega) { ...cond IgnitionConfigured False/reason... }, "30s", "1s")`
async fn eventually_ignition_configured_false(
    s: &Suite,
    name: &str,
    namespace: &str,
    reason: &str,
) {
    eventually(secs(30), secs(1), async || {
        let fresh: provisioningv1alpha1::DPFHCPProvisioner =
            get(&s.k8s_client, name, namespace).await?;
        let Some(cond) = find_cond(&fresh, provisioningv1alpha1::IGNITION_CONFIGURED) else {
            return Err("IgnitionConfigured condition should exist".into());
        };
        equal(cond.status.as_str(), CONDITION_FALSE)?;
        equal(cond.reason.as_str(), reason)
    })
    .await;
}

mod dpu_deployment_flavor_change_integration_envtest {
    use super::*;

    const PROVISIONER_NAME: &str = "test-provisioner-dpudeploy";
    const PROVISIONER_NS: &str = "clusters";
    const DPU_CLUSTER_NS: &str = "dpudeployment-integ-dpucluster";
    const DPU_CLUSTER_NAME: &str = "integ-dpucluster";
    const DEPLOYMENT_NAME: &str = "integ-deployment";

    #[test]
    fn should_detect_flavor_change_and_set_ignition_configured_false_via_controller_reconcile() {
        run(async |s| {
            let c = &s.k8s_client;

            // By("creating DPUCluster namespace")
            create_namespace(c, DPU_CLUSTER_NS)
                .await
                .expect("create namespace");

            // By("creating DPUFlavor stubs")
            c.create(&mut dpu_flavor("old-flavor", DPU_CLUSTER_NS))
                .await
                .expect("create old-flavor");
            c.create(&mut dpu_flavor("new-flavor", DPU_CLUSTER_NS))
                .await
                .expect("create new-flavor");

            // By("creating DPUDeployment stub with old-flavor")
            c.create(&mut stub_dpu_deployment(
                DEPLOYMENT_NAME,
                DPU_CLUSTER_NS,
                "old-flavor",
            ))
            .await
            .expect("create DPUDeployment");

            // By("creating ignition ConfigMap with old-flavor annotation")
            c.create(&mut integ_ignition_cm(
                DPU_CLUSTER_NAME,
                DPU_CLUSTER_NS,
                "old-flavor",
            ))
            .await
            .expect("create ConfigMap");

            // By("creating DPFHCPProvisioner CR with IgnitionConfigured=True (simulating Ready state)")
            c.create(&mut integ_provisioner(
                PROVISIONER_NAME,
                PROVISIONER_NS,
                DPU_CLUSTER_NAME,
                DPU_CLUSTER_NS,
                DEPLOYMENT_NAME,
            ))
            .await
            .expect("create provisioner");

            // By("setting IgnitionConfigured=True on the CR status (simulating post-ignition state)")
            set_ignition_configured_true(s, PROVISIONER_NAME, PROVISIONER_NS).await;

            // By("updating DPUDeployment flavor from old-flavor to new-flavor")
            eventually(secs(10), secs(1), async || {
                let mut fresh_dd: dpuservicev1alpha1::DPUDeployment =
                    get(c, DEPLOYMENT_NAME, DPU_CLUSTER_NS).await?;
                fresh_dd.spec.dpus.flavor = Some("new-flavor".into());
                c.update(&mut fresh_dd).await.map_err(|e| e.to_string())
            })
            .await;

            // By("verifying the controller detects the flavor change and sets IgnitionConfigured=False")
            eventually_ignition_configured_false(
                s,
                PROVISIONER_NAME,
                PROVISIONER_NS,
                provisioningv1alpha1::REASON_DPU_DEPLOYMENT_CHANGED,
            )
            .await;
        });
    }
}

mod dpu_flavor_deletion_integration_envtest {
    use super::*;

    const PROVISIONER_NAME: &str = "test-prov-fldel";
    const PROVISIONER_NS: &str = "clusters";
    const DPU_CLUSTER_NS: &str = "fldel-dpucluster";
    const DPU_CLUSTER_NAME: &str = "fldel-cluster";
    const DEPLOYMENT_NAME: &str = "fldel-deploy";
    const FLAVOR_NAME: &str = "fldel-flavor";

    #[test]
    fn should_delete_config_map_and_set_ignition_configured_false_when_dpu_flavor_is_deleted() {
        run(async |s| {
            let c = &s.k8s_client;

            // By("creating DPUCluster namespace")
            create_namespace(c, DPU_CLUSTER_NS)
                .await
                .expect("create namespace");

            // By("creating DPUFlavor")
            let mut flavor = dpu_flavor(FLAVOR_NAME, DPU_CLUSTER_NS);
            c.create(&mut flavor).await.expect("create flavor");

            // By("creating DPUDeployment referencing the flavor")
            c.create(&mut stub_dpu_deployment(
                DEPLOYMENT_NAME,
                DPU_CLUSTER_NS,
                FLAVOR_NAME,
            ))
            .await
            .expect("create DPUDeployment");

            // By("creating ignition ConfigMap")
            let cm_name = ignition::config_map_name(DPU_CLUSTER_NAME);
            c.create(&mut integ_ignition_cm(
                DPU_CLUSTER_NAME,
                DPU_CLUSTER_NS,
                FLAVOR_NAME,
            ))
            .await
            .expect("create ConfigMap");

            // By("creating DPFHCPProvisioner CR")
            c.create(&mut integ_provisioner(
                PROVISIONER_NAME,
                PROVISIONER_NS,
                DPU_CLUSTER_NAME,
                DPU_CLUSTER_NS,
                DEPLOYMENT_NAME,
            ))
            .await
            .expect("create provisioner");

            // By("setting IgnitionConfigured=True (simulating post-ignition state)")
            set_ignition_configured_true(s, PROVISIONER_NAME, PROVISIONER_NS).await;

            // By("deleting the DPUFlavor")
            c.delete(&flavor).await.expect("delete flavor");

            // By("verifying IgnitionConfigured=False with DependencyDeleted reason")
            eventually_ignition_configured_false(
                s,
                PROVISIONER_NAME,
                PROVISIONER_NS,
                provisioningv1alpha1::REASON_DEPENDENCY_DELETED,
            )
            .await;

            // By("verifying the ignition ConfigMap was deleted")
            eventually(secs(10), secs(1), async || {
                match get::<ConfigMap>(c, &cm_name, DPU_CLUSTER_NS).await {
                    Err(_) => Ok(()),
                    Ok(_) => Err("ConfigMap should be deleted when DPUFlavor is removed".into()),
                }
            })
            .await;
        });
    }
}
