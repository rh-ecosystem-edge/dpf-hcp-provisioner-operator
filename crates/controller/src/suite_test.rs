//! Port of `internal/controller/suite_test.go` (envtest bootstrap) plus Gomega-like helpers.
//!
//! Go runs every spec of the package serially against one envtest control plane and one
//! controller manager (`k8sManager`) started in `BeforeSuite`. Here the envtest-backed specs
//! are `#[test]`s that call [`run`]: the first one starts the shared environment and manager
//! (exactly as `BeforeSuite` configures them) on a process-wide tokio runtime, and a global
//! lock serializes the specs like Ginkgo does (the Go specs share fixed names in the
//! `default`/`clusters` namespaces and clean up cluster-wide in `AfterEach`). `AfterEach`
//! blocks run even when the spec body fails, like Ginkgo.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{Client, ManagerHandle, Recorder};
use ctrlrt_envtest::Environment;
use ctrlrt_runtime::{Manager, ManagerOptions, RestConfig};
use futures::FutureExt;
use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use tokio_util::sync::CancellationToken;

use crate::DPFHCPProvisionerReconciler;

/// `fakeImageChecker` implements `bfocplookup.ImageChecker` for testing
/// to avoid hitting real container registries in unit/integration tests.
pub(crate) struct FakeImageChecker;

#[async_trait::async_trait]
impl ctrl_bfocplookup::ImageChecker for FakeImageChecker {
    async fn check_tag(
        &self,
        _ctx: &ctrlrt::Ctx,
        _reference: &registry::name::Reference,
        _keychain: Arc<dyn registry::authn::Keychain>,
    ) -> ctrlrt::Result<()> {
        Ok(())
    }
}

/// The suite globals of `suite_test.go` (`testEnv`, `k8sClient`, `k8sManager`, `cancel`).
pub(crate) struct Suite {
    #[allow(dead_code)]
    pub test_env: Arc<Environment>,
    /// `k8sClient` — a direct (uncached) client.
    pub k8s_client: Client,
    #[allow(dead_code)]
    pub k8s_manager: Manager,
    #[allow(dead_code)]
    pub cancel: CancellationToken,
}

static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("tokio runtime")
});

static SUITE: tokio::sync::OnceCell<Suite> = tokio::sync::OnceCell::const_new();
static SERIAL: Mutex<()> = Mutex::new(());

/// `BeforeSuite`
async fn before_suite() -> Suite {
    // logf.SetLogger(zap.New(zap.WriteTo(GinkgoWriter), zap.UseDevMode(true))) — Rust tests
    // keep the default (discarding) sink unless CONTROLLER_TEST_LOG is set.
    if std::env::var_os("CONTROLLER_TEST_LOG").is_some() {
        ctrlrt::log::set_sink(Arc::new(ctrlrt::log::StderrSink { max_level: 1 }));
    }

    let cancel = CancellationToken::new();

    // By("bootstrapping test environment")
    let test_env = ctrlrt_envtest::shared(|| {
        Environment::builder()
            .crd_directory_paths(["config/crd/bases", "config/crd/external"])
            .error_if_crd_path_missing(true)
            // Retrieve the first found binary directory to allow running tests from IDEs
            .binary_assets_directory(None)
    })
    .await
    .expect("start envtest");

    let cfg = RestConfig::new(test_env.kube_config());
    let (k8s_client, _) = ctrlrt_runtime::client_from_rest_config(&cfg).expect("create k8sClient");

    // By("creating controller manager")
    let k8s_manager = Manager::new(cfg, ManagerOptions::default())
        .await
        .expect("create manager");

    // By("creating clusters namespace")
    for ns in ["clusters", "dpf-operator-system"] {
        let mut namespace = Namespace {
            metadata: ObjectMeta {
                name: Some(ns.into()),
                ..Default::default()
            },
            ..Default::default()
        };
        k8s_client
            .create(&mut namespace)
            .await
            .unwrap_or_else(|e| panic!("create namespace {ns}: {e}"));
    }

    // By("creating DPFHCPProvisionerConfig CR")
    let mut config_cr = provisioningv1alpha1::DPFHCPProvisionerConfig {
        metadata: ObjectMeta {
            name: Some(provisioningv1alpha1::DEFAULT_CONFIG_NAME.into()),
            ..Default::default()
        },
        ..Default::default()
    };
    k8s_client
        .create(&mut config_cr)
        .await
        .expect("create DPFHCPProvisionerConfig");

    // By("setting up DPFHCPProvisioner controller")
    let mgr = &k8s_manager;
    let rec = |name: &str| -> Recorder { mgr.event_recorder_for(name) };
    let provisioner_recorder = rec(common::PROVISIONER_CONTROLLER_NAME);
    let kubeconfig_injector = ctrl_kubeconfiginjection::KubeconfigInjector::new(
        mgr.client(),
        provisioner_recorder.clone(),
    );

    // Initialize Finalizer Manager with pluggable cleanup handlers
    let mut finalizer_manager =
        ctrl_finalizer::Manager::new(mgr.client(), provisioner_recorder.clone());
    // Register cleanup handlers in order (dependent resources first)
    finalizer_manager.register_handler(ctrl_kubeconfiginjection::CleanupHandler::new(
        mgr.client(),
        provisioner_recorder.clone(),
    ));
    finalizer_manager.register_handler(ctrl_hostedcluster::CleanupHandler::new(
        mgr.client(),
        provisioner_recorder.clone(),
    ));

    let mut lookup =
        ctrl_bfocplookup::ImageLookup::new(mgr.client(), rec("bluefield-ocp-layer-lookup"));
    lookup.image_checker = Arc::new(FakeImageChecker);

    let reconciler = Arc::new(DPFHCPProvisionerReconciler {
        client: mgr.client(),
        recorder: provisioner_recorder.clone(),
        image_lookup: Some(Arc::new(lookup)),
        dpu_cluster_validator: Some(Arc::new(ctrl_dpucluster::Validator::new(
            mgr.client(),
            rec("dpucluster-validator"),
        ))),
        secrets_validator: Some(Arc::new(ctrl_secrets::Validator::new(
            mgr.client(),
            rec("secrets-validator"),
        ))),
        secret_manager: Some(Arc::new(ctrl_hostedcluster::SecretManager::new(
            mgr.client(),
        ))),
        node_pool_manager: Some(Arc::new(ctrl_hostedcluster::NodePoolManager::new(
            mgr.client(),
        ))),
        hosted_cluster_manager: Some(Arc::new(ctrl_hostedcluster::HostedClusterManager::new(
            mgr.client(),
        ))),
        finalizer_manager: Some(Arc::new(finalizer_manager)),
        status_syncer: Some(Arc::new(ctrl_hostedcluster::StatusSyncer::new(
            mgr.client(),
        ))),
        kubeconfig_injector: Some(Arc::new(kubeconfig_injector)),
        ..Default::default()
    });
    reconciler
        .setup_with_manager(mgr)
        .expect("SetupWithManager");

    // By("starting controller manager")
    let m = k8s_manager.clone();
    let c = cancel.clone();
    tokio::spawn(async move {
        if let Err(err) = m.start(c).await {
            panic!("manager exited: {err}");
        }
    });

    Suite {
        test_env,
        k8s_client,
        k8s_manager,
        cancel,
    }
}

