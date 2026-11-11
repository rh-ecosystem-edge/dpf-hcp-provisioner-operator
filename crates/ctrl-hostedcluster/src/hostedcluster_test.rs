//! Port of internal/controller/hostedcluster/hostedcluster_test.go
//! ("HostedCluster Builder" and "HostedCluster Upgrade").

use apis::hypershift::util::ipnet::IPNet;
use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::LocalObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use ctrlrt::{Ctx, ObjectKey};
use ctrlrt_fake::{FakeClientBuilder, Scheme, obj};

use crate::*;

// ---------------------------------------------------------------------------------------------
// Describe("HostedCluster Builder")
// ---------------------------------------------------------------------------------------------

/// BeforeEach: `hm = &HostedClusterManager{}` and the default CR (LoadBalancer mode).
fn before_each() -> (
    HostedClusterManager,
    provisioningv1alpha1::DPFHCPProvisioner,
) {
    let hm = HostedClusterManager::new(FakeClientBuilder::new().build());
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr.metadata.name = Some("test-provisioner".into());
    cr.metadata.namespace = Some("default".into());
    cr.spec.ocp_release_image = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi".into();
    cr.spec.base_domain = "example.com".into();
    cr.spec.etcd_storage_class = "ceph-rbd".into();
    cr.spec.control_plane_availability_policy = hyperv1::HIGHLY_AVAILABLE.into();
    cr.spec.virtual_ip = "192.168.1.100".into(); // Default to LoadBalancer mode for tests
    (hm, cr)
}

/// Helper function to find strategy for a specific service
fn find_service_strategy<'a>(
    strategies: &'a Option<Vec<hyperv1::ServicePublishingStrategyMapping>>,
    service: &str,
) -> Option<&'a hyperv1::ServicePublishingStrategy> {
    strategies
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|s| s.service == service)
        .map(|s| &s.service_publishing_strategy)
}

fn cidr_networking(
    service: Option<&str>,
    cluster: Option<&str>,
    machine: Option<&str>,
) -> provisioningv1alpha1::ClusterNetworkConfig {
    provisioningv1alpha1::ClusterNetworkConfig {
        service_network: service
            .map(|c| {
                vec![hyperv1::ServiceNetworkEntry {
                    cidr: IPNet::must_parse_cidr(c),
                    ..Default::default()
                }]
            })
            .unwrap_or_default(),
        cluster_network: cluster
            .map(|c| {
                vec![hyperv1::ClusterNetworkEntry {
                    cidr: IPNet::must_parse_cidr(c),
                    ..Default::default()
                }]
            })
            .unwrap_or_default(),
        machine_network: machine
            .map(|c| {
                vec![hyperv1::MachineNetworkEntry {
                    cidr: IPNet::must_parse_cidr(c),
                    ..Default::default()
                }]
            })
            .unwrap_or_default(),
        ..Default::default()
    }
}

// Context("Basic HostedCluster Fields")

#[test]
fn hosted_cluster_builder_basic_hosted_cluster_fields_should_set_correct_metadata() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.metadata.name.as_deref(), Some("test-provisioner"));
    assert_eq!(hc.metadata.namespace.as_deref(), Some("default"));
}

#[test]
fn hosted_cluster_builder_basic_hosted_cluster_fields_should_set_release_image_from_dpfhcp_provisioner_spec()
 {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.release.image, cr.spec.ocp_release_image);
}

#[test]
fn hosted_cluster_builder_basic_hosted_cluster_fields_should_reference_correct_secret_names() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.pull_secret.name, pull_secret_copy_name(&cr));
    assert_eq!(hc.spec.ssh_key.name, ssh_key_copy_name(&cr));
}

#[test]
fn hosted_cluster_builder_basic_hosted_cluster_fields_should_set_dns_base_domain() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.dns.base_domain, "example.com");
}

