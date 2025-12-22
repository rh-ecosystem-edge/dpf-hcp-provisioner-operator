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

// DPFHCPBridgeSpec defines the desired state of DPFHCPBridge
// +kubebuilder:validation:XValidation:rule="self.controlPlaneAvailabilityPolicy != 'HighlyAvailable' || (has(self.virtualIP) && size(self.virtualIP) > 0)",message="virtualIP is required when controlPlaneAvailabilityPolicy is HighlyAvailable"
type DPFHCPBridgeSpec struct {
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
	// The operator uses this to look up the corresponding BlueField container image from the central ConfigMap
	// +kubebuilder:validation:Required
	// +required
	OCPReleaseImage string `json:"ocpReleaseImage"`

	// SSHKeySecretRef is a reference to a Secret containing the SSH public key for cluster node access
	// Secret must be in the same namespace as the DPFHCPBridge CR and contain key 'ssh-publickey'
	// This field is immutable.
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="sshKeySecretRef is immutable"
	// +immutable
	// +required
	SSHKeySecretRef corev1.LocalObjectReference `json:"sshKeySecretRef"`

	// PullSecretRef is a reference to a Secret containing the container registry pull secret
	// Secret must be in the same namespace as the DPFHCPBridge CR and contain key 'pullsecret'
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
	// +immutable
	// +optional
	VirtualIP string `json:"virtualIP,omitempty"`

	// NodeSelector defines the node selector for the hosted control plane pods
	// It specifies which nodes in the management cluster can host the control plane workloads
	// Default: {"node-role.kubernetes.io/master": ""} (schedules on master nodes)
	// This field is immutable.
	// +kubebuilder:validation:XValidation:rule="self == oldSelf",message="nodeSelector is immutable"
	// +kubebuilder:validation:XValidation:rule="size(self) <= 20",message="nodeSelector map can have at most 20 entries"
	// +immutable
	// +optional
	NodeSelector map[string]string `json:"nodeSelector,omitempty"`
}

// DPFHCPBridgePhase represents the lifecycle phase of the DPFHCPBridge
// +kubebuilder:validation:Enum=Pending;Provisioning;Ready;Failed;Deleting
type DPFHCPBridgePhase string

const (
	// PhasePending indicates initial state, validation in progress
	PhasePending DPFHCPBridgePhase = "Pending"

	// PhaseProvisioning indicates HostedCluster and related resources are being created
	PhaseProvisioning DPFHCPBridgePhase = "Provisioning"

	// PhaseReady indicates HostedCluster is operational, kubeconfig injected, CSR auto-approval active
	PhaseReady DPFHCPBridgePhase = "Ready"

	// PhaseFailed indicates permanent failure requiring user intervention
	PhaseFailed DPFHCPBridgePhase = "Failed"

	// PhaseDeleting indicates finalizer cleanup in progress
	PhaseDeleting DPFHCPBridgePhase = "Deleting"
)

// Condition types for DPFHCPBridge.
// These will be populated in the Conditions array as implementation progresses.
const (
	// Conditions mirrored from HostedCluster (with DPFHCPBridge-specific naming).

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

	// DPFHCPBridge-specific conditions.

	// Ready indicates the overall operational status of the DPFHCPBridge.
	Ready string = "Ready"

	// KubeConfigInjected indicates whether the kubeconfig was successfully injected into the DPUCluster CR.
	KubeConfigInjected string = "KubeConfigInjected"

	// HostedClusterCleanup indicates the status of HostedCluster deletion during finalizer cleanup.
	HostedClusterCleanup string = "HostedClusterCleanup"
)

// DPFHCPBridgeStatus defines the observed state of DPFHCPBridge
type DPFHCPBridgeStatus struct {
	// Phase represents the current lifecycle phase
	// +optional
	Phase DPFHCPBridgePhase `json:"phase,omitempty"`

	// Conditions represent the latest available observations of the DPFHCPBridge's state
	// +optional
	Conditions []metav1.Condition `json:"conditions,omitempty"`

	// HostedClusterRef is a reference to the created HostedCluster CR
	// +optional
	HostedClusterRef *corev1.ObjectReference `json:"hostedClusterRef,omitempty"`

	// KubeConfigSecretRef is a reference to the created kubeconfig Secret in the DPUCluster's namespace
	// +optional
	KubeConfigSecretRef *corev1.LocalObjectReference `json:"kubeConfigSecretRef,omitempty"`

	// BlueFieldContainerImage is the resolved BlueField container image URL
	// +optional
	BlueFieldContainerImage string `json:"blueFieldContainerImage,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Namespaced,shortName=dpfhcp
// +kubebuilder:printcolumn:name="Phase",type=string,JSONPath=`.status.phase`
// +kubebuilder:printcolumn:name="Ready",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`
// +kubebuilder:printcolumn:name="HostedCluster",type=string,JSONPath=`.status.hostedClusterRef.name`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

// DPFHCPBridge is the Schema for the dpfhcpbridges API
type DPFHCPBridge struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   DPFHCPBridgeSpec   `json:"spec,omitempty"`
	Status DPFHCPBridgeStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

// DPFHCPBridgeList contains a list of DPFHCPBridge
type DPFHCPBridgeList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []DPFHCPBridge `json:"items"`
}

func init() {
	SchemeBuilder.Register(&DPFHCPBridge{}, &DPFHCPBridgeList{})
}

// ShouldExposeThroughLoadBalancer determines whether to expose services via LoadBalancer or NodePort
// Returns true if:
// - ControlPlaneAvailabilityPolicy is HighlyAvailable (VIP is required in this case)
// - ControlPlaneAvailabilityPolicy is SingleReplica AND VirtualIP is provided
// Returns false if:
// - ControlPlaneAvailabilityPolicy is SingleReplica AND VirtualIP is not provided
func (b *DPFHCPBridge) ShouldExposeThroughLoadBalancer() bool {
	// If ControlPlane is HighlyAvailable, we must expose through LoadBalancer
	if b.Spec.ControlPlaneAvailabilityPolicy == hyperv1.HighlyAvailable {
		return true
	}

	// If ControlPlane is SingleReplica and VIP is provided, expose through LoadBalancer
	if b.Spec.ControlPlaneAvailabilityPolicy == hyperv1.SingleReplica && b.Spec.VirtualIP != "" {
		return true
	}

	// If ControlPlane is SingleReplica and no VIP, use NodePort
	return false
}

// IsVIPRequired determines if VirtualIP is required for the given configuration
// Returns true if ControlPlaneAvailabilityPolicy is HighlyAvailable
func (b *DPFHCPBridge) IsVIPRequired() bool {
	return b.Spec.ControlPlaneAvailabilityPolicy == hyperv1.HighlyAvailable
}
