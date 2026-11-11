//! Port of `test/utils/utils.go`.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use ctrlrt::error::go_quote;

use crate::gw;

const PROMETHEUS_OPERATOR_VERSION: &str = "v0.77.1";
const PROMETHEUS_OPERATOR_URL: &str =
    "https://github.com/prometheus-operator/prometheus-operator/releases/download/%s/bundle.yaml";

const CERTMANAGER_VERSION: &str = "v1.16.3";
const CERTMANAGER_URL_TMPL: &str =
    "https://github.com/cert-manager/cert-manager/releases/download/%s/cert-manager.yaml";

fn warn_error(err: &anyhow::Error) {
    gw!("warning: {}\n", err);
}

/// Go's `syscall.Errno` text for common I/O errors.
fn io_error_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        std::io::ErrorKind::IsADirectory => "is a directory".into(),
        std::io::ErrorKind::NotADirectory => "not a directory".into(),
        _ => {
            let s = e.to_string();
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

/// `*fs.PathError` text: `<op> <path>: <errno>`.
fn path_error(op: &str, path: &str, e: &std::io::Error) -> String {
    format!("{op} {path}: {}", io_error_text(e))
}

/// `exec.ExitError.Error()` / `exec.Error.Error()` text.
fn exec_error_text(program: &str, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound && !program.contains('/') {
        format!(
            "exec: {}: executable file not found in $PATH",
            go_quote(program)
        )
    } else {
        format!("fork/exec {program}: {}", io_error_text(e))
    }
}

fn exit_status_text(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    match status.signal() {
        Some(sig) => {
            let name = match sig {
                1 => "hangup",
                2 => "interrupt",
                3 => "quit",
                6 => "aborted",
                9 => "killed",
                11 => "segmentation fault",
                13 => "broken pipe",
                15 => "terminated",
                n => return format!("signal: signal {n}"),
            };
            format!("signal: {name}")
        }
        None => "exit status unknown".into(),
    }
}

/// `Run(cmd)`: runs the command from the project directory (also `chdir`ing the process there,
/// like Go), with `GO111MODULE=on` added to the environment, and returns the combined
/// stdout+stderr. On failure the error reads
/// `"<command>" failed with error "<output>": <cause>`.
pub fn run(mut cmd: Command) -> anyhow::Result<String> {
    let dir = get_project_dir().unwrap_or_else(|(wd, _)| wd);
    cmd.current_dir(&dir);

    if let Err(e) = std::env::set_current_dir(&dir) {
        gw!("chdir dir: {}\n", go_quote(&path_error("chdir", &dir, &e)));
    }

    cmd.env("GO111MODULE", "on");
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut args = vec![program.clone()];
    args.extend(cmd.get_args().map(|a| a.to_string_lossy().into_owned()));
    let command = args.join(" ");
    gw!("running: {}\n", go_quote(&command));

    let (output, err) = combined_output(cmd, &program);
    match err {
        None => Ok(output),
        Some(cause) => Err(anyhow::anyhow!(
            "{} failed with error {}: {}",
            go_quote(&command),
            go_quote(&output),
            cause
        )),
    }
}

/// `cmd.CombinedOutput()`: stdout and stderr share one pipe; stdin is `/dev/null`.
fn combined_output(mut cmd: Command, program: &str) -> (String, Option<String>) {
    let (mut reader, writer) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return (String::new(), Some(format!("pipe: {}", io_error_text(&e)))),
    };
    let writer2 = match writer.try_clone() {
        Ok(w) => w,
        Err(e) => return (String::new(), Some(format!("pipe: {}", io_error_text(&e)))),
    };
    cmd.stdin(Stdio::null()).stdout(writer).stderr(writer2);
    let spawned = cmd.spawn();
    // Close the parent's copies of the write end so reading sees EOF when the child exits.
    drop(cmd);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return (String::new(), Some(exec_error_text(program, &e))),
    };
    let mut buf = Vec::new();
    let read_err = reader.read_to_end(&mut buf).err();
    let status = child.wait();
    let output = String::from_utf8_lossy(&buf).into_owned();
    match status {
        Err(e) => (output, Some(io_error_text(&e))),
        Ok(s) if !s.success() => (output, Some(exit_status_text(s))),
        Ok(_) => match read_err {
            Some(e) => (output, Some(io_error_text(&e))),
            None => (output, None),
        },
    }
}

/// `InstallPrometheusOperator()`
pub fn install_prometheus_operator() -> anyhow::Result<()> {
    let url = PROMETHEUS_OPERATOR_URL.replace("%s", PROMETHEUS_OPERATOR_VERSION);
    let mut cmd = Command::new("kubectl");
    cmd.args(["create", "-f", &url]);
    run(cmd).map(|_| ())
}