#[test]
fn hosted_cluster_builder_basic_hosted_cluster_fields_should_set_platform_to_none() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.platform.type_, hyperv1::NONE_PLATFORM);
}

// Context("ETCD Configuration")

#[test]
fn hosted_cluster_builder_etcd_configuration_should_configure_managed_etcd_with_persistent_volume()
{
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.etcd.management_type, hyperv1::MANAGED);
    let managed = hc.spec.etcd.managed.as_ref().expect("managed");
    assert_eq!(
        managed.storage.type_,
        hyperv1::PERSISTENT_VOLUME_ETCD_STORAGE
    );
    assert!(managed.storage.persistent_volume.is_some());
}

#[test]
fn hosted_cluster_builder_etcd_configuration_should_use_storage_class_from_dpfhcp_provisioner_spec()
{
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    let pv = hc
        .spec
        .etcd
        .managed
        .unwrap()
        .storage
        .persistent_volume
        .unwrap();
    assert_eq!(pv.storage_class_name.as_deref(), Some("ceph-rbd"));
}

#[test]
fn hosted_cluster_builder_etcd_configuration_should_set_etcd_volume_size_to_8gi() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    let pv = hc
        .spec
        .etcd
        .managed
        .unwrap()
        .storage
        .persistent_volume
        .unwrap();
    // Size.String() is the canonical quantity
    assert_eq!(
        apis::resource::canonical(&pv.size.unwrap().0).unwrap(),
        "8Gi"
    );
}

// Context("Network Configuration")

#[test]
fn hosted_cluster_builder_network_configuration_should_set_network_type_to_other() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.networking.network_type, hyperv1::OTHER);
}

#[test]
fn hosted_cluster_builder_network_configuration_should_set_default_service_network_cidr() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.networking.service_network.len(), 1);
    assert_eq!(
        hc.spec.networking.service_network[0].cidr.to_string(),
        "172.31.0.0/16"
    );
}

#[test]
fn hosted_cluster_builder_network_configuration_should_set_default_cluster_network_cidr() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.networking.cluster_network.len(), 1);
    assert_eq!(
        hc.spec.networking.cluster_network[0].cidr.to_string(),
        "10.132.0.0/14"
    );
}

#[test]
fn hosted_cluster_builder_network_configuration_should_have_empty_machine_network() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert!(hc.spec.networking.machine_network.is_empty());
}

#[test]
fn hosted_cluster_builder_network_configuration_should_not_set_allocate_node_cidrs_when_flannel_enabled_is_false()
 {
    let (hm, mut cr) = before_each();
    cr.spec.flannel_enabled = Some(false);
    let hc = hm.build_hosted_cluster(&cr, "");
    assert!(hc.spec.networking.allocate_node_cidrs.is_none());
}

#[test]
fn hosted_cluster_builder_network_configuration_should_set_allocate_node_cidrs_to_enabled_when_flannel_enabled_is_true()
 {
    let (hm, mut cr) = before_each();
    cr.spec.flannel_enabled = Some(true);
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(
        hc.spec.networking.allocate_node_cidrs.as_deref(),
        Some(hyperv1::ALLOCATE_NODE_CIDRS_ENABLED)
    );
}

#[test]
fn hosted_cluster_builder_network_configuration_should_set_allocate_node_cidrs_to_enabled_when_flannel_enabled_is_nil_default_true()
 {
    let (hm, mut cr) = before_each();
    cr.spec.flannel_enabled = None;
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(
        hc.spec.networking.allocate_node_cidrs.as_deref(),
        Some(hyperv1::ALLOCATE_NODE_CIDRS_ENABLED)
    );
}

#[test]
fn hosted_cluster_builder_network_configuration_should_keep_network_type_as_other_when_flannel_enabled_is_true()
 {
    let (hm, mut cr) = before_each();
    cr.spec.flannel_enabled = Some(true);
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.networking.network_type, hyperv1::OTHER);
}

// Context("Availability Policies")

