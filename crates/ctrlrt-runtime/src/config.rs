//! REST configuration: `ctrl.GetConfigOrDie()` (controller-runtime `pkg/client/config`), the
//! client-go user agent, client-side rate limiting (`rest.Config.QPS/Burst`), request timeouts
//! (`rest.Config.Timeout`) and the `rest_client_requests_total` metric, plus helpers to build
//! clients for hosted clusters from kubeconfig bytes.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http::{HeaderName, HeaderValue, Request, Response};
use kube::client::{Body, ClientBuilder};
use kube::config::{KubeConfigOptions, Kubeconfig};
use tower::{Layer, Service};

use crate::metrics;
use crate::ratelimit::TokenBucket;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `clientcmd.RecommendedConfigPathEnvVar`
pub const RECOMMENDED_CONFIG_PATH_ENV_VAR: &str = "KUBECONFIG";

/// client-go's `rest.DefaultQPS` / `rest.DefaultBurst` (used when QPS/Burst are 0).
pub const DEFAULT_QPS: f32 = 5.0;
pub const DEFAULT_BURST: i32 = 10;

/// `rest.Config` analogue: a kube [`kube::Config`] plus the client-go knobs kube does not have.
#[derive(Clone)]
pub struct RestConfig {
    /// Connection / TLS / auth settings.
    pub kube: kube::Config,
    /// `QPS`: 0 means client-go's default (5), negative disables client-side rate limiting.
    pub qps: f32,
    /// `Burst`: 0 means client-go's default (10).
    pub burst: i32,
    /// `UserAgent`; empty means [`default_kubernetes_user_agent`].
    pub user_agent: String,
    /// `Timeout`: overall per-request timeout (zero/None = no timeout).
    pub timeout: Option<Duration>,
}

