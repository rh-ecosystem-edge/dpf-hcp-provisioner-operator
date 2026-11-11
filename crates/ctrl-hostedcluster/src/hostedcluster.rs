//! Port of internal/controller/hostedcluster/hostedcluster.go

use std::collections::BTreeMap;
use std::time::Duration;

use apis::hypershift::support::infraid;
use apis::hypershift::util::ipnet::IPNet;
use apis::hypershift::v1beta1 as hyperv1;
use apis::k8s::core::v1::LocalObjectReference;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::api::core::v1::Node;
use ctrlrt::k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::meta;
use ctrlrt::{Client, Ctx, Object, ObjectKey, ReconcileResult, kv, wrapf};

use crate::secrets::{pull_secret_copy_name, ssh_key_copy_name};
use crate::servicepublishing::build_service_publishing_strategy;

/// HostedClusterManager manages HostedCluster resources
#[derive(Clone)]
pub struct HostedClusterManager {
    client: Client,
}

fn local_ref(name: impl Into<String>) -> LocalObjectReference {
    LocalObjectReference {
        name: name.into(),
        ..Default::default()
    }
}

impl HostedClusterManager {
    /// NewHostedClusterManager creates a new HostedClusterManager
    pub fn new(c: Client) -> Self {
        Self { client: c }
    }

    /// The underlying client (Go embeds `client.Client`).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// CreateOrUpdateHostedCluster creates or updates the HostedCluster resource
    /// Returns ctrl.Result and error for reconciliation flow
    ///
    /// This function:
    /// - Checks if HostedCluster already exists with matching OwnerReference (idempotency)
    /// - Creates new HostedCluster if it doesn't exist
    /// - Handles name conflicts (HC exists with different owner)
    /// - Uses infraid.New() for consistent infraID generation
    pub async fn create_or_update_hosted_cluster(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        let hc_name = cr.name();
        let hc_namespace = cr.namespace();

        // Check if HostedCluster already exists
        let hc_key = ObjectKey::new(hc_namespace, hc_name);
        match self.client.get::<hyperv1::HostedCluster>(&hc_key).await {
            Ok(mut existing_hc) => {
                // HostedCluster exists - verify ownership via OwnerReference
                if meta::is_controlled_by(&existing_hc, cr) {
                    // Check if secret references need to be updated (user rotated credentials by
                    // pointing PullSecretRef/SSHKeySecretRef to a new secret). A name change causes
                    // HyperShift to create a new ignition-server token with the correct hash.
                    let expected_pull_secret_name = pull_secret_copy_name(cr);
                    let expected_ssh_key_name = ssh_key_copy_name(cr);
                    if existing_hc.spec.pull_secret.name != expected_pull_secret_name
                        || existing_hc.spec.ssh_key.name != expected_ssh_key_name
                    {
                        log.info(
                            "Updating HostedCluster secret references",
                            kv![
                                "hostedCluster" => hc_name,
                                "pullSecret" => expected_pull_secret_name,
                                "sshKey" => expected_ssh_key_name
                            ],
                        );
                        existing_hc.spec.pull_secret.name = expected_pull_secret_name;
                        existing_hc.spec.ssh_key.name = expected_ssh_key_name;
                        if let Err(err) = self.client.update(&mut existing_hc).await {
                            return Err(wrapf!(err, "failed to update HostedCluster: {}", err));
                        }
                        // Requeue to let HyperShift sync the new pull secret into its ignition
                        // server before we attempt to download ignition. Without this delay the
                        // operator downloads ignition in the same reconcile as the HC spec update
                        // and gets stale content because HyperShift hasn't propagated the new
                        // secret yet.
                        return Ok(ReconcileResult::requeue_after(Duration::from_secs(5)));
                    }
                    log.v(1).info(
                        "HostedCluster already exists and is owned by this DPFHCPProvisioner, no update needed",
                        kv!["hostedCluster" => hc_name, "namespace" => hc_namespace],
                    );
                    return Ok(ReconcileResult::default());
                }

                // Name conflict - HC exists but owned by different DPFHCPProvisioner
                return Err(anyhow::anyhow!(
                    "hostedCluster {} exists in {} but is owned by different DPFHCPProvisioner",
                    hc_name,
                    hc_namespace
                ));
            }
            Err(err) => {
                if !ctrlrt::error::is_not_found(&err) {
                    return Err(wrapf!(
                        err,
                        "failed to check existing HostedCluster: {}",
                        err
                    ));
                }
            }
        }

        // HostedCluster doesn't exist - create it
        let expose_through_lb = cr.should_expose_through_load_balancer();
        log.info(
            "Creating HostedCluster",
            kv![
                "hostedCluster" => hc_name,
                "namespace" => hc_namespace,
                "releaseImage" => cr.spec.ocp_release_image,
                "exposeThroughLoadBalancer" => expose_through_lb
            ],
        );

        // Detect node address if using NodePort mode
        let mut node_address = String::new();
        if !expose_through_lb {
            log.v(1)
                .info("Detecting node address for NodePort mode", kv![]);
            match detect_node_address(ctx, &self.client).await {
                Ok(addr) => node_address = addr,
                Err(err) => {
                    log.error(&err, "Failed to detect node address", kv![]);
                    return Err(wrapf!(err, "failed to detect node address: {}", err));
                }
            }
            log.info("Detected node address", kv!["address" => node_address]);
        }

        let mut hc = self.build_hosted_cluster(cr, &node_address);

        // Set owner reference for automatic garbage collection
        if let Err(err) = meta::set_controller_reference(cr, &mut hc) {
            log.error(
                &err,
                "Failed to set owner reference on HostedCluster",
                kv![],
            );
            return Err(wrapf!(
                err,
                "failed to set owner reference on HostedCluster: {}",
                err
            ));
        }

        if let Err(err) = self.client.create(&mut hc).await {
            log.error(
                &err,
                "Failed to create HostedCluster",
                kv!["hostedCluster" => hc_name, "namespace" => hc_namespace],
            );
            return Err(wrapf!(err, "failed to create HostedCluster: {}", err));
        }

        log.info(
            "HostedCluster created successfully",
            kv!["hostedCluster" => hc_name, "namespace" => hc_namespace],
        );

        Ok(ReconcileResult::default())
    }

