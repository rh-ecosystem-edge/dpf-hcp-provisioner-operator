//! Port of `internal/controller/dpuflavor_watch_test.go`.

use std::sync::Arc;

use apis::dpf::dpuservice::v1alpha1 as dpuservicev1alpha1;
use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::meta::{CONDITION_FALSE, CONDITION_TRUE};
use ctrlrt::{Ctx, FakeRecorder, ObjectKey, Recorder};
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj, obj_mut};
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta};

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

fn provisioner(
    name: &str,
    dd_name: Option<&str>,
    conditions: Vec<Condition>,
) -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioningv1alpha1::DPFHCPProvisioner {
        metadata: object_meta(name, "default"),
        spec: provisioningv1alpha1::DPFHCPProvisionerSpec {
            dpu_cluster_ref: dpu_cluster_ref("cluster", "dpu-system"),
            dpu_deployment_ref: dd_name.and_then(|n| dpu_deployment_ref(n, "dpf-operator-system")),
            ..Default::default()
        },
        status: provisioningv1alpha1::DPFHCPProvisionerStatus {
            conditions,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn dpu_deployment(bfb: Option<&str>, flavor: &str) -> dpuservicev1alpha1::DPUDeployment {
    dpuservicev1alpha1::DPUDeployment {
        metadata: object_meta("my-deployment", "dpf-operator-system"),
        spec: dpuservicev1alpha1::DPUDeploymentSpec {
            dpus: dpuservicev1alpha1::DPUs {
                bfb: bfb.map(String::from),
                flavor: Some(flavor.into()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn dpu_flavor(name: &str) -> dpuprovisioningv1alpha1::DPUFlavor {
    dpuprovisioningv1alpha1::DPUFlavor {
        metadata: object_meta(name, "dpf-operator-system"),
        ..Default::default()
    }
}

fn ignition_cm(annotations: Option<&[(&str, &str)]>) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            labels: Some(
                [(
                    ignition::BFCFG_TEMPLATE_LABEL.to_string(),
                    "true".to_string(),
                )]
                .into(),
            ),
            annotations: annotations.map(|a| {
                a.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            }),
            ..object_meta(&ignition::config_map_name("cluster"), "dpu-system")
        },
        ..Default::default()
    }
}

fn ignition_configured_true() -> Vec<Condition> {
    vec![cond_now(
        provisioningv1alpha1::IGNITION_CONFIGURED,
        CONDITION_TRUE,
        provisioningv1alpha1::REASON_IGNITION_GENERATED,
        "",
    )]
}

mod dpu_flavor_watch {
    use super::*;

    mod dpu_flavor_to_requests {
        use super::*;

        #[tokio::test]
        async fn should_map_to_the_dpfhcp_provisioner_when_dpu_deployment_references_the_flavor() {
            let p = provisioner("test-provisioner", Some("my-deployment"), vec![]);
            let dd = dpu_deployment(None, "my-flavor");
            let flavor = dpu_flavor("my-flavor");

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&p), obj(&dd)])
                .build();

            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                ..Default::default()
            };
            let requests = r.dpu_flavor_to_requests(&Ctx::background(), &flavor).await;

            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0], ObjectKey::new("default", "test-provisioner"));
        }

        #[tokio::test]
        async fn should_return_empty_when_no_dpu_deployment_references_the_flavor() {
            let p = provisioner("test-provisioner", Some("my-deployment"), vec![]);
            let dd = dpu_deployment(None, "other-flavor");
            let flavor = dpu_flavor("my-flavor");

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&p), obj(&dd)])
                .build();

            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                ..Default::default()
            };
            let requests = r.dpu_flavor_to_requests(&Ctx::background(), &flavor).await;

            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_return_empty_when_dpu_deployment_references_a_different_flavor() {
            let p = provisioner("test-provisioner", Some("my-deployment"), vec![]);
            let dd = dpu_deployment(None, "other-flavor");
            let flavor = dpu_flavor("my-flavor");

            let fake_client = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj(&p), obj(&dd)])
                .build();

            let r = DPFHCPProvisionerReconciler {
                client: fake_client,
                ..Default::default()
            };
            let requests = r.dpu_flavor_to_requests(&Ctx::background(), &flavor).await;

            assert!(requests.is_empty());
        }
    }

    mod handle_dependency_changes_with_missing_dependencies {
        use super::*;

        #[tokio::test]
        async fn should_delete_config_map_and_set_dependency_deleted_when_dpu_deployment_is_deleted()
         {
            let mut p = provisioner(
                "test",
                Some("deleted-deployment"),
                ignition_configured_true(),
            );
            let cm = ignition_cm(None);

            let fake = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj_mut(&mut p), obj(&cm)])
                .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake.clone(),
                recorder: recorder(),
                ..Default::default()
            };

            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut p)
                .await
                .expect("no error");
            assert!(changed);

            let cond = find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(cond.reason, provisioningv1alpha1::REASON_DEPENDENCY_DELETED);

            // ConfigMap should be deleted
            assert!(
                get::<ConfigMap>(&fake, &ignition::config_map_name("cluster"), "dpu-system")
                    .await
                    .is_err()
            );
        }

        #[tokio::test]
        async fn should_delete_config_map_and_set_dependency_deleted_when_dpu_flavor_is_deleted() {
            let mut p = provisioner("test", Some("my-deployment"), ignition_configured_true());
            let dd = dpu_deployment(None, "deleted-flavor");
            let cm = ignition_cm(Some(&[(
                ignition::BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION,
                "deleted-flavor",
            )]));

            let fake = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj_mut(&mut p), obj(&dd), obj(&cm)])
                .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake.clone(),
                recorder: recorder(),
                ..Default::default()
            };

            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut p)
                .await
                .expect("no error");
            assert!(changed);

            let cond = find_cond(&p, provisioningv1alpha1::IGNITION_CONFIGURED).unwrap();
            assert_eq!(cond.status, CONDITION_FALSE);
            assert_eq!(cond.reason, provisioningv1alpha1::REASON_DEPENDENCY_DELETED);
            assert!(
                cond.message
                    .contains("DPUFlavor dpf-operator-system/deleted-flavor not found"),
                "{}",
                cond.message
            );

            // ConfigMap should be deleted
            assert!(
                get::<ConfigMap>(&fake, &ignition::config_map_name("cluster"), "dpu-system")
                    .await
                    .is_err()
            );
        }

        #[tokio::test]
        async fn should_not_trigger_when_dpu_flavor_exists() {
            let mut p = provisioner("test", Some("my-deployment"), ignition_configured_true());
            let dd = dpu_deployment(Some("my-bfb"), "my-flavor");
            let flavor = dpu_flavor("my-flavor");
            let cm = ignition_cm(Some(&[
                (ignition::BFCFG_TEMPLATE_CLUSTER_NAME_ANNOTATION, "cluster"),
                (
                    ignition::BFCFG_TEMPLATE_DPU_FLAVOR_NAME_ANNOTATION,
                    "my-flavor",
                ),
                (ignition::BFCFG_TEMPLATE_BFB_NAME_ANNOTATION, "my-bfb"),
            ]));

            let fake = FakeClientBuilder::new()
                .with_scheme(scheme())
                .with_objects([obj_mut(&mut p), obj(&dd), obj(&flavor), obj(&cm)])
                .with_status_subresource::<provisioningv1alpha1::DPFHCPProvisioner>()
                .build();
            let r = DPFHCPProvisionerReconciler {
                client: fake,
                recorder: recorder(),
                ..Default::default()
            };

            let (changed, _) = r
                .handle_dependency_changes(&Ctx::background(), &mut p)
                .await
                .expect("no error");
            assert!(!changed);
        }
    }

    mod phase_computation_with_dependency_deleted {
        use super::*;

        #[tokio::test]
        async fn should_transition_to_ignition_generating_not_failed_when_reason_is_dependency_deleted()
         {
            let mut p = provisioner(
                "test",
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
                        provisioningv1alpha1::REASON_DEPENDENCY_DELETED,
                        "",
                    ),
                ],
            );
            p.metadata.generation = Some(1);
            p.status.hosted_cluster_ref = object_ref("test", "default");

            let r = DPFHCPProvisionerReconciler {
                recorder: recorder(),
                ..Default::default()
            };
            r.update_phase_from_conditions(&Ctx::background(), &mut p)
                .await;

            assert_ne!(p.status.phase, provisioningv1alpha1::PHASE_FAILED);
            assert_eq!(
                p.status.phase,
                provisioningv1alpha1::PHASE_GENERATING_IGNITION
            );
        }
    }
}