impl std::fmt::Debug for RestConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestConfig")
            .field("host", &self.kube.cluster_url.to_string())
            .field("qps", &self.qps)
            .field("burst", &self.burst)
            .field("user_agent", &self.user_agent)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl RestConfig {
    /// Wrap a kube config with the settings `ctrl.GetConfig()` produces: client-side rate
    /// limiting disabled (QPS -1), default user agent, no timeout.
    pub fn new(kube: kube::Config) -> Self {
        Self {
            kube,
            qps: -1.0,
            burst: 0,
            user_agent: String::new(),
            timeout: None,
        }
    }

    /// Enable client-side rate limiting.
    pub fn with_rate_limit(mut self, qps: f32, burst: i32) -> Self {
        self.qps = qps;
        self.burst = burst;
        self
    }

    /// Set the request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// `rest.AddUserAgent(config, userAgent)`: default user agent + `/` + suffix.
    pub fn add_user_agent(mut self, suffix: &str) -> Self {
        self.user_agent = format!("{}/{}", default_kubernetes_user_agent(), suffix);
        self
    }

    /// Effective user agent (`rest.Config.UserAgent` or the client-go default).
    pub fn effective_user_agent(&self) -> String {
        if self.user_agent.is_empty() {
            default_kubernetes_user_agent()
        } else {
            self.user_agent.clone()
        }
    }

    /// The host of the API server (`rest.Config.Host`).
    pub fn host(&self) -> String {
        self.kube.cluster_url.to_string()
    }

    /// Build a `kube::Client` with rate limiting, timeout, user agent and request metrics.
    pub fn build_client(&self) -> crate::Result<kube::Client> {
        let mut cfg = self.kube.clone();
        let ua = self.effective_user_agent();
        if let Ok(v) = HeaderValue::from_str(&ua) {
            cfg.headers.retain(|(k, _)| k != http::header::USER_AGENT);
            cfg.headers.push((HeaderName::from_static("user-agent"), v));
        }
        if let Some(t) = self.timeout.filter(|t| !t.is_zero()) {
            cfg.read_timeout = Some(t);
            if cfg.connect_timeout.is_none_or(|c| c > t) {
                cfg.connect_timeout = Some(t);
            }
        }
        let builder = ClientBuilder::try_from(cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
        let layer = ClientGoLayer {
            limiter: self.rate_limiter(),
            timeout: self.timeout.filter(|t| !t.is_zero()),
        };
        Ok(builder.with_layer(&layer).build())
    }

    fn rate_limiter(&self) -> Option<Arc<TokenBucket>> {
        // rest.RESTClientFor: qps == 0 -> DefaultQPS; burst == 0 -> DefaultBurst; a limiter is
        // only created for qps > 0.
        let qps = if self.qps == 0.0 {
            DEFAULT_QPS
        } else {
            self.qps
        };
        let burst = if self.burst == 0 {
            DEFAULT_BURST
        } else {
            self.burst
        };
        (qps > 0.0).then(|| Arc::new(TokenBucket::new(qps as f64, burst as i64)))
    }
}

/// `rest.DefaultKubernetesUserAgent()`: `<command>/<version> (<os>/<arch>) kubernetes/<commit>`.
///
/// The Go binary is built without client-go version ldflags, so the version is `v0.0.0`
/// (`v0.0.0-master+$Format:%H$` adjusted) and the commit `$Format` (first 7 characters).
pub fn default_kubernetes_user_agent() -> String {
    let command = std::env::args_os()
        .next()
        .map(|a| {
            let p = PathBuf::from(a);
            p.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "powerpc64" => "ppc64le",
        other => other,
    };
    format!(
        "{}/{} ({}/{}) kubernetes/{}",
        command,
        "v0.0.0",
        std::env::consts::OS,
        arch,
        "$Format"
    )
}

fn config_log() -> ctrlrt::Logger {
    ctrlrt::Logger::root()
        .with_name("controller-runtime")
        .with_name("client")
        .with_name("config")
}

const NO_CONFIG_ERR: &str = "invalid configuration: no configuration has been provided, try setting KUBERNETES_MASTER environment variable";

/// `ctrl.GetConfig()` with the value of the `--kubeconfig` flag (empty when not set).
///
/// Precedence: `--kubeconfig` file, `$KUBECONFIG`, in-cluster config, `$HOME/.kube/config`.
pub async fn get_config(kubeconfig_flag: &str) -> crate::Result<RestConfig> {
    let mut cfg = load_config(kubeconfig_flag).await?;
    // GetConfigWithContext: disable the client-side rate limiter by default.
    if cfg.qps == 0.0 {
        cfg.qps = -1.0;
    }
    Ok(cfg)
}

/// `ctrl.GetConfigOrDie()`: logs `unable to get kubeconfig` and exits with code 1 on error.
pub async fn get_config_or_die(kubeconfig_flag: &str) -> RestConfig {
    match get_config(kubeconfig_flag).await {
        Ok(c) => c,
        Err(e) => {
            config_log().error(&e, "unable to get kubeconfig", ctrlrt::kv![]);
            std::process::exit(1);
        }
    }
}

async fn load_config(kubeconfig_flag: &str) -> crate::Result<RestConfig> {
    // If a flag is specified with the config location, use that
    if !kubeconfig_flag.is_empty() {
        let path = Path::new(kubeconfig_flag);
        if let Err(e) = std::fs::metadata(path) {
            return Err(anyhow::anyhow!(
                "stat {}: {}",
                kubeconfig_flag,
                io_error_text(&e)
            ));
        }
        let kc = Kubeconfig::read_from(path).map_err(|e| anyhow::anyhow!("{e}"))?;
        return rest_config_from_kubeconfig(kc).await;
    }

    // If the recommended kubeconfig env variable is not specified, try the in-cluster config.
    let kubeconfig_env = std::env::var(RECOMMENDED_CONFIG_PATH_ENV_VAR).unwrap_or_default();
    let mut in_cluster_err: Option<crate::Error> = None;
    if kubeconfig_env.is_empty() {
        match in_cluster_config() {
            Ok(c) => return Ok(c),
            Err(e) => in_cluster_err = Some(e),
        }
    }

    // If the recommended kubeconfig env variable is set, or there is no in-cluster config,
    // try the default recommended locations.
    let res = load_default_rules(&kubeconfig_env).await;
    if res.is_err()
        && let Some(e) = in_cluster_err
    {
        config_log().error(&e, "unable to load in-cluster config", ctrlrt::kv![]);
    }
    res
}

async fn load_default_rules(kubeconfig_env: &str) -> crate::Result<RestConfig> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if !kubeconfig_env.is_empty() {
        paths.extend(std::env::split_paths(kubeconfig_env).filter(|p| !p.as_os_str().is_empty()));
    } else if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".kube").join("config"));
    }
    let mut merged: Option<Kubeconfig> = None;
    for p in paths {
        // clientcmd silently skips files that do not exist
        if !p.exists() {
            continue;
        }
        let kc = Kubeconfig::read_from(&p).map_err(|e| anyhow::anyhow!("{e}"))?;
        merged = Some(match merged {
            None => kc,
            Some(m) => m.merge(kc).map_err(|e| anyhow::anyhow!("{e}"))?,
        });
    }
    let Some(kc) = merged else {
        return Err(anyhow::anyhow!(NO_CONFIG_ERR));
    };
    if kc.clusters.is_empty() && kc.current_context.as_deref().unwrap_or("").is_empty() {
        return Err(anyhow::anyhow!(NO_CONFIG_ERR));
    }
    rest_config_from_kubeconfig(kc).await
}

