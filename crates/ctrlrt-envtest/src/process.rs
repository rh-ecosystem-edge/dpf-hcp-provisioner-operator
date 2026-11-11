//! Port of `controller-runtime/pkg/internal/testing/process`: binary lookup, flag
//! arguments, process start (with health polling) and stop, plus the orphan watchdog.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// `process.EnvAssetsPath`
pub const ENV_ASSETS_PATH: &str = "KUBEBUILDER_ASSETS";
/// `process.EnvAssetOverridePrefix`
pub const ENV_ASSET_OVERRIDE_PREFIX: &str = "TEST_ASSET_";
/// `process.AssetsDefaultPath`
pub const ASSETS_DEFAULT_PATH: &str = "/usr/local/kubebuilder/bin";

const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// `process.BinPathFinder(symbolicName, assetDirectory)`.
///
/// Lookup order: `TEST_ASSET_<NAME>` env var, `$KUBEBUILDER_ASSETS/<name>`,
/// `<assetDirectory>/<name>`, `/usr/local/kubebuilder/bin/<name>`.
pub fn bin_path_finder(symbolic_name: &str, asset_directory: Option<&Path>) -> PathBuf {
    let mut sanitized = String::new();
    let mut in_punct = false;
    for c in symbolic_name.to_uppercase().chars() {
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            sanitized.push(c);
            in_punct = false;
        } else if !in_punct {
            sanitized.push('_');
            in_punct = true;
        }
    }
    let sanitized = sanitized.trim_start_matches(|c: char| c.is_ascii_digit());
    let env_var = format!("{ENV_ASSET_OVERRIDE_PREFIX}{sanitized}");
    if let Some(val) = std::env::var_os(&env_var) {
        return PathBuf::from(val);
    }
    if let Some(val) = std::env::var_os(ENV_ASSETS_PATH) {
        return PathBuf::from(val).join(symbolic_name);
    }
    if let Some(dir) = asset_directory
        && !dir.as_os_str().is_empty()
    {
        return dir.join(symbolic_name);
    }
    Path::new(ASSETS_DEFAULT_PATH).join(symbolic_name)
}

/// One flag value of [`Arguments`] (`process.Arg`).
#[derive(Clone, Debug)]
enum Arg {
    /// `userArg` — values replace the defaults.
    User(Vec<String>),
    /// `defaultedArg` — values are appended to the defaults.
    Defaulted(Vec<String>),
    /// `DontPass` — the flag is not passed at all.
    DontPass,
    /// `PassAsName` — the flag is passed as `--name` without a value.
    PassAsName,
}

impl Arg {
    fn append(self, vals: &[String]) -> Arg {
        match self {
            Arg::User(mut v) => {
                v.extend_from_slice(vals);
                Arg::User(v)
            }
            Arg::Defaulted(mut v) => {
                v.extend_from_slice(vals);
                Arg::Defaulted(v)
            }
            Arg::DontPass => Arg::User(vals.to_vec()),
            Arg::PassAsName => Arg::PassAsName,
        }
    }

    /// `None` = don't pass, `Some([])` = pass as name.
    fn get(&self, defaults: &[String]) -> Option<Vec<String>> {
        match self {
            Arg::User(v) => Some(v.clone()),
            Arg::Defaulted(v) => {
                let mut res = defaults.to_vec();
                res.extend_from_slice(v);
                if res.is_empty() {
                    // Go: `append([]string(nil), nil...)` is nil -> don't pass.
                    return None;
                }
                Some(res)
            }
            Arg::DontPass => None,
            Arg::PassAsName => Some(Vec::new()),
        }
    }
}

/// `process.Arguments` — command line flags of a control plane binary, layered on top of the
/// built-in defaults.
#[derive(Clone, Debug, Default)]
pub struct Arguments {
    values: BTreeMap<String, Arg>,
}

impl Arguments {
    /// `EmptyArguments()`
    pub fn new() -> Self {
        Self::default()
    }

    /// `Set(key, values...)` — overrides the default values.
    pub fn set<S: Into<String>>(
        &mut self,
        key: &str,
        values: impl IntoIterator<Item = S>,
    ) -> &mut Self {
        self.values.insert(
            key.to_string(),
            Arg::User(values.into_iter().map(Into::into).collect()),
        );
        self
    }

