//! `internal/ignition/resources/common`.

use std::borrow::Cow;

use crate::content::{ContentSource, EmbeddedProvider, FileDefinition, embed_file};

fn f(name: &str) -> ContentSource {
    ContentSource::Bytes(Cow::Borrowed(embed_file(&super::COMMON_FILES, name)))
}

fn file(path: &str, mode: i64, content_source: ContentSource) -> FileDefinition {
    FileDefinition {
        path: path.to_string(),
        mode,
        content_source,
    }
}

/// `NewProvider`.
pub fn new_provider(is_zero_trust: bool) -> EmbeddedProvider {
    let oob_net_file = if is_zero_trust {
        "oob_net0_zt.nmconnection"
    } else {
        "oob_net0.nmconnection"
    };
    EmbeddedProvider {
        files: vec![
            file(
                "/etc/systemd/network/10-tmfifo_net.link",
                0o644,
                f("10-tmfifo_net.link"),
            ),
            file(
                "/etc/systemd/network/10-oob_net.link",
                0o644,
                f("10-oob_net.link"),
            ),
            file(
                "/etc/NetworkManager/system-connections/tmfifo_net0.nmconnection",
                0o600,
                f("tmfifo_net0.nmconnection"),
            ),
            file(
                "/etc/NetworkManager/system-connections/oob_net0.nmconnection",
                0o600,
                f(oob_net_file),
            ),
            file(
                "/etc/NetworkManager/conf.d/10-oob-net0.conf",
                0o644,
                f("10-oob-net0.conf"),
            ),
            file(
                "/usr/local/bin/dpuagent-client.py",
                0o755,
                f("dpuagent-client.py"),
            ),
            file("/usr/local/bin/bflog.sh", 0o755, f("bflog.sh")),
            file("/usr/local/bin/bfupsignal.sh", 0o755, f("bfupsignal.sh")),
            file(
                "/usr/local/bin/setup-vfs-devlink.sh",
                0o755,
                f("setup-vfs-devlink.sh"),
            ),
        ],
        links: Vec::new(),
        systemd_fs: Some(&super::COMMON_SYSTEMD),
        skip_units: Vec::new(),
    }
}