/// `UninstallPrometheusOperator()`
pub fn uninstall_prometheus_operator() {
    let url = PROMETHEUS_OPERATOR_URL.replace("%s", PROMETHEUS_OPERATOR_VERSION);
    let mut cmd = Command::new("kubectl");
    cmd.args(["delete", "-f", &url]);
    if let Err(e) = run(cmd) {
        warn_error(&e);
    }
}

fn any_line_contains(output: &str, wanted: &[&str]) -> bool {
    let lines = get_non_empty_lines(output);
    wanted
        .iter()
        .any(|crd| lines.iter().any(|line| line.contains(crd)))
}

/// `IsPrometheusCRDsInstalled()`
pub fn is_prometheus_crds_installed() -> bool {
    let prometheus_crds = [
        "prometheuses.monitoring.coreos.com",
        "prometheusrules.monitoring.coreos.com",
        "prometheusagents.monitoring.coreos.com",
    ];
    let mut cmd = Command::new("kubectl");
    cmd.args(["get", "crds", "-o", "custom-columns=NAME:.metadata.name"]);
    match run(cmd) {
        Ok(output) => any_line_contains(&output, &prometheus_crds),
        Err(_) => false,
    }
}

/// `UninstallCertManager()`
pub fn uninstall_cert_manager() {
    let url = CERTMANAGER_URL_TMPL.replace("%s", CERTMANAGER_VERSION);
    let mut cmd = Command::new("kubectl");
    cmd.args(["delete", "-f", &url]);
    if let Err(e) = run(cmd) {
        warn_error(&e);
    }
}

/// `InstallCertManager()`
pub fn install_cert_manager() -> anyhow::Result<()> {
    let url = CERTMANAGER_URL_TMPL.replace("%s", CERTMANAGER_VERSION);
    let mut cmd = Command::new("kubectl");
    cmd.args(["apply", "-f", &url]);
    run(cmd)?;
    // Wait for cert-manager-webhook to be ready, which can take time if cert-manager
    // was re-installed after uninstalling on a cluster.
    let mut cmd = Command::new("kubectl");
    cmd.args([
        "wait",
        "deployment.apps/cert-manager-webhook",
        "--for",
        "condition=Available",
        "--namespace",
        "cert-manager",
        "--timeout",
        "5m",
    ]);
    run(cmd).map(|_| ())
}

/// `IsCertManagerCRDsInstalled()`
pub fn is_cert_manager_crds_installed() -> bool {
    let cert_manager_crds = [
        "certificates.cert-manager.io",
        "issuers.cert-manager.io",
        "clusterissuers.cert-manager.io",
        "certificaterequests.cert-manager.io",
        "orders.acme.cert-manager.io",
        "challenges.acme.cert-manager.io",
    ];
    let mut cmd = Command::new("kubectl");
    cmd.args(["get", "crds"]);
    match run(cmd) {
        Ok(output) => any_line_contains(&output, &cert_manager_crds),
        Err(_) => false,
    }
}

/// `LoadImageToKindClusterWithName(name)`
pub fn load_image_to_kind_cluster_with_name(name: &str) -> anyhow::Result<()> {
    let cluster = match std::env::var_os("KIND_CLUSTER") {
        Some(v) => v.to_string_lossy().into_owned(),
        None => "kind".to_string(),
    };
    let mut cmd = Command::new("kind");
    cmd.args(["load", "docker-image", name, "--name", &cluster]);
    run(cmd).map(|_| ())
}

/// `GetNonEmptyLines(output)`: split on `\n`, dropping empty elements.
pub fn get_non_empty_lines(output: &str) -> Vec<String> {
    output
        .split('\n')
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect()
}

/// `GetProjectDir()`: the working directory with the crate path (`/crates/e2e`, the Rust
/// counterpart of Go's `/test/e2e`) removed. On error returns the (possibly empty) working
/// directory together with the error, like Go.
pub fn get_project_dir() -> Result<String, (String, anyhow::Error)> {
    match std::env::current_dir() {
        Ok(wd) => Ok(project_dir_from(&wd.to_string_lossy())),
        Err(e) => Err((
            String::new(),
            anyhow::anyhow!(
                "failed to get current working directory: {}",
                path_error("getwd", ".", &e)
            ),
        )),
    }
}

/// The pure part of [`get_project_dir`].
pub fn project_dir_from(wd: &str) -> String {
    wd.replace("/crates/e2e", "")
}