/// `rest.InClusterConfig()`.
pub fn in_cluster_config() -> crate::Result<RestConfig> {
    let host = std::env::var("KUBERNETES_SERVICE_HOST").unwrap_or_default();
    let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_default();
    if host.is_empty() || port.is_empty() {
        return Err(anyhow::anyhow!(
            "unable to load in-cluster configuration, KUBERNETES_SERVICE_HOST and KUBERNETES_SERVICE_PORT must be defined"
        ));
    }
    const TOKEN_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
    if let Err(e) = std::fs::metadata(TOKEN_FILE) {
        return Err(anyhow::anyhow!(
            "open {}: {}",
            TOKEN_FILE,
            io_error_text(&e)
        ));
    }
    let kube = kube::Config::incluster_env().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(RestConfig::new(kube))
}

/// Go-style text for common `syscall.Errno` values.
pub(crate) fn io_error_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        std::io::ErrorKind::AddrInUse => "address already in use".into(),
        std::io::ErrorKind::AddrNotAvailable => "cannot assign requested address".into(),
        std::io::ErrorKind::ConnectionRefused => "connection refused".into(),
        _ => {
            let s = e.to_string();
            // strip Rust's " (os error N)" suffix and lowercase like Go's errno strings
            let s = match s.find(" (os error") {
                Some(i) => s[..i].to_string(),
                None => s,
            };
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
                None => s,
            }
        }
    }
}

async fn rest_config_from_kubeconfig(kc: Kubeconfig) -> crate::Result<RestConfig> {
    let kube = kube::Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(RestConfig::new(kube))
}

/// `clientcmd.Load(data)`: parse kubeconfig bytes.
pub fn parse_kubeconfig(data: &[u8]) -> crate::Result<Kubeconfig> {
    let text = std::str::from_utf8(data).map_err(|e| anyhow::anyhow!("{e}"))?;
    Kubeconfig::from_yaml(text).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Overrides applied when building a client from kubeconfig bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct KubeconfigOverrides {
    /// Replace the server URL of the current context's cluster.
    pub server: Option<String>,
    /// `rest.Config.Timeout` (default 30s).
    pub timeout: Duration,
    /// `rest.Config.QPS` (default 5).
    pub qps: f32,
    /// `rest.Config.Burst` (default 10).
    pub burst: i32,
}

impl Default for KubeconfigOverrides {
    fn default() -> Self {
        Self {
            server: None,
            timeout: Duration::from_secs(30),
            qps: 5.0,
            burst: 10,
        }
    }
}

/// `clientcmd.NewDefaultClientConfig(kubeconfig, &ConfigOverrides{}).ClientConfig()` with
/// the hosted-cluster settings (`Timeout` 30s, `QPS` 5, `Burst` 10 by default).
pub async fn rest_config_from_kubeconfig_with(
    kc: Kubeconfig,
    overrides: &KubeconfigOverrides,
) -> crate::Result<RestConfig> {
    let mut kube = kube::Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(server) = &overrides.server {
        kube.cluster_url = server
            .parse::<http::Uri>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let mut cfg = RestConfig::new(kube);
    cfg.qps = overrides.qps;
    cfg.burst = overrides.burst;
    cfg.timeout = Some(overrides.timeout).filter(|t| !t.is_zero());
    Ok(cfg)
}

/// Build a direct (uncached) typed client plus the underlying `kube::Client` from kubeconfig
/// bytes, for talking to a hosted cluster (`kubernetes.NewForConfig` equivalent).
pub async fn client_from_kubeconfig_bytes(
    data: &[u8],
    overrides: &KubeconfigOverrides,
) -> crate::Result<(ctrlrt::Client, kube::Client)> {
    let kc = parse_kubeconfig(data)?;
    let cfg = rest_config_from_kubeconfig_with(kc, overrides).await?;
    client_from_rest_config(&cfg)
}

/// Build a direct typed client and the `kube::Client` from a [`RestConfig`].
pub fn client_from_rest_config(cfg: &RestConfig) -> crate::Result<(ctrlrt::Client, kube::Client)> {
    let kc = cfg.build_client()?;
    let backend = crate::client::KubeBackend::new(kc.clone());
    Ok((ctrlrt::Client::new(Arc::new(backend)), kc))
}

/// `clientset.Discovery().ServerVersion()`.
pub async fn server_version(
    client: &kube::Client,
) -> crate::Result<k8s_openapi::apimachinery::pkg::version::Info> {
    let rest = crate::client::RestClient::new(client.clone());
    let v = rest.get_json("/version", None).await?;
    serde_json::from_value(v)
        .map_err(|e| anyhow::anyhow!("unable to parse the server version: {e}"))
}

// ---------------------------------------------------------------------------------------------
// tower layer: rate limiting, timeout and request metrics
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct ClientGoLayer {
    limiter: Option<Arc<TokenBucket>>,
    timeout: Option<Duration>,
}

impl<S> Layer<S> for ClientGoLayer {
    type Service = ClientGoService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        ClientGoService {
            inner,
            limiter: self.limiter.clone(),
            timeout: self.timeout,
            wait: None,
            permit: false,
        }
    }
}

struct ClientGoService<S> {
    inner: S,
    limiter: Option<Arc<TokenBucket>>,
    timeout: Option<Duration>,
    wait: Option<Pin<Box<tokio::time::Sleep>>>,
    permit: bool,
}

/// Error returned when `rest.Config.Timeout` elapses.
#[derive(Debug)]
pub struct ClientTimeoutError;
impl std::fmt::Display for ClientTimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("context deadline exceeded (Client.Timeout exceeded while awaiting headers)")
    }
}
impl std::error::Error for ClientTimeoutError {}

