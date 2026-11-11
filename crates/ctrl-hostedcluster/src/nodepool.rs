//! Port of internal/controller/hostedcluster/nodepool.go

use apis::hypershift::v1beta1 as hyperv1;
use apis::provisioning::v1alpha1 as provisioningv1alpha1;
use ctrlrt::k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ctrlrt::meta;
use ctrlrt::{Client, Ctx, Object, ObjectKey, ReconcileResult, kv, wrapf};

/// NodePoolManager manages NodePool resources
#[derive(Clone)]
pub struct NodePoolManager {
    client: Client,
}

impl NodePoolManager {
    /// NewNodePoolManager creates a new NodePoolManager
    pub fn new(c: Client) -> Self {
        Self { client: c }
    }

    /// The underlying client (Go embeds `client.Client`).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// CreateNodePool creates the NodePool resource
    /// Returns ctrl.Result and error for reconciliation flow
    ///
    /// NodePool is created with:
    /// - replicas=0 (DPU workers will join manually via CSR approval)
    /// - None platform type
    /// - Matching release image from DPFHCPProvisioner
    /// - Upgrade type: Replace (as per spec)
    pub async fn create_node_pool(
        &self,
        ctx: &Ctx,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> ctrlrt::Result<ReconcileResult> {
        let log = &ctx.log;

        let np_name = cr.name();
        let np_namespace = cr.namespace();

        // Check if NodePool already exists (idempotency)
        let np_key = ObjectKey::new(np_namespace, np_name);
        match self.client.get::<hyperv1::NodePool>(&np_key).await {
            Ok(existing_np) => {
                // NodePool exists - verify ownership via OwnerReference
                if meta::is_controlled_by(&existing_np, cr) {
                    log.v(1).info(
                        "NodePool already exists and is owned by this DPFHCPProvisioner, no update needed",
                        kv!["nodePool" => np_name, "namespace" => np_namespace],
                    );
                    return Ok(ReconcileResult::default());
                }

                // Name conflict - NP exists but owned by different DPFHCPProvisioner
                return Err(anyhow::anyhow!(
                    "nodePool {} exists in {} but is owned by different DPFHCPProvisioner",
                    np_name,
                    np_namespace
                ));
            }
            Err(err) => {
                if !ctrlrt::error::is_not_found(&err) {
                    return Err(wrapf!(err, "failed to check existing NodePool: {}", err));
                }
            }
        }

        // NodePool doesn't exist - create it
        log.info(
            "Creating NodePool",
            kv!["nodePool" => np_name, "namespace" => np_namespace, "replicas" => 0],
        );

        let mut np = self.build_node_pool(cr);

        // Set owner reference for automatic garbage collection
        if let Err(err) = meta::set_controller_reference(cr, &mut np) {
            log.error(&err, "Failed to set owner reference on NodePool", kv![]);
            return Err(wrapf!(
                err,
                "failed to set owner reference on NodePool: {}",
                err
            ));
        }

        if let Err(err) = self.client.create(&mut np).await {
            log.error(
                &err,
                "Failed to create NodePool",
                kv!["nodePool" => np_name, "namespace" => np_namespace],
            );
            return Err(wrapf!(err, "failed to create NodePool: {}", err));
        }

        log.info(
            "NodePool created successfully",
            kv!["nodePool" => np_name, "namespace" => np_namespace],
        );

        Ok(ReconcileResult::default())
    }

    /// buildNodePool constructs the NodePool spec
    pub fn build_node_pool(
        &self,
        cr: &provisioningv1alpha1::DPFHCPProvisioner,
    ) -> hyperv1::NodePool {
        hyperv1::NodePool {
            metadata: ObjectMeta {
                name: Some(cr.name().to_string()),
                namespace: Some(cr.namespace().to_string()),
                ..Default::default()
            },
            spec: hyperv1::NodePoolSpec {
                // ClusterName links this NodePool to the HostedCluster
                cluster_name: cr.name().to_string(),

                // Replicas=0 - DPU workers will be added manually
                replicas: Some(0),

                // Management settings
                management: hyperv1::NodePoolManagement {
                    upgrade_type: hyperv1::UPGRADE_TYPE_REPLACE.to_string(),
                    ..Default::default()
                },

                // Platform: None (DPU environment)
                platform: hyperv1::NodePoolPlatform {
                    type_: hyperv1::NONE_PLATFORM.to_string(),
                    ..Default::default()
                },

                // Release image matches HostedCluster
                release: hyperv1::Release {
                    image: cr.spec.ocp_release_image.clone(),
                    ..Default::default()
                },

                ..Default::default()
            },
            ..Default::default()
        }
    }
}