#[test]
fn hosted_cluster_builder_availability_policies_should_set_single_replica_when_specified() {
    let (hm, mut cr) = before_each();
    cr.spec.control_plane_availability_policy = hyperv1::SINGLE_REPLICA.into();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(
        hc.spec.controller_availability_policy,
        hyperv1::SINGLE_REPLICA
    );
}

#[test]
fn hosted_cluster_builder_availability_policies_should_set_highly_available_when_specified() {
    let (hm, mut cr) = before_each();
    cr.spec.control_plane_availability_policy = hyperv1::HIGHLY_AVAILABLE.into();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(
        hc.spec.controller_availability_policy,
        hyperv1::HIGHLY_AVAILABLE
    );
}

// Context("Secret Encryption")

#[test]
fn hosted_cluster_builder_secret_encryption_should_configure_aescbc_encryption() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    let se = hc.spec.secret_encryption.expect("secretEncryption");
    assert_eq!(se.type_, hyperv1::AESCBC);
    assert!(se.aescbc.is_some());
}

#[test]
fn hosted_cluster_builder_secret_encryption_should_reference_etcd_encryption_key_secret() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(
        hc.spec
            .secret_encryption
            .unwrap()
            .aescbc
            .unwrap()
            .active_key
            .name,
        "test-provisioner-etcd-encryption-key"
    );
}

// Context("Service Publishing Strategy")

#[test]
fn hosted_cluster_builder_service_publishing_strategy_should_configure_4_services_in_load_balancer_mode()
 {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.services.as_ref().map(Vec::len), Some(4));
}

#[test]
fn hosted_cluster_builder_service_publishing_strategy_should_use_load_balancer_for_api_server() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    let s = find_service_strategy(&hc.spec.services, hyperv1::API_SERVER).expect("strategy");
    assert_eq!(s.type_, hyperv1::LOAD_BALANCER);
}

#[test]
fn hosted_cluster_builder_service_publishing_strategy_should_use_route_for_oauth_server() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    let s = find_service_strategy(&hc.spec.services, hyperv1::OAUTH_SERVER).expect("strategy");
    assert_eq!(s.type_, hyperv1::ROUTE);
}

#[test]
fn hosted_cluster_builder_service_publishing_strategy_should_use_route_for_konnectivity() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    let s = find_service_strategy(&hc.spec.services, hyperv1::KONNECTIVITY).expect("strategy");
    assert_eq!(s.type_, hyperv1::ROUTE);
}

#[test]
fn hosted_cluster_builder_service_publishing_strategy_should_use_route_for_ignition() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    let s = find_service_strategy(&hc.spec.services, hyperv1::IGNITION).expect("strategy");
    assert_eq!(s.type_, hyperv1::ROUTE);
}

// Context("Networking Configuration")

fn assert_default_networks(hc: &hyperv1::HostedCluster) {
    let n = &hc.spec.networking;
    assert_eq!(n.service_network.len(), 1);
    assert_eq!(n.service_network[0].cidr.to_string(), "172.31.0.0/16");
    assert_eq!(n.cluster_network.len(), 1);
    assert_eq!(n.cluster_network[0].cidr.to_string(), "10.132.0.0/14");
    assert!(n.machine_network.is_empty());
}

#[test]
fn hosted_cluster_builder_networking_configuration_should_use_defaults_when_networking_field_is_nil()
 {
    let (hm, mut cr) = before_each();
    cr.spec.networking = None;
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_default_networks(&hc);
}

#[test]
fn hosted_cluster_builder_networking_configuration_should_use_defaults_when_networking_is_an_empty_struct()
 {
    let (hm, mut cr) = before_each();
    cr.spec.networking = Some(Default::default());
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_default_networks(&hc);
}

