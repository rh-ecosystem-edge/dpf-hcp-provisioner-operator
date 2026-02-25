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

package csrapproval

import (
	"context"
	"fmt"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"sigs.k8s.io/controller-runtime/pkg/client"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
)

// ValidationResult contains the result of CSR validation
type ValidationResult struct {
	Valid  bool
	Reason string
}

// Validator validates CSRs against DPU and Node state
type Validator struct {
	mgmtClient   client.Client        // Client for management cluster (where DPU objects live)
	hostedClient kubernetes.Interface // Client for hosted cluster (where CSRs and Nodes live)
	dpuNamespace string               // Namespace where DPU objects are located
}

// NewValidator creates a new CSR validator
func NewValidator(mgmtClient client.Client, hostedClient kubernetes.Interface, dpuNamespace string) *Validator {
	return &Validator{
		mgmtClient:   mgmtClient,
		hostedClient: hostedClient,
		dpuNamespace: dpuNamespace,
	}
}

// ValidateCSROwner validates CSR ownership by checking:
// 1. DPU object with matching hostname exists in management cluster (ownership check)
// 2. DPU is in the correct phase for CSR approval (security check - time-limited window)
// 3. For bootstrap CSRs: DPU must be in "DPU Cluster Config" phase (joining cluster)
// 4. For serving CSRs: DPU must be in "DPU Cluster Config" OR "Ready" phase.
func (v *Validator) ValidateCSROwner(ctx context.Context, hostname string, isBootstrapCSR bool) (*ValidationResult, error) {
	// Check if DPU exists and get its phase
	dpu, err := v.getDPU(ctx, hostname)
	if err != nil {
		return nil, fmt.Errorf("failed to get DPU: %w", err)
	}

	if dpu == nil {
		return &ValidationResult{
			Valid:  false,
			Reason: fmt.Sprintf("no matching DPU found for hostname %s in namespace %s", hostname, v.dpuNamespace),
		}, nil
	}

	// Validate DPU phase for CSR approval (security: time-limited approval window)
	if isBootstrapCSR {
		// Bootstrap CSR: Only approve during "DPU Cluster Config" phase (node is joining)
		// CSRs can only be approved when the DPU is actively being provisioned
		if dpu.Status.Phase != dpuprovisioningv1alpha1.DPUClusterConfig {
			return &ValidationResult{
				Valid:  false,
				Reason: fmt.Sprintf("DPU %s is in phase %s, bootstrap CSRs only approved in phase %s", hostname, dpu.Status.Phase, dpuprovisioningv1alpha1.DPUClusterConfig),
			}, nil
		}
		return &ValidationResult{
			Valid:  true,
			Reason: fmt.Sprintf("DPU in %s phase, bootstrap CSR approved", dpuprovisioningv1alpha1.DPUClusterConfig),
		}, nil
	}

	// Serving CSR: Allow in "DPU Cluster Config" or "Ready"
	if dpu.Status.Phase != dpuprovisioningv1alpha1.DPUClusterConfig && dpu.Status.Phase != dpuprovisioningv1alpha1.DPUReady {
		return &ValidationResult{
			Valid:  false,
			Reason: fmt.Sprintf("DPU %s is in phase %s, serving CSRs only approved in phases %s or %s", hostname, dpu.Status.Phase, dpuprovisioningv1alpha1.DPUClusterConfig, dpuprovisioningv1alpha1.DPUReady),
		}, nil
	}

	// For serving CSRs, also verify Node exists (should have joined via bootstrap CSR)
	nodeExists, err := v.nodeExists(ctx, hostname)
	if err != nil {
		return nil, fmt.Errorf("failed to check Node existence: %w", err)
	}

	if !nodeExists {
		return &ValidationResult{
			Valid:  false,
			Reason: fmt.Sprintf("node %s does not exist yet in hosted cluster", hostname),
		}, nil
	}

	return &ValidationResult{
		Valid:  true,
		Reason: fmt.Sprintf("DPU in %s phase and node exists, serving CSR approved", dpu.Status.Phase),
	}, nil
}

// getDPU retrieves a DPU object by hostname from the management cluster
func (v *Validator) getDPU(ctx context.Context, hostname string) (*dpuprovisioningv1alpha1.DPU, error) {
	dpuList := &dpuprovisioningv1alpha1.DPUList{}
	if err := v.mgmtClient.List(ctx, dpuList, client.InNamespace(v.dpuNamespace)); err != nil {
		return nil, err
	}

	for i := range dpuList.Items {
		if dpuList.Items[i].Name == hostname {
			return &dpuList.Items[i], nil
		}
	}

	return nil, nil
}

// nodeExists checks if a Node with the given hostname exists in the hosted cluster
func (v *Validator) nodeExists(ctx context.Context, hostname string) (bool, error) {
	nodeList, err := v.hostedClient.CoreV1().Nodes().List(ctx, metav1.ListOptions{})
	if err != nil {
		return false, err
	}

	for _, node := range nodeList.Items {
		if node.Name == hostname {
			return true, nil
		}
	}

	return false, nil
}
