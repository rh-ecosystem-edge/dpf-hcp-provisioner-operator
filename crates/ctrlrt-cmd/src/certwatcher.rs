//! controller-runtime `pkg/certwatcher`: keeps a TLS certificate/key pair loaded from disk
//! and reloads it on file system events (inotify) and on a 10s polling interval.
//!
//! ```ignore
//! let cw = CertWatcher::new("/certs/tls.crt", "/certs/tls.key")?;   // certwatcher.New
//! tokio::spawn({ let cw = cw.clone(); async move { cw.start(cancel).await } }); // mgr.Add(cw)
//! let tls = rustls::ServerConfig::builder().with_no_client_auth().with_cert_resolver(Arc::new(cw));
//! ```

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use ctrlrt::kv;
use ctrlrt::log::Logger;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio_util::sync::CancellationToken;

use crate::{godur, tlsutil};

/// `defaultWatchInterval`
pub const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_secs(10);

type Callback = Arc<dyn Fn(Arc<CertifiedKey>) + Send + Sync>;

struct State {
    current: Option<Arc<CertifiedKey>>,
    cached_cert_der: Vec<u8>,
    cached_key_pem: Vec<u8>,
}

struct Inner {
    cert_path: String,
    key_path: String,
    interval: Mutex<Duration>,
    log: Logger,
    state: RwLock<State>,
    callback: Mutex<Option<Callback>>,
}

/// `certwatcher.CertWatcher` (cheap to clone; clones share state).
#[derive(Clone)]
pub struct CertWatcher {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for CertWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertWatcher")
            .field("cert_path", &self.inner.cert_path)
            .field("key_path", &self.inner.key_path)
            .finish()
    }
}

/// Logger of the package (`logf.RuntimeLog.WithName("certwatcher")`).
pub fn log() -> Logger {
    Logger::root()
        .with_name("controller-runtime")
        .with_name("certwatcher")
}

impl CertWatcher {
    /// `certwatcher.New(certPath, keyPath)`: loads the pair once; the error is the Go error
    /// message (`open <path>: no such file or directory`, `tls: ...`).
    pub fn new(cert_path: impl Into<String>, key_path: impl Into<String>) -> ctrlrt::Result<Self> {
        let cert_path = cert_path.into();
        let key_path = key_path.into();
        let cw = Self {
            inner: Arc::new(Inner {
                log: log().with_values(kv!["cert" => cert_path, "key" => key_path]),
                cert_path,
                key_path,
                interval: Mutex::new(DEFAULT_WATCH_INTERVAL),
                state: RwLock::new(State {
                    current: None,
                    cached_cert_der: Vec::new(),
                    cached_key_pem: Vec::new(),
                }),
                callback: Mutex::new(None),
            }),
        };
        cw.read_certificate()?;
        Ok(cw)
    }

    /// `WithWatchInterval`
    pub fn with_watch_interval(self, interval: Duration) -> Self {
        if let Ok(mut i) = self.inner.interval.lock() {
            *i = interval;
        }
        self
    }

    pub fn cert_path(&self) -> &str {
        &self.inner.cert_path
    }

    pub fn key_path(&self) -> &str {
        &self.inner.key_path
    }

    /// `RegisterCallback`: called (on a new thread) whenever the certificate changes, and
    /// immediately with the current certificate.
    pub fn register_callback(&self, callback: impl Fn(Arc<CertifiedKey>) + Send + Sync + 'static) {
        let cb: Callback = Arc::new(callback);
        let mut slot = match self.inner.callback.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        if let Some(c) = self.get_certificate() {
            cb(c);
        }
        *slot = Some(cb);
    }

    /// `GetCertificate`: the currently loaded certificate.
    pub fn get_certificate(&self) -> Option<Arc<CertifiedKey>> {
        self.inner.state.read().ok().and_then(|s| s.current.clone())
    }

    /// `NeedLeaderElection` (runnable hint): false.
    pub fn need_leader_election(&self) -> bool {
        false
    }

    fn update_cached_certificate(&self, cert: Arc<CertifiedKey>, key_pem: Vec<u8>) -> bool {
        let Ok(mut st) = self.inner.state.write() else {
            return false;
        };
        let der0 = cert
            .cert
            .first()
            .map(|c| c.as_ref().to_vec())
            .unwrap_or_default();
        if st.current.is_some() && st.cached_cert_der == der0 && st.cached_key_pem == key_pem {
            self.inner
                .log
                .v(7)
                .info("certificate already cached", kv![]);
            return false;
        }
        st.current = Some(cert);
        st.cached_cert_der = der0;
        st.cached_key_pem = key_pem;
        true
    }