#[test]
fn hosted_cluster_builder_networking_configuration_should_override_only_service_network_when_custom_service_network_provided()
 {
    let (hm, mut cr) = before_each();
    cr.spec.networking = Some(cidr_networking(Some("10.96.0.0/12"), None, None));
    let hc = hm.build_hosted_cluster(&cr, "");
    let n = &hc.spec.networking;
    assert_eq!(n.service_network.len(), 1);
    assert_eq!(n.service_network[0].cidr.to_string(), "10.96.0.0/12");
    // Others should remain defaults
    assert_eq!(n.cluster_network[0].cidr.to_string(), "10.132.0.0/14");
    assert!(n.machine_network.is_empty());
}

#[test]
fn hosted_cluster_builder_networking_configuration_should_override_only_cluster_network_when_custom_cluster_network_provided()
 {
    let (hm, mut cr) = before_each();
    cr.spec.networking = Some(cidr_networking(None, Some("10.244.0.0/16"), None));
    let hc = hm.build_hosted_cluster(&cr, "");
    let n = &hc.spec.networking;
    assert_eq!(n.cluster_network.len(), 1);
    assert_eq!(n.cluster_network[0].cidr.to_string(), "10.244.0.0/16");
    // Others should remain defaults
    assert_eq!(n.service_network[0].cidr.to_string(), "172.31.0.0/16");
    assert!(n.machine_network.is_empty());
}

#[test]
fn hosted_cluster_builder_networking_configuration_should_set_machine_network_when_provided() {
    let (hm, mut cr) = before_each();
    cr.spec.networking = Some(cidr_networking(None, None, Some("192.168.0.0/24")));
    let hc = hm.build_hosted_cluster(&cr, "");
    let n = &hc.spec.networking;
    assert_eq!(n.machine_network.len(), 1);
    assert_eq!(n.machine_network[0].cidr.to_string(), "192.168.0.0/24");
    // Others should remain defaults
    assert_eq!(n.service_network[0].cidr.to_string(), "172.31.0.0/16");
    assert_eq!(n.cluster_network[0].cidr.to_string(), "10.132.0.0/14");
}

#[test]
fn hosted_cluster_builder_networking_configuration_should_override_all_networks_when_all_provided()
{
    let (hm, mut cr) = before_each();
    cr.spec.networking = Some(cidr_networking(
        Some("10.96.0.0/12"),
        Some("10.244.0.0/16"),
        Some("192.168.0.0/24"),
    ));
    let hc = hm.build_hosted_cluster(&cr, "");
    let n = &hc.spec.networking;
    assert_eq!(n.service_network.len(), 1);
    assert_eq!(n.service_network[0].cidr.to_string(), "10.96.0.0/12");
    assert_eq!(n.cluster_network.len(), 1);
    assert_eq!(n.cluster_network[0].cidr.to_string(), "10.244.0.0/16");
    assert_eq!(n.machine_network.len(), 1);
    assert_eq!(n.machine_network[0].cidr.to_string(), "192.168.0.0/24");
}

#[test]
fn hosted_cluster_builder_networking_configuration_should_always_set_network_type_to_other_regardless_of_networking_config()
 {
    let (hm, mut cr) = before_each();
    cr.spec.networking = Some(cidr_networking(Some("10.96.0.0/12"), None, None));
    let hc = hm.build_hosted_cluster(&cr, "");
    assert_eq!(hc.spec.networking.network_type, hyperv1::OTHER);
}

// Context("InfraID Generation")

#[test]
fn hosted_cluster_builder_infra_id_generation_should_generate_non_empty_infra_id() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert!(!hc.spec.infra_id.is_empty());
}

#[test]
fn hosted_cluster_builder_infra_id_generation_should_generate_infra_id_with_cluster_name_prefix() {
    let (hm, cr) = before_each();
    let hc = hm.build_hosted_cluster(&cr, "");
    assert!(hc.spec.infra_id.starts_with("test-provisioner-"));
}

