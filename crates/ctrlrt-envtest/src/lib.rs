//! Rust port of `sigs.k8s.io/controller-runtime/pkg/envtest`.
//!
//! Starts a local etcd + kube-apiserver control plane (same flags, certificates and
//! startup checks as Go's envtest), provisions an `admin` user in `system:masters`
//! authenticated by a generated client certificate, and installs CRDs from directories.
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use std::path::PathBuf;
//! let env = ctrlrt_envtest::Environment::builder()
//!     .crd_directory_paths(vec![
//!         PathBuf::from("config/crd/bases"),
//!         PathBuf::from("config/crd/external"),
//!     ])
//!     .error_if_crd_path_missing(true)
//!     .start()
//!     .await?;
//! let client = env.kube_client();
//! # let _ = client;
//! env.stop().await?;
//! # Ok(()) }
//! ```
//!
//! # Binaries
//!
//! Located like Go's `process.BinPathFinder`: `TEST_ASSET_KUBE_APISERVER` /
//! `TEST_ASSET_ETCD` / `TEST_ASSET_KUBECTL`, then `$KUBEBUILDER_ASSETS/<name>`, then
//! `<binary_assets_directory>/<name>` — which defaults to the first directory (sorted by
//! name) under `<workspace root>/bin/k8s/` (what `getFirstFoundEnvTestBinaryDir()` does in
//! the Go suites) — and finally `/usr/local/kubebuilder/bin/<name>`.
//!
//! # Cleanup
//!
//! * [`Environment::stop`] stops the API server then etcd (SIGTERM, SIGKILL after the stop
//!   timeout) and removes the temporary directories, like Go.
//! * Dropping an [`Environment`] SIGKILLs the processes and removes the directories.
//! * Rust never drops statics and libtest ends with `process::exit`, so a shared
//!   environment (see [`shared`]) — or any environment leaked by a panic/abort/SIGKILL/
//!   Ctrl-C — is cleaned up by a per-environment *watchdog*: a `/bin/sh` process in its own
//!   process group that blocks on a pipe only this process holds. When the test process
//!   dies the pipe closes and the watchdog terminates the control plane processes and
//!   deletes their directories. No `unsafe`/`pre_exec`/`PR_SET_PDEATHSIG` is involved.

mod addr;
mod certs;
mod controlplane;
mod crd;
mod duration;
mod process;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::Api;

pub use controlplane::{RestConfig, User, kubeconfig_from_rest};
pub use crd::{
    CrdInstallOptions, create_crds, install_crds, read_crd_files, uninstall_crds, wait_for_crds,
};
pub use duration::parse_go_duration;
pub use process::{Arguments, bin_path_finder};

use controlplane::{ControlPlane, PlaneOptions};

/// `USE_EXISTING_CLUSTER` is not supported by this port (always starts a local control plane).
const ENV_START_TIMEOUT: &str = "KUBEBUILDER_CONTROLPLANE_START_TIMEOUT";
const ENV_STOP_TIMEOUT: &str = "KUBEBUILDER_CONTROLPLANE_STOP_TIMEOUT";
const ENV_ATTACH_OUTPUT: &str = "KUBEBUILDER_ATTACH_CONTROL_PLANE_OUTPUT";
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(20);

/// `logf.RuntimeLog.WithName("test-env")`
pub(crate) fn log() -> ctrlrt::Logger {
    ctrlrt::Logger::root()
        .with_name("controller-runtime")
        .with_name("test-env")
}

/// Converts a kube error into the ctrlrt Go-compatible error model (`*apierrors.StatusError`
/// for API errors) so `ctrlrt::error::is_not_found` & co. work.
pub(crate) fn kube_error(err: kube::Error) -> anyhow::Error {
    match err {
        kube::Error::Api(status) => {
            let st = match status.status {
                Some(kube::core::response::StatusSummary::Success) => "Success",
                Some(kube::core::response::StatusSummary::Failure) => "Failure",
                None => "",
            };
            anyhow::Error::new(ctrlrt::ApiError {
                status: st.to_string(),
                message: status.message.clone(),
                reason: status.reason.clone(),
                details: None,
                code: status.code,
            })
        }
        other => anyhow::Error::new(other),
    }
}

