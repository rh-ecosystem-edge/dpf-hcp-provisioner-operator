//! This repository's API: `provisioning.dpu.hcp.io/v1alpha1` (CRDs in `config/crd/bases`).

pub mod v1alpha1 {
    //! Types are hand-maintained in `crates/apis/src/provisioning_types.rs` (kept in sync with
    //! `config/crd/bases`); constants and methods below were ported from the former Go
    //! `api/v1alpha1/*.go`.

    pub use crate::provisioning_types::*;

    use crate::hypershift::v1beta1 as hyperv1;

    /// `GroupVersion.Group`
    pub const GROUP: &str = "provisioning.dpu.hcp.io";
    /// `GroupVersion.Version`
    pub const VERSION: &str = "v1alpha1";

    /// DefaultConfigName is the well-known singleton name for DPFHCPProvisionerConfig
    pub const DEFAULT_CONFIG_NAME: &str = "default";

    // DPFHCPProvisionerPhase values.
    pub const PHASE_PENDING: &str = "Pending";
    pub const PHASE_WAITING_FOR_CONTROL_PLANE: &str = "WaitingForControlPlane";
    pub const PHASE_GENERATING_IGNITION: &str = "GeneratingIgnition";
    pub const PHASE_CLUSTER_VERSION_PROGRESSING: &str = "ClusterVersionProgressing";
    pub const PHASE_READY: &str = "Ready";
    pub const PHASE_FAILED: &str = "Failed";
    pub const PHASE_ERROR: &str = "Error";
    pub const PHASE_DELETING: &str = "Deleting";

    // Condition types for DPFHCPProvisioner.
    // Conditions mirrored from HostedCluster (with DPFHCPProvisioner-specific naming).

    /// HostedClusterAvailable indicates the HostedCluster has a healthy control plane.
    pub const HOSTED_CLUSTER_AVAILABLE: &str = "HostedClusterAvailable";
    /// HostedClusterProgressing indicates the HostedCluster is attempting deployment or upgrade.
    pub const HOSTED_CLUSTER_PROGRESSING: &str = "HostedClusterProgressing";
    /// HostedClusterDegraded indicates the HostedCluster is encountering errors requiring intervention.
    pub const HOSTED_CLUSTER_DEGRADED: &str = "HostedClusterDegraded";
    /// ValidReleaseImage indicates the release image in spec is valid for HostedCluster.
    pub const VALID_RELEASE_IMAGE: &str = "ValidReleaseImage";
    /// ValidReleaseInfo indicates the release contains all required HyperShift images.
    pub const VALID_RELEASE_INFO: &str = "ValidReleaseInfo";
    /// IgnitionEndpointAvailable indicates the ignition server is available.
    pub const IGNITION_ENDPOINT_AVAILABLE: &str = "IgnitionEndpointAvailable";
    /// IgnitionServerValidReleaseInfo indicates the release has local ignition provider images.
    pub const IGNITION_SERVER_VALID_RELEASE_INFO: &str = "IgnitionServerValidReleaseInfo";

    // DPFHCPProvisioner-specific conditions.

    /// Ready indicates the overall operational status of the DPFHCPProvisioner.
    pub const READY: &str = "Ready";
    /// KubeConfigInjected indicates whether the kubeconfig was successfully injected into the DPUCluster CR.
    pub const KUBE_CONFIG_INJECTED: &str = "KubeConfigInjected";
    /// HostedClusterCleanup indicates the status of HostedCluster deletion during finalizer cleanup.
    pub const HOSTED_CLUSTER_CLEANUP: &str = "HostedClusterCleanup";
    /// CSRAutoApprovalActive indicates whether CSR auto-approval is active and watching for CSRs
    pub const CSR_AUTO_APPROVAL_ACTIVE: &str = "CSRAutoApprovalActive";

    // Validation conditions.