impl<S, B> Service<Request<Body>> for ClientGoService<S>
where
    S: Service<Request<Body>, Response = Response<B>>,
    S::Error: Into<BoxError>,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response<B>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Response<B>, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if !self.permit
            && let Some(l) = &self.limiter
        {
            if self.wait.is_none() {
                let d = l.reserve_delay();
                if d.is_zero() || d == Duration::MAX {
                    self.permit = true;
                } else {
                    self.wait = Some(Box::pin(tokio::time::sleep(d)));
                }
            }
            if let Some(w) = self.wait.as_mut() {
                match w.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        self.wait = None;
                        self.permit = true;
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }
        }
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        self.permit = false;
        let method = req.method().as_str().to_string();
        let host = req
            .uri()
            .authority()
            .map(|a| a.as_str().to_string())
            .unwrap_or_default();
        let fut = self.inner.call(req);
        let timeout = self.timeout;
        Box::pin(async move {
            let res = match timeout {
                Some(t) => match tokio::time::timeout(t, fut).await {
                    Ok(r) => r.map_err(Into::into),
                    Err(_) => Err(Box::new(ClientTimeoutError) as BoxError),
                },
                None => fut.await.map_err(Into::into),
            };
            let code = match &res {
                Ok(r) => r.status().as_u16().to_string(),
                Err(_) => "<error>".to_string(),
            };
            metrics::REST_CLIENT_REQUESTS
                .with_label_values(&[&code, &method, &host])
                .inc();
            res
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_matches_client_go_format() {
        let ua = default_kubernetes_user_agent();
        assert!(ua.contains("/v0.0.0 (linux/"), "{ua}");
        assert!(ua.ends_with(") kubernetes/$Format"), "{ua}");
    }

    #[test]
    fn rate_limiter_defaults() {
        let base = RestConfig::new(kube::Config::new("https://example:6443".parse().unwrap()));
        assert!(
            base.rate_limiter().is_none(),
            "GetConfig disables client-side rate limiting"
        );
        let mut c = base.clone();
        c.qps = 0.0;
        assert!(
            c.rate_limiter().is_some(),
            "QPS 0 means client-go default 5"
        );
        let c = base.with_rate_limit(5.0, 10);
        assert!(c.rate_limiter().is_some());
    }

    #[tokio::test]
    async fn get_config_explicit_missing_file() {
        let err = get_config("/nonexistent/kubeconfig").await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "stat /nonexistent/kubeconfig: no such file or directory"
        );
    }

    #[tokio::test]
    async fn kubeconfig_bytes_with_server_override() {
        let kc = br#"
apiVersion: v1
kind: Config
clusters:
- name: c
  cluster:
    server: https://10.0.0.1:6443
    insecure-skip-tls-verify: true
contexts:
- name: ctx
  context:
    cluster: c
    user: u
current-context: ctx
users:
- name: u
  user:
    token: abc
"#;
        let parsed = parse_kubeconfig(kc).unwrap();
        let cfg = rest_config_from_kubeconfig_with(
            parsed,
            &KubeconfigOverrides {
                server: Some("https://kube-apiserver.ns-hc.svc.cluster.local:6443".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            cfg.kube.cluster_url.to_string(),
            "https://kube-apiserver.ns-hc.svc.cluster.local:6443/"
        );
        assert_eq!(cfg.qps, 5.0);
        assert_eq!(cfg.burst, 10);
        assert_eq!(cfg.timeout, Some(Duration::from_secs(30)));
        let (_c, _k) = client_from_rest_config(&cfg).unwrap();
    }
}