/// The root of the cargo workspace (the repository root).
///
/// Found by walking up from `$CARGO_MANIFEST_DIR` (set by `cargo test` for the crate under
/// test) or the current directory to the first `Cargo.toml` declaring `[workspace]`; falls
/// back to the location this crate was compiled from.
pub fn workspace_root() -> PathBuf {
    let starts = [
        std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from),
        std::env::current_dir().ok(),
    ];
    for start in starts.into_iter().flatten() {
        let mut dir: Option<&Path> = Some(start.as_path());
        while let Some(d) = dir {
            if let Ok(s) = std::fs::read_to_string(d.join("Cargo.toml"))
                && s.lines().any(|l| l.trim() == "[workspace]")
            {
                return d.to_path_buf();
            }
            dir = d.parent();
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The first directory (sorted by name) in `<workspace root>/bin/k8s`, if any
/// (`getFirstFoundEnvTestBinaryDir()` of the Go suites).
pub fn default_binary_assets_directory() -> Option<PathBuf> {
    let base = workspace_root().join("bin").join("k8s");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

fn resolve_path(p: PathBuf) -> PathBuf {
    if p.is_relative() {
        workspace_root().join(p)
    } else {
        p
    }
}

/// Builder for [`Environment`] (the fields of Go's `envtest.Environment`).
#[derive(Clone, Debug, Default)]
pub struct EnvironmentBuilder {
    crd_directory_paths: Vec<PathBuf>,
    crds: Vec<CustomResourceDefinition>,
    error_if_crd_path_missing: bool,
    binary_assets_directory: Option<PathBuf>,
    control_plane_start_timeout: Option<Duration>,
    control_plane_stop_timeout: Option<Duration>,
    attach_control_plane_output: bool,
    crd_install_max_time: Option<Duration>,
    crd_install_poll_interval: Option<Duration>,
    etcd_args: Arguments,
    api_server_args: Arguments,
}

impl EnvironmentBuilder {
    /// `CRDDirectoryPaths` — directories (or files) to read CRDs from. Relative paths are
    /// resolved against [`workspace_root`].
    pub fn crd_directory_paths<P: Into<PathBuf>>(
        mut self,
        paths: impl IntoIterator<Item = P>,
    ) -> Self {
        self.crd_directory_paths = paths.into_iter().map(Into::into).collect();
        self
    }

    /// `CRDs` — CRD objects to install in addition to those read from the directories.
    pub fn crds(mut self, crds: Vec<CustomResourceDefinition>) -> Self {
        self.crds = crds;
        self
    }

    /// `ErrorIfCRDPathMissing`
    pub fn error_if_crd_path_missing(mut self, v: bool) -> Self {
        self.error_if_crd_path_missing = v;
        self
    }

    /// `BinaryAssetsDirectory`. `None` = [`default_binary_assets_directory`].
    /// `KUBEBUILDER_ASSETS` / `TEST_ASSET_*` still take precedence, like in Go.
    pub fn binary_assets_directory(mut self, dir: Option<PathBuf>) -> Self {
        self.binary_assets_directory = dir;
        self
    }

    /// `ControlPlaneStartTimeout` (default: `KUBEBUILDER_CONTROLPLANE_START_TIMEOUT` or 20s).
    pub fn control_plane_start_timeout(mut self, d: Duration) -> Self {
        self.control_plane_start_timeout = Some(d);
        self
    }

    /// `ControlPlaneStopTimeout` (default: `KUBEBUILDER_CONTROLPLANE_STOP_TIMEOUT` or 20s).
    pub fn control_plane_stop_timeout(mut self, d: Duration) -> Self {
        self.control_plane_stop_timeout = Some(d);
        self
    }

    /// `AttachControlPlaneOutput` (also enabled by `KUBEBUILDER_ATTACH_CONTROL_PLANE_OUTPUT=true`).
    pub fn attach_control_plane_output(mut self, v: bool) -> Self {
        self.attach_control_plane_output = v;
        self
    }

    /// `CRDInstallOptions.MaxTime` (default 10s).
    pub fn crd_install_max_time(mut self, d: Duration) -> Self {
        self.crd_install_max_time = Some(d);
        self
    }

    /// `CRDInstallOptions.PollInterval` (default 100ms).
    pub fn crd_install_poll_interval(mut self, d: Duration) -> Self {
        self.crd_install_poll_interval = Some(d);
        self
    }

    /// `ControlPlane.GetAPIServer().Configure()` — extra/overridden kube-apiserver flags.
    pub fn configure_api_server(mut self, f: impl FnOnce(&mut Arguments)) -> Self {
        f(&mut self.api_server_args);
        self
    }

    /// `ControlPlane.Etcd.Configure()` — extra/overridden etcd flags.
    pub fn configure_etcd(mut self, f: impl FnOnce(&mut Arguments)) -> Self {
        f(&mut self.etcd_args);
        self
    }

    /// `Environment.Start()`
    pub async fn start(self) -> anyhow::Result<Environment> {
        Environment::start(self).await
    }
}

/// An authenticated user of the test control plane (`envtest.AuthenticatedUser`).
#[derive(Clone, Debug)]
pub struct AuthenticatedUser {
    rest: RestConfig,
}

impl AuthenticatedUser {
    /// Raw connection details (host, CA, client certificate and key).
    pub fn rest_config(&self) -> &RestConfig {
        &self.rest
    }

    /// `KubeConfig()` — kubeconfig YAML for this user.
    pub fn kubeconfig_yaml(&self) -> anyhow::Result<String> {
        kubeconfig_from_rest(&self.rest)
    }

    /// `Config()` as a [`kube::Config`].
    pub async fn kube_config(&self) -> anyhow::Result<kube::Config> {
        kube_config_from_yaml(&self.kubeconfig_yaml()?).await
    }
}

async fn kube_config_from_yaml(yaml: &str) -> anyhow::Result<kube::Config> {
    let kc = kube::config::Kubeconfig::from_yaml(yaml)?;
    Ok(
        kube::Config::from_custom_kubeconfig(kc, &kube::config::KubeConfigOptions::default())
            .await?,
    )
}

/// A running test control plane (`envtest.Environment` after `Start()`).
pub struct Environment {
    config: kube::Config,
    kubeconfig: String,
    rest: RestConfig,
    crds: Vec<CustomResourceDefinition>,
    etcd_url: String,
    cert_dir: PathBuf,
    kubectl_path: PathBuf,
    default_kubeconfig_path: PathBuf,
    plane: Mutex<Option<ControlPlane>>,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("api_server", &self.rest.host)
            .field("etcd", &self.etcd_url)
            .field("cert_dir", &self.cert_dir)
            .finish()
    }
}

fn default_timeout(
    explicit: Option<Duration>,
    env: &str,
    default: Duration,
) -> anyhow::Result<Duration> {
    if let Some(d) = explicit
        && !d.is_zero()
    {
        return Ok(d);
    }
    match std::env::var(env) {
        Ok(v) if !v.is_empty() => parse_go_duration(&v),
        _ => Ok(default),
    }
}

impl Environment {
    /// A builder with Go's zero-value defaults.
    pub fn builder() -> EnvironmentBuilder {
        EnvironmentBuilder::default()
    }

    async fn start(b: EnvironmentBuilder) -> anyhow::Result<Environment> {
        let attach = b.attach_control_plane_output
            || std::env::var(ENV_ATTACH_OUTPUT).is_ok_and(|v| v == "true");

        // configureBinaryPaths
        let assets_dir = b
            .binary_assets_directory
            .clone()
            .or_else(default_binary_assets_directory);
        let assets_dir = assets_dir.map(resolve_path);
        let mut opts = PlaneOptions {
            apiserver_path: bin_path_finder("kube-apiserver", assets_dir.as_deref()),
            etcd_path: bin_path_finder("etcd", assets_dir.as_deref()),
            kubectl_path: bin_path_finder("kubectl", assets_dir.as_deref()),
            attach_output: attach,
            etcd_args: b.etcd_args.clone(),
            apiserver_args: b.api_server_args.clone(),
            ..Default::default()
        };

        // defaultTimeouts
        let timeouts = (|| -> anyhow::Result<(Duration, Duration)> {
            Ok((
                default_timeout(
                    b.control_plane_start_timeout,
                    ENV_START_TIMEOUT,
                    DEFAULT_START_TIMEOUT,
                )?,
                default_timeout(
                    b.control_plane_stop_timeout,
                    ENV_STOP_TIMEOUT,
                    DEFAULT_STOP_TIMEOUT,
                )?,
            ))
        })()
        .map_err(|err| ctrlrt::wrapf!(err, "failed to default controlplane timeouts: {}", err))?;
        opts.start_timeout = timeouts.0;
        opts.stop_timeout = timeouts.1;

        log().v(1).info("starting control plane", ctrlrt::kv![]);
        let mut plane = start_control_plane(&opts)
            .await
            .map_err(|err| ctrlrt::wrapf!(err, "unable to start control plane itself: {}", err))?;

        let admin = plane
            .add_user(&User {
                name: "admin".into(),
                groups: vec!["system:masters".into()],
            })
            .map_err(|err| ctrlrt::wrapf!(err, "unable to provision admin user: {}", err))?;

        let kubeconfig = kubeconfig_from_rest(&admin)
            .map_err(|err| ctrlrt::wrapf!(err, "unable to set KubeConfig field: {}", err))?;
        let config = kube_config_from_yaml(&kubeconfig)
            .await
            .map_err(|err| ctrlrt::wrapf!(err, "unable to create client configuration: {}", err))?;
        let client = kube::Client::try_from(config.clone())
            .map_err(|err| ctrlrt::wrapf!(err, "unable to create client: {}", err))?;

        wait_for_default_namespace(&client).await.map_err(|err| {
            ctrlrt::wrapf!(
                err,
                "default namespace didn't register within deadline: {}",
                err
            )
        })?;

        log().v(1).info("installing CRDs", ctrlrt::kv![]);
        let crd_opts = CrdInstallOptions {
            paths: b
                .crd_directory_paths
                .iter()
                .cloned()
                .map(resolve_path)
                .collect(),
            crds: b.crds.clone(),
            error_if_path_missing: b.error_if_crd_path_missing,
            max_time: b.crd_install_max_time.unwrap_or_default(),
            poll_interval: b.crd_install_poll_interval.unwrap_or_default(),
        };
        let crds = install_crds(&client, crd_opts).await.map_err(|err| {
            ctrlrt::wrapf!(err, "unable to install CRDs onto control plane: {}", err)
        })?;

        Ok(Environment {
            config,
            kubeconfig,
            rest: admin,
            crds,
            etcd_url: plane.etcd_url.clone(),
            cert_dir: plane.cert_dir.clone(),
            kubectl_path: plane.kubectl_path.clone(),
            default_kubeconfig_path: plane.default_kubeconfig_path.clone(),
            plane: Mutex::new(Some(plane)),
        })
    }

    /// The admin (`system:masters`) client configuration (`Environment.Config`).
    pub fn kube_config(&self) -> kube::Config {
        self.config.clone()
    }

    /// A new client for the admin user.
    ///
    /// Must be called inside a Tokio runtime; a `kube::Client` is bound to the runtime it
    /// was created in, so create one per test (runtime) instead of caching it in a static.
    ///
    /// # Panics
    /// If the client cannot be constructed (outside a Tokio runtime); the same
    /// configuration was already used successfully during [`EnvironmentBuilder::start`].
    pub fn kube_client(&self) -> kube::Client {
        #[allow(clippy::expect_used)]
        self.try_kube_client()
            .expect("unable to create a kube client for the envtest environment")
    }

    /// Fallible variant of [`Environment::kube_client`].
    pub fn try_kube_client(&self) -> anyhow::Result<kube::Client> {
        Ok(kube::Client::try_from(self.config.clone())?)
    }

    /// The admin kubeconfig (`Environment.KubeConfig`), as written by `clientcmd.Write`.
    pub fn kubeconfig_yaml(&self) -> String {
        self.kubeconfig.clone()
    }

    /// Raw admin connection details (host, CA, client certificate and key).
    pub fn rest_config(&self) -> &RestConfig {
        &self.rest
    }

    /// The API server URL, e.g. `https://127.0.0.1:41235/`.
    pub fn api_server_url(&self) -> &str {
        &self.rest.host
    }

    /// The etcd client URL, e.g. `http://127.0.0.1:41233`.
    pub fn etcd_url(&self) -> &str {
        &self.etcd_url
    }

    /// The API server certificate directory.
    pub fn cert_dir(&self) -> &Path {
        &self.cert_dir
    }

    /// The kubectl binary path (`ControlPlane.KubectlPath`).
    pub fn kubectl_path(&self) -> &Path {
        &self.kubectl_path
    }

    /// kubeconfig file of the default (legacy) `system:masters` user, as used by
    /// `ControlPlane.KubeCtl()` (`--kubeconfig=<this>`).
    pub fn default_kubeconfig_path(&self) -> &Path {
        &self.default_kubeconfig_path
    }

    /// The CRDs installed during start (`Environment.CRDs`), as returned by the API server.
    pub fn crds(&self) -> &[CustomResourceDefinition] {
        &self.crds
    }

    /// `Environment.AddUser(user, baseConfig)` — provisions a client certificate for a
    /// user (CN = name, O = groups).
    pub fn add_user(&self, user: User) -> anyhow::Result<AuthenticatedUser> {
        let mut guard = self
            .plane
            .lock()
            .map_err(|_| anyhow::anyhow!("envtest control plane state poisoned"))?;
        let plane = guard.as_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "no API server authentication is configured yet.  The API server defaults one when Start is called, did you mean to use that?"
            )
        })?;
        Ok(AuthenticatedUser {
            rest: plane.add_user(&user)?,
        })
    }

    /// `Environment.Stop()` — stops the API server and etcd and removes their temporary
    /// directories. Idempotent; also happens (synchronously, with SIGKILL) on drop.
    pub async fn stop(&self) -> anyhow::Result<()> {
        let plane = match self.plane.lock() {
            Ok(mut g) => g.take(),
            Err(p) => p.into_inner().take(),
        };
        match plane {
            Some(mut plane) => plane.stop().await,
            None => Ok(()),
        }
    }
}

