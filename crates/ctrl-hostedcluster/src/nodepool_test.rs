//! Port of internal/controller/hostedcluster/nodepool_test.go
//! ("NodePool Builder" and "NodePool Upgrade").

use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::{Ctx, ObjectKey};
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj};

use crate::hostedcluster_test::controller_owner_ref;
use crate::*;

// ---------------------------------------------------------------------------------------------
// Describe("NodePool Builder")
// ---------------------------------------------------------------------------------------------

/// BeforeEach: `npm = &NodePoolManager{}` and the default CR.
fn before_each() -> (NodePoolManager, provisioningv1alpha1::DPFHCPProvisioner) {
    let npm = NodePoolManager::new(FakeClientBuilder::new().build());
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr.metadata.name = Some("test-provisioner".into());
    cr.metadata.namespace = Some("default".into());
    cr.spec.ocp_release_image = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi".into();
    (npm, cr)
}

// Context("Basic NodePool Fields")

#[test]
fn node_pool_builder_basic_node_pool_fields_should_set_correct_metadata() {
    let (npm, cr) = before_each();
    let np = npm.build_node_pool(&cr);
    assert_eq!(np.metadata.name.as_deref(), Some("test-provisioner"));
    assert_eq!(np.metadata.namespace.as_deref(), Some("default"));
}

#[test]
fn node_pool_builder_basic_node_pool_fields_should_set_cluster_name_to_match_hosted_cluster() {
    let (npm, cr) = before_each();
    let np = npm.build_node_pool(&cr);
    assert_eq!(np.spec.cluster_name, "test-provisioner");
}

#[test]
fn node_pool_builder_basic_node_pool_fields_should_set_replicas_to_0() {
    let (npm, cr) = before_each();
    let np = npm.build_node_pool(&cr);
    assert_eq!(np.spec.replicas, Some(0));
}

#[test]
fn node_pool_builder_basic_node_pool_fields_should_set_platform_to_none() {
    let (npm, cr) = before_each();
    let np = npm.build_node_pool(&cr);
    assert_eq!(np.spec.platform.type_, hyperv1::NONE_PLATFORM);
}

#[test]
fn node_pool_builder_basic_node_pool_fields_should_set_release_image_from_dpfhcp_provisioner_spec()
{
    let (npm, cr) = before_each();
    let np = npm.build_node_pool(&cr);
    assert_eq!(np.spec.release.image, cr.spec.ocp_release_image);
}

// Context("Management Configuration")

#[test]
fn node_pool_builder_management_configuration_should_set_upgrade_type_to_replace() {
    let (npm, cr) = before_each();
    let np = npm.build_node_pool(&cr);
    assert_eq!(
        np.spec.management.upgrade_type,
        hyperv1::UPGRADE_TYPE_REPLACE
    );
}

// ---------------------------------------------------------------------------------------------
// Describe("NodePool Upgrade")
// ---------------------------------------------------------------------------------------------

const OLD_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.18.0-multi";
const NEW_IMAGE: &str = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi";

/// BeforeEach: scheme with DPFHCPProvisioner + hypershift types.
fn upgrade_scheme() -> Scheme {
    Scheme::new()
        .register::<provisioningv1alpha1::DPFHCPProvisioner>()
        .register::<hyperv1::HostedCluster>()
        .register::<hyperv1::NodePool>()
}

fn upgrade_cr(image: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr.metadata.name = Some("test-provisioner".into());
    cr.metadata.namespace = Some("default".into());
    cr.metadata.uid = Some("test-uid".into());
    cr.spec.ocp_release_image = image.into();
    cr
}

fn existing_np() -> hyperv1::NodePool {
    let mut np = hyperv1::NodePool::default();
    np.metadata.name = Some("test-provisioner".into());
    np.metadata.namespace = Some("default".into());
    np.metadata.owner_references = Some(vec![controller_owner_ref()]);
    np.spec.release.image = OLD_IMAGE.into();
    np
}

async fn run_create_node_pool(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> hyperv1::NodePool {
    let ctx = Ctx::background();
    let c = FakeClientBuilder::new()
        .with_scheme(upgrade_scheme())
        .with_objects([obj(&existing_np())])
        .build();

    let npm = NodePoolManager::new(c.clone());
    let result = npm.create_node_pool(&ctx, cr).await.unwrap();
    assert!(result.requeue_after.is_zero());

    c.get(&ObjectKey::new("default", "test-provisioner"))
        .await
        .unwrap()
}

#[tokio::test]
async fn node_pool_upgrade_should_not_update_node_pool_release_image_when_spec_changes_upgrade_is_handled_by_handle_upgrade()
 {
    let updated = run_create_node_pool(&upgrade_cr(NEW_IMAGE)).await;
    assert_eq!(
        updated.spec.release.image, OLD_IMAGE,
        "release image should not be updated by CreateNodePool"
    );
}

#[tokio::test]
async fn node_pool_upgrade_should_not_update_node_pool_when_release_image_is_unchanged() {
    let unchanged = run_create_node_pool(&upgrade_cr(OLD_IMAGE)).await;
    assert_eq!(unchanged.spec.release.image, OLD_IMAGE);
}
