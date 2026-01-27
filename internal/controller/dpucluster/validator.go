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

package dpucluster

import (
	"context"
	"fmt"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

const (
	// Event reasons
	ReasonDPUClusterFound        = "DPUClusterFound"
	ReasonDPUClusterNotFound     = "DPUClusterNotFound"
	ReasonDPUClusterDeleted      = "DPUClusterDeleted"
	ReasonDPUClusterAccessDenied = "DPUClusterAccessDenied"
	ReasonClusterTypeUnsupported = "ClusterTypeUnsupported"
	ReasonClusterTypeValid       = "ClusterTypeValid"
	ReasonDPUClusterInUse        = "DPUClusterInUse"
	ReasonDPUClusterAvailable    = "DPUClusterAvailable"
)

// Validator validates DPUCluster references and updates status accordingly
type Validator struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewValidator creates a new DPUCluster validator
func NewValidator(client client.Client, recorder record.EventRecorder) *Validator {
	return &Validator{
		client:   client,
		recorder: recorder,
	}
}

// ValidateDPUCluster validates that the referenced DPUCluster exists
func (v *Validator) ValidateDPUCluster(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-validation")

	// Get reference to DPUCluster
	dpuClusterRef := cr.Spec.DPUClusterRef
	log.V(1).Info("Validating DPUCluster reference",
		"dpuClusterName", dpuClusterRef.Name,
		"dpuClusterNamespace", dpuClusterRef.Namespace)

	// Attempt to fetch the DPUCluster
	var dpuCluster dpuprovisioningv1alpha1.DPUCluster
	err := v.client.Get(ctx, types.NamespacedName{
		Name:      dpuClusterRef.Name,
		Namespace: dpuClusterRef.Namespace,
	}, &dpuCluster)

	if err != nil {
		if apierrors.IsNotFound(err) {
			// DPUCluster not found - set DPUClusterMissing=True
			return v.handleDPUClusterMissing(ctx, cr, dpuClusterRef)
		}

		if apierrors.IsForbidden(err) {
			// RBAC permission denied - permanent error
			return v.handleDPUClusterAccessDenied(ctx, cr, dpuClusterRef, err)
		}

		// Other transient errors (network timeout, API server error)
		log.V(1).Info("Transient error fetching DPUCluster, will retry",
			"error", err.Error())
		return ctrl.Result{Requeue: true}, err
	}

	// DPUCluster exists - validate cluster type is not kamaji
	if result, err := v.validateClusterType(ctx, cr, &dpuCluster); err != nil || result.Requeue || result.RequeueAfter > 0 {
		return result, err
	}

	// DPUCluster exists and type is valid - validate exclusivity (not in use by another DPFHCPBridge)
	if result, err := v.validateDPUClusterExclusivity(ctx, cr, &dpuCluster); err != nil || result.Requeue || result.RequeueAfter > 0 {
		return result, err
	}

	// DPUCluster exists, type is valid, and not in use - set DPUClusterMissing=False
	return v.handleDPUClusterFound(ctx, cr, &dpuCluster)
}

// validateClusterType validates that DPUCluster.Spec.Type is not kamaji
// This operator only supports non-Kamaji cluster types
func (v *Validator) validateClusterType(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuCluster *dpuprovisioningv1alpha1.DPUCluster) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-validation")

	log.V(1).Info("Validating cluster type",
		"dpuClusterType", dpuCluster.Spec.Type)

	if dpuCluster.Spec.Type == string(dpuprovisioningv1alpha1.KamajiCluster) {
		log.Error(nil, "Kamaji cluster type is not supported",
			"dpuClusterType", dpuCluster.Spec.Type)
		return v.handleClusterTypeInvalid(ctx, cr, dpuCluster)
	}

	// Type is valid (not kamaji) - set ClusterTypeValid=True
	return v.handleClusterTypeValid(ctx, cr, dpuCluster)
}

// handleClusterTypeInvalid handles the case when DPUCluster type is kamaji (unsupported)
func (v *Validator) handleClusterTypeInvalid(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuCluster *dpuprovisioningv1alpha1.DPUCluster) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-validation")

	// Note: Phase will be computed from conditions by the reconciler

	message := fmt.Sprintf("DPUCluster '%s' has unsupported type '%s'. This operator only supports non-Kamaji cluster types",
		dpuCluster.Name, dpuCluster.Spec.Type)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.ClusterTypeValid,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonClusterTypeUnsupported,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}
	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonClusterTypeUnsupported, message)
		log.Info("Unsupported cluster type detected",
			"dpuClusterType", dpuCluster.Spec.Type)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Do NOT requeue - permanent error requiring user to reference a different DPUCluster
	return ctrl.Result{}, nil
}

