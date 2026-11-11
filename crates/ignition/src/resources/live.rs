//! `internal/ignition/resources/live`.

use std::borrow::Cow;

use crate::content::{ContentSource, EmbeddedProvider, FileDefinition, embed_file};

/// URL-encoded newline for data URIs.
const NL: &str = "%0A";

fn f(name: &str) -> ContentSource {
    ContentSource::Bytes(Cow::Borrowed(embed_file(&super::LIVE_FILES, name)))
}

fn file(path: &str, mode: i64, content_source: ContentSource) -> FileDefinition {
    FileDefinition {
        path: path.to_string(),
        mode,
        content_source,
    }
}

fn data(s: impl Into<String>) -> ContentSource {
    ContentSource::DataUri(s.into())
}

/// `NewProvider`.
pub fn new_provider(zero_trust: bool) -> EmbeddedProvider {
    let dpu_mode = if zero_trust { "zero-trust" } else { "trusted" };
    let mut files = vec![
        // Trick DPF checking for the existence of these strings in /etc/bf.env
        file(
            "/etc/temp_bfcfg_strings.env",
            0o644,
            data("data:,bfb_pre_install%20bfb_modify_os%20bfb_post_install"),
        ),
        file("/etc/hostname", 0o644, data("data:,{{.DPUHostName}}")),
        file(
            "/etc/dpf/environment",
            0o644,
            data(format!(
                "data:,DPUName={{{{.DPUName}}}}{NL}DPUNamespace={{{{.DPUNamespace}}}}{NL}DPUUID={{{{.DPUUID}}}}{NL}DPUMode={dpu_mode}{NL}ControlPlaneMTU={{{{.ControlPlaneMTU}}}}{NL}"
            )),
        ),
        file(
            "/usr/local/bin/install-rhcos-dpf.sh",
            0o755,
            f("install-rhcos-dpf.sh"),
        ),
        file(
            "/usr/local/bin/update_ignition.py",
            0o755,
            f("update_ignition.py"),
        ),
    ];
    if zero_trust {
        files.push(file(
            "/var/lib/dpf/dpuagent/bootstrap-kubeconfig",
            0o600,
            data("data:;base64,{{ .BootstrapKubeconfig | b64enc }}"),
        ));
    }
    EmbeddedProvider {
        files,
        links: Vec::new(),
        systemd_fs: Some(&super::LIVE_SYSTEMD),
        skip_units: Vec::new(),
    }
}
