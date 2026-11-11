//! Port of `controller-runtime/pkg/internal/testing/controlplane` (etcd, kube-apiserver,
//! certificate authn, kubeconfig rendering).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;

use crate::addr;
use crate::certs::{ClientInfo, TinyCA};
use crate::process::{Arguments, ProcessState, Watchdog, check_flag};

const SA_KEY_FILE: &str = "sa-signer.key";
const SA_CERT_FILE: &str = "sa-signer.crt";
const ENVTEST_NAME: &str = "envtest";

/// `controlplane.User` — a user to provision credentials for.
#[derive(Clone, Debug, Default)]
pub struct User {
    /// User name (certificate CN).
    pub name: String,
    /// Groups (certificate O).
    pub groups: Vec<String>,
}

/// Connection details of an authenticated user (`rest.Config` subset used by envtest).
#[derive(Clone, Debug)]
pub struct RestConfig {
    /// `Host`, e.g. `https://127.0.0.1:12345/`.
    pub host: String,
    /// `APIPath` (always empty for envtest).
    pub api_path: String,
    /// `CAData` (PEM).
    pub ca_data: Vec<u8>,
    /// `CertData` (PEM).
    pub cert_data: Vec<u8>,
    /// `KeyData` (PEM, PKCS#8).
    pub key_data: Vec<u8>,
}

/// `controlplane.KubeConfigFromREST(cfg)` — renders a kubeconfig exactly like
/// `clientcmd.Write` (sigs.k8s.io/yaml output, keys sorted, empty fields omitted).
pub fn kubeconfig_from_rest(cfg: &RestConfig) -> anyhow::Result<String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let protocol = if cfg.host.starts_with("http://") {
        "http"
    } else {
        "https"
    };
    let uri: http::Uri = cfg.host.parse().map_err(|err| {
        ctrlrt::wrapf!(
            anyhow::Error::from(err),
            "unable to interpret config's host value as a URL: {}",
            err
        )
    })?;
    let host = uri
        .authority()
        .map(|a| a.as_str().to_string())
        .unwrap_or_default();
    let server = format!("{protocol}://{host}{}", cfg.api_path);

    let mut out = String::new();
    out.push_str("apiVersion: v1\nclusters:\n- cluster:\n");
    if !cfg.ca_data.is_empty() {
        out.push_str(&format!(
            "    certificate-authority-data: {}\n",
            b64.encode(&cfg.ca_data)
        ));
    }
    out.push_str(&format!("    server: {server}\n  name: {ENVTEST_NAME}\n"));
    out.push_str(&format!(
        "contexts:\n- context:\n    cluster: {ENVTEST_NAME}\n    user: {ENVTEST_NAME}\n  name: {ENVTEST_NAME}\n"
    ));
    out.push_str(&format!("current-context: {ENVTEST_NAME}\nkind: Config\n"));
    out.push_str(&format!("users:\n- name: {ENVTEST_NAME}\n  user:"));
    if cfg.cert_data.is_empty() && cfg.key_data.is_empty() {
        out.push_str(" {}\n");
    } else {
        out.push('\n');
        if !cfg.cert_data.is_empty() {
            out.push_str(&format!(
                "    client-certificate-data: {}\n",
                b64.encode(&cfg.cert_data)
            ));
        }
        if !cfg.key_data.is_empty() {
            out.push_str(&format!(
                "    client-key-data: {}\n",
                b64.encode(&cfg.key_data)
            ));
        }
    }
    Ok(out)
}

fn write_file_0640(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o640)
        .open(path)?;
    f.write_all(data)
}

/// Options for starting a [`ControlPlane`].
#[derive(Clone, Debug, Default)]
pub(crate) struct PlaneOptions {
    pub etcd_path: PathBuf,
    pub apiserver_path: PathBuf,
    pub kubectl_path: PathBuf,
    pub start_timeout: Duration,
    pub stop_timeout: Duration,
    pub attach_output: bool,
    pub etcd_args: Arguments,
    pub apiserver_args: Arguments,
}

/// A running etcd + kube-apiserver pair (`controlplane.ControlPlane`).
pub(crate) struct ControlPlane {
    // Field order matters for Drop: API server first, then etcd, then the watchdog.
    pub apiserver: ProcessState,
    pub etcd: ProcessState,
    pub watchdog: Watchdog,
    /// Client certificate authn CA (`CertAuthn`).
    client_ca: TinyCA,
    /// `SecureServing.CA` — the serving CA (PEM).
    pub serving_ca: Vec<u8>,
    /// `SecureServing.URL("https", "/")`
    pub secure_url: String,
    /// `Etcd.URL`
    pub etcd_url: String,
    pub cert_dir: PathBuf,
    pub kubectl_path: PathBuf,
    /// `--kubeconfig=<file>` of the default (legacy) user, for kubectl.
    pub default_kubeconfig_path: PathBuf,
}

