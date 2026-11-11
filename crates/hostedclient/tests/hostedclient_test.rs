//! Tests for `internal/hostedclient`. The Go package has no `_test.go` files; these are
//! Rust-only tests pinning the ported behaviour (error strings, cache semantics,
//! CreateOrUpdate decisions). See `TEST_MAPPING.md`.

use std::collections::BTreeMap;
use std::sync::Arc;

use ctrlrt::{Ctx, Object};
use ctrlrt_fake::{FakeClientBuilder, Scheme, new_simple_clientset, obj};
use hostedclient::{
    ClientManager, HostedClient, OperationResult, create_or_update,
    replace_server_with_internal_endpoint, semantic_deep_equal, test_connection,
};
use k8s_openapi::ByteString;
use k8s_openapi::api::apps::v1::DaemonSet;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::config::Kubeconfig;

const KUBECONFIG: &str = r#"apiVersion: v1
kind: Config
clusters:
- name: hc
  cluster:
    server: https://10.6.135.42:6443
    insecure-skip-tls-verify: true
- name: other
  cluster:
    server: https://other:6443
contexts:
- name: admin
  context:
    cluster: hc
    user: admin
current-context: admin
users:
- name: admin
  user:
    token: abc
"#;

fn kubeconfig() -> Kubeconfig {
    Kubeconfig::from_yaml(KUBECONFIG).unwrap()
}

fn secret(ns: &str, name: &str, data: Option<BTreeMap<String, ByteString>>) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(name.into()),
            namespace: Some(ns.into()),
            ..Default::default()
        },
        data,
        ..Default::default()
    }
}

fn manager(objs: Vec<Secret>) -> ClientManager {
    let c = FakeClientBuilder::new()
        .with_scheme(Scheme::new().register::<Secret>())
        .with_objects(objs.iter().map(obj).collect::<Vec<_>>())
        .build();
    ClientManager::new(c)
}

mod replace_server_with_internal_endpoint_tests {
    use super::*;

    #[test]
    fn rewrites_current_context_cluster_server() {
        let mut kc = kubeconfig();
        replace_server_with_internal_endpoint(Some(&mut kc), "clusters", "doca").unwrap();
        assert_eq!(
            kc.clusters[0].cluster.as_ref().unwrap().server.as_deref(),
            Some("https://kube-apiserver.clusters-doca.svc.cluster.local:6443")
        );
        // Other clusters are untouched.
        assert_eq!(
            kc.clusters[1].cluster.as_ref().unwrap().server.as_deref(),
            Some("https://other:6443")
        );
    }

    #[test]
    fn nil_kubeconfig() {
        let err = replace_server_with_internal_endpoint(None, "ns", "n").unwrap_err();
        assert_eq!(err.to_string(), "kubeconfig is nil");
    }

    #[test]
    fn no_current_context() {
        let mut kc = kubeconfig();
        kc.current_context = None;
        let err = replace_server_with_internal_endpoint(Some(&mut kc), "ns", "n").unwrap_err();
        assert_eq!(err.to_string(), "kubeconfig has no current context");
    }

    #[test]
    fn context_not_found() {
        let mut kc = kubeconfig();
        kc.current_context = Some("missing".into());
        let err = replace_server_with_internal_endpoint(Some(&mut kc), "ns", "n").unwrap_err();
        assert_eq!(err.to_string(), "context missing not found in kubeconfig");
    }

    #[test]
    fn cluster_not_found() {
        let mut kc = kubeconfig();
        kc.contexts[0].context.as_mut().unwrap().cluster = "gone".into();
        let err = replace_server_with_internal_endpoint(Some(&mut kc), "ns", "n").unwrap_err();
        assert_eq!(err.to_string(), "cluster gone not found in kubeconfig");
    }
}

mod get_kubeconfig_data {
    use super::*;

    #[tokio::test]
    async fn returns_kubeconfig_bytes() {
        let data = BTreeMap::from([("kubeconfig".to_string(), ByteString(b"abc".to_vec()))]);
        let cm = manager(vec![secret(
            "clusters",
            "doca-admin-kubeconfig",
            Some(data),
        )]);
        let got = cm
            .get_kubeconfig_data(&Ctx::background(), "clusters", "doca")
            .await
            .unwrap();
        assert_eq!(got, b"abc");
    }

    #[tokio::test]
    async fn secret_not_found() {
        let cm = manager(vec![]);
        let err = cm
            .get_kubeconfig_data(&Ctx::background(), "clusters", "doca")
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "failed to get kubeconfig secret clusters/doca-admin-kubeconfig: secrets \"doca-admin-kubeconfig\" not found"
        );
        assert!(ctrlrt::error::is_not_found(&err));
    }

    #[tokio::test]
    async fn key_not_found() {
        let cm = manager(vec![secret("clusters", "doca-admin-kubeconfig", None)]);
        let err = cm
            .get_kubeconfig_data(&Ctx::background(), "clusters", "doca")
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "kubeconfig key not found in secret clusters/doca-admin-kubeconfig"
        );
    }

    #[tokio::test]
    async fn empty_data() {
        let data = BTreeMap::from([("kubeconfig".to_string(), ByteString(vec![]))]);
        let cm = manager(vec![secret(
            "clusters",
            "doca-admin-kubeconfig",
            Some(data),
        )]);
        let err = cm
            .get_kubeconfig_data(&Ctx::background(), "clusters", "doca")
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "kubeconfig data is empty in secret clusters/doca-admin-kubeconfig"
        );
    }
}