    /// `ReadCertificate`: (re)load the pair from disk.
    pub fn read_certificate(&self) -> ctrlrt::Result<()> {
        let cert_pem = tlsutil::read_file(&self.inner.cert_path).map_err(anyhow::Error::msg)?;
        let key_pem = tlsutil::read_file(&self.inner.key_path).map_err(anyhow::Error::msg)?;
        let cert = tlsutil::x509_key_pair(&cert_pem, &key_pem).map_err(anyhow::Error::msg)?;
        if !self.update_cached_certificate(cert.clone(), key_pem) {
            return Ok(());
        }
        self.inner
            .log
            .info("Updated current TLS certificate", kv![]);
        let cb = self.inner.callback.lock().ok().and_then(|c| c.clone());
        if let Some(cb) = cb {
            std::thread::spawn(move || cb(cert));
        }
        Ok(())
    }

    /// `Start(ctx)`: watch the files and poll every interval until `cancel` fires.
    pub async fn start(&self, cancel: CancellationToken) -> ctrlrt::Result<()> {
        use notify::Watcher;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .map_err(|e| anyhow::anyhow!("{}", notify_err_text(&e)))?;

        // Poll (1s interval, 10s timeout, immediate) until both watches are added.
        let mut files: BTreeSet<String> =
            [self.inner.cert_path.clone(), self.inner.key_path.clone()]
                .into_iter()
                .collect();
        let mut watch_err: Option<String> = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let pending: Vec<String> = files.iter().cloned().collect();
            for f in pending {
                match watcher.watch(
                    std::path::Path::new(&f),
                    notify::RecursiveMode::NonRecursive,
                ) {
                    Ok(()) => {
                        files.remove(&f);
                    }
                    Err(e) => {
                        watch_err = Some(notify_err_text(&e));
                        break;
                    }
                }
            }
            if files.is_empty() {
                break;
            }
            let wait = tokio::time::sleep(Duration::from_secs(1));
            let poll_err = tokio::select! {
                _ = cancel.cancelled() => Some("context canceled"),
                _ = tokio::time::sleep_until(deadline) => Some("context deadline exceeded"),
                _ = wait => None,
            };
            if let Some(pe) = poll_err {
                let errs: Vec<String> = [Some(pe.to_string()), watch_err.clone()]
                    .into_iter()
                    .flatten()
                    .collect();
                return Err(anyhow::anyhow!(
                    "failed to add watches: {}",
                    aggregate(&errs)
                ));
            }
        }

        let interval = self
            .inner
            .interval
            .lock()
            .map(|i| *i)
            .unwrap_or(DEFAULT_WATCH_INTERVAL);
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        self.inner.log.info(
            "Starting certificate poll+watcher",
            kv!["interval" => godur::format(interval)],
        );
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    drop(watcher);
                    return Ok(());
                }
                _ = ticker.tick() => {
                    if let Err(err) = self.read_certificate() {
                        self.inner.log.error(&err, "failed read certificate", kv![]);
                    }
                }
                ev = rx.recv() => {
                    match ev {
                        Some(Ok(ev)) => self.handle_event(&mut watcher, ev),
                        Some(Err(e)) => self.inner.log.error(&notify_err_text(&e), "certificate watch error", kv![]),
                        None => {}
                    }
                }
            }
        }
    }

    fn handle_event(&self, watcher: &mut notify::RecommendedWatcher, ev: notify::Event) {
        use notify::EventKind;
        use notify::Watcher;
        use notify::event::{ModifyKind, RemoveKind};

        let op = match ev.kind {
            EventKind::Modify(ModifyKind::Data(_)) | EventKind::Modify(ModifyKind::Any) => "WRITE",
            EventKind::Create(_) => "CREATE",
            EventKind::Modify(ModifyKind::Metadata(_)) => "CHMOD",
            EventKind::Remove(RemoveKind::File | RemoveKind::Any) | EventKind::Remove(_) => {
                "REMOVE"
            }
            _ => return,
        };
        for path in &ev.paths {
            let name = path.to_string_lossy().into_owned();
            if (op == "CHMOD" || op == "REMOVE")
                && let Err(e) = watcher.watch(path, notify::RecursiveMode::NonRecursive)
            {
                self.inner
                    .log
                    .error(&notify_err_text(&e), "error re-watching file", kv![]);
            }
            // fsnotify.Event.String(): fmt.Sprintf("%-13s %q", e.Op.String(), e.Name)
            let ev_str = format!("{op:<13} {}", ctrlrt::error::go_quote(&name));
            self.inner
                .log
                .v(1)
                .info("certificate event", kv!["event" => ev_str]);
            if let Err(err) = self.read_certificate() {
                self.inner
                    .log
                    .error(&err, "error re-reading certificate", kv![]);
            }
        }
    }
}

