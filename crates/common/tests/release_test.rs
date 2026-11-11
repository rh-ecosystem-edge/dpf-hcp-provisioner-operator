//! Behavioural tests for internal/common/release.go (no Go test file).

use common::aarch64_release_image;

#[test]
fn aarch64_release_image_strips_tag_or_digest() {
    assert_eq!(
        aarch64_release_image("quay.io/openshift-release-dev/ocp-release:4.19.0-multi", "4.19.0"),
        "quay.io/openshift-release-dev/ocp-release:4.19.0-aarch64"
    );
    assert_eq!(
        aarch64_release_image("quay.io/openshift-release-dev/ocp-release@sha256:abc", "4.19.0"),
        "quay.io/openshift-release-dev/ocp-release:4.19.0-aarch64"
    );
    assert_eq!(
        aarch64_release_image("registry:5000/ocp/release:4.18.2", "4.18.2"),
        "registry:5000/ocp/release:4.18.2-aarch64"
    );
}

#[tokio::test]
async fn multi_suffix_short_circuits() {
    let kc = common::new_keychain_from_docker_config(b"{}").unwrap();
    assert!(common::is_multi_arch_release_image(&ctrlrt::Ctx::background(), "x/y:4.19.0-multi", kc.clone()).await);
    // unparsable reference → false without network
    assert!(!common::is_multi_arch_release_image(&ctrlrt::Ctx::background(), "UPPER/Case:tag", kc).await);
}
