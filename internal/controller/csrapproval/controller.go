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

	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// CSRApprovalReconciler is a dedicated controller for CSR auto-approval
// This controller runs independently from the main DPFHCPProvisioner controller
// and continuously polls the hosted cluster for pending CSRs
type CSRApprovalReconciler struct {
	client.Client
	Scheme   *runtime.Scheme
	Approver *CSRApprover
}

// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisioners,verbs=get;list;watch
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisioners/status,verbs=get;update;patch
// +kubebuilder:rbac:groups="",resources=events,verbs=create;patch
// +kubebuilder:rbac:groups="",resources=secrets,verbs=get;list;watch
// +kubebuilder:rbac:groups=provisioning.dpu.nvidia.com,resources=dpus,verbs=get;list;watch

// Reconcile handles CSR auto-approval for a DPFHCPProvisioner
// This controller is solely responsible for:
// 1. Connecting to the hosted cluster
// 2. Polling for pending CSRs
// 3. Validating and approving CSRs for DPU worker nodes
// 4. Updating the CSRAutoApprovalActive condition
func (r *CSRApprovalReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := logf.FromContext(ctx)
	log.Info("CSR Approval Controller reconciling", "namespace", req.Namespace, "name", req.Name)

	// Fetch the DPFHCPProvisioner CR
	var cr provisioningv1alpha1.DPFHCPProvisioner
	if err := r.Get(ctx, req.NamespacedName, &cr); err != nil {
		// CR not found - likely deleted
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Handle deletion - stop CSR watch
	if !cr.DeletionTimestamp.IsZero() {
		log.Info("DPFHCPProvisioner is being deleted, stopping CSR watch")
		if r.Approver != nil {
			r.Approver.StopCSRWatch(ctx, &cr)
		}
		return ctrl.Result{}, nil
	}

	// Only process if kubeconfig is available
	// CSR approval requires kubeconfig to connect to hosted cluster
	if cr.Status.KubeConfigSecretRef == nil {
		log.V(1).Info("Kubeconfig not available yet, skipping CSR approval")
		return ctrl.Result{}, nil
	}

	// Process CSRs - this is the ONLY thing this controller does
	// The CSRApprover will:
	// 1. Connect to hosted cluster using kubeconfig
	// 2. List and filter pending CSRs
	// 3. Validate CSRs against DPU objects
	// 4. Approve valid CSRs
	// 5. Update CSRAutoApprovalActive condition
	if r.Approver == nil {
		log.V(1).Info("Approver not initialized, skipping CSR approval")
		return ctrl.Result{}, nil
	}
	result, err := r.Approver.ProcessCSRs(ctx, &cr)
	if err != nil {
		log.Error(err, "Failed to process CSRs")
		// Continue polling even on error - CSR approval is best-effort
		// The next reconciliation will retry
		return ctrl.Result{RequeueAfter: CSRPollingInterval}, nil
	}

	log.V(1).Info("CSR approval reconciliation complete", "requeueAfter", result.RequeueAfter)
	return result, nil
}

// SetupWithManager sets up the controller with the Manager
func (r *CSRApprovalReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&provisioningv1alpha1.DPFHCPProvisioner{}).
		Named("csrapproval").
		Complete(r)
}
