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

import metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

const (
	// DefaultConfigName is the well-known singleton name for DPFHCPProvisionerConfig
	DefaultConfigName = "default"
)

// DPFHCPProvisionerConfigSpec defines the operator-wide configuration
type DPFHCPProvisionerConfigSpec struct {
	// BlueFieldOCPLayerRepo is the container registry repository for BlueField OCP layer images.
	// The operator queries this repository to find an image tag matching the OCP version.
	// TODO: Replace with the official registry once we have one
	// +kubebuilder:default="quay.io/eelgaev/rhcos-bfb"
	// +optional
	BlueFieldOCPLayerRepo string `json:"blueFieldOCPLayerRepo,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:resource:scope=Cluster,shortName=dpfhcpconfig
// +kubebuilder:validation:XValidation:rule="self.metadata.name == 'default'",message="DPFHCPProvisionerConfig must be named 'default'"
// +kubebuilder:printcolumn:name="BlueFieldOCPLayerRepo",type=string,JSONPath=`.spec.blueFieldOCPLayerRepo`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

// DPFHCPProvisionerConfig is the cluster-scoped singleton configuration for the DPF HCP Provisioner operator.
// Only one instance named "default" is allowed.
type DPFHCPProvisionerConfig struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec DPFHCPProvisionerConfigSpec `json:"spec,omitempty"`
}

// +kubebuilder:object:root=true

// DPFHCPProvisionerConfigList contains a list of DPFHCPProvisionerConfig
type DPFHCPProvisionerConfigList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []DPFHCPProvisionerConfig `json:"items"`
}

func init() {
	SchemeBuilder.Register(&DPFHCPProvisionerConfig{}, &DPFHCPProvisionerConfigList{})
}
