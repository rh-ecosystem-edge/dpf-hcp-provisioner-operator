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

package controller

import (
	"context"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/bluefield"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/dpucluster"
)

// DPFHCPBridgeReconciler reconciles a DPFHCPBridge object
type DPFHCPBridgeReconciler struct {
	client.Client
	Scheme              *runtime.Scheme
	Recorder            record.EventRecorder
	ImageResolver       *bluefield.ImageResolver
	DPUClusterValidator *dpucluster.Validator
}

// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpbridges,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpbridges/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpbridges/finalizers,verbs=update
// +kubebuilder:rbac:groups=coordination.k8s.io,resources=leases,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups="",resources=events,verbs=create;patch
// +kubebuilder:rbac:groups="",resources=configmaps,verbs=get;list;watch
// +kubebuilder:rbac:groups=provisioning.dpu.nvidia.com,resources=dpuclusters,verbs=get;list;watch

// Reconcile is part of the main kubernetes reconciliation loop which aims to
// move the current state of the cluster closer to the desired state.
//
// For more details, check Reconcile and its Result here:
// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.21.0/pkg/reconcile
func (r *DPFHCPBridgeReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := logf.FromContext(ctx)
	log.Info("Reconciling DPFHCPBridge", "namespace", req.Namespace, "name", req.Name)

	// Fetch the DPFHCPBridge CR
	var cr provisioningv1alpha1.DPFHCPBridge
	if err := r.Get(ctx, req.NamespacedName, &cr); err != nil {
		// CR not found - likely deleted
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Handle deletion (finalizer logic will go here in future features)
	if !cr.DeletionTimestamp.IsZero() {
		log.Info("DPFHCPBridge is being deleted", "namespace", cr.Namespace, "name", cr.Name)
		// Finalizer handling will be added in future features
		return ctrl.Result{}, nil
	}

	// Compute phase from conditions at the start
	// This ensures phase reflects the current state for feature gating
	r.updatePhaseFromConditions(&cr)

	// Feature: DPUCluster Validation
	log.V(1).Info("Running DPUCluster validation feature")
	if result, err := r.DPUClusterValidator.ValidateDPUCluster(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "DPUCluster validation failed")
		}
		return result, err
	}

	// Feature: Resolve BlueField Image
	// Only validate image during initial creation/retry (Pending/Failed phases)
	// Once cluster is provisioned (Provisioning/Ready), skip validation to avoid
	// false failures when old OCP versions are removed from ConfigMap
	if cr.Status.Phase == provisioningv1alpha1.PhasePending || cr.Status.Phase == provisioningv1alpha1.PhaseFailed {
		log.V(1).Info("Running BlueField image resolution feature")
		if result, err := r.ImageResolver.ResolveBlueFieldImage(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
			return result, err
		}
	} else {
		log.V(1).Info("Skipping BlueField image resolution - cluster already provisioned or being deleted", "phase", cr.Status.Phase)
	}

	// Future features will be added here

	// Compute final phase from all conditions after features have updated them
	r.updatePhaseFromConditions(&cr)

	// Persist status with computed phase
	if err := r.Status().Update(ctx, &cr); err != nil {
		log.Error(err, "Failed to update status with computed phase")
		return ctrl.Result{}, err
	}

	log.Info("Reconciliation complete", "namespace", cr.Namespace, "name", cr.Name, "phase", cr.Status.Phase)
	return ctrl.Result{}, nil
}

// SetupWithManager sets up the controller with the Manager.
func (r *DPFHCPBridgeReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&provisioningv1alpha1.DPFHCPBridge{}).
		Watches(
			&corev1.ConfigMap{},
			handler.EnqueueRequestsFromMapFunc(r.configMapToRequests),
			builder.WithPredicates(configMapPredicate()),
		).
		Watches(
			&dpuprovisioningv1alpha1.DPUCluster{},
			handler.EnqueueRequestsFromMapFunc(r.dpuClusterToRequests),
			builder.WithPredicates(dpuClusterPredicate()),
		).
		Named("dpfhcpbridge").
		Complete(r)
}

// configMapPredicate filters ConfigMap events to only watch ocp-bluefield-images
func configMapPredicate() predicate.Predicate {
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			return e.Object.GetName() == "ocp-bluefield-images" &&
				e.Object.GetNamespace() == "dpf-hcp-bridge-system"
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			return e.ObjectNew.GetName() == "ocp-bluefield-images" &&
				e.ObjectNew.GetNamespace() == "dpf-hcp-bridge-system"
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			return e.Object.GetName() == "ocp-bluefield-images" &&
				e.Object.GetNamespace() == "dpf-hcp-bridge-system"
		},
	}
}

