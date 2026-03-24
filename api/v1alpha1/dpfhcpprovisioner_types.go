/*
Copyright 2025.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package v1alpha1

import (
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
)

// DPUClusterReference defines a cross-namespace reference to a DPUCluster CR
type DPUClusterReference struct {
	// Name is the name of the DPUCluster CR
	// +kubebuilder:validation:Required
	// +required
	Name string `json:"name"`

	// Namespace is the namespace of the DPUCluster CR
	// +kubebuilder:validation:Required
	// +required
	Namespace string `json:"namespace"`
}

// DPUDeploymentReference defines a cross-namespace reference to a DPUDeployment CR
type DPUDeploymentReference struct {
	// Name is the name of the DPUDeployment CR
	// +kubebuilder:validation:Required
	// +required
	Name string `json:"name"`

	// Namespace is the namespace of the DPUDeployment CR
	// +kubebuilder:validation:Required
	// +required
	Namespace string `json:"namespace"`
}

// DPFHCPProvisionerSpec defines the desired state of DPFHCPProvisioner
// +kubebuilder:validation:XValidation:rule="self.controlPlaneAvailabilityPolicy != 'HighlyAvailable' || (has(self.virtualIP) && size(self.virtualIP) > 0)",message="virtualIP is required when controlPlaneAvailabilityPolicy is HighlyAvailable"
type DPFHCPProvisionerSpec struct {
	// DPUClusterRef is a cross-namespace reference to a DPUCluster CR for validation and kubeconfig injection
	// This field is immutable.
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="dpuClusterRef is immutable"
	// +immutable
	// +required
	DPUClusterRef DPUClusterReference `json:"dpuClusterRef"`

	// BaseDomain is the base domain for the hosted cluster's DNS records
	// Example: clusters.example.com results in API endpoint at api.prod-cluster.clusters.example.com
	// This field is immutable.
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:Pattern=`^([a-z0-9]([-a-z0-9]*[a-z0-9])?\.)+[a-z]{2,}$`
	// +kubebuilder:validation:MinLength=4
	// +kubebuilder:validation:MaxLength=253
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="baseDomain is immutable"
	// +immutable
	// +required
	BaseDomain string `json:"baseDomain"`

	// OCPReleaseImage is the full pull-spec URL for the OCP release image
	// The operator extracts the OCP version from this image and queries the BlueField OCP layer
	// registry to find the corresponding BlueField OCP layer image by matching the tag.
	// +kubebuilder:validation:Required
	// +required
	OCPReleaseImage string `json:"ocpReleaseImage"`

	// SSHKeySecretRef is a reference to a Secret containing the SSH public key for cluster node access
	// Secret must be in the same namespace as the DPFHCPProvisioner CR and contain key 'id_rsa.pub'
	// This field is immutable.
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="sshKeySecretRef is immutable"
	// +immutable
	// +required
	SSHKeySecretRef corev1.LocalObjectReference `json:"sshKeySecretRef"`

	// PullSecretRef is a reference to a Secret containing the container registry pull secret
	// Secret must be in the same namespace as the DPFHCPProvisioner CR and contain key '.dockerconfigjson'
	// This field is immutable.
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="pullSecretRef is immutable"
	// +immutable
	// +required
	PullSecretRef corev1.LocalObjectReference `json:"pullSecretRef"`

	// EtcdStorageClass is the storage class name for etcd persistent volumes in the hosted cluster control plane
	// This field is immutable.
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="etcdStorageClass is immutable"
	// +immutable
	// +optional
	EtcdStorageClass string `json:"etcdStorageClass,omitempty"`

	// ControlPlaneAvailabilityPolicy specifies the availability policy for the control plane
	// Valid values: SingleReplica, HighlyAvailable
	// This field is immutable.
	// +kubebuilder:validation:Enum=SingleReplica;HighlyAvailable
	// +kubebuilder:default=HighlyAvailable
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="controlPlaneAvailabilityPolicy is immutable"
	// +immutable
	// +optional
	ControlPlaneAvailabilityPolicy hyperv1.AvailabilityPolicy `json:"controlPlaneAvailabilityPolicy,omitempty"`

	// VirtualIP is the virtual IP address for load balancer
	// Required when ControlPlaneAvailabilityPolicy is HighlyAvailable
	// Must be a routable IP in the management cluster network
	// This field is immutable.
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="virtualIP is immutable"
	// +kubebuilder:validation:XValidation:rule="self == '' || isIP(self)",message="virtualIP must be a valid IP address"
	// +immutable
	// +optional
	VirtualIP string `json:"virtualIP,omitempty"`

	// NodeSelector defines the node selector for the hosted control plane pods
	// It specifies which nodes in the management cluster can host the control plane workloads
	// Default: {"node-role.kubernetes.io/control-plane": ""} (schedules on control-plane nodes)
	// This field is immutable.
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="nodeSelector is immutable"
	// +kubebuilder:validation:XValidation:rule="size(self) <= 20",message="nodeSelector map can have at most 20 entries"
	// +immutable
	// +optional
	NodeSelector map[string]string `json:"nodeSelector,omitempty"`

	// FlannelEnabled indicates whether Flannel should be used as the CNI plugin for the hosted cluster.
	// When set to true, the HostedCluster will be configured with NetworkType "Other" and
	// AllocateNodeCIDRs set to "Enabled", which allows kube-controller-manager to manage node CIDR
	// allocation as required by Flannel.
	// This field is immutable.
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="flannelEnabled is immutable"
	// +kubebuilder:default=true
	// +immutable
	// +optional
	FlannelEnabled *bool `json:"flannelEnabled,omitempty"`

	// DPUDeploymentRef is a cross-namespace reference to a DPUDeployment CR for ignition generation
	// This field is used for ignition generation to retrieve DPU flavor information
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="dpuDeploymentRef is immutable"
	// +required
	DPUDeploymentRef *DPUDeploymentReference `json:"dpuDeploymentRef,omitempty"`

	// MachineOSURL is the URL for the DPU machine OS image to be used in the ignition configuration
	// This URL replaces the default OS image URL in the HyperShift-generated ignition
	// +optional
	MachineOSURL string `json:"machineOSURL,omitempty"`
}

// DPFHCPProvisionerPhase represents the lifecycle phase of the DPFHCPProvisioner
// +kubebuilder:validation:Enum=Pending;Provisioning;IgnitionGenerating;Ready;Failed;Deleting
type DPFHCPProvisionerPhase string

const (
	// PhasePending indicates initial state, validation in progress
	PhasePending DPFHCPProvisionerPhase = "Pending"

	// PhaseProvisioning indicates HostedCluster and related resources are being created
	PhaseProvisioning DPFHCPProvisionerPhase = "Provisioning"

	// PhaseIgnitionGenerating indicates ignition generation is in progress
	PhaseIgnitionGenerating DPFHCPProvisionerPhase = "IgnitionGenerating"

	// PhaseReady indicates HostedCluster is operational, kubeconfig injected, CSR auto-approval active
	PhaseReady DPFHCPProvisionerPhase = "Ready"

	// PhaseFailed indicates permanent failure requiring user intervention
	PhaseFailed DPFHCPProvisionerPhase = "Failed"

	// PhaseDeleting indicates finalizer cleanup in progress
	PhaseDeleting DPFHCPProvisionerPhase = "Deleting"
)

// Condition types for DPFHCPProvisioner.
// These will be populated in the Conditions array as implementation progresses.
const (
	// Conditions mirrored from HostedCluster (with DPFHCPProvisioner-specific naming).

	// HostedClusterAvailable indicates the HostedCluster has a healthy control plane.
	HostedClusterAvailable string = "HostedClusterAvailable"

	// HostedClusterProgressing indicates the HostedCluster is attempting deployment or upgrade.
	HostedClusterProgressing string = "HostedClusterProgressing"

	// HostedClusterDegraded indicates the HostedCluster is encountering errors requiring intervention.
	HostedClusterDegraded string = "HostedClusterDegraded"

	// ValidReleaseImage indicates the release image in spec is valid for HostedCluster.
	ValidReleaseImage string = "ValidReleaseImage"

	// ValidReleaseInfo indicates the release contains all required HyperShift images.
	ValidReleaseInfo string = "ValidReleaseInfo"

	// IgnitionEndpointAvailable indicates the ignition server is available.
	IgnitionEndpointAvailable string = "IgnitionEndpointAvailable"

	// IgnitionServerValidReleaseInfo indicates the release has local ignition provider images.
	IgnitionServerValidReleaseInfo string = "IgnitionServerValidReleaseInfo"

	// DPFHCPProvisioner-specific conditions.

	// Ready indicates the overall operational status of the DPFHCPProvisioner.
	Ready string = "Ready"

	// KubeConfigInjected indicates whether the kubeconfig was successfully injected into the DPUCluster CR.
	KubeConfigInjected string = "KubeConfigInjected"

	// HostedClusterCleanup indicates the status of HostedCluster deletion during finalizer cleanup.
	HostedClusterCleanup string = "HostedClusterCleanup"

	// CSRAutoApprovalActive indicates whether CSR auto-approval is active and watching for CSRs
	CSRAutoApprovalActive string = "CSRAutoApprovalActive"

	// Validation conditions.

	// SecretsValid indicates whether required secrets (pull secret, SSH key) are valid.
	SecretsValid string = "SecretsValid"

	// BlueFieldOCPLayerImageFound indicates whether the BlueField OCP layer image was successfully found.
	BlueFieldOCPLayerImageFound string = "BlueFieldOCPLayerImageFound"

	// DPUClusterMissing indicates whether the referenced DPUCluster exists.
	DPUClusterMissing string = "DPUClusterMissing"

	// ClusterTypeValid indicates whether the DPUCluster type is supported.
	ClusterTypeValid string = "ClusterTypeValid"

	// DPUClusterInUse indicates whether the DPUCluster is already in use by another DPFHCPProvisioner.
	DPUClusterInUse string = "DPUClusterInUse"

	// MetalLBConfigured indicates whether MetalLB resources (IPAddressPool and L2Advertisement)
	// have been successfully created and are in sync with the DPFHCPProvisioner spec.
	MetalLBConfigured string = "MetalLBConfigured"

	// IgnitionConfigured indicates whether the ignition configuration has been successfully generated and deployed
	IgnitionConfigured string = "IgnitionConfigured"
)

// Condition reasons for DPFHCPProvisioner Ready status.
// These are used as the Reason field in the Ready condition to indicate why the provisioner is ready or not ready.
const (
	// ReasonAllComponentsOperational indicates all required components are operational and healthy.
	ReasonAllComponentsOperational string = "AllComponentsOperational"

	// ReasonHostedClusterNotReady indicates the HostedCluster is not yet available or healthy.
	// Used when: HostedClusterAvailable condition is False or not set.
	ReasonHostedClusterNotReady string = "HostedClusterNotReady"

	// ReasonKubeConfigNotInjected indicates the kubeconfig has not been injected into DPUCluster.
	// Used when: KubeConfigInjected condition is False or not set.
	ReasonKubeConfigNotInjected string = "KubeConfigNotInjected"
)

// Condition reasons for DPFHCPProvisioner KubeConfigInjected status.
// These are used as the Reason field in the KubeConfigInjected condition.
const (
	// ReasonKubeConfigInjected indicates kubeconfig was successfully injected into DPUCluster.
	ReasonKubeConfigInjected string = "Injected"

	// ReasonKubeConfigPending indicates waiting for Hypershift to create the kubeconfig secret.
	ReasonKubeConfigPending string = "KubeconfigPending"

	// ReasonKubeConfigInjectionFailed indicates kubeconfig injection failed.
	ReasonKubeConfigInjectionFailed string = "InjectionFailed"
)

// Condition reasons for DPFHCPProvisioner CSRAutoApprovalActive status.
// These are used as the Reason field in the CSRAutoApprovalActive condition.
const (
	// ReasonCSRApprovalActive indicates CSR auto-approval is actively processing CSRs
	ReasonCSRApprovalActive string = "Active"

	// ReasonKubeconfigNotAvailable indicates the kubeconfig is not available
	ReasonKubeconfigNotAvailable string = "KubeconfigNotAvailable"

	// ReasonHostedClusterNotReachable indicates the hosted cluster is not reachable
	ReasonHostedClusterNotReachable string = "HostedClusterNotReachable"
)

// DPFHCPProvisionerStatus defines the observed state of DPFHCPProvisioner
type DPFHCPProvisionerStatus struct {
	// Phase represents the current lifecycle phase
	// +optional
	Phase DPFHCPProvisionerPhase `json:"phase,omitempty"`

	// Conditions represent the latest available observations of the DPFHCPProvisioner's state
	// +optional
	Conditions []metav1.Condition `json:"conditions,omitempty"`

	// HostedClusterRef is a reference to the created HostedCluster CR
	// +optional
	HostedClusterRef *corev1.ObjectReference `json:"hostedClusterRef,omitempty"`

	// KubeConfigSecretRef is a reference to the created kubeconfig Secret in the DPUCluster's namespace
	// +optional
	KubeConfigSecretRef *corev1.LocalObjectReference `json:"kubeConfigSecretRef,omitempty"`

	// BlueFieldOCPLayerImage is the BlueField OCP layer image URL found via registry lookup
	// +optional
	BlueFieldOCPLayerImage string `json:"blueFieldOCPLayerImage,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Namespaced,shortName=dpfhcp
// +kubebuilder:printcolumn:name="Phase",type=string,JSONPath=`.status.phase`
// +kubebuilder:printcolumn:name="Ready",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`
// +kubebuilder:printcolumn:name="HostedCluster",type=string,JSONPath=`.status.hostedClusterRef.name`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

// DPFHCPProvisioner is the Schema for the dpfhcpprovisioners API
type DPFHCPProvisioner struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   DPFHCPProvisionerSpec   `json:"spec,omitempty"`
	Status DPFHCPProvisionerStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

// DPFHCPProvisionerList contains a list of DPFHCPProvisioner
type DPFHCPProvisionerList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []DPFHCPProvisioner `json:"items"`
}

func init() {
	SchemeBuilder.Register(&DPFHCPProvisioner{}, &DPFHCPProvisionerList{})
}

// ShouldExposeThroughLoadBalancer determines whether to expose services via LoadBalancer or NodePort
// Returns true if:
// - ControlPlaneAvailabilityPolicy is HighlyAvailable (VIP is required in this case)
// - ControlPlaneAvailabilityPolicy is SingleReplica AND VirtualIP is provided
// Returns false if:
// - ControlPlaneAvailabilityPolicy is SingleReplica AND VirtualIP is not provided
func (p *DPFHCPProvisioner) ShouldExposeThroughLoadBalancer() bool {
	// If ControlPlane is HighlyAvailable, we must expose through LoadBalancer
	if p.Spec.ControlPlaneAvailabilityPolicy == hyperv1.HighlyAvailable {
		return true
	}

	// If ControlPlane is SingleReplica and VIP is provided, expose through LoadBalancer
	if p.Spec.ControlPlaneAvailabilityPolicy == hyperv1.SingleReplica && p.Spec.VirtualIP != "" {
		return true
	}

	// If ControlPlane is SingleReplica and no VIP, use NodePort
	return false
}

// IsVIPRequired determines if VirtualIP is required for the given configuration
// Returns true if ControlPlaneAvailabilityPolicy is HighlyAvailable
func (p *DPFHCPProvisioner) IsVIPRequired() bool {
	return p.Spec.ControlPlaneAvailabilityPolicy == hyperv1.HighlyAvailable
}