#[test]
fn hosted_cluster_builder_infra_id_generation_should_generate_infra_id_with_random_suffix() {
    let (hm, cr) = before_each();
    let hc1 = hm.build_hosted_cluster(&cr, "");
    let hc2 = hm.build_hosted_cluster(&cr, "");

    // InfraID includes random suffix, so they should be different
    assert_ne!(hc1.spec.infra_id, hc2.spec.infra_id);
    // But both should start with the cluster name
    assert!(hc1.spec.infra_id.starts_with("test-provisioner-"));
    assert!(hc2.spec.infra_id.starts_with("test-provisioner-"));
}

// ---------------------------------------------------------------------------------------------
// Describe("HostedCluster Upgrade")
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

pub(crate) fn controller_owner_ref() -> OwnerReference {
    OwnerReference {
        api_version: "provisioning.dpu.hcp.io/v1alpha1".into(),
        kind: "DPFHCPProvisioner".into(),
        name: "test-provisioner".into(),
        uid: "test-uid".into(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}

fn upgrade_cr(image: &str) -> provisioningv1alpha1::DPFHCPProvisioner {
    let mut cr = provisioningv1alpha1::DPFHCPProvisioner::default();
    cr.metadata.name = Some("test-provisioner".into());
    cr.metadata.namespace = Some("default".into());
    cr.metadata.uid = Some("test-uid".into());
    cr.spec.ocp_release_image = image.into();
    cr
}

fn existing_hc(cr: &provisioningv1alpha1::DPFHCPProvisioner) -> hyperv1::HostedCluster {
    let mut hc = hyperv1::HostedCluster::default();
    hc.metadata.name = Some("test-provisioner".into());
    hc.metadata.namespace = Some("default".into());
    hc.metadata.owner_references = Some(vec![controller_owner_ref()]);
    hc.spec.release.image = OLD_IMAGE.into();
    hc.spec.pull_secret = LocalObjectReference {
        name: pull_secret_copy_name(cr),
        ..Default::default()
    };
    hc.spec.ssh_key = LocalObjectReference {
        name: ssh_key_copy_name(cr),
        ..Default::default()
    };
    hc
}

#[tokio::test]
async fn hosted_cluster_upgrade_should_not_update_hosted_cluster_release_image_when_spec_changes_upgrade_is_handled_by_handle_upgrade()
 {
    let ctx = Ctx::background();
    let mut cr = upgrade_cr(NEW_IMAGE);
    cr.spec.pull_secret_ref = LocalObjectReference {
        name: "test-pull-secret".into(),
        ..Default::default()
    };
    cr.spec.ssh_key_secret_ref = LocalObjectReference {
        name: "test-ssh-key".into(),
        ..Default::default()
    };
    let hc = existing_hc(&cr);

    let c = FakeClientBuilder::new()
        .with_scheme(upgrade_scheme())
        .with_objects([obj(&hc)])
        .build();

    let hm = HostedClusterManager::new(c.clone());
    let result = hm.create_or_update_hosted_cluster(&ctx, &cr).await.unwrap();
    assert!(result.requeue_after.is_zero());

    let updated: hyperv1::HostedCluster = c
        .get(&ObjectKey::new("default", "test-provisioner"))
        .await
        .unwrap();
    assert_eq!(
        updated.spec.release.image, OLD_IMAGE,
        "release image should not be updated by CreateOrUpdateHostedCluster"
    );
}

#[tokio::test]
async fn hosted_cluster_upgrade_should_not_update_hosted_cluster_when_release_image_is_unchanged() {
    let ctx = Ctx::background();
    let cr = upgrade_cr(OLD_IMAGE);
    let hc = existing_hc(&cr);

    let c = FakeClientBuilder::new()
        .with_scheme(upgrade_scheme())
        .with_objects([obj(&hc)])
        .build();

    let hm = HostedClusterManager::new(c.clone());
    let result = hm.create_or_update_hosted_cluster(&ctx, &cr).await.unwrap();
    assert!(
        result.requeue_after.is_zero(),
        "should not requeue when no change"
    );
}