impl ControlPlane {
    /// `ControlPlane.Start()`
    pub(crate) async fn start(opts: &PlaneOptions) -> anyhow::Result<ControlPlane> {
        let mut watchdog = Watchdog::spawn();

        // ---- etcd ----
        let mut etcd = ProcessState::init(
            "etcd",
            Some(opts.etcd_path.clone()),
            None,
            opts.start_timeout,
            opts.stop_timeout,
            &mut watchdog,
        )?;
        etcd.attach_output = opts.attach_output;
        let (port, host) = addr::suggest()?;
        let etcd_url = format!("http://{host}:{port}");
        let (peer_port, peer_host) = addr::suggest()?;
        let listen_peer_url = format!("http://{peer_host}:{peer_port}");
        etcd.health_base = etcd_url.clone();
        etcd.health_path = "/health".to_string();
        let mut etcd_defaults: BTreeMap<String, Vec<String>> = BTreeMap::new();
        etcd_defaults.insert("listen-peer-urls".into(), vec![listen_peer_url]);
        etcd_defaults.insert("data-dir".into(), vec![etcd.dir.display().to_string()]);
        etcd_defaults.insert("advertise-client-urls".into(), vec![etcd_url.clone()]);
        etcd_defaults.insert("listen-client-urls".into(), vec![etcd_url.clone()]);
        if check_flag(&etcd.path, "unsafe-no-fsync")
            .await
            .unwrap_or(false)
        {
            etcd_defaults.insert("unsafe-no-fsync".into(), vec!["true".into()]);
        }
        etcd.args = opts.etcd_args.as_strings(&etcd_defaults);
        if let Err(err) = etcd.start(&mut watchdog).await {
            let _ = etcd.stop().await;
            return Err(err);
        }

        // ---- kube-apiserver ----
        let res = Self::start_apiserver(opts, &etcd_url, &mut watchdog).await;
        let (apiserver, serving_ca, client_ca, secure_url) = match res {
            Ok(v) => v,
            Err(err) => {
                let _ = etcd.stop().await;
                return Err(err);
            }
        };
        let cert_dir = apiserver.dir.clone();

        let mut plane = ControlPlane {
            apiserver,
            etcd,
            watchdog,
            client_ca,
            serving_ca,
            secure_url,
            etcd_url,
            cert_dir,
            kubectl_path: opts.kubectl_path.clone(),
            default_kubeconfig_path: PathBuf::new(),
        };

        // Default (legacy) user + kubeconfig file for kubectl.
        let user = plane
            .add_user(&User {
                name: "default".into(),
                groups: vec!["system:masters".into()],
            })
            .map_err(|err| {
                ctrlrt::wrapf!(
                    err,
                    "unable to provision the default (legacy) user: {}",
                    err
                )
            });
        let user = match user {
            Ok(u) => u,
            Err(err) => {
                let _ = plane.stop().await;
                return Err(err);
            }
        };
        match plane.write_kubeconfig_file(&user) {
            Ok(p) => plane.default_kubeconfig_path = p,
            Err(err) => {
                let _ = plane.stop().await;
                return Err(ctrlrt::wrapf!(
                    err,
                    "unable to provision the default (legacy) kubeconfig: {}",
                    err
                ));
            }
        }
        Ok(plane)
    }

    async fn start_apiserver(
        opts: &PlaneOptions,
        etcd_url: &str,
        watchdog: &mut Watchdog,
    ) -> anyhow::Result<(ProcessState, Vec<u8>, TinyCA, String)> {
        let mut ps = ProcessState::init(
            "kube-apiserver",
            Some(opts.apiserver_path.clone()),
            None,
            opts.start_timeout,
            opts.stop_timeout,
            watchdog,
        )?;
        ps.attach_output = opts.attach_output;
        let res = Self::configure_and_start_apiserver(&mut ps, opts, etcd_url, watchdog).await;
        match res {
            Ok((ca, client_ca, url)) => Ok((ps, ca, client_ca, url)),
            Err(err) => {
                let _ = ps.stop().await;
                Err(err)
            }
        }
    }