    /// SecretsValid indicates whether required secrets (pull secret, SSH key) are valid.
    pub const SECRETS_VALID: &str = "SecretsValid";
    /// BlueFieldOCPLayerImageFound indicates whether the BlueField OCP layer image was successfully found.
    pub const BLUE_FIELD_OCP_LAYER_IMAGE_FOUND: &str = "BlueFieldOCPLayerImageFound";
    /// DPUClusterMissing indicates whether the referenced DPUCluster exists.
    pub const DPU_CLUSTER_MISSING: &str = "DPUClusterMissing";
    /// ClusterTypeValid indicates whether the DPUCluster type is supported.
    pub const CLUSTER_TYPE_VALID: &str = "ClusterTypeValid";
    /// DPUClusterInUse indicates whether the DPUCluster is already in use by another DPFHCPProvisioner.
    pub const DPU_CLUSTER_IN_USE: &str = "DPUClusterInUse";
    /// HostedClusterUpgrading indicates whether a HostedCluster release image upgrade is in progress.
    /// True when the user updates spec.ocpReleaseImage and the operator is propagating the change.
    /// False when the HostedCluster upgrade is complete and ignition regeneration may proceed.
    pub const HOSTED_CLUSTER_UPGRADING: &str = "HostedClusterUpgrading";
    /// MetalLBConfigured indicates whether MetalLB resources (IPAddressPool and L2Advertisement)
    /// have been successfully created and are in sync with the DPFHCPProvisioner spec.
    pub const METAL_LB_CONFIGURED: &str = "MetalLBConfigured";
    /// IgnitionConfigured indicates whether the ignition configuration has been successfully generated and deployed
    pub const IGNITION_CONFIGURED: &str = "IgnitionConfigured";

    // Condition reasons for DPFHCPProvisioner Ready status.

    /// ReasonAllComponentsOperational indicates all required components are operational and healthy.
    pub const REASON_ALL_COMPONENTS_OPERATIONAL: &str = "AllComponentsOperational";
    /// ReasonHostedClusterNotReady indicates the HostedCluster is not yet available or healthy.
    /// Used when: HostedClusterAvailable condition is False or not set.
    pub const REASON_HOSTED_CLUSTER_NOT_READY: &str = "HostedClusterNotReady";
    /// ReasonKubeConfigNotInjected indicates the kubeconfig has not been injected into DPUCluster.
    /// Used when: KubeConfigInjected condition is False or not set.
    pub const REASON_KUBE_CONFIG_NOT_INJECTED: &str = "KubeConfigNotInjected";

    // Condition reasons for DPFHCPProvisioner KubeConfigInjected status.

    /// ReasonKubeConfigInjected indicates kubeconfig was successfully injected into DPUCluster.
    pub const REASON_KUBE_CONFIG_INJECTED: &str = "Injected";
    /// ReasonKubeConfigPending indicates waiting for Hypershift to create the kubeconfig secret.
    pub const REASON_KUBE_CONFIG_PENDING: &str = "KubeconfigPending";
    /// ReasonKubeConfigInjectionFailed indicates kubeconfig injection failed.
    pub const REASON_KUBE_CONFIG_INJECTION_FAILED: &str = "InjectionFailed";

    // Condition reasons for DPFHCPProvisioner CSRAutoApprovalActive status.

    /// ReasonCSRApprovalActive indicates CSR auto-approval is actively processing CSRs
    pub const REASON_CSR_APPROVAL_ACTIVE: &str = "Active";
    /// ReasonKubeconfigNotAvailable indicates the kubeconfig is not available
    pub const REASON_KUBECONFIG_NOT_AVAILABLE: &str = "KubeconfigNotAvailable";
    /// ReasonHostedClusterNotReachable indicates the hosted cluster is not reachable
    pub const REASON_HOSTED_CLUSTER_NOT_REACHABLE: &str = "HostedClusterNotReachable";

    // Condition reasons for DPFHCPProvisioner HostedClusterUpgrading status.