/// Run one envtest-backed spec (serialized, on the suite runtime).
pub(crate) fn run<F, Fut>(f: F)
where
    F: FnOnce(&'static Suite) -> Fut,
    Fut: Future<Output = ()>,
{
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    RUNTIME.block_on(async move {
        let suite = SUITE.get_or_init(before_suite).await;
        f(suite).await;
    });
}

/// Ginkgo `BeforeEach` / `It` / `AfterEach`: `after` runs even if `body` panics.
pub(crate) async fn spec<B, A>(body: B, after: A)
where
    B: Future<Output = ()>,
    A: Future<Output = ()>,
{
    let res = AssertUnwindSafe(body).catch_unwind().await;
    after.await;
    if let Err(panic) = res {
        std::panic::resume_unwind(panic);
    }
}

/// Gomega `Eventually(f, timeout, interval).Should(...)`: poll `f` until it returns `Ok`.
pub(crate) async fn eventually<T, F>(timeout: Duration, interval: Duration, mut f: F) -> T
where
    F: AsyncFnMut() -> Result<T, String>,
{
    let start = Instant::now();
    loop {
        match f().await {
            Ok(v) => return v,
            Err(msg) => {
                if start.elapsed() >= timeout {
                    panic!("Timed out after {timeout:?}: {msg}");
                }
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Gomega `Consistently(f, duration, interval).Should(...)`: `f` must return `Ok` on every
/// poll for `duration`.
pub(crate) async fn consistently<F>(duration: Duration, interval: Duration, mut f: F)
where
    F: AsyncFnMut() -> Result<(), String>,
{
    let start = Instant::now();
    loop {
        if let Err(msg) = f().await {
            panic!("Failed after {:?}: {msg}", start.elapsed());
        }
        if start.elapsed() >= duration {
            return;
        }
        tokio::time::sleep(interval).await;
    }
}

/// `Expect(actual).To(Equal(expected))` as a poll result.
pub(crate) fn equal<T: PartialEq + std::fmt::Debug>(actual: T, expected: T) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("Expected {actual:?} to equal {expected:?}"))
    }
}

/// `Expect(cond).To(BeTrue())` as a poll result.
pub(crate) fn be_true(cond: bool, what: &str) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(format!("Expected {what} to be true"))
    }
}

pub(crate) fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

pub(crate) fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

// ---------------------------------------------------------------------------------------------
// Fixture helpers shared by the spec files
// ---------------------------------------------------------------------------------------------

/// `metav1.Condition{Type, Status, Reason, Message, LastTransitionTime: metav1.Now()}`
pub(crate) fn cond_now(
    type_: &str,
    status: &str,
    reason: &str,
    message: &str,
) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition {
    let mut c = ctrlrt::meta::condition(type_, status, reason, message);
    c.last_transition_time = ctrlrt::meta::now();
    c
}

