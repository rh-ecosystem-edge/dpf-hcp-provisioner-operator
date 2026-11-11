//! Port of `internal/controller/kubeconfiginjection/watch_test.go`.
//! `Describe` → nested module, `It` → test function.

use std::collections::BTreeMap;

use apis::k8s::core::v1::ObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{Client, Ctx, ObjectKey, Predicate};
use ctrlrt_fake::{FakeClientBuilder, InitObject, Scheme};
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::watch::*;

/// BeforeEach state.
struct Suite {
    ctx: Ctx,
    scheme: Scheme,
}

/// BeforeEach
fn before_each() -> Suite {
    Suite {
        ctx: Ctx::background(),
        scheme: Scheme::new()
            // provisioningv1alpha1.AddToScheme
            .register::<provisioningv1alpha1::DPFHCPProvisioner>()
            .register::<provisioningv1alpha1::DPFHCPProvisionerConfig>()
            // corev1.AddToScheme (kinds used here)
            .register::<Secret>()
            .register::<ConfigMap>(),
    }
}

impl Suite {
    fn client(&self, objs: Vec<InitObject>) -> Client {
        FakeClientBuilder::new()
            .with_scheme(self.scheme.clone())
            .with_objects(objs)
            .build()
    }
}

fn meta(name: &str, namespace: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.into()),
        namespace: Some(namespace.into()),
        ..Default::default()
    }
}

fn secret(name: &str) -> Secret {
    Secret {
        metadata: meta(name, "test-ns"),
        ..Default::default()
    }
}

fn secret_with_kubeconfig(data: &str) -> Secret {
    Secret {
        metadata: meta("test-provisioner-admin-kubeconfig", "test-ns"),
        data: Some(BTreeMap::from([(
            "kubeconfig".to_string(),
            ByteString(data.as_bytes().to_vec()),
        )])),
        ..Default::default()
    }
}

fn provisioner(name: &str, namespace: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
    provisioningv1alpha1::DPFHCPProvisioner {
        metadata: meta(name, namespace),
        ..Default::default()
    }
}

mod watch_functions {
    use super::*;

    mod is_hosted_cluster_kubeconfig_secret {
        use super::*;
        use crate::watch::is_hosted_cluster_kubeconfig_secret;

        #[test]
        fn should_return_true_for_valid_hc_kubeconfig_secret() {
            let _s = before_each();
            let secret = secret("test-cluster-admin-kubeconfig");

            let result = is_hosted_cluster_kubeconfig_secret(&secret);
            assert!(result);
        }

        #[test]
        fn should_return_false_for_non_kubeconfig_secret() {
            let _s = before_each();
            let secret = secret("random-secret");

            let result = is_hosted_cluster_kubeconfig_secret(&secret);
            assert!(!result);
        }

        #[test]
        fn should_return_false_for_secret_with_partial_match() {
            let _s = before_each();
            let secret = secret("admin-kubeconfig-extra");

            let result = is_hosted_cluster_kubeconfig_secret(&secret);
            assert!(!result);
        }

        #[test]
        fn should_return_false_for_non_secret_object() {
            let _s = before_each();
            let config_map = ConfigMap {
                metadata: meta("test-admin-kubeconfig", "test-ns"),
                ..Default::default()
            };

            let result = is_hosted_cluster_kubeconfig_secret(&config_map);
            assert!(!result);
        }
    }

    mod is_hosted_cluster_kubeconfig_secret_predicate {
        use super::*;

        fn predicate() -> ctrlrt::Funcs<Secret> {
            is_hosted_cluster_kubeconfig_secret_predicate::<Secret>()
        }

        #[test]
        fn should_accept_create_event_with_valid_kubeconfig_secret() {
            let _s = before_each();
            let secret = secret("test-cluster-admin-kubeconfig");

            let result = predicate().create(&secret);
            assert!(result);
        }

        #[test]
        fn should_reject_create_event_with_non_kubeconfig_secret() {
            let _s = before_each();
            let secret = secret("random-secret");

            let result = predicate().create(&secret);
            assert!(!result);
        }

        #[test]
        fn should_accept_update_event_with_valid_kubeconfig_secret() {
            let _s = before_each();
            let secret = secret("test-cluster-admin-kubeconfig");

            // Go passes `event.UpdateEvent{ObjectNew: secret}` (ObjectOld nil).
            let result = predicate().update(&Secret::default(), &secret);
            assert!(result);
        }

        #[test]
        fn should_accept_delete_event_with_valid_kubeconfig_secret() {
            let _s = before_each();
            let secret = secret("test-cluster-admin-kubeconfig");

            let result = predicate().delete(&secret);
            assert!(result);
        }

        #[test]
        fn should_reject_generic_event() {
            let _s = before_each();
            let secret = secret("test-cluster-admin-kubeconfig");

            let result = predicate().generic(&secret);
            assert!(!result);
        }
    }

