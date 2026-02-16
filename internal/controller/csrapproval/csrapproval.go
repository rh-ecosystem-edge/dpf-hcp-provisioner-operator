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
	"time"

	certv1 "k8s.io/api/certificates/v1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

const (
	// CSRPollingInterval is how often to check for pending CSRs in the hosted cluster
	// Polls continuously to handle bootstrap CSRs, serving CSRs, and edge cases like node recreation
	CSRPollingInterval = 30 * time.Second
)

// CSRApprover manages CSR auto-approval for a DPFHCPProvisioner
type CSRApprover struct {
	mgmtClient    client.Client // Client for management cluster (where operator runs)
	recorder      record.EventRecorder
	clientManager *ClientManager // Manages cached clients to hosted clusters
}

// NewCSRApprover creates a new CSR approver
func NewCSRApprover(mgmtClient client.Client, recorder record.EventRecorder) *CSRApprover {
	return &CSRApprover{
		mgmtClient:    mgmtClient,
		recorder:      recorder,
		clientManager: NewClientManager(mgmtClient),
	}
}

// ProcessCSRs processes and approves pending CSRs for a DPFHCPProvisioner
// Called during each reconciliation loop to handle CSRs from worker nodes
func (a *CSRApprover) ProcessCSRs(ctx context.Context, dpfhcp *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	// Step 1: Verify prerequisites (kubeconfig secret exists and is populated)
	if !a.isKubeconfigAvailable(ctx, dpfhcp) {
		log.V(1).Info("Kubeconfig secret not available, CSR auto-approval cannot start")
		if err := a.setCondition(ctx, dpfhcp, metav1.ConditionFalse, provisioningv1alpha1.ReasonKubeconfigNotAvailable, "Kubeconfig secret not available for hosted cluster"); err != nil {
			return ctrl.Result{}, err
		}
		// Don't requeue - reconciliation will be triggered when main controller updates DPFHCPProvisioner status
		// (main controller watches kubeconfig secrets and updates DPFHCPProvisioner.Status.KubeConfigSecretRef)
		return ctrl.Result{}, nil
	}

	// Step 2: Get or create hosted cluster client
	hcClient, err := a.clientManager.GetHostedClusterClient(ctx, dpfhcp.Namespace, dpfhcp.Name)
	if err != nil {
		log.Error(err, "Failed to create hosted cluster client")
		if err := a.setCondition(ctx, dpfhcp, metav1.ConditionFalse, provisioningv1alpha1.ReasonHostedClusterNotReachable, fmt.Sprintf("Cannot connect to hosted cluster: %v", err)); err != nil {
			return ctrl.Result{}, err
		}
		a.recorder.Event(dpfhcp, corev1.EventTypeWarning, "HostedClusterUnreachable", fmt.Sprintf("Cannot connect to hosted cluster: %v", err))
		// Don't requeue - reconciliation will be triggered when main controller updates DPFHCPProvisioner status
		// (main controller watches HostedCluster and updates DPFHCPProvisioner.Status.HostedClusterAvailable)
		return ctrl.Result{}, nil
	}

	// Step 3: Test connection
	if err := TestConnection(ctx, hcClient); err != nil {
		log.Error(err, "Hosted cluster not reachable")
		// Invalidate cached client so next reconciliation creates a fresh one
		// This handles cases like expired credentials, kubeconfig rotation, etc.
		a.clientManager.InvalidateClient(dpfhcp.Namespace, dpfhcp.Name)
		if err := a.setCondition(ctx, dpfhcp, metav1.ConditionFalse, provisioningv1alpha1.ReasonHostedClusterNotReachable, fmt.Sprintf("Cannot connect to hosted cluster: %v", err)); err != nil {
			return ctrl.Result{}, err
		}
		a.recorder.Event(dpfhcp, corev1.EventTypeWarning, "HostedClusterUnreachable", fmt.Sprintf("Cannot connect to hosted cluster: %v", err))
		// Don't requeue - reconciliation will be triggered when main controller updates DPFHCPProvisioner status
		// (main controller watches HostedCluster and updates DPFHCPProvisioner.Status.HostedClusterAvailable)
		return ctrl.Result{}, nil
	}

	// Step 4: Process any pending CSRs
	if err := a.processPendingCSRs(ctx, dpfhcp, hcClient); err != nil {
		log.Error(err, "Failed to process pending CSRs")
		// Transient error during normal operation - don't fail the reconciliation
		// Continue polling to retry CSR processing
		if condErr := a.setCondition(ctx, dpfhcp, metav1.ConditionTrue, provisioningv1alpha1.ReasonCSRApprovalActive,
			fmt.Sprintf("CSR processing encountered transient error: %v", err)); condErr != nil {
			return ctrl.Result{}, condErr
		}
		// Retry after polling interval (assume CSRs might be pending)
		return ctrl.Result{RequeueAfter: CSRPollingInterval}, nil
	}

	// Step 5: Update condition and schedule next poll
	if err := a.setCondition(ctx, dpfhcp, metav1.ConditionTrue, provisioningv1alpha1.ReasonCSRApprovalActive,
		"CSR auto-approval active for worker nodes"); err != nil {
		return ctrl.Result{}, err
	}

	// Always poll every 30 seconds to check for new CSRs
	// This handles all cases: initial bootstrap CSRs, serving CSRs after bootstrap, node recreation, etc.
	return ctrl.Result{RequeueAfter: CSRPollingInterval}, nil
}