/// `Environment.startControlPlane()` — up to 5 attempts.
async fn start_control_plane(opts: &PlaneOptions) -> anyhow::Result<ControlPlane> {
    let max_retries = 5;
    let mut last_err = None;
    for num_tries in 0..max_retries {
        match ControlPlane::start(opts).await {
            Ok(p) => return Ok(p),
            Err(err) => {
                log().error(
                    &err,
                    "unable to start the controlplane",
                    ctrlrt::kv!["tries" => num_tries],
                );
                last_err = Some(err);
            }
        }
    }
    let err = last_err.unwrap_or_else(|| anyhow::anyhow!("unknown error"));
    Err(ctrlrt::wrapf!(
        err,
        "failed to start the controlplane. retried {} times: {}",
        max_retries,
        err
    ))
}

/// `Environment.waitForDefaultNamespace()` — poll every 50ms for up to 5s.
async fn wait_for_default_namespace(client: &kube::Client) -> anyhow::Result<()> {
    let api: Api<Namespace> = Api::all(client.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if api.get("default").await.is_ok() {
            return Ok(());
        }
        if Instant::now() + Duration::from_millis(50) > deadline {
            anyhow::bail!("context deadline exceeded");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One environment shared by all tests of a test binary.
///
/// The first call starts the environment with `builder()`; later calls (from any test /
/// Tokio runtime) return the same instance and ignore their argument. A start failure is
/// cached and returned to every caller. The environment is never dropped (Rust does not drop
/// statics): its processes and directories are removed by the watchdog when the test
/// process exits (see the crate docs).
///
/// Create clients per test with [`Environment::kube_client`].
pub async fn shared<F>(builder: F) -> anyhow::Result<Arc<Environment>>
where
    F: FnOnce() -> EnvironmentBuilder,
{
    static SHARED: tokio::sync::OnceCell<Result<Arc<Environment>, String>> =
        tokio::sync::OnceCell::const_new();
    let res = SHARED
        .get_or_init(|| async move {
            builder()
                .start()
                .await
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .await;
    match res {
        Ok(env) => Ok(env.clone()),
        Err(msg) => Err(anyhow::anyhow!("{}", msg)),
    }
}
