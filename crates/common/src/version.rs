//! `DPFMajorMinorFromVersion` from `internal/common/config.go`.

use ctrlrt::Result;

/// Reduce a DPF version (`v26.4.1`, `26.4.0-f314aa17`) to `major.minor`.
pub fn dpf_major_minor_from_version(version: &str) -> Result<String> {
    let v = version.strip_prefix('v').unwrap_or(version);
    let parts: Vec<&str> = v.splitn(3, '.').collect();
    if parts.len() < 2 {
        return Err(anyhow::anyhow!(
            "DPF version {} does not contain a minor component",
            ctrlrt::error::go_quote(version)
        ));
    }
    Ok(format!("{}.{}", parts[0], parts[1]))
}