    mod find_provisioner_for_kubeconfig_secret {
        use super::*;

        #[tokio::test]
        async fn should_return_reconcile_request_when_provisioner_exists() {
            let s = before_each();
            // Given: DPFHCPProvisioner and its HC kubeconfig secret
            let mut provisioner = provisioner("test-provisioner", "test-ns");

            let secret = secret("test-provisioner-admin-kubeconfig");

            let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)]);

            // When: Mapping function is called
            let requests =
                find_provisioner_for_kubeconfig_secret(&s.ctx, &fake_client, &secret).await;

            // Then: Should return reconcile request for the provisioner
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].name, "test-provisioner");
            assert_eq!(requests[0].namespace, "test-ns");
        }

        #[tokio::test]
        async fn should_return_empty_when_provisioner_not_found() {
            let s = before_each();
            // Given: HC kubeconfig secret but no corresponding provisioner
            let secret = secret("test-provisioner-admin-kubeconfig");

            let fake_client = s.client(vec![]);

            // When: Mapping function is called
            let requests =
                find_provisioner_for_kubeconfig_secret(&s.ctx, &fake_client, &secret).await;

            // Then: Should return empty (no reconciliation needed)
            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_return_empty_when_provisioner_exists_but_in_different_namespace() {
            let s = before_each();
            // Given: Provisioner in different namespace than secret
            let mut provisioner = provisioner("test-provisioner", "different-ns");

            let secret = secret("test-provisioner-admin-kubeconfig");

            let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)]);

            // When: Mapping function is called
            let requests =
                find_provisioner_for_kubeconfig_secret(&s.ctx, &fake_client, &secret).await;

            // Then: Should return empty (namespace mismatch)
            assert!(requests.is_empty());
        }

        #[tokio::test]
        async fn should_handle_name_extraction_correctly() {
            let s = before_each();
            // Given: Provisioner with complex name
            let mut provisioner = provisioner("cluster-with-dashes", "test-ns");

            let secret = secret("cluster-with-dashes-admin-kubeconfig");

            let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)]);

            // When: Mapping function is called
            let requests =
                find_provisioner_for_kubeconfig_secret(&s.ctx, &fake_client, &secret).await;

            // Then: Should correctly extract name and find provisioner
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].name, "cluster-with-dashes");
        }

        #[tokio::test]
        async fn should_return_empty_for_invalid_object_type() {
            let s = before_each();
            // Given: Non-secret object
            let config_map = ConfigMap {
                metadata: meta("test-admin-kubeconfig", "test-ns"),
                ..Default::default()
            };

            let fake_client = s.client(vec![]);

            // When: Mapping function is called
            let requests =
                find_provisioner_for_kubeconfig_secret(&s.ctx, &fake_client, &config_map).await;

            // Then: Should return empty
            assert!(requests.is_empty());
        }
    }

    mod watch_integration {
        use super::*;

        #[tokio::test]
        async fn should_trigger_reconciliation_when_hc_kubeconfig_secret_is_created() {
            let s = before_each();
            // Given: DPFHCPProvisioner waiting for kubeconfig
            let mut provisioner = provisioner("test-provisioner", "test-ns");
            provisioner.status.hosted_cluster_ref = Some(ObjectReference {
                name: "test-provisioner".into(),
                namespace: "test-ns".into(),
                ..Default::default()
            });

            let secret = secret_with_kubeconfig("fake-kubeconfig-data");

            let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)]);

            let predicate = is_hosted_cluster_kubeconfig_secret_predicate::<Secret>();

            // When: Secret is created
            let should_reconcile = predicate.create(&secret);

            // Then: Predicate should accept
            assert!(should_reconcile);

            // And: Mapping function should return reconcile request
            let requests =
                find_provisioner_for_kubeconfig_secret(&s.ctx, &fake_client, &secret).await;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0], ObjectKey::new("test-ns", "test-provisioner"));
        }

        #[tokio::test]
        async fn should_trigger_reconciliation_when_hc_kubeconfig_secret_is_updated_drift() {
            let s = before_each();
            // Given: Existing secret and provisioner
            let mut provisioner = provisioner("test-provisioner", "test-ns");

            let old_secret = secret_with_kubeconfig("old-kubeconfig");
            let new_secret = secret_with_kubeconfig("new-rotated-kubeconfig");

            let fake_client = s.client(vec![ctrlrt_fake::obj_mut(&mut provisioner)]);

            let predicate = is_hosted_cluster_kubeconfig_secret_predicate::<Secret>();

            // When: Secret is updated
            let should_reconcile = predicate.update(&old_secret, &new_secret);

            // Then: Predicate should accept
            assert!(should_reconcile);

            // And: Mapping function should return reconcile request
            let requests =
                find_provisioner_for_kubeconfig_secret(&s.ctx, &fake_client, &new_secret).await;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].name, "test-provisioner");
        }
    }
}
