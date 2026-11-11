//! Port of `internal/common/ocp.go`.

use ctrlrt::Result;

/// `ExtractOCPVersion` extracts the OCP version from a release image reference by taking the
/// tag (text after the last `:`) and stripping architecture suffixes.
pub fn extract_ocp_version(ocp_release_image: &str) -> Result<String> {
    let parts: Vec<&str> = ocp_release_image.split(':').collect();
    if parts.len() < 2 {
        return Err(anyhow::anyhow!("missing tag separator ':' in image URL"));
    }
    let tag = parts[parts.len() - 1];
    if tag.is_empty() {
        return Err(anyhow::anyhow!("empty tag in image URL"));
    }
    const SUFFIXES: [&str; 6] = [
        "-multi", "-amd64", "-arm64", "-ppc64le", "-s390x", "-x86_64",
    ];
    let mut version = tag;
    for suffix in SUFFIXES {
        version = version.strip_suffix(suffix).unwrap_or(version);
    }
    if version.is_empty() {
        return Err(anyhow::anyhow!(
            "extracted version is empty after processing tag: {tag}"
        ));
    }
    Ok(version.to_string())
}