mod client_cache {
    use super::*;

    #[tokio::test]
    async fn missing_secret_error_is_wrapped() {
        let cm = manager(vec![]);
        let err = cm
            .get_hosted_cluster_client(&Ctx::background(), "clusters", "doca")
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "failed to get kubeconfig: failed to get kubeconfig secret clusters/doca-admin-kubeconfig: secrets \"doca-admin-kubeconfig\" not found"
        );
        assert!(!cm.has_cached_client("clusters", "doca"));
    }

    #[tokio::test]
    async fn invalid_kubeconfig_is_reported() {
        let data = BTreeMap::from([(
            "kubeconfig".to_string(),
            ByteString(b"apiVersion: v1\nkind: Config\nclusters: []\ncontexts: []\n".to_vec()),
        )]);
        let cm = manager(vec![secret(
            "clusters",
            "doca-admin-kubeconfig",
            Some(data),
        )]);
        let err = cm
            .get_hosted_cluster_client(&Ctx::background(), "clusters", "doca")
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "failed to replace server endpoint: kubeconfig has no current context"
        );
    }

    #[tokio::test]
    async fn creates_and_caches_client_from_kubeconfig() {
        let data = BTreeMap::from([(
            "kubeconfig".to_string(),
            ByteString(KUBECONFIG.as_bytes().to_vec()),
        )]);
        let cm = manager(vec![secret(
            "clusters",
            "doca-admin-kubeconfig",
            Some(data),
        )]);
        let ctx = Ctx::background();
        let c = cm
            .get_hosted_cluster_client(&ctx, "clusters", "doca")
            .await
            .unwrap();
        assert!(c.kube_client().is_some());
        assert!(cm.has_cached_client("clusters", "doca"));
        assert_eq!(cm.cached_client_count(), 1);

        cm.invalidate_client("clusters", "doca");
        assert!(!cm.has_cached_client("clusters", "doca"));
    }

    #[tokio::test]
    async fn injected_client_is_returned_from_cache() {
        let cm = manager(vec![]);
        let ns = Namespace {
            metadata: ObjectMeta {
                name: Some("seeded".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let hc = HostedClient::fake(new_simple_clientset([obj(&ns)]));
        cm.set_hosted_cluster_client("clusters", "doca", hc);

        let ctx = Ctx::background();
        let got = cm
            .get_hosted_cluster_client(&ctx, "clusters", "doca")
            .await
            .unwrap();
        let fetched: Namespace = got.get_named("", "seeded").await.unwrap();
        assert_eq!(fetched.name(), "seeded");
        test_connection(&ctx, &got).await.unwrap();

        cm.invalidate_client("clusters", "doca");
        assert!(
            cm.get_hosted_cluster_client(&ctx, "clusters", "doca")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_connection_wraps_discovery_error() {
        let hc = HostedClient::fake(new_simple_clientset(Vec::<ctrlrt_fake::InitObject>::new()))
            .with_server_version(Arc::new(|| Err(anyhow::anyhow!("connection refused"))));
        let err = test_connection(&Ctx::background(), &hc).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "failed to connect to hosted cluster API server: connection refused"
        );
    }
}

mod create_or_update_tests {
    use super::*;

    fn namespace(name: &str) -> Namespace {
        Namespace {
            metadata: ObjectMeta {
                name: Some(name.into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn add_labels(ns: &mut Namespace) -> ctrlrt::Result<()> {
        ns.metadata
            .labels
            .get_or_insert_with(Default::default)
            .insert("app".into(), "x".into());
        Ok(())
    }

    #[tokio::test]
    async fn creates_when_missing_then_unchanged_then_updated() {
        let ctx = Ctx::background();
        let hc = HostedClient::fake(new_simple_clientset(Vec::<ctrlrt_fake::InitObject>::new()));
        let nsc = hc.resource::<Namespace>("");

        let mut ns = namespace("holder");
        let r = create_or_update(&ctx, &nsc, &mut ns, Some(&mut add_labels))
            .await
            .unwrap();
        assert_eq!(r, OperationResult::Created);
        assert_eq!(r.to_string(), "created");
        // The mutate function ran on the passed object.
        assert_eq!(ns.labels().get("app").map(String::as_str), Some("x"));

        let mut ns = namespace("holder");
        let r = create_or_update(&ctx, &nsc, &mut ns, Some(&mut add_labels))
            .await
            .unwrap();
        assert_eq!(r, OperationResult::None);
        assert_eq!(r.as_str(), "unchanged");

        let mut ns = namespace("holder");
        let mut relabel = |ns: &mut Namespace| -> ctrlrt::Result<()> {
            ns.metadata
                .labels
                .get_or_insert_with(Default::default)
                .insert("app".into(), "y".into());
            Ok(())
        };
        let r = create_or_update(&ctx, &nsc, &mut ns, Some(&mut relabel))
            .await
            .unwrap();
        assert_eq!(r, OperationResult::Updated);
        assert_eq!(r.as_str(), "updated");
        let got = nsc.get(&ctx, "holder").await.unwrap();
        assert_eq!(got.labels().get("app").map(String::as_str), Some("y"));
    }

    #[tokio::test]
    async fn nil_mutate_fn_creates_and_is_unchanged() {
        let ctx = Ctx::background();
        let hc = HostedClient::fake(new_simple_clientset(Vec::<ctrlrt_fake::InitObject>::new()));
        let nsc = hc.resource::<Namespace>("");
        let mut ns = namespace("a");
        assert_eq!(
            create_or_update(&ctx, &nsc, &mut ns, None).await.unwrap(),
            OperationResult::Created
        );
        assert_eq!(
            create_or_update(&ctx, &nsc, &mut ns, None).await.unwrap(),
            OperationResult::None
        );
    }

    #[tokio::test]
    async fn mutate_cannot_change_name() {
        let ctx = Ctx::background();
        let hc = HostedClient::fake(new_simple_clientset(Vec::<ctrlrt_fake::InitObject>::new()));
        let nsc = hc.resource::<Namespace>("");
        let mut rename = |ns: &mut Namespace| -> ctrlrt::Result<()> {
            ns.metadata.name = Some("other".into());
            Ok(())
        };
        let mut ns = namespace("a");
        let err = create_or_update(&ctx, &nsc, &mut ns, Some(&mut rename))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "mutate function cannot change object name or namespace"
        );

        // Same check on the update path.
        let mut ns = namespace("a");
        create_or_update(&ctx, &nsc, &mut ns, None).await.unwrap();
        let mut ns = namespace("a");
        let err = create_or_update(&ctx, &nsc, &mut ns, Some(&mut rename))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "mutate function cannot change object name or namespace"
        );
    }

    #[tokio::test]
    async fn mutate_error_is_returned() {
        let ctx = Ctx::background();
        let hc = HostedClient::fake(new_simple_clientset(Vec::<ctrlrt_fake::InitObject>::new()));
        let nsc = hc.resource::<Namespace>("");
        let mut fail = |_: &mut Namespace| -> ctrlrt::Result<()> { Err(anyhow::anyhow!("boom")) };
        let mut ns = namespace("a");
        let err = create_or_update(&ctx, &nsc, &mut ns, Some(&mut fail))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "boom");
    }

    #[tokio::test]
    async fn get_error_is_returned_unwrapped() {
        let ctx = Ctx::background();
        let fake = ctrlrt_fake::FakeClientsetBuilder::new().build_fake();
        fake.prepend_reactor("get", "namespaces", |_| {
            ctrlrt_fake::Reaction::error(ctrlrt::error::ApiError::internal("etcd down"))
        });
        let hc = HostedClient::fake(fake.client());
        let nsc = hc.resource::<Namespace>("");
        let mut ns = namespace("a");
        let err = create_or_update(&ctx, &nsc, &mut ns, None)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Internal error occurred: etcd down");
    }

    #[tokio::test]
    async fn namespaced_resource_client_scopes_requests() {
        let ctx = Ctx::background();
        let hc = HostedClient::fake(new_simple_clientset(Vec::<ctrlrt_fake::InitObject>::new()));
        let dsc = hc.resource::<DaemonSet>("holder");
        assert_eq!(dsc.namespace(), "holder");
        let mut ds = DaemonSet {
            metadata: ObjectMeta {
                name: Some("ds".into()),
                namespace: Some("holder".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            create_or_update(&ctx, &dsc, &mut ds, None).await.unwrap(),
            OperationResult::Created
        );
        assert_eq!(dsc.list(&ctx).await.unwrap().len(), 1);
        assert!(
            hc.resource::<DaemonSet>("other")
                .list(&ctx)
                .await
                .unwrap()
                .is_empty()
        );
        dsc.delete(&ctx, "ds").await.unwrap();
        assert!(ctrlrt::error::is_not_found(
            &dsc.get(&ctx, "ds").await.unwrap_err()
        ));
    }

    #[test]
    fn semantic_equality_treats_nil_and_empty_as_equal() {
        let a = namespace("a");
        let mut b = namespace("a");
        b.metadata.labels = Some(BTreeMap::new());
        b.metadata.finalizers = Some(vec![]);
        assert!(semantic_deep_equal(&a, &b));
        b.metadata.labels = Some(BTreeMap::from([("k".into(), "v".into())]));
        assert!(!semantic_deep_equal(&a, &b));
    }
}