// configMapToRequests maps ConfigMap events to reconcile requests for DPFHCPBridge CRs
// that need image resolution (Pending/Failed phases only)
func (r *DPFHCPBridgeReconciler) configMapToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	// List all DPFHCPBridge CRs cluster-wide
	var bridgeList provisioningv1alpha1.DPFHCPBridgeList
	if err := r.List(ctx, &bridgeList); err != nil {
		log.Error(err, "Failed to list DPFHCPBridge CRs for ConfigMap watch")
		return []reconcile.Request{}
	}

	// Filter to only CRs that need ConfigMap for image resolution
	// (Pending: awaiting provisioning, Failed: retry after ConfigMap update)
	// Skip Ready/Provisioning/Deleting to avoid unnecessary reconciliations
	requests := make([]reconcile.Request, 0)
	for _, bridge := range bridgeList.Items {
		// Only reconcile if phase needs image resolution
		if bridge.Status.Phase == provisioningv1alpha1.PhasePending ||
			bridge.Status.Phase == provisioningv1alpha1.PhaseFailed ||
			bridge.Status.Phase == "" { // Include empty phase (new CRs)
			requests = append(requests, reconcile.Request{
				NamespacedName: types.NamespacedName{
					Name:      bridge.Name,
					Namespace: bridge.Namespace,
				},
			})
		}
	}

	log.Info("ConfigMap changed, reconciling DPFHCPBridge CRs that need image resolution",
		"configMap", obj.GetName(),
		"totalCRs", len(bridgeList.Items),
		"reconcileCount", len(requests))

	return requests
}

// dpuClusterPredicate filters DPUCluster events to watch for deletion and updates
func dpuClusterPredicate() predicate.Predicate {
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			// Watch creation - reconcile affected DPFHCPBridge CR
			return true
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			// Watch updates - reconcile affected DPFHCPBridge CR
			return true
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			// CRITICAL: Watch deletion to alert users
			return true
		},
	}
}

// dpuClusterToRequests maps DPUCluster events to reconcile requests for DPFHCPBridge CRs
// that reference the affected DPUCluster.
// Note: the relationship is 1:1 (one DPFHCPBridge per DPUCluster), but we still
// iterate to find the matching CR since we don't know the CR name from the DPUCluster.
func (r *DPFHCPBridgeReconciler) dpuClusterToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	dpuCluster, ok := obj.(*dpuprovisioningv1alpha1.DPUCluster)
	if !ok {
		log.Error(nil, "Failed to convert object to DPUCluster", "object", obj)
		return []reconcile.Request{}
	}

	// List all DPFHCPBridge CRs cluster-wide
	var bridgeList provisioningv1alpha1.DPFHCPBridgeList
	if err := r.List(ctx, &bridgeList); err != nil {
		log.Error(err, "Failed to list DPFHCPBridge CRs for DPUCluster watch")
		return []reconcile.Request{}
	}

	// Find the DPFHCPBridge CR that references this DPUCluster (should be at most one per 1:1 relationship)
	requests := make([]reconcile.Request, 0, 1)
	for _, bridge := range bridgeList.Items {
		if bridge.Spec.DPUClusterRef.Name == dpuCluster.Name &&
			bridge.Spec.DPUClusterRef.Namespace == dpuCluster.Namespace {
			requests = append(requests, reconcile.Request{
				NamespacedName: types.NamespacedName{
					Name:      bridge.Name,
					Namespace: bridge.Namespace,
				},
			})

			// there should only be one DPFHCPBridge per DPUCluster
			// If we find multiple, log a warning but still reconcile all of them
			if len(requests) > 1 {
				log.Info("WARNING: Multiple DPFHCPBridge CRs reference the same DPUCluster (violates 1:1 relationship)",
					"dpuCluster", dpuCluster.Name,
					"dpuClusterNamespace", dpuCluster.Namespace,
					"count", len(requests))
			}
		}
	}

	if len(requests) > 0 {
		log.Info("DPUCluster changed, reconciling DPFHCPBridge CR",
			"dpuCluster", dpuCluster.Name,
			"dpuClusterNamespace", dpuCluster.Namespace,
			"affectedCRs", len(requests))
	}

	return requests
}

// updatePhaseFromConditions computes the phase based on all conditions
// This follows the Kubernetes pattern where phase is derived from conditions,
// not set by individual features (similar to NVIDIA DPUCluster controller)
func (r *DPFHCPBridgeReconciler) updatePhaseFromConditions(cr *provisioningv1alpha1.DPFHCPBridge) {
	// List of validation conditions that must pass before provisioning
	// Order matters: check critical validations first
	validationChecks := []struct {
		condType string
		negative bool // true if ConditionTrue = bad, false if ConditionFalse = bad
	}{
		{"DPUClusterMissing", true},       // True = cluster missing = bad
		{"ClusterTypeValid", false},       // False = type invalid = bad
		{"DPUClusterInUse", true},         // True = cluster already in use = bad
		{"BlueFieldImageResolved", false}, // False = image not resolved = bad
	}

	// Check all validation conditions
	for _, check := range validationChecks {
		cond := meta.FindStatusCondition(cr.Status.Conditions, check.condType)
		if cond == nil {
			// Condition not set yet - still initializing
			continue
		}

		// Determine if this condition represents a failure
		// For negative conditions: True = bad (e.g., DPUClusterMissing=True means missing)
		// For positive conditions: False = bad (e.g., ClusterTypeValid=False means invalid)
		isFailed := (check.negative && cond.Status == metav1.ConditionTrue) ||
			(!check.negative && cond.Status == metav1.ConditionFalse)

		if isFailed {
			cr.Status.Phase = provisioningv1alpha1.PhaseFailed
			return
		}
	}

	// All validations passed
	// Future: Check provisioning/ready conditions here
	// For now, if all validations pass, phase is Pending (waiting for provisioning)
	cr.Status.Phase = provisioningv1alpha1.PhasePending
}