    /// `Append(key, values...)` — appends to the default values.
    pub fn append<S: Into<String>>(
        &mut self,
        key: &str,
        values: impl IntoIterator<Item = S>,
    ) -> &mut Self {
        let vals: Vec<String> = values.into_iter().map(Into::into).collect();
        let cur = self
            .values
            .remove(key)
            .unwrap_or(Arg::Defaulted(Vec::new()));
        self.values.insert(key.to_string(), cur.append(&vals));
        self
    }

    /// `AppendNoDefaults(key, values...)`
    pub fn append_no_defaults<S: Into<String>>(
        &mut self,
        key: &str,
        values: impl IntoIterator<Item = S>,
    ) -> &mut Self {
        let vals: Vec<String> = values.into_iter().map(Into::into).collect();
        let cur = self.values.remove(key).unwrap_or(Arg::User(Vec::new()));
        self.values.insert(key.to_string(), cur.append(&vals));
        self
    }

    /// `Enable(key)` — pass `--key` without a value.
    pub fn enable(&mut self, key: &str) -> &mut Self {
        self.values.insert(key.to_string(), Arg::PassAsName);
        self
    }

    /// `Disable(key)` — never pass `--key`.
    pub fn disable(&mut self, key: &str) -> &mut Self {
        self.values.insert(key.to_string(), Arg::DontPass);
        self
    }

    /// `AsStrings(defaults)` — renders `--key=value` flags sorted by key.
    pub fn as_strings(&self, defaults: &BTreeMap<String, Vec<String>>) -> Vec<String> {
        let mut keys: Vec<&String> = defaults
            .keys()
            .filter(|k| !self.values.contains_key(*k))
            .chain(self.values.keys())
            .collect();
        keys.sort();
        let mut res = Vec::new();
        let empty = Vec::new();
        for key in keys {
            let def = defaults.get(key).unwrap_or(&empty);
            let arg = self
                .values
                .get(key)
                .cloned()
                .unwrap_or(Arg::Defaulted(Vec::new()));
            match arg.get(def) {
                None => continue,
                Some(vals) if vals.is_empty() => res.push(format!("--{key}")),
                Some(vals) => {
                    for v in vals {
                        res.push(format!("--{key}={v}"));
                    }
                }
            }
        }
        res
    }
}

fn exec_error(path: &Path, err: &std::io::Error) -> anyhow::Error {
    let msg = match err.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => err.to_string(),
    };
    anyhow::anyhow!("fork/exec {}: {}", path.display(), msg)
}

static FLAG_CACHE: LazyLock<Mutex<HashMap<(PathBuf, String), bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn help_has_flag(output: &str, flag: &str) -> bool {
    let needle = format!("--{flag}");
    output.lines().any(|line| {
        let line = line.trim_start();
        match line.strip_prefix(&needle) {
            Some(rest) => !rest
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'),
            None => false,
        }
    })
}

/// `State.CheckFlag(flag)` — whether `<binary> --help` mentions `--flag` (cached per binary).
pub async fn check_flag(path: &Path, flag: &str) -> anyhow::Result<bool> {
    let key = (path.to_path_buf(), flag.to_string());
    if let Ok(cache) = FLAG_CACHE.lock()
        && let Some(v) = cache.get(&key)
    {
        return Ok(*v);
    }
    let p = path.to_path_buf();
    let out = tokio::task::spawn_blocking(move || Command::new(&p).arg("--help").output())
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let err = anyhow::anyhow!("{}", o.status);
            return Err(ctrlrt::wrapf!(
                err,
                "unable to run command {} to check for flag {}: {}",
                ctrlrt::error::go_quote(&path.display().to_string()),
                ctrlrt::error::go_quote(flag),
                err
            ));
        }
        Err(e) => {
            let err = exec_error(path, &e);
            return Err(ctrlrt::wrapf!(
                err,
                "unable to run command {} to check for flag {}: {}",
                ctrlrt::error::go_quote(&path.display().to_string()),
                ctrlrt::error::go_quote(flag),
                err
            ));
        }
    };
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push('\n');
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    let found = help_has_flag(&combined, flag);
    if let Ok(mut cache) = FLAG_CACHE.lock() {
        cache.insert(key, found);
    }
    Ok(found)
}

