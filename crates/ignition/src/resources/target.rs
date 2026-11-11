//! `internal/ignition/resources/target`.

use std::borrow::Cow;

use crate::content::{ContentSource, EmbeddedProvider, FileDefinition, LinkDefinition, embed_file};

fn bytes(name: &str) -> &'static [u8] {
    embed_file(&super::TARGET_FILES, name)
}

fn f(name: &str) -> ContentSource {
    ContentSource::Bytes(Cow::Borrowed(bytes(name)))
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
    let mut p = EmbeddedProvider {
        files: vec![
            file("/etc/mellanox/mlnx-bf.conf", 0o644, f("mlnx-bf.conf")),
            file("/etc/mellanox/mlnx-ovs.conf", 0o644, f("mlnx-ovs.conf")),
            file(
                "/etc/NetworkManager/system-connections/pf0vf0.nmconnection",
                0o600,
                data(format!(
                    "data:text/plain,{}",
                    gocompat::url::query_escape(bytes("pf0vf0.nmconnection"))
                )),
            ),
            file(
                "/etc/NetworkManager/system-connections/br-comm-ch.nmconnection",
                0o600,
                f("br-comm-ch.nmconnection"),
            ),
            file(
                "/etc/crio/crio.conf.d/99-ulimits.conf",
                0o644,
                f("99-ulimits.conf"),
            ),
            file("/etc/sysctl.d/98-dpunet.conf", 0o644, f("98-dpunet.conf")),
            file(
                "/etc/systemd/system/chrony-wait.service.d/override.conf",
                0o644,
                f("10-chrony-wait-timeout.conf"),
            ),
            file(
                "/etc/systemd/system/NetworkManager-wait-online.service.d/override.conf",
                0o644,
                f("10-nm-wait-online-unstrict.conf"),
            ),
            file(
                "/etc/yum.repos.d/agentrepo.repo",
                0o644,
                f("agentrepo.repo"),
            ),
            file(
                "/etc/systemd/system/machine-config-daemon-firstboot.service.d/10-mcd-firstboot-dpuagent.conf",
                0o644,
                f("10-mcd-firstboot-dpuagent.conf"),
            ),
            file(
                "/etc/systemd/system/machine-config-daemon-pull.service.d/10-require-setup-vfs.conf",
                0o644,
                f("10-require-setup-vfs.conf"),
            ),
            file(
                "/usr/local/bin/install-dpu-agent.sh",
                0o755,
                f("install-dpu-agent.sh"),
            ),
            file(
                "/usr/local/bin/dpu-fw-upgrade.sh",
                0o755,
                f("dpu-fw-upgrade.sh"),
            ),
            file(
                "/usr/local/bin/wait-for-sfs.sh",
                0o755,
                f("wait-for-sfs.sh"),
            ),
            file(
                "/usr/local/bin/tmfifo-agent-link.sh",
                0o755,
                f("tmfifo-agent-link.sh"),
            ),
            file(
                "/usr/local/bin/report-machineosurl.py",
                0o755,
                f("report-machineosurl.py"),
            ),
            file(
                "/etc/modules-load.d/br_netfilter.conf",
                0o644,
                data("data:,br_netfilter"),
            ),
            file("/etc/sysconfig/openvswitch", 0o600, f("openvswitch")),
            file(
                "/etc/tmpfiles.d/99-hugetlbfs-dpf.conf",
                0o644,
                f("tmpfiles-hugetlbfs-dpf.conf"),
            ),
            file(
                "/etc/openshift/kubelet.conf.d/kubelet-dpf-override.conf",
                0o644,
                f("kubelet-dpf-override.conf"),
            ),
            file("/usr/local/bin/pf-monitor.sh", 0o755, f("pf-monitor.sh")),
        ],
        links: vec![LinkDefinition {
            path: "/etc/localtime".into(),
            target: "/usr/share/zoneinfo/UTC".into(),
        }],
        systemd_fs: Some(&super::TARGET_SYSTEMD),
        skip_units: Vec::new(),
    };
    if zero_trust {
        p.skip_units = vec![
            "pf-monitor.service".into(),
            "report-machineosurl.service".into(),
        ];
        p.files.retain(|f| {
            f.path != "/usr/local/bin/pf-monitor.sh"
                && f.path != "/usr/local/bin/report-machineosurl.py"
                && f.path != "/etc/yum.repos.d/agentrepo.repo"
        });
    }
    p
}

/// `RenderDPUAgentServiceUnit`: renders `files/dpu-agent.service.tmpl` with
/// `{ZeroTrust: zero_trust}`; returns the unit name and contents.
pub fn render_dpu_agent_service_unit(zero_trust: bool) -> anyhow::Result<(String, String)> {
    let tmpl = String::from_utf8_lossy(bytes("dpu-agent.service.tmpl")).into_owned();
    let t = crate::template::parse(&tmpl)
        .map_err(|e| anyhow::anyhow!("failed to parse dpu-agent.service template: {}", e))?;
    let out = t
        .execute(&|field| match field {
            "ZeroTrust" => Some(zero_trust),
            _ => None,
        })
        .map_err(|e| anyhow::anyhow!("failed to render dpu-agent.service template: {}", e))?;
    Ok(("dpu-agent.service".to_string(), out))
}