// handleClusterTypeValid handles the case when DPUCluster type is valid (not kamaji)
func (v *Validator) handleClusterTypeValid(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuCluster *dpuprovisioningv1alpha1.DPUCluster) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-validation")

	// Note: Phase will be computed from conditions by the reconciler

	message := fmt.Sprintf("DPUCluster type '%s' is supported", dpuCluster.Spec.Type)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.ClusterTypeValid,
		Status:             metav1.ConditionTrue,
		Reason:             ReasonClusterTypeValid,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed (e.g., recovered from unsupported type)
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeNormal, ReasonClusterTypeValid, message)
		log.Info("ClusterType validated",
			"dpuClusterType", dpuCluster.Spec.Type)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Success - continue with reconciliation
	return ctrl.Result{}, nil
}

// validateDPUClusterExclusivity validates that the DPUCluster is not already in use by another DPFHCPBridge
// Ensures 1:1 relationship between DPFHCPBridge and DPUCluster
func (v *Validator) validateDPUClusterExclusivity(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuCluster *dpuprovisioningv1alpha1.DPUCluster) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-exclusivity")

	log.V(1).Info("Validating DPUCluster exclusivity",
		"dpuClusterName", dpuCluster.Name,
		"dpuClusterNamespace", dpuCluster.Namespace)

	// List all DPFHCPBridge resources in the cluster
	var bridgeList provisioningv1alpha1.DPFHCPBridgeList
	if err := v.client.List(ctx, &bridgeList); err != nil {
		log.Error(err, "Failed to list DPFHCPBridge resources")
		return ctrl.Result{Requeue: true}, err
	}

	// Check if any OTHER DPFHCPBridge references the same DPUCluster
	for _, bridge := range bridgeList.Items {
		// Skip the current DPFHCPBridge (compare by namespace/name)
		if bridge.Namespace == cr.Namespace && bridge.Name == cr.Name {
			continue
		}

		// Check if this bridge references the same DPUCluster
		if bridge.Spec.DPUClusterRef.Name == dpuCluster.Name &&
			bridge.Spec.DPUClusterRef.Namespace == dpuCluster.Namespace {
			// Found another DPFHCPBridge using this DPUCluster
			return v.handleDPUClusterInUse(ctx, cr, dpuCluster, &bridge)
		}
	}

	// No other DPFHCPBridge is using this DPUCluster - it's available
	return v.handleDPUClusterAvailable(ctx, cr, dpuCluster)
}

// handleDPUClusterInUse handles the case when DPUCluster is already in use by another DPFHCPBridge
func (v *Validator) handleDPUClusterInUse(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuCluster *dpuprovisioningv1alpha1.DPUCluster, conflictingBridge *provisioningv1alpha1.DPFHCPBridge) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-exclusivity")

	message := fmt.Sprintf("DPUCluster '%s/%s' is already in use by DPFHCPBridge '%s/%s'. Each DPUCluster can only be referenced by one DPFHCPBridge",
		dpuCluster.Namespace, dpuCluster.Name,
		conflictingBridge.Namespace, conflictingBridge.Name)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.DPUClusterInUse,
		Status:             metav1.ConditionTrue,
		Reason:             ReasonDPUClusterInUse,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonDPUClusterInUse, message)
		log.Info("DPUCluster already in use",
			"dpuClusterName", dpuCluster.Name,
			"dpuClusterNamespace", dpuCluster.Namespace,
			"conflictingBridge", conflictingBridge.Namespace+"/"+conflictingBridge.Name)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Do NOT requeue - permanent error requiring user to reference a different DPUCluster
	return ctrl.Result{}, nil
}