/// Kills the control plane processes and removes their directories if this test process
/// dies without stopping the environment (e.g. libtest's `process::exit`, a panic abort,
/// SIGKILL, Ctrl-C).
///
/// It is a tiny `/bin/sh` script in its own process group that ignores INT/TERM/HUP and
/// blocks reading its stdin, a pipe whose only write end is owned by this process (Rust
/// creates pipes with `O_CLOEXEC`, so no other child inherits it). We send it `pid <n>` /
/// `dir <path>` lines while the control plane comes up. When the test process exits for any
/// reason the kernel closes the pipe, `read` hits EOF and the script stops the tracked
/// processes in reverse start order (API server before etcd, like `ControlPlane.Stop`):
/// SIGTERM, then SIGKILL after 5s; finally it `rm -rf`s the tracked directories. On a regular
/// [`crate::Environment::stop`] / `Drop` the watchdog itself is SIGKILLed after cleanup.
pub(crate) struct Watchdog {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

const WATCHDOG_SCRIPT: &str = r#"trap '' INT TERM HUP QUIT PIPE
nl='
'
pids=''
dirs=''
while IFS= read -r line; do
  case "$line" in
    "pid "*) pids="${line#pid } $pids" ;;
    "dir "*) dirs="$dirs$nl${line#dir }" ;;
  esac
done
for p in $pids; do
  kill -TERM "$p" 2>/dev/null || continue
  i=0
  while [ $i -lt 50 ] && kill -0 "$p" 2>/dev/null; do
    sleep 0.1
    i=$((i+1))
  done
  kill -KILL "$p" 2>/dev/null
done
IFS="$nl"
for d in $dirs; do
  [ -n "$d" ] && rm -rf -- "$d"
done
"#;

impl Watchdog {
    pub(crate) fn spawn() -> Self {
        let res = Command::new("/bin/sh")
            .arg("-c")
            .arg(WATCHDOG_SCRIPT)
            .arg("ctrlrt-envtest-watchdog")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        match res {
            Ok(mut child) => {
                let stdin = child.stdin.take();
                Self {
                    child: Some(child),
                    stdin,
                }
            }
            Err(err) => {
                crate::log().error(
                    &err,
                    "unable to start the envtest watchdog; control plane processes may leak if the test process dies",
                    ctrlrt::kv![],
                );
                Self {
                    child: None,
                    stdin: None,
                }
            }
        }
    }

    fn send(&mut self, line: String) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(line.as_bytes());
            let _ = stdin.flush();
        }
    }

    pub(crate) fn track_pid(&mut self, pid: u32) {
        self.send(format!("pid {pid}\n"));
    }

    pub(crate) fn track_dir(&mut self, dir: &Path) {
        if let Some(s) = dir.to_str()
            && !s.contains('\n')
        {
            self.send(format!("dir {s}\n"));
        }
    }

    /// Simulates the death of this process: closes the pipe and waits for the watchdog to
    /// finish its cleanup.
    #[cfg(test)]
    pub(crate) fn trigger_and_wait(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }

    /// Stops the watchdog without letting it act (everything was cleaned up already).
    pub(crate) fn disarm(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.disarm();
    }
}

/// `process.State` — one control plane process.
pub(crate) struct ProcessState {
    pub path: PathBuf,
    pub args: Vec<String>,
    pub dir: PathBuf,
    pub dir_needs_cleaning: bool,
    pub start_timeout: Duration,
    pub stop_timeout: Duration,
    /// Base URL (scheme://host:port) for the health check.
    pub health_base: String,
    pub health_path: String,
    /// Extra readiness path polled after the health check passed (`/readyz` for the API server).
    pub ready_path: Option<String>,
    pub attach_output: bool,
    child: Option<Child>,
}

