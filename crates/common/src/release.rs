//! Port of `internal/common/release.go`.

use std::sync::Arc;

use ctrlrt::Ctx;
use registry::authn::Keychain;
use registry::{name, remote};

/// `IsMultiArchReleaseImage`: true when the tag ends with `-multi`, or when the registry
/// reports a manifest list / OCI index for the reference. Any lookup failure → false.
pub async fn is_multi_arch_release_image(
    _ctx: &Ctx,
    image: &str,
    keychain: Arc<dyn Keychain>,
) -> bool {
    if image.ends_with("-multi") {
        return true;
    }
    let Ok(r) = name::parse_reference(image, &[]) else {
        return false;
    };
    match remote::head(&r, &[remote::with_auth_from_keychain(keychain)]).await {
        Ok(desc) => desc.media_type.is_index(),
        Err(_) => false,
    }
}

/// `Aarch64ReleaseImage`: `<repo>:<version>-aarch64`, stripping a digest or tag.
pub fn aarch64_release_image(release_image: &str, version: &str) -> String {
    let mut registry = release_image;
    if let Some(idx) = registry.find('@').filter(|i| *i > 0) {
        registry = &registry[..idx];
    } else if let Some(idx) = registry.rfind(':').filter(|i| *i > 0) {
        registry = &registry[..idx];
    }
    format!("{registry}:{version}-aarch64")
}