// StopCSRWatch cleans up resources for a DPFHCPProvisioner
// Called during finalizer cleanup
func (a *CSRApprover) StopCSRWatch(ctx context.Context, dpfhcp *provisioningv1alpha1.DPFHCPProvisioner) {
	if a == nil || a.clientManager == nil || dpfhcp == nil {
		// Approver not initialized or CR is nil, nothing to stop
		return
	}

	log := logf.FromContext(ctx)
	log.Info("Cleaning up CSR approver resources")

	// Invalidate cached client
	a.clientManager.InvalidateClient(dpfhcp.Namespace, dpfhcp.Name)
}

// isKubeconfigAvailable checks if the kubeconfig secret exists and is populated
func (a *CSRApprover) isKubeconfigAvailable(ctx context.Context, dpfhcp *provisioningv1alpha1.DPFHCPProvisioner) bool {
	_, err := a.clientManager.getKubeconfigData(ctx, dpfhcp.Namespace, dpfhcp.Name)
	return err == nil
}

// processPendingCSRs processes all pending CSRs in the hosted cluster
func (a *CSRApprover) processPendingCSRs(ctx context.Context, dpfhcp *provisioningv1alpha1.DPFHCPProvisioner, hcClient *kubernetes.Clientset) error {
	log := logf.FromContext(ctx)

	// List all CSRs
	csrList, err := hcClient.CertificatesV1().CertificateSigningRequests().List(ctx, metav1.ListOptions{})
	if err != nil {
		return fmt.Errorf("failed to list CSRs: %w", err)
	}

	// Filter to pending CSRs
	pendingCSRs := make([]certv1.CertificateSigningRequest, 0)
	for _, csr := range csrList.Items {
		if IsPending(&csr) {
			pendingCSRs = append(pendingCSRs, csr)
		}
	}

	if len(pendingCSRs) == 0 {
		log.V(1).Info("No pending CSRs found")
		return nil
	}

	log.Info("Found pending CSRs", "count", len(pendingCSRs))

	// Filter to bootstrap and serving CSRs only
	bootstrapCSRs := make([]certv1.CertificateSigningRequest, 0)
	servingCSRs := make([]certv1.CertificateSigningRequest, 0)
	otherCSRs := make([]certv1.CertificateSigningRequest, 0)
	for _, csr := range pendingCSRs {
		switch csr.Spec.SignerName {
		case SignerBootstrap:
			bootstrapCSRs = append(bootstrapCSRs, csr)
		case SignerServing:
			servingCSRs = append(servingCSRs, csr)
		default:
			otherCSRs = append(otherCSRs, csr)
		}
	}

	log.Info("Filtered CSRs by signer",
		"bootstrapCount", len(bootstrapCSRs),
		"servingCount", len(servingCSRs),
		"otherCount", len(otherCSRs))

	// Log other signers for debugging
	for _, csr := range otherCSRs {
		log.Info("Skipping CSR with unsupported signer",
			"csrName", csr.Name,
			"signerName", csr.Spec.SignerName)
	}

	// Process bootstrap CSRs
	for _, csr := range bootstrapCSRs {
		err := a.processCSR(ctx, dpfhcp, hcClient, &csr, "bootstrap")
		if err != nil {
			// Actual error - log and continue
			log.Error(err, "Failed to process bootstrap CSR", "csrName", csr.Name)
			continue
		}
	}

	// Process serving CSRs
	for _, csr := range servingCSRs {
		err := a.processCSR(ctx, dpfhcp, hcClient, &csr, "serving")
		if err != nil {
			// Actual error - log and continue
			log.Error(err, "Failed to process serving CSR", "csrName", csr.Name)
			continue
		}
	}

	return nil
}

