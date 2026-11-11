//! Integration tests: real etcd + kube-apiserver from `bin/k8s/<version>/`.

use std::path::PathBuf;

use k8s_openapi::api::core::v1::Namespace;
use kube::api::{Api, ApiResource, DynamicObject, GroupVersionKind, PostParams};

fn repo_crd_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("config").join("crd").join("bases"),
        PathBuf::from("internal")
            .join("controller")
            .join("testdata")
            .join("crds"),
    ]
}

fn provisioner_config_api(client: kube::Client) -> Api<DynamicObject> {
    let gvk = GroupVersionKind::gvk(
        "provisioning.dpu.hcp.io",
        "v1alpha1",
        "DPFHCPProvisionerConfig",
    );
    let ar = ApiResource::from_gvk_with_plural(&gvk, "dpfhcpprovisionerconfigs");
    Api::all_with(client, &ar)
}

fn provisioner_config(name: &str) -> DynamicObject {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "provisioning.dpu.hcp.io/v1alpha1",
        "kind": "DPFHCPProvisionerConfig",
        "metadata": {"name": name},
        "spec": {}
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installs_repo_crds_and_enforces_cel_validation() {
    let env = ctrlrt_envtest::Environment::builder()
        .crd_directory_paths(repo_crd_paths())
        .error_if_crd_path_missing(true)
        .binary_assets_directory(None)
        .start()
        .await
        .unwrap_or_else(|e| panic!("start: {e}"));

    // The CRDs from both directories were installed and are served.
    let names: Vec<String> = env
        .crds()
        .iter()
        .filter_map(|c| c.metadata.name.clone())
        .collect();
    assert!(names.contains(&"dpfhcpprovisionerconfigs.provisioning.dpu.hcp.io".to_string()));
    assert!(names.contains(&"dpfhcpprovisioners.provisioning.dpu.hcp.io".to_string()));
    assert!(names.contains(&"hostedclusters.hypershift.openshift.io".to_string()));
    assert!(names.contains(&"dpuclusters.provisioning.dpu.nvidia.com".to_string()));

    let client = env.kube_client();

    // Namespaces (like the Go suite's BeforeSuite).
    let ns_api: Api<Namespace> = Api::all(client.clone());
    for ns in ["clusters", "dpf-operator-system"] {
        let obj: Namespace = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1", "kind": "Namespace", "metadata": {"name": ns}
        }))
        .unwrap();
        ns_api.create(&PostParams::default(), &obj).await.unwrap();
    }
    assert!(ns_api.get("clusters").await.is_ok());

    // DPFHCPProvisionerConfig "default" is accepted.
    let api = provisioner_config_api(client.clone());
    let created = api
        .create(&PostParams::default(), &provisioner_config("default"))
        .await
        .unwrap();
    assert_eq!(created.metadata.name.as_deref(), Some("default"));
    assert!(created.metadata.uid.is_some());

    // Any other name is rejected by the CRD's x-kubernetes-validations rule.
    let err = api
        .create(&PostParams::default(), &provisioner_config("not-default"))
        .await
        .unwrap_err();
    match err {
        kube::Error::Api(status) => {
            assert_eq!(status.code, 422, "{status:?}");
            assert_eq!(status.reason, "Invalid", "{status:?}");
            assert!(
                status
                    .message
                    .contains("DPFHCPProvisionerConfig must be named 'default'"),
                "{}",
                status.message
            );
        }
        other => panic!("unexpected error: {other}"),
    }

    // The kubeconfig is usable on its own.
    let kc = kube::config::Kubeconfig::from_yaml(&env.kubeconfig_yaml()).unwrap();
    assert_eq!(kc.current_context.as_deref(), Some("envtest"));
    let cfg = kube::Config::from_custom_kubeconfig(kc, &Default::default())
        .await
        .unwrap();
    let c2 = kube::Client::try_from(cfg).unwrap();
    let ver = c2.apiserver_version().await.unwrap();
    assert_eq!(ver.major, "1");

    // Additional users authenticate via client certificates; RBAC applies to them.
    let user = env
        .add_user(ctrlrt_envtest::User {
            name: "bob".into(),
            groups: vec![],
        })
        .unwrap();
    let bob = kube::Client::try_from(user.kube_config().await.unwrap()).unwrap();
    let err = Api::<Namespace>::all(bob).get("default").await.unwrap_err();
    match err {
        kube::Error::Api(status) => assert_eq!(status.code, 403, "{status:?}"),
        other => panic!("unexpected error: {other}"),
    }

    let cert_dir = env.cert_dir().to_path_buf();
    assert!(cert_dir.join("apiserver.crt").exists());
    assert!(cert_dir.join("client-cert-auth-ca.crt").exists());
    assert!(env.default_kubeconfig_path().exists());
    env.stop().await.unwrap();
    assert!(!cert_dir.exists(), "cert dir must be removed on stop");
    // Idempotent.
    env.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_environments_and_drop_cleanup() {
    let (a, b) = tokio::join!(
        ctrlrt_envtest::Environment::builder().start(),
        ctrlrt_envtest::Environment::builder().start()
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.api_server_url(), b.api_server_url());
    assert_ne!(a.etcd_url(), b.etcd_url());
    assert_ne!(a.cert_dir(), b.cert_dir());

    // Objects are isolated between environments.
    let ca: Api<Namespace> = Api::all(a.kube_client());
    let cb: Api<Namespace> = Api::all(b.kube_client());
    let obj: Namespace = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "only-in-a"}
    }))
    .unwrap();
    ca.create(&PostParams::default(), &obj).await.unwrap();
    assert!(cb.get("only-in-a").await.is_err());

    // Drop kills the processes and removes the directories.
    let dir_b = b.cert_dir().to_path_buf();
    drop(b);
    assert!(!dir_b.exists(), "cert dir must be removed on drop");
    let err = cb.get("default").await;
    assert!(err.is_err(), "API server must be gone after drop");

    a.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_crd_path_is_an_error() {
    let err = ctrlrt_envtest::Environment::builder()
        .crd_directory_paths(vec![PathBuf::from("does/not/exist")])
        .error_if_crd_path_missing(true)
        .start()
        .await
        .unwrap_err();
    let expected = format!(
        "unable to install CRDs onto control plane: unable to read CRD files: stat {}: no such file or directory",
        ctrlrt_envtest::workspace_root()
            .join("does/not/exist")
            .display()
    );
    assert_eq!(err.to_string(), expected);
}

async fn shared_env() -> std::sync::Arc<ctrlrt_envtest::Environment> {
    ctrlrt_envtest::shared(|| {
        ctrlrt_envtest::Environment::builder()
            .crd_directory_paths(repo_crd_paths())
            .error_if_crd_path_missing(true)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn shared_environment_first_user() {
    let env = shared_env().await;
    let api = Api::<Namespace>::all(env.kube_client());
    api.get("default").await.unwrap();
    let again = shared_env().await;
    assert_eq!(env.api_server_url(), again.api_server_url());
}

#[tokio::test]
async fn shared_environment_second_user() {
    let env = shared_env().await;
    let api = provisioner_config_api(env.kube_client());
    // Works from a different runtime than the one that started the environment.
    let _ = api.list(&Default::default()).await.unwrap();
}