impl ProcessState {
    /// `State.Init(name)` — defaults path, working directory and timeouts.
    pub(crate) fn init(
        name: &str,
        path: Option<PathBuf>,
        dir: Option<PathBuf>,
        start_timeout: Duration,
        stop_timeout: Duration,
        watchdog: &mut Watchdog,
    ) -> anyhow::Result<Self> {
        let path = match path {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => {
                if name.is_empty() {
                    anyhow::bail!("must have at least one of name or path");
                }
                bin_path_finder(name, None)
            }
        };
        let (dir, dir_needs_cleaning) = match dir {
            Some(d) if !d.as_os_str().is_empty() => (d, false),
            _ => {
                let d = tempfile::Builder::new()
                    .prefix("k8s_test_framework_")
                    .tempdir()?
                    .keep();
                watchdog.track_dir(&d);
                (d, true)
            }
        };
        Ok(Self {
            path,
            args: Vec::new(),
            dir,
            dir_needs_cleaning,
            start_timeout: if start_timeout.is_zero() {
                DEFAULT_START_TIMEOUT
            } else {
                start_timeout
            },
            stop_timeout: if stop_timeout.is_zero() {
                DEFAULT_STOP_TIMEOUT
            } else {
                stop_timeout
            },
            health_base: String::new(),
            health_path: String::new(),
            ready_path: None,
            attach_output: false,
            child: None,
        })
    }

    fn base_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    fn signal_term(&self) -> std::io::Result<()> {
        match &self.child {
            Some(child) => rustix::process::kill_process(
                rustix::process::Pid::from_child(child),
                rustix::process::Signal::TERM,
            )
            .map_err(std::io::Error::from),
            None => Ok(()),
        }
    }