    async fn configure_and_start_apiserver(
        ps: &mut ProcessState,
        opts: &PlaneOptions,
        etcd_url: &str,
        watchdog: &mut Watchdog,
    ) -> anyhow::Result<(Vec<u8>, TinyCA, String)> {
        // configurePorts
        let (port, host) = addr::suggest().map_err(|err| {
            ctrlrt::wrapf!(err, "unable to provision unused secure port: {}", err)
        })?;
        let host_port = format!("{host}:{port}");
        ps.health_base = format!("https://{host_port}");
        ps.health_path = "/healthz".into();
        ps.ready_path = Some("/readyz".into());
        let cert_dir = ps.dir.clone();

        // populateAPIServerCerts
        let mut ca = TinyCA::new()?;
        let serving = ca.new_serving_cert(&["localhost", &host])?;
        let (cert_data, key_data) = serving.as_bytes();
        write_file_0640(&cert_dir.join("apiserver.crt"), &cert_data)?;
        write_file_0640(&cert_dir.join("apiserver.key"), &key_data)?;
        let serving_ca = ca.ca.cert_bytes();
        let sa_ca = TinyCA::new()?;
        let (sa_cert, sa_key) = sa_ca.ca.as_bytes();
        write_file_0640(&cert_dir.join(SA_CERT_FILE), &sa_cert)?;
        write_file_0640(&cert_dir.join(SA_KEY_FILE), &sa_key)?;

        // NewCertAuthn + Configure
        let client_ca = TinyCA::new().map_err(|err| {
            ctrlrt::wrapf!(
                err,
                "unable to provision client certificate auth CA: {}",
                err
            )
        })?;
        let client_ca_path = cert_dir.join("client-cert-auth-ca.crt");
        let mut args = opts.apiserver_args.clone();
        args.set("client-ca-file", [client_ca_path.display().to_string()]);

        // discoverFlags
        if !check_flag(&ps.path, "insecure-port").await? {
            args.disable("insecure-port");
        }

        let secure_url = format!("https://{host_port}/");
        let mut defaults: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut d = |k: &str, v: String| {
            defaults.insert(k.to_string(), vec![v]);
        };
        d("service-cluster-ip-range", "10.0.0.0/24".into());
        d("allow-privileged", "true".into());
        // Disable the ServiceAccount admission plugin as we don't have the
        // serviceaccount controller running.
        d("disable-admission-plugins", "ServiceAccount".into());
        d("cert-dir", cert_dir.display().to_string());
        d("authorization-mode", "RBAC".into());
        d("secure-port", port.to_string());
        d("bind-address", host.clone());
        d("service-account-issuer", secure_url.clone());
        d(
            "service-account-key-file",
            cert_dir.join(SA_CERT_FILE).display().to_string(),
        );
        d(
            "service-account-signing-key-file",
            cert_dir.join(SA_KEY_FILE).display().to_string(),
        );
        d("etcd-servers", etcd_url.to_string());
        d("insecure-port", "0".into());
        ps.args = args.as_strings(&defaults);

        // Authn.Start
        write_file_0640(&client_ca_path, &client_ca.ca.cert_bytes()).map_err(|err| {
            ctrlrt::wrapf!(
                err,
                "unable to save the client certificate CA to {}: {}",
                client_ca_path.display(),
                err
            )
        })?;

        ps.start(watchdog).await?;
        Ok((serving_ca, client_ca, secure_url))
    }

    /// `ControlPlane.AddUser(user, baseConfig)` via `CertAuthn.AddUser`.
    pub(crate) fn add_user(&mut self, user: &User) -> anyhow::Result<RestConfig> {
        let pair = self
            .client_ca
            .new_client_cert(&ClientInfo {
                name: user.name.clone(),
                groups: user.groups.clone(),
            })
            .map_err(|err| {
                ctrlrt::wrapf!(
                    err,
                    "unable to create client certificates for {}: {}",
                    user.name,
                    err
                )
            })?;
        let (cert_data, key_data) = pair.as_bytes();
        Ok(RestConfig {
            host: self.secure_url.clone(),
            api_path: String::new(),
            ca_data: self.serving_ca.clone(),
            cert_data,
            key_data,
        })
    }

    fn write_kubeconfig_file(&self, user: &RestConfig) -> anyhow::Result<PathBuf> {
        let file = tempfile::Builder::new()
            .prefix("")
            .suffix(".kubecfg")
            .tempfile_in(&self.cert_dir)
            .map_err(|err| ctrlrt::wrapf!(err, "unable to create file for kubeconfig: {}", err))?;
        let contents = kubeconfig_from_rest(user)?;
        let (mut f, path) = file
            .keep()
            .map_err(|err| ctrlrt::wrapf!(err, "unable to create file for kubeconfig: {}", err))?;
        f.write_all(contents.as_bytes()).map_err(|err| {
            ctrlrt::wrapf!(
                err,
                "unable to write kubeconfig to disk at {}: {}",
                path.display(),
                err
            )
        })?;
        Ok(path)
    }

    /// `ControlPlane.Stop()` — API server first, then etcd; errors are aggregated.
    pub(crate) async fn stop(&mut self) -> anyhow::Result<()> {
        let mut errs: Vec<anyhow::Error> = Vec::new();
        if let Err(e) = self.apiserver.stop().await {
            errs.push(e);
        }
        if let Err(e) = self.etcd.stop().await {
            errs.push(e);
        }
        self.watchdog.disarm();
        aggregate(errs)
    }

    /// Synchronous teardown for `Drop`.
    pub(crate) fn kill_now(&mut self) {
        self.apiserver.kill_now();
        self.etcd.kill_now();
        self.watchdog.disarm();
    }
}

impl Drop for ControlPlane {
    fn drop(&mut self) {
        self.kill_now();
    }
}

/// `kerrors.NewAggregate(errs)`
pub(crate) fn aggregate(errs: Vec<anyhow::Error>) -> anyhow::Result<()> {
    let mut msgs: Vec<String> = Vec::new();
    for e in &errs {
        let m = e.to_string();
        if !msgs.contains(&m) {
            msgs.push(m);
        }
    }
    match msgs.len() {
        0 => Ok(()),
        1 => Err(errs
            .into_iter()
            .next()
            .unwrap_or_else(|| anyhow::anyhow!("{}", msgs[0]))),
        _ => Err(anyhow::anyhow!("[{}]", msgs.join(", "))),
    }
}