/// `bufio.ScanLines` tokens of `s`: lines without `\n` (and a trailing `\r` dropped), no
/// final empty token.
fn scan_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = s.split('\n').collect();
    if s.ends_with('\n') {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// `UncommentCode(filename, target, prefix)`: searches for target in the file and removes the
/// comment prefix of the target content. The target content may span multiple lines.
pub fn uncomment_code(filename: &str, target: &str, prefix: &str) -> anyhow::Result<()> {
    let content = std::fs::read(Path::new(filename)).map_err(|e| {
        anyhow::anyhow!(
            "failed to read file {}: {}",
            go_quote(filename),
            path_error("open", filename, &e)
        )
    })?;
    let out = match uncomment_bytes(&content, target, prefix)? {
        Some(out) => out,
        None => return Ok(()),
    };
    std::fs::write(Path::new(filename), out).map_err(|e| {
        anyhow::anyhow!(
            "failed to write file {}: {}",
            go_quote(filename),
            path_error("open", filename, &e)
        )
    })
}

/// The pure part of [`uncomment_code`]: `None` when `target` has no lines (Go returns early
/// without rewriting the file).
pub fn uncomment_bytes(
    content: &[u8],
    target: &str,
    prefix: &str,
) -> anyhow::Result<Option<Vec<u8>>> {
    let idx = find_bytes(content, target.as_bytes()).ok_or_else(|| {
        anyhow::anyhow!(
            "unable to find the code {} to be uncomment",
            go_quote(target)
        )
    })?;
    let lines = scan_lines(target);
    if lines.is_empty() {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(content.len());
    out.extend_from_slice(&content[..idx]);
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(line.strip_prefix(prefix).unwrap_or(line).as_bytes());
    }
    out.extend_from_slice(&content[idx + target.len()..]);
    Ok(Some(out))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_empty_lines() {
        assert_eq!(get_non_empty_lines("a\n\nb\n"), vec!["a", "b"]);
        assert!(get_non_empty_lines("").is_empty());
        assert_eq!(get_non_empty_lines("NAME\nx.y\r\n"), vec!["NAME", "x.y\r"]);
    }

    #[test]
    fn project_dir() {
        assert_eq!(project_dir_from("/src/repo/crates/e2e"), "/src/repo");
        assert_eq!(project_dir_from("/src/repo"), "/src/repo");
    }

    #[test]
    fn crd_detection() {
        let out = "NAME\nprometheuses.monitoring.coreos.com\nfoo.bar\n";
        assert!(any_line_contains(
            out,
            &["prometheuses.monitoring.coreos.com"]
        ));
        assert!(!any_line_contains(out, &["issuers.cert-manager.io"]));
    }

    #[test]
    fn uncomment() {
        let content = b"a\n#- ../prometheus\n#- ../certmanager\nb\n";
        let out = uncomment_bytes(content, "#- ../prometheus\n#- ../certmanager", "#")
            .unwrap()
            .unwrap();
        assert_eq!(out, b"a\n- ../prometheus\n- ../certmanager\nb\n");

        // A trailing newline in target is consumed (bufio.Scanner yields no final empty line).
        let out = uncomment_bytes(b"x#y\nz", "#y\n", "#").unwrap().unwrap();
        assert_eq!(out, b"xyz");

        // \r is dropped by ScanLines.
        let out = uncomment_bytes(b"#a\r\n#b", "#a\r\n#b", "#")
            .unwrap()
            .unwrap();
        assert_eq!(out, b"a\nb");

        let err = uncomment_bytes(b"abc", "zzz", "#").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unable to find the code \"zzz\" to be uncomment"
        );

        assert_eq!(uncomment_bytes(b"abc", "", "#").unwrap(), None);
    }

    #[test]
    fn uncomment_code_on_file() {
        let dir = std::env::temp_dir().join(format!("e2e-utils-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("kustomization.yaml");
        std::fs::write(&f, "#- ../prometheus\n").unwrap();
        uncomment_code(f.to_str().unwrap(), "#- ../prometheus", "#").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "- ../prometheus\n");
        let missing = dir.join("missing");
        let err = uncomment_code(missing.to_str().unwrap(), "x", "#").unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "failed to read file \"{0}\": open {0}: no such file or directory",
                missing.display()
            )
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_combines_output_and_formats_errors() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out; echo err 1>&2"]);
        let out = run(cmd).unwrap();
        assert_eq!(out, "out\nerr\n");

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo bad; exit 3"]);
        let err = run(cmd).unwrap_err();
        assert_eq!(
            err.to_string(),
            "\"sh -c echo bad; exit 3\" failed with error \"bad\\n\": exit status 3"
        );

        let cmd = Command::new("definitely-not-a-command-e2e");
        let err = run(cmd).unwrap_err();
        assert_eq!(
            err.to_string(),
            "\"definitely-not-a-command-e2e\" failed with error \"\": exec: \"definitely-not-a-command-e2e\": executable file not found in $PATH"
        );

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf \"$GO111MODULE\""]);
        assert_eq!(run(cmd).unwrap(), "on");
    }
}