impl ResolvesServerCert for CertWatcher {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.get_certificate()
    }
}

/// `utilerrors.NewAggregate(errs).Error()` for already-deduplicated messages.
pub fn aggregate(errs: &[String]) -> String {
    let mut seen = BTreeSet::new();
    let msgs: Vec<&String> = errs.iter().filter(|e| seen.insert((*e).clone())).collect();
    match msgs.len() {
        0 => String::new(),
        1 => msgs[0].clone(),
        _ => format!(
            "[{}]",
            msgs.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn notify_err_text(e: &notify::Error) -> String {
    match &e.kind {
        notify::ErrorKind::Io(io) => tlsutil::go_errno_text(io),
        notify::ErrorKind::PathNotFound => "no such file or directory".into(),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tlsutil::tests::gen_pair;

    fn write_pair(dir: &std::path::Path) -> (String, String, String) {
        let (c, k) = gen_pair(&rcgen::PKCS_ECDSA_P256_SHA256);
        let cp = dir.join("tls.crt");
        let kp = dir.join("tls.key");
        std::fs::write(&cp, &c).unwrap();
        std::fs::write(&kp, &k).unwrap();
        (cp.to_string_lossy().into(), kp.to_string_lossy().into(), c)
    }

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ctrlrt-cmd-cw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn new_errors_like_go() {
        let err = CertWatcher::new("/nonexistent/tls.crt", "/nonexistent/tls.key").unwrap_err();
        assert_eq!(
            err.to_string(),
            "open /nonexistent/tls.crt: no such file or directory"
        );
        let d = tmpdir("bad");
        std::fs::write(d.join("tls.crt"), "x").unwrap();
        std::fs::write(d.join("tls.key"), "x").unwrap();
        let err = CertWatcher::new(
            d.join("tls.crt").to_string_lossy(),
            d.join("tls.key").to_string_lossy(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "tls: failed to find any PEM data in certificate input"
        );
    }

    #[tokio::test]
    async fn reloads_on_change() {
        let d = tmpdir("reload");
        let (cp, kp, _) = write_pair(&d);
        let cw = CertWatcher::new(&cp, &kp)
            .unwrap()
            .with_watch_interval(Duration::from_millis(200));
        let first = cw.get_certificate().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        cw.register_callback(move |c| {
            let _ = tx.send(c);
        });
        // immediate callback with the current cert
        let c0 = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(c0.cert[0], first.cert[0]);

        let cancel = CancellationToken::new();
        let h = tokio::spawn({
            let cw = cw.clone();
            let cancel = cancel.clone();
            async move { cw.start(cancel).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        // replace both files
        let (c, k) = gen_pair(&rcgen::PKCS_ECDSA_P256_SHA256);
        std::fs::write(&kp, &k).unwrap();
        std::fs::write(&cp, &c).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let cur = cw.get_certificate().unwrap();
            if cur.cert[0] != first.cert[0] && cur.keys_match().is_ok() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "certificate not reloaded"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        cancel.cancel();
        h.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn start_fails_when_files_vanish() {
        tokio::time::pause();
        let d = tmpdir("vanish");
        let (cp, kp, _) = write_pair(&d);
        let cw = CertWatcher::new(&cp, &kp).unwrap();
        std::fs::remove_file(&cp).unwrap();
        std::fs::remove_file(&kp).unwrap();
        let err = cw.start(CancellationToken::new()).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "failed to add watches: [context deadline exceeded, no such file or directory]"
        );
    }

    #[test]
    fn aggregate_formats() {
        assert_eq!(aggregate(&["a".into()]), "a");
        assert_eq!(aggregate(&["a".into(), "b".into()]), "[a, b]");
        assert_eq!(aggregate(&["a".into(), "a".into()]), "a");
    }
}
