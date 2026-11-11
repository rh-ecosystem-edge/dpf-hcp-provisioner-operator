//! Port of `internal/controller/csrapproval/csr_owner_validation.go`.

use apis::dpf::provisioning::v1alpha1 as dpuprovisioningv1alpha1;
use ctrlrt::error::Result;
use ctrlrt::{Client, Ctx, ListOptions, wrapf};
use hostedclient::HostedClient;
use k8s_openapi::api::core::v1::Node;

/// ValidationResult contains the result of CSR validation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationResult {
    pub valid: bool,
    pub reason: String,
}

/// Validator validates CSRs against DPU and Node state.
pub struct Validator {
    /// Client for management cluster (where DPU objects live)
    mgmt_client: Client,
    /// Client for hosted cluster (where CSRs and Nodes live); `None` mirrors a nil
    /// `kubernetes.Interface`.
    hosted_client: Option<HostedClient>,
    /// Namespace where DPU objects are located
    dpu_namespace: String,
}

impl Validator {
    /// `NewValidator(mgmtClient, hostedClient, dpuNamespace)`
    pub fn new(mgmt_client: Client, hosted_client: Option<HostedClient>, dpu_namespace: &str) -> Self {
        Self {
            mgmt_client,
            hosted_client,
            dpu_namespace: dpu_namespace.to_string(),
        }
    }

    /// ValidateCSROwner validates CSR ownership by checking:
    /// 1. DPU object with matching hostname exists in management cluster (ownership check)
    /// 2. DPU is in the correct phase for CSR approval (security check - time-limited window)
    /// 3. For bootstrap CSRs: DPU must be in "DPU Cluster Config" phase (joining cluster)
    /// 4. For serving CSRs: DPU must be in "DPU Cluster Config", "Ready", or
    ///    "Node Effect Removal" phase.
    pub async fn validate_csr_owner(
        &self,
        ctx: &Ctx,
        hostname: &str,
        is_bootstrap_csr: bool,
    ) -> Result<ValidationResult> {
        // Check if DPU exists and get its phase
        let dpu = self
            .get_dpu(ctx, hostname)
            .await
            .map_err(|err| wrapf!(err, "failed to get DPU: {}", err))?;

        let Some(dpu) = dpu else {
            return Ok(ValidationResult {
                valid: false,
                reason: format!(
                    "no matching DPU found for hostname {} in namespace {}",
                    hostname, self.dpu_namespace
                ),
            });
        };

        let phase = dpu.status.phase.as_str();

        // Validate DPU phase for CSR approval (security: time-limited approval window)
        if is_bootstrap_csr {
            // Bootstrap CSR: Only approve during "DPU Cluster Config" phase (node is joining)
            if phase != dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG {
                return Ok(ValidationResult {
                    valid: false,
                    reason: format!(
                        "DPU {} is in phase {}, bootstrap CSRs only approved in phase {}",
                        hostname,
                        phase,
                        dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG
                    ),
                });
            }
            return Ok(ValidationResult {
                valid: true,
                reason: format!(
                    "DPU in {} phase, bootstrap CSR approved",
                    dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG
                ),
            });
        }

        // Serving CSR: Allow in "DPU Cluster Config", "Ready", or "Node Effect Removal".
        // Node Effect Removal phase still requires a valid serving certificate for node
        // operations like log access, metrics, and exec.
        let allowed_serving_phases = [
            dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG,
            dpuprovisioningv1alpha1::DPU_READY,
            dpuprovisioningv1alpha1::DPU_NODE_EFFECT_REMOVAL,
        ];
        if !allowed_serving_phases.contains(&phase) {
            return Ok(ValidationResult {
                valid: false,
                reason: format!(
                    "DPU {} is in phase {}, serving CSRs only approved in phases {}, {}, or {}",
                    hostname,
                    phase,
                    dpuprovisioningv1alpha1::DPU_CLUSTER_CONFIG,
                    dpuprovisioningv1alpha1::DPU_READY,
                    dpuprovisioningv1alpha1::DPU_NODE_EFFECT_REMOVAL
                ),
            });
        }

        // For serving CSRs, also verify Node exists (should have joined via bootstrap CSR)
        let node_exists = self
            .node_exists(ctx, hostname)
            .await
            .map_err(|err| wrapf!(err, "failed to check Node existence: {}", err))?;
        if !node_exists {
            return Ok(ValidationResult {
                valid: false,
                reason: format!("node {hostname} does not exist yet in hosted cluster"),
            });
        }

        Ok(ValidationResult {
            valid: true,
            reason: format!("DPU in {phase} phase and node exists, serving CSR approved"),
        })
    }

    /// getDPU retrieves a DPU object by hostname from the management cluster.
    pub(crate) async fn get_dpu(
        &self,
        _ctx: &Ctx,
        hostname: &str,
    ) -> Result<Option<dpuprovisioningv1alpha1::DPU>> {
        let dpu_list: Vec<dpuprovisioningv1alpha1::DPU> = self
            .mgmt_client
            .list(&ListOptions::new().in_namespace(self.dpu_namespace.clone()))
            .await?;
        Ok(dpu_list
            .into_iter()
            .find(|d| d.metadata.name.as_deref().unwrap_or("") == hostname))
    }

    /// nodeExists checks if a Node with the given hostname exists in the hosted cluster.
    ///
    /// Go would panic on a nil hosted client; here an error is returned instead.
    async fn node_exists(&self, _ctx: &Ctx, hostname: &str) -> Result<bool> {
        let Some(hc) = &self.hosted_client else {
            return Err(anyhow::anyhow!("hosted cluster client is nil"));
        };
        let node_list: Vec<Node> = hc.list_all().await?;
        Ok(node_list
            .iter()
            .any(|n| n.metadata.name.as_deref().unwrap_or("") == hostname))
    }
}