/// `metav1.Condition{Type, Status, Reason, ObservedGeneration}` (zero LastTransitionTime).
pub(crate) fn cond_gen(
    type_: &str,
    status: &str,
    reason: &str,
    observed_generation: i64,
) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition {
    ctrlrt::meta::condition_gen(type_, status, reason, "", observed_generation)
}

/// `ObjectMeta{Name, Namespace}`
pub(crate) fn object_meta(name: &str, namespace: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.into()),
        namespace: Some(namespace.into()).filter(|s: &String| !s.is_empty()),
        ..Default::default()
    }
}

/// `provisioningv1alpha1.DPUClusterReference{Name, Namespace}`
pub(crate) fn dpu_cluster_ref(
    name: &str,
    namespace: &str,
) -> provisioningv1alpha1::DPUClusterReference {
    provisioningv1alpha1::DPUClusterReference {
        name: name.into(),
        namespace: namespace.into(),
        ..Default::default()
    }
}

/// `&provisioningv1alpha1.DPUDeploymentReference{Name, Namespace}`
pub(crate) fn dpu_deployment_ref(
    name: &str,
    namespace: &str,
) -> Option<provisioningv1alpha1::DPUDeploymentReference> {
    Some(provisioningv1alpha1::DPUDeploymentReference {
        name: name.into(),
        namespace: namespace.into(),
        ..Default::default()
    })
}

/// `corev1.LocalObjectReference{Name}`
pub(crate) fn local_ref(name: &str) -> apis::k8s::core::v1::LocalObjectReference {
    apis::k8s::core::v1::LocalObjectReference {
        name: name.into(),
        ..Default::default()
    }
}

/// `&corev1.ObjectReference{Name, Namespace}`
pub(crate) fn object_ref(
    name: &str,
    namespace: &str,
) -> Option<apis::k8s::core::v1::ObjectReference> {
    Some(apis::k8s::core::v1::ObjectReference {
        name: name.into(),
        namespace: namespace.into(),
        ..Default::default()
    })
}

/// `meta.FindStatusCondition(cr.Status.Conditions, t)`
pub(crate) fn find_cond<'a>(
    cr: &'a provisioningv1alpha1::DPFHCPProvisioner,
    t: &str,
) -> Option<&'a k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition> {
    ctrlrt::meta::find_status_condition(&cr.status.conditions, t)
}

/// `k8sClient.Get(ctx, types.NamespacedName{Name, Namespace}, &obj)` with the error rendered.
pub(crate) async fn get<K: ctrlrt::Object>(
    c: &Client,
    name: &str,
    namespace: &str,
) -> Result<K, String> {
    c.get::<K>(&ctrlrt::ObjectKey::new(namespace, name))
        .await
        .map_err(|e| e.to_string())
}

/// `_ = k8sClient.Delete(ctx, &K{ObjectMeta{Name, Namespace}})`
pub(crate) async fn delete_ignore<K: ctrlrt::Object + Default>(
    c: &Client,
    name: &str,
    namespace: &str,
) {
    let mut o = K::default();
    *o.object_meta_mut() = object_meta(name, namespace);
    let _ = c.delete(&o).await;
}

/// `k8sClient.Create(ctx, &corev1.Namespace{Name})`
pub(crate) async fn create_namespace(c: &Client, name: &str) -> ctrlrt::Result<()> {
    let mut ns = Namespace {
        metadata: object_meta(name, ""),
        ..Default::default()
    };
    c.create(&mut ns).await
}

/// The pull secret / SSH key secrets created by the envtest specs' `BeforeEach`.
pub(crate) fn test_pull_secret(name: &str, namespace: &str) -> k8s_openapi::api::core::v1::Secret {
    use base64::Engine as _;
    // Generate auth at runtime to avoid security scanner false positives
    let test_auth = base64::engine::general_purpose::STANDARD.encode(b"test:test");
    let pull_secret_data = format!(r#"{{"auths":{{"quay.io":{{"auth":"{test_auth}"}}}}}}"#);
    k8s_openapi::api::core::v1::Secret {
        metadata: object_meta(name, namespace),
        type_: Some("kubernetes.io/dockerconfigjson".into()),
        data: Some(
            [(
                ".dockerconfigjson".to_string(),
                k8s_openapi::ByteString(pull_secret_data.into_bytes()),
            )]
            .into(),
        ),
        ..Default::default()
    }
}

/// SSH key secret - must contain "id_rsa.pub" key as expected by validator
pub(crate) fn test_ssh_key(name: &str, namespace: &str) -> k8s_openapi::api::core::v1::Secret {
    k8s_openapi::api::core::v1::Secret {
        metadata: object_meta(name, namespace),
        type_: Some("Opaque".into()),
        data: Some(
            [(
                "id_rsa.pub".to_string(),
                k8s_openapi::ByteString(b"ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ...".to_vec()),
            )]
            .into(),
        ),
        ..Default::default()
    }
}