// handleDPUClusterAvailable handles the case when DPUCluster is available (not in use)
func (v *Validator) handleDPUClusterAvailable(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuCluster *dpuprovisioningv1alpha1.DPUCluster) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-exclusivity")

	message := fmt.Sprintf("DPUCluster '%s/%s' is available (not in use by another DPFHCPBridge)",
		dpuCluster.Namespace, dpuCluster.Name)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.DPUClusterInUse,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonDPUClusterAvailable,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed (e.g., recovered from in-use state)
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeNormal, ReasonDPUClusterAvailable, message)
		log.Info("DPUCluster is available",
			"dpuClusterName", dpuCluster.Name,
			"dpuClusterNamespace", dpuCluster.Namespace)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Success - continue with reconciliation
	return ctrl.Result{}, nil
}

// handleDPUClusterMissing handles the case when DPUCluster is not found
func (v *Validator) handleDPUClusterMissing(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuClusterRef provisioningv1alpha1.DPUClusterReference) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-validation")

	// Get previous condition to determine if this is a new error or deletion
	previousCondition := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.DPUClusterMissing)

	// Determine message based on whether cluster was previously found
	var message string
	var reason string
	if previousCondition != nil && previousCondition.Status == metav1.ConditionFalse {
		// DPUCluster was previously found but now deleted
		message = fmt.Sprintf("Referenced DPUCluster '%s' in namespace '%s' has been deleted. Please delete this DPFHCPBridge to clean up",
			dpuClusterRef.Name, dpuClusterRef.Namespace)
		reason = ReasonDPUClusterDeleted
	} else {
		// DPUCluster never existed or still missing
		message = fmt.Sprintf("Referenced DPUCluster '%s' not found in namespace '%s'",
			dpuClusterRef.Name, dpuClusterRef.Namespace)
		reason = ReasonDPUClusterNotFound
	}

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.DPUClusterMissing,
		Status:             metav1.ConditionTrue,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, reason, message)
		log.Info("DPUCluster not found",
			"dpuClusterName", dpuClusterRef.Name,
			"dpuClusterNamespace", dpuClusterRef.Namespace,
			"reason", reason)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Do NOT requeue - this is a permanent error requiring manual intervention
	// User must either create the DPUCluster or delete the DPFHCPBridge
	return ctrl.Result{}, nil
}

// handleDPUClusterAccessDenied handles RBAC permission errors
func (v *Validator) handleDPUClusterAccessDenied(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuClusterRef provisioningv1alpha1.DPUClusterReference, err error) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-validation")

	message := fmt.Sprintf("Operator lacks RBAC permissions to access DPUCluster '%s' in namespace '%s': %v",
		dpuClusterRef.Name, dpuClusterRef.Namespace, err)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.DPUClusterMissing,
		Status:             metav1.ConditionTrue,
		Reason:             ReasonDPUClusterAccessDenied,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeWarning, ReasonDPUClusterAccessDenied, message)
		log.Error(err, "RBAC permission denied for DPUCluster",
			"dpuClusterName", dpuClusterRef.Name,
			"dpuClusterNamespace", dpuClusterRef.Namespace)
	}

	// Update status
	if updateErr := v.client.Status().Update(ctx, cr); updateErr != nil {
		log.Error(updateErr, "Failed to update status")
		return ctrl.Result{}, updateErr
	}

	// Do NOT requeue - permanent error requiring admin to fix RBAC
	return ctrl.Result{}, nil
}

// handleDPUClusterFound handles the case when DPUCluster is found
func (v *Validator) handleDPUClusterFound(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge, dpuCluster *dpuprovisioningv1alpha1.DPUCluster) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("feature", "dpucluster-validation")

	message := fmt.Sprintf("DPUCluster '%s' found in namespace '%s'",
		dpuCluster.Name, dpuCluster.Namespace)

	// Set condition and check if it changed
	condition := metav1.Condition{
		Type:               provisioningv1alpha1.DPUClusterMissing,
		Status:             metav1.ConditionFalse,
		Reason:             ReasonDPUClusterFound,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: cr.Generation,
	}

	// Emit event only if condition changed (e.g., recovered from missing state)
	if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
		v.recorder.Event(cr, corev1.EventTypeNormal, ReasonDPUClusterFound, message)
		log.Info("DPUCluster found",
			"dpuClusterName", dpuCluster.Name,
			"dpuClusterNamespace", dpuCluster.Namespace)
	}

	// Update status
	if err := v.client.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status")
		return ctrl.Result{}, err
	}

	// Success - continue with reconciliation
	return ctrl.Result{}, nil
}
