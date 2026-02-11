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

package hostedcluster

import (
	"context"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// StatusSyncer manages status synchronization from HostedCluster to DPFHCPProvisioner
type StatusSyncer struct {
	client.Client
}

// NewStatusSyncer creates a new StatusSyncer
func NewStatusSyncer(c client.Client) *StatusSyncer {
	return &StatusSyncer{Client: c}
}

// SyncStatusFromHostedCluster mirrors HostedCluster status conditions to DPFHCPProvisioner status
// This function:
// - Only syncs status when hostedClusterRef is set in DPFHCPProvisioner status
// - Handles missing HostedCluster gracefully (may be creating or deleted)
//
// Returns ctrl.Result and error for reconciliation flow
func (ss *StatusSyncer) SyncStatusFromHostedCluster(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	// Only sync status if hostedClusterRef is set
	if cr.Status.HostedClusterRef == nil {
		log.V(1).Info("Skipping status sync - hostedClusterRef not set yet")
		return ctrl.Result{}, nil
	}

	// Get HostedCluster
	hc := &hyperv1.HostedCluster{}
	hcKey := types.NamespacedName{
		Name:      cr.Status.HostedClusterRef.Name,
		Namespace: cr.Status.HostedClusterRef.Namespace,
	}

	if err := ss.Get(ctx, hcKey, hc); err != nil {
		if apierrors.IsNotFound(err) {
			// HostedCluster not found - may be creating or deleted
			// Don't fail, just log and skip sync
			log.V(1).Info("HostedCluster not found, skipping status sync",
				"hostedCluster", hcKey.String())
			return ctrl.Result{}, nil
		}

		// Handle "no matches for kind" error (CRD not installed) gracefully
		if meta.IsNoMatchError(err) {
			log.V(1).Info("HostedCluster CRD not installed, skipping status sync",
				"hostedCluster", hcKey.String())
			return ctrl.Result{}, nil
		}

		// Other errors (network, RBAC, API server issues) should be retried
		log.Error(err, "Failed to get HostedCluster for status sync",
			"hostedCluster", hcKey.String())
		return ctrl.Result{}, err
	}

	// Check if HostedCluster status is populated yet
	if hc.Status.Conditions == nil || len(hc.Status.Conditions) == 0 {
		log.V(1).Info("HostedCluster status not yet populated, skipping sync",
			"hostedCluster", hcKey.String())
		// Don't requeue - the HostedCluster watch will trigger reconciliation when status changes.
		return ctrl.Result{}, nil
	}

	log.V(1).Info("Syncing status from HostedCluster",
		"hostedCluster", hcKey.String(),
		"conditions", len(hc.Status.Conditions))

	// Mirror conditions from HostedCluster to DPFHCPProvisioner
	ss.mirrorConditions(ctx, cr, hc)

	log.V(1).Info("Status sync completed successfully",
		"hostedCluster", hcKey.String())

	return ctrl.Result{}, nil
}

// mirrorConditions mirrors the 7 specific HostedCluster conditions to DPFHCPProvisioner
// This simply copies the condition status, reason, and message from HostedCluster to DPFHCPProvisioner
// No additional logic - just mirroring
func (ss *StatusSyncer) mirrorConditions(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, hc *hyperv1.HostedCluster) {
	log := logf.FromContext(ctx)

	// Map of HostedCluster condition types to DPFHCPProvisioner condition types
	// Key: HostedCluster condition type
	// Value: DPFHCPProvisioner condition type
	// Only mirror the 7 conditions specified in the DPFHCPProvisioner API
	conditionMappings := map[string]string{
		string(hyperv1.HostedClusterAvailable):         provisioningv1alpha1.HostedClusterAvailable,
		string(hyperv1.HostedClusterProgressing):       provisioningv1alpha1.HostedClusterProgressing,
		string(hyperv1.HostedClusterDegraded):          provisioningv1alpha1.HostedClusterDegraded,
		string(hyperv1.ValidReleaseInfo):               provisioningv1alpha1.ValidReleaseInfo,
		string(hyperv1.ValidReleaseImage):              provisioningv1alpha1.ValidReleaseImage,
		string(hyperv1.IgnitionEndpointAvailable):      provisioningv1alpha1.IgnitionEndpointAvailable,
		string(hyperv1.IgnitionServerValidReleaseInfo): provisioningv1alpha1.IgnitionServerValidReleaseInfo,
	}

	// Mirror each HostedCluster condition to DPFHCPProvisioner
	for hcCondType, dpfCondType := range conditionMappings {
		hcCond := meta.FindStatusCondition(hc.Status.Conditions, hcCondType)
		if hcCond != nil {
			// Found the condition, mirror it
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:               dpfCondType,
				Status:             hcCond.Status,
				Reason:             hcCond.Reason,
				Message:            hcCond.Message,
				ObservedGeneration: cr.Generation,
			})
			log.V(2).Info("Mirrored condition from HostedCluster",
				"conditionType", dpfCondType,
				"status", hcCond.Status,
				"reason", hcCond.Reason)
		}
	}
}