    fn exited(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => !matches!(child.try_wait(), Ok(None)),
            None => true,
        }
    }

    /// `State.Start(stdout, stderr)` — starts the process and polls the health endpoint
    /// until it returns 200 OK, the process exits, or the start timeout elapses.
    pub(crate) async fn start(&mut self, watchdog: &mut Watchdog) -> anyhow::Result<()> {
        if self.child.is_some() {
            return Ok(());
        }
        let deadline = Instant::now() + self.start_timeout;
        let health_client = health_client(&self.health_base)?;

        let mut cmd = Command::new(&self.path);
        cmd.args(&self.args).stdin(Stdio::null()).process_group(0);
        if self.attach_output {
            cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
        let child = cmd.spawn().map_err(|e| exec_error(&self.path, &e))?;
        watchdog.track_pid(child.id());
        self.child = Some(child);

        let mut paths = vec![self.health_path.clone()];
        if let Some(p) = &self.ready_path {
            paths.push(p.clone());
        }
        for path in paths {
            loop {
                if self.exited() {
                    return Err(anyhow::anyhow!(
                        "timeout waiting for process {} to start successfully \
                         (it may have failed to start, or stopped unexpectedly before becoming ready)",
                        self.base_name()
                    ));
                }
                let now = Instant::now();
                if now >= deadline {
                    let _ = self.signal_term();
                    return Err(anyhow::anyhow!(
                        "timeout waiting for process {} to start",
                        self.base_name()
                    ));
                }
                let budget = (deadline - now).min(Duration::from_secs(2));
                if get_ok(&health_client, &path, budget).await {
                    break;
                }
                tokio::time::sleep(DEFAULT_POLL_INTERVAL).await;
            }
        }
        Ok(())
    }

    /// `State.Stop()` — SIGTERM, wait up to the stop timeout, SIGKILL; always removes the
    /// working directory if it was generated.
    pub(crate) async fn stop(&mut self) -> anyhow::Result<()> {
        let res = self.stop_process().await;
        if self.dir_needs_cleaning {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
        res
    }

    async fn stop_process(&mut self) -> anyhow::Result<()> {
        if self.child.is_none() {
            return Ok(());
        }
        if self.exited() {
            self.child = None;
            return Ok(());
        }
        if let Err(err) = self.signal_term() {
            return Err(ctrlrt::wrapf!(
                err,
                "unable to signal for process {} to stop: {}",
                self.path.display(),
                err
            ));
        }
        let deadline = Instant::now() + self.stop_timeout;
        loop {
            if self.exited() {
                self.child = None;
                return Ok(());
            }
            if Instant::now() >= deadline {
                if let Some(child) = self.child.as_mut() {
                    if let Err(err) = child.kill() {
                        return Err(ctrlrt::wrapf!(
                            err,
                            "unable to kill process {}: {}",
                            self.path.display(),
                            err
                        ));
                    }
                    let _ = child.wait();
                }
                self.child = None;
                return Err(anyhow::anyhow!(
                    "timeout waiting for process {} to stop",
                    self.base_name()
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Synchronous best-effort teardown (used from `Drop`): SIGKILL, reap, remove dir.
    pub(crate) fn kill_now(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.dir_needs_cleaning {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Drop for ProcessState {
    fn drop(&mut self) {
        self.kill_now();
    }
}

/// An unauthenticated client that skips TLS verification (Go's health poller uses
/// `InsecureSkipVerify: true`).
fn health_client(base: &str) -> anyhow::Result<kube::Client> {
    let uri: http::Uri = base.parse()?;
    let mut cfg = kube::Config::new(uri);
    cfg.accept_invalid_certs = true;
    cfg.connect_timeout = Some(Duration::from_secs(1));
    cfg.read_timeout = Some(Duration::from_secs(2));
    cfg.write_timeout = Some(Duration::from_secs(2));
    Ok(kube::Client::try_from(cfg)?)
}

async fn get_ok(client: &kube::Client, path: &str, budget: Duration) -> bool {
    let Ok(req) = http::Request::get(path).body(Vec::new()) else {
        return false;
    };
    matches!(
        tokio::time::timeout(budget, client.request_text(req)).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_render_sorted_with_defaults() {
        let mut defaults = BTreeMap::new();
        defaults.insert("b".to_string(), vec!["1".to_string()]);
        defaults.insert("a".to_string(), vec!["x".to_string()]);
        defaults.insert("c".to_string(), vec!["y".to_string()]);
        let mut args = Arguments::new();
        args.set("client-ca-file", ["/ca"])
            .disable("c")
            .append("b", ["2"])
            .enable("z");
        assert_eq!(
            args.as_strings(&defaults),
            vec!["--a=x", "--b=1", "--b=2", "--client-ca-file=/ca", "--z"]
        );
    }

    #[test]
    fn bin_path_finder_uses_asset_dir() {
        // TEST_ASSET_/KUBEBUILDER_ASSETS may be set by the caller environment; only
        // assert the fallback when they are not.
        if std::env::var_os("TEST_ASSET_KUBE_APISERVER").is_none()
            && std::env::var_os(ENV_ASSETS_PATH).is_none()
        {
            assert_eq!(
                bin_path_finder("kube-apiserver", Some(Path::new("/x"))),
                PathBuf::from("/x/kube-apiserver")
            );
            assert_eq!(
                bin_path_finder("kube-apiserver", None),
                PathBuf::from("/usr/local/kubebuilder/bin/kube-apiserver")
            );
        }
    }

    #[test]
    fn watchdog_kills_tracked_processes_and_removes_dirs_on_eof() {
        let mut wd = Watchdog::spawn();
        let dir = tempfile::Builder::new()
            .prefix("ctrlrt_envtest_wd_")
            .tempdir()
            .unwrap()
            .keep();
        std::fs::write(dir.join("f"), b"x").unwrap();
        let mut sleeper = Command::new("sleep").arg("1000").spawn().unwrap();
        wd.track_pid(sleeper.id());
        wd.track_dir(&dir);
        // Reap concurrently (in the real scenario init reaps the orphans), otherwise the
        // zombie looks alive to the watchdog until its SIGKILL grace period ends.
        let reaper = std::thread::spawn(move || sleeper.wait().unwrap());
        let started = Instant::now();
        wd.trigger_and_wait();
        let status = reaper.join().unwrap();
        assert!(!status.success());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!dir.exists());
    }

    #[test]
    fn help_flag_detection() {
        let help = "Usage:\n      --insecure-port int   blah\n  --unsafe-no-fsync  x\n";
        assert!(help_has_flag(help, "insecure-port"));
        assert!(help_has_flag(help, "unsafe-no-fsync"));
        assert!(!help_has_flag(help, "secure-port"));
    }
}