    /// buildHostedCluster constructs the HostedCluster spec from DPFHCPProvisioner fields
    /// nodeAddress is only used when exposeThroughLoadBalancer=false (NodePort mode)
    pub fn build_hosted_cluster(
        &self,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
        node_address: &str,
    ) -> hyperv1::HostedCluster {
        // Build etcd storage spec
        // Only set StorageClassName if explicitly provided (matches HyperShift CLI behavior)
        // If not set, Kubernetes will use the default StorageClass
        let mut etcd_storage = hyperv1::PersistentVolumeEtcdStorageSpec {
            size: Some(Quantity("8Gi".to_string())),
            ..Default::default()
        };
        if !cr.spec.etcd_storage_class.is_empty() {
            etcd_storage.storage_class_name = Some(cr.spec.etcd_storage_class.clone());
        }

        hyperv1::HostedCluster {
            metadata: ObjectMeta {
                name: Some(cr.name().to_string()),
                namespace: Some(cr.namespace().to_string()),
                ..Default::default()
            },
            spec: hyperv1::HostedClusterSpec {
                // Release image
                release: hyperv1::Release {
                    image: cr.spec.ocp_release_image.clone(),
                    ..Default::default()
                },

                // Pull secret reference (copied to clusters namespace, name encodes source ref)
                pull_secret: local_ref(pull_secret_copy_name(cr)),

                // SSH key reference (copied to clusters namespace, name encodes source ref)
                ssh_key: local_ref(ssh_key_copy_name(cr)),

                // DNS configuration
                dns: hyperv1::DNSSpec {
                    base_domain: cr.spec.base_domain.clone(),
                    ..Default::default()
                },

                // ETCD configuration with managed storage
                etcd: hyperv1::EtcdSpec {
                    management_type: hyperv1::MANAGED.to_string(),
                    managed: Some(hyperv1::ManagedEtcdSpec {
                        storage: hyperv1::ManagedEtcdStorageSpec {
                            type_: hyperv1::PERSISTENT_VOLUME_ETCD_STORAGE.to_string(),
                            persistent_volume: Some(etcd_storage),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    ..Default::default()
                },

                // Networking configuration with Other network type
                // Default CIDRs
                networking: build_networking(cr),

                // Platform: None (for DPU environments)
                platform: hyperv1::PlatformSpec {
                    type_: hyperv1::NONE_PLATFORM.to_string(),
                    ..Default::default()
                },

                // Availability policy from DPFHCPProvisioner spec
                controller_availability_policy: cr.spec.control_plane_availability_policy.clone(),

                // InfraID: Generate deterministically from cluster name
                infra_id: infraid::new(cr.name()),

                // Secret encryption with AESCBC
                secret_encryption: Some(hyperv1::SecretEncryptionSpec {
                    type_: hyperv1::AESCBC.to_string(),
                    aescbc: Some(hyperv1::AESCBCSpec {
                        active_key: local_ref(format!("{}-etcd-encryption-key", cr.name())),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),

                // Service publishing strategy (LoadBalancer or NodePort mode)
                services: Some(build_service_publishing_strategy(
                    cr.should_expose_through_load_balancer(),
                    node_address,
                )),

                // Capabilities: Disable optional cluster capabilities
                // These capabilities are disabled to reduce resource consumption in DPU environments
                capabilities: Some(hyperv1::Capabilities {
                    disabled: [
                        "ImageRegistry",
                        "Insights",
                        "Console",
                        "openshift-samples",
                        "Ingress",
                        "NodeTuning",
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                    ..Default::default()
                }),

                // NodeSelector: Schedule control plane pods based on user preference
                // Default: control-plane nodes
                node_selector: get_node_selector(cr),

                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// buildNetworking constructs the ClusterNetworking spec from DPFHCPProvisioner fields.
/// User-provided networking values override the defaults. When FlannelEnabled is nil (default)
/// or true, AllocateNodeCIDRs is set to Enabled so that kube-controller-manager manages
/// node CIDR allocation as required by Flannel.
pub(crate) fn build_networking(
    cr: &provisioningv1alpha1::DPFHCPProvisioner,
) -> hyperv1::ClusterNetworking {
    // Defaults
    let mut service_network = vec![hyperv1::ServiceNetworkEntry {
        cidr: IPNet::must_parse_cidr("172.31.0.0/16"),
        ..Default::default()
    }];
    let mut cluster_network = vec![hyperv1::ClusterNetworkEntry {
        cidr: IPNet::must_parse_cidr("10.132.0.0/14"),
        ..Default::default()
    }];
    let mut machine_network: Vec<hyperv1::MachineNetworkEntry> = Vec::new();

    // Override with user-provided values
    if let Some(networking) = &cr.spec.networking {
        if !networking.service_network.is_empty() {
            service_network = networking.service_network.clone();
        }
        if !networking.cluster_network.is_empty() {
            cluster_network = networking.cluster_network.clone();
        }
        if !networking.machine_network.is_empty() {
            machine_network = networking.machine_network.clone();
        }
    }

    let mut networking = hyperv1::ClusterNetworking {
        network_type: hyperv1::OTHER.to_string(),
        service_network,
        cluster_network,
        machine_network,
        ..Default::default()
    };

    if cr.spec.flannel_enabled.unwrap_or(true) {
        networking.allocate_node_cidrs = Some(hyperv1::ALLOCATE_NODE_CIDRS_ENABLED.to_string());
    }

    networking
}

/// getNodeSelector returns the NodeSelector from DPFHCPProvisioner spec or the default if not specified
pub(crate) fn get_node_selector(
    cr: &provisioningv1alpha1::DPFHCPProvisioner,
) -> BTreeMap<String, String> {
    if !cr.spec.node_selector.is_empty() {
        return cr.spec.node_selector.clone();
    }
    // Default: Schedule control plane pods only on control-plane nodes
    BTreeMap::from([(
        "node-role.kubernetes.io/control-plane".to_string(),
        String::new(),
    )])
}

/// detectNodeAddress auto-detects the management cluster node address for NodePort publishing
/// Priority: ExternalDNS > ExternalIP > InternalIP
/// This matches the HyperShift CLI pattern (GetAPIServerAddressByNode)
pub(crate) async fn detect_node_address(_ctx: &Ctx, c: &Client) -> ctrlrt::Result<String> {
    let nodes: Vec<Node> = match c.list_all().await {
        Ok(n) => n,
        Err(err) => return Err(wrapf!(err, "failed to list nodes: {}", err)),
    };

    // Use first node and check addresses in priority order
    let Some(node) = nodes.first() else {
        return Err(anyhow::anyhow!("no nodes found in cluster"));
    };

    // Priority order: ExternalDNS > ExternalIP > InternalIP
    let address_types = ["ExternalDNS", "ExternalIP", "InternalIP"];

    let addresses = node
        .status
        .as_ref()
        .and_then(|s| s.addresses.as_deref())
        .unwrap_or(&[]);
    for addr_type in address_types {
        for addr in addresses {
            if addr.type_ == addr_type {
                return Ok(addr.address.clone());
            }
        }
    }

    Err(anyhow::anyhow!(
        "no valid address found on node {}",
        node.name()
    ))
}