// processCSR processes a single CSR (validate and approve if valid)
// Returns:
// - nil: CSR was successfully approved or skipped (validation failed but not an error condition)
// - error: Actual error occurred during processing
func (a *CSRApprover) processCSR(ctx context.Context, dpfhcp *provisioningv1alpha1.DPFHCPProvisioner, hcClient *kubernetes.Clientset, csr *certv1.CertificateSigningRequest, csrType string) error {
	log := logf.FromContext(ctx)

	// Extract hostname from CSR
	hostname, err := ExtractHostname(csr)
	if err != nil {
		log.Info("Skipping CSR: cannot extract hostname", "csrName", csr.Name, "error", err.Error())
		return nil
	}

	log.Info("Processing CSR", "csrName", csr.Name, "hostname", hostname, "type", csrType)

	// Perform comprehensive CSR validation based on type
	switch csr.Spec.SignerName {
	case SignerBootstrap:
		if err := ValidateBootstrapCSR(csr, hostname); err != nil {
			log.Info("Skipping CSR: bootstrap validation failed", "csrName", csr.Name, "hostname", hostname, "error", err.Error())
			return nil
		}
	case SignerServing:
		if err := ValidateServingCSR(csr, hostname); err != nil {
			log.Info("Skipping CSR: serving validation failed", "csrName", csr.Name, "hostname", hostname, "error", err.Error())
			return nil
		}
	}

	// Validate CSR ownership (check if DPU exists and node state matches CSR type)
	validator := NewValidator(a.mgmtClient, hcClient, dpfhcp.Spec.DPUClusterRef.Namespace)
	isBootstrapCSR := csr.Spec.SignerName == SignerBootstrap
	result, err := validator.ValidateCSROwner(ctx, hostname, isBootstrapCSR)
	if err != nil {
		return fmt.Errorf("failed to validate CSR ownership: %w", err)
	}
	if !result.Valid {
		log.Info("Skipping CSR: owner validation failed", "csrName", csr.Name, "hostname", hostname, "reason", result.Reason)
		return nil
	}

	// Approve the CSR
	if err := a.approveCSR(ctx, hcClient, csr, hostname, csrType); err != nil {
		return fmt.Errorf("failed to approve CSR: %w", err)
	}

	// Emit event for successful approval
	eventMsg := fmt.Sprintf("Approved %s CSR for DPU worker node %s", csrType, hostname)
	a.recorder.Event(dpfhcp, corev1.EventTypeNormal, "CSRApproved", eventMsg)

	log.Info("CSR approved", "csrName", csr.Name, "hostname", hostname, "type", csrType)
	return nil
}

// approveCSR approves a CSR by updating its status
func (a *CSRApprover) approveCSR(ctx context.Context, hcClient *kubernetes.Clientset, csr *certv1.CertificateSigningRequest, hostname, csrType string) error {
	// Create approval condition
	csr.Status.Conditions = append(csr.Status.Conditions, certv1.CertificateSigningRequestCondition{
		Type:           certv1.CertificateApproved,
		Status:         corev1.ConditionTrue,
		Reason:         "AutoApproved",
		Message:        fmt.Sprintf("Auto-approved %s CSR for DPU worker node %s", csrType, hostname),
		LastUpdateTime: metav1.Now(),
	})

	// Update CSR status
	_, err := hcClient.CertificatesV1().CertificateSigningRequests().UpdateApproval(ctx, csr.Name, csr, metav1.UpdateOptions{})
	return err
}

// setCondition updates the CSRAutoApprovalActive condition
func (a *CSRApprover) setCondition(ctx context.Context, dpfhcp *provisioningv1alpha1.DPFHCPProvisioner, status metav1.ConditionStatus, reason, message string) error {
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.CSRAutoApprovalActive,
		Status:             status,
		Reason:             reason,
		Message:            message,
		ObservedGeneration: dpfhcp.Generation,
	}

	// This handles finding/updating/appending the condition correctly
	if changed := meta.SetStatusCondition(&dpfhcp.Status.Conditions, condition); changed {
		// Persist status update first; only emit event on success
		if err := a.mgmtClient.Status().Update(ctx, dpfhcp); err != nil {
			return err
		}

		// Emit event only when condition changed and persisted successfully (avoid spam and ghost events)
		eventType := corev1.EventTypeNormal
		if status == metav1.ConditionFalse {
			eventType = corev1.EventTypeWarning
		}
		a.recorder.Event(dpfhcp, eventType, reason, message)
	}

	return nil
}