    /// ReasonUpgradeInProgress indicates a release image upgrade is in progress.
    pub const REASON_UPGRADE_IN_PROGRESS: &str = "UpgradeInProgress";
    /// ReasonUpgradeComplete indicates the HostedCluster upgrade completed and ignition regeneration may proceed.
    pub const REASON_UPGRADE_COMPLETE: &str = "UpgradeComplete";

    // Condition reasons for DPFHCPProvisioner IgnitionConfigured status.

    /// ReasonIgnitionGenerated indicates ignition was successfully generated and deployed.
    pub const REASON_IGNITION_GENERATED: &str = "IgnitionGenerated";
    /// ReasonIgnitionGenerationFailed indicates ignition generation encountered an error.
    /// This triggers a Failed phase transition so the user can see the error and take action.
    pub const REASON_IGNITION_GENERATION_FAILED: &str = "IgnitionGenerationFailed";
    /// ReasonIgnitionCredentialSyncPending indicates HyperShift has not yet propagated the
    /// current HostedCluster credentials into the ignition server.
    pub const REASON_IGNITION_CREDENTIAL_SYNC_PENDING: &str = "IgnitionCredentialSyncPending";
    /// ReasonMachineOSURLMissing indicates no machine OS URL is available from any source.
    /// This triggers a Failed phase transition because ignition cannot be generated without it.
    pub const REASON_MACHINE_OSURL_MISSING: &str = "MachineOSURLMissing";
    /// ReasonDPUDeploymentChanged indicates the DPUDeployment's Flavor changed since last ignition generation.
    /// This invalidates the current ignition ConfigMap and triggers regeneration.
    pub const REASON_DPU_DEPLOYMENT_CHANGED: &str = "DPUDeploymentChanged";
    /// ReasonDependencyDeleted indicates a required dependency (DPUDeployment or DPUFlavor) is missing.
    /// The ignition ConfigMap is removed and will be regenerated once the dependency is recreated.
    pub const REASON_DEPENDENCY_DELETED: &str = "DependencyDeleted";
    /// ReasonReleaseImageUpdated indicates ignition was invalidated because the OCP release image changed.
    pub const REASON_RELEASE_IMAGE_UPDATED: &str = "ReleaseImageUpdated";
    /// ReasonControlPlaneProgressing indicates the control plane is still rolling out the target version.
    pub const REASON_CONTROL_PLANE_PROGRESSING: &str = "ControlPlaneProgressing";

    impl DPFHCPProvisioner {
        /// ShouldExposeThroughLoadBalancer determines whether to expose services via LoadBalancer or NodePort
        /// Returns true if:
        /// - ControlPlaneAvailabilityPolicy is HighlyAvailable (VIP is required in this case)
        /// - ControlPlaneAvailabilityPolicy is SingleReplica AND VirtualIP is provided
        ///
        /// Returns false if:
        /// - ControlPlaneAvailabilityPolicy is SingleReplica AND VirtualIP is not provided
        pub fn should_expose_through_load_balancer(&self) -> bool {
            // If ControlPlane is HighlyAvailable, we must expose through LoadBalancer
            if self.spec.control_plane_availability_policy == hyperv1::HIGHLY_AVAILABLE {
                return true;
            }

            // If ControlPlane is SingleReplica and VIP is provided, expose through LoadBalancer
            if self.spec.control_plane_availability_policy == hyperv1::SINGLE_REPLICA
                && !self.spec.virtual_ip.is_empty()
            {
                return true;
            }

            // If ControlPlane is SingleReplica and no VIP, use NodePort
            false
        }

        /// IsVIPRequired determines if VirtualIP is required for the given configuration
        /// Returns true if ControlPlaneAvailabilityPolicy is HighlyAvailable
        pub fn is_vip_required(&self) -> bool {
            self.spec.control_plane_availability_policy == hyperv1::HIGHLY_AVAILABLE
        }
    }
}
