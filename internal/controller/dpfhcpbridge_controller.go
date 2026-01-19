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

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/bluefield"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/dpucluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/hostedcluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/secrets"
)

// DPFHCPBridgeReconciler reconciles a DPFHCPBridge object
type DPFHCPBridgeReconciler struct {
	client.Client
	Scheme               *runtime.Scheme
	Recorder             record.EventRecorder
	ImageResolver        *bluefield.ImageResolver
	DPUClusterValidator  *dpucluster.Validator
	SecretsValidator     *secrets.Validator
	SecretManager        *hostedcluster.SecretManager
	HostedClusterManager *hostedcluster.HostedClusterManager
	NodePoolManager      *hostedcluster.NodePoolManager
	FinalizerManager     *hostedcluster.FinalizerManager
	StatusSyncer         *hostedcluster.StatusSyncer
}

const (
	// FinalizerName is the finalizer added to DPFHCPBridge resources
	FinalizerName = "dpfhcpbridge.provisioning.dpu.hcp.io/finalizer"
)

// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpbridges,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpbridges/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpbridges/finalizers,verbs=update
// +kubebuilder:rbac:groups=coordination.k8s.io,resources=leases,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups="",resources=events,verbs=create;patch
// +kubebuilder:rbac:groups="",resources=configmaps,verbs=get;list;watch
// +kubebuilder:rbac:groups=provisioning.dpu.nvidia.com,resources=dpuclusters,verbs=get;list;watch
// +kubebuilder:rbac:groups="",resources=secrets,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups="",resources=namespaces,verbs=get;list;watch;create
// +kubebuilder:rbac:groups="",resources=nodes,verbs=get;list;watch
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=hostedclusters,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=hostedclusters/status,verbs=get
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=nodepools,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=nodepools/status,verbs=get

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

	// Compute phase from conditions at the start
	// This ensures phase reflects the current state (including Deleting phase)
	r.updatePhaseFromConditions(&cr)

	// Handle deletion - run finalizer cleanup
	if !cr.DeletionTimestamp.IsZero() {
		return r.handleDeletion(ctx, &cr)
	}

	// Add finalizer if not present (Phase 1: Foundation)
	if !controllerutil.ContainsFinalizer(&cr, FinalizerName) {
		log.Info("Adding finalizer to DPFHCPBridge", "finalizer", FinalizerName)
		controllerutil.AddFinalizer(&cr, FinalizerName)
		if err := r.Update(ctx, &cr); err != nil {
			log.Error(err, "Failed to add finalizer")
			return ctrl.Result{}, err
		}
		// Return and requeue to continue with the updated CR
		return ctrl.Result{Requeue: true}, nil
	}

	// Feature: DPUCluster Validation
	log.V(1).Info("Running DPUCluster validation feature")
	if result, err := r.DPUClusterValidator.ValidateDPUCluster(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "DPUCluster validation failed")
		}
		return result, err
	}

	// Feature: Secrets Validation
	log.V(1).Info("Running secrets validation feature")
	if result, err := r.SecretsValidator.ValidateSecrets(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "Secrets validation failed")
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

	// Recompute phase after validations to ensure HostedCluster creation only proceeds if all validations pass
	r.updatePhaseFromConditions(&cr)

	// Feature: Copy Secrets to clusters namespace
	// Only run during Pending phase (all validations must pass first)
	// Note: We only check for Pending (not Failed) to prevent secret operations when validations fail
	if cr.Status.Phase == provisioningv1alpha1.PhasePending {
		log.V(1).Info("Copying secrets to clusters namespace")
		if result, err := r.SecretManager.CopySecrets(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "Secret copying failed")
			}
			return result, err
		}

		// Generate ETCD encryption key
		log.V(1).Info("Generating ETCD encryption key")
		if result, err := r.SecretManager.GenerateETCDEncryptionKey(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "ETCD key generation failed")
			}
			return result, err
		}
	} else {
		log.V(1).Info("Skipping secret management - cluster already provisioned or being deleted", "phase", cr.Status.Phase)
	}

	// Feature: HostedCluster & NodePool Creation
	// Only run during Pending phase (all validations must pass first)
	// Note: We only check for Pending (not Failed) to prevent creation when validations fail
	// If user fixes validation issues, phase will transition back to Pending and creation will proceed
	if cr.Status.Phase == provisioningv1alpha1.PhasePending {
		log.V(1).Info("Creating HostedCluster and NodePool")

		// Create or update HostedCluster
		if result, err := r.HostedClusterManager.CreateOrUpdateHostedCluster(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "HostedCluster creation failed")
			}
			return result, err
		}

		// Create NodePool
		if result, err := r.NodePoolManager.CreateNodePool(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "NodePool creation failed")
			}
			return result, err
		}

		// Set hostedClusterRef in status after successful creation
		cr.Status.HostedClusterRef = &corev1.ObjectReference{
			Name:       cr.Name,
			Namespace:  cr.Namespace,
			Kind:       "HostedCluster",
			APIVersion: "hypershift.openshift.io/v1beta1",
		}
	} else {
		log.V(1).Info("Skipping HostedCluster/NodePool creation - cluster already provisioned or being deleted", "phase", cr.Status.Phase)
	}

	// Feature: HostedCluster Status Mirroring
	// Sync status from HostedCluster to DPFHCPBridge
	// This runs in all phases (Pending, Provisioning, Ready) to keep status up-to-date
	// Only syncs if hostedClusterRef is set (after HostedCluster creation)
	log.V(1).Info("Syncing status from HostedCluster")
	if result, err := r.StatusSyncer.SyncStatusFromHostedCluster(ctx, &cr); err != nil || result.Requeue || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "Status sync failed")
		}
		return result, err
	}

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
		Watches(
			&corev1.Secret{},
			handler.EnqueueRequestsFromMapFunc(r.secretToRequests),
			builder.WithPredicates(secretPredicate()),
		).
		Watches(
			&hyperv1.HostedCluster{},
			handler.EnqueueRequestsFromMapFunc(r.hostedClusterToRequests),
			builder.WithPredicates(hostedClusterPredicate()),
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

// secretPredicate filters Secret events to watch for changes to referenced secrets
func secretPredicate() predicate.Predicate {
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			// Watch creation - reconcile affected DPFHCPBridge CRs
			return true
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			// Watch updates - reconcile affected DPFHCPBridge CRs
			return true
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			// Watch deletion - reconcile affected DPFHCPBridge CRs
			return true
		},
	}
}

// secretToRequests maps Secret events to reconcile requests for DPFHCPBridge CRs
// that reference the secret via sshKeySecretRef or pullSecretRef
func (r *DPFHCPBridgeReconciler) secretToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	secret, ok := obj.(*corev1.Secret)
	if !ok {
		log.Error(nil, "Failed to convert object to Secret", "object", obj)
		return []reconcile.Request{}
	}

	// List all DPFHCPBridge CRs cluster-wide
	var bridgeList provisioningv1alpha1.DPFHCPBridgeList
	if err := r.List(ctx, &bridgeList); err != nil {
		log.Error(err, "Failed to list DPFHCPBridge CRs for Secret watch")
		return []reconcile.Request{}
	}

	// Find all DPFHCPBridge CRs that reference this secret
	requests := make([]reconcile.Request, 0)
	for _, bridge := range bridgeList.Items {
		// Check if this secret is referenced by sshKeySecretRef or pullSecretRef
		// Note: Secrets are namespace-scoped, so we need to check both name and namespace
		isSSHKeySecret := bridge.Spec.SSHKeySecretRef.Name == secret.Name &&
			bridge.Namespace == secret.Namespace
		isPullSecret := bridge.Spec.PullSecretRef.Name == secret.Name &&
			bridge.Namespace == secret.Namespace

		if isSSHKeySecret || isPullSecret {
			requests = append(requests, reconcile.Request{
				NamespacedName: types.NamespacedName{
					Name:      bridge.Name,
					Namespace: bridge.Namespace,
				},
			})

			log.V(1).Info("Secret referenced by DPFHCPBridge CR",
				"secret", secret.Name,
				"secretNamespace", secret.Namespace,
				"bridge", bridge.Name,
				"bridgeNamespace", bridge.Namespace,
				"isSSHKey", isSSHKeySecret,
				"isPullSecret", isPullSecret)
		}
	}

	if len(requests) > 0 {
		log.Info("Secret changed, reconciling DPFHCPBridge CRs",
			"secret", secret.Name,
			"secretNamespace", secret.Namespace,
			"affectedCRs", len(requests))
	}

	return requests
}

// hostedClusterPredicate filters HostedCluster events to watch for status changes
func hostedClusterPredicate() predicate.Predicate {
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			// Watch creation - reconcile to set initial status
			return true
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			// Only reconcile if status changed (not spec)
			// This prevents unnecessary reconciliations when we update the HostedCluster spec
			oldHC, oldOK := e.ObjectOld.(*hyperv1.HostedCluster)
			newHC, newOK := e.ObjectNew.(*hyperv1.HostedCluster)
			if !oldOK || !newOK {
				return false
			}

			// Compare status conditions to detect changes
			return !conditionsEqual(oldHC.Status.Conditions, newHC.Status.Conditions)
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			// Watch deletion - reconcile to handle cleanup
			return true
		},
	}
}

// conditionsEqual compares two condition slices for equality
func conditionsEqual(oldConds, newConds []metav1.Condition) bool {
	if len(oldConds) != len(newConds) {
		return false
	}

	// Create maps for O(n) comparison
	oldMap := make(map[string]metav1.Condition)
	for _, c := range oldConds {
		oldMap[c.Type] = c
	}

	// Compare each new condition
	for _, newCond := range newConds {
		oldCond, exists := oldMap[newCond.Type]
		if !exists {
			return false
		}
		if oldCond.Status != newCond.Status ||
			oldCond.Reason != newCond.Reason ||
			oldCond.Message != newCond.Message {
			return false
		}
	}

	return true
}

// hostedClusterToRequests maps HostedCluster events to reconcile requests for DPFHCPBridge CRs
// that own the HostedCluster (via labels)
func (r *DPFHCPBridgeReconciler) hostedClusterToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	hc, ok := obj.(*hyperv1.HostedCluster)
	if !ok {
		log.Error(nil, "Failed to convert object to HostedCluster", "object", obj)
		return []reconcile.Request{}
	}

	// Extract DPFHCPBridge name and namespace from labels
	bridgeName := hc.Labels["dpfhcpbridge.provisioning.dpu.hcp.io/name"]
	bridgeNamespace := hc.Labels["dpfhcpbridge.provisioning.dpu.hcp.io/namespace"]

	if bridgeName == "" || bridgeNamespace == "" {
		// HostedCluster not owned by DPFHCPBridge (no labels)
		log.V(2).Info("HostedCluster not owned by DPFHCPBridge, skipping",
			"hostedCluster", hc.Name,
			"namespace", hc.Namespace)
		return []reconcile.Request{}
	}

	log.V(1).Info("HostedCluster changed, reconciling owning DPFHCPBridge",
		"hostedCluster", hc.Name,
		"namespace", hc.Namespace,
		"bridge", bridgeName,
		"bridgeNamespace", bridgeNamespace)

	return []reconcile.Request{
		{
			NamespacedName: types.NamespacedName{
				Name:      bridgeName,
				Namespace: bridgeNamespace,
			},
		},
	}
}

// updatePhaseFromConditions computes the phase based on all conditions
func (r *DPFHCPBridgeReconciler) updatePhaseFromConditions(cr *provisioningv1alpha1.DPFHCPBridge) {
	// Phase 1: Check for deletion (highest priority)
	if !cr.DeletionTimestamp.IsZero() {
		cr.Status.Phase = provisioningv1alpha1.PhaseDeleting
		return
	}

	// Phase 2: List of validation conditions that must pass before provisioning
	// Order matters: check critical validations first
	validationChecks := []struct {
		condType string
		negative bool // true if ConditionTrue = bad, false if ConditionFalse = bad
	}{
		{"DPUClusterMissing", true},       // True = cluster missing = bad
		{"ClusterTypeValid", false},       // False = type invalid = bad
		{"DPUClusterInUse", true},         // True = cluster already in use = bad
		{"SecretsValid", false},           // False = secrets invalid = bad
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

	// Phase 3: Check for Ready condition (HostedCluster is operational)
	readyCond := meta.FindStatusCondition(cr.Status.Conditions, "Ready")
	if readyCond != nil && readyCond.Status == metav1.ConditionTrue {
		cr.Status.Phase = provisioningv1alpha1.PhaseReady
		return
	}

	// Phase 4: Check if HostedCluster provisioning has started
	if cr.Status.HostedClusterRef != nil {
		cr.Status.Phase = provisioningv1alpha1.PhaseProvisioning
		return
	}

	// Phase 5: All validations passed, waiting for provisioning to start
	cr.Status.Phase = provisioningv1alpha1.PhasePending
}

// handleDeletion handles the deletion of a DPFHCPBridge CR by running finalizer cleanup
func (r *DPFHCPBridgeReconciler) handleDeletion(ctx context.Context, cr *provisioningv1alpha1.DPFHCPBridge) (ctrl.Result, error) {
	log := logf.FromContext(ctx)
	log.Info("DPFHCPBridge is being deleted", "namespace", cr.Namespace, "name", cr.Name)

	// Persist the Deleting phase before removing finalizer
	if err := r.Status().Update(ctx, cr); err != nil {
		log.Error(err, "Failed to update status to Deleting phase")
		return ctrl.Result{}, err
	}

	if !controllerutil.ContainsFinalizer(cr, FinalizerName) {
		// No finalizer, nothing to clean up
		return ctrl.Result{}, nil
	}

	// Run finalizer cleanup
	result, err := r.FinalizerManager.HandleFinalizerCleanup(ctx, cr)
	if err != nil {
		log.Error(err, "Finalizer cleanup failed")
		return result, err
	}

	// If cleanup is still in progress (requeue requested), don't remove finalizer yet
	if result.Requeue || result.RequeueAfter > 0 {
		log.Info("Cleanup still in progress, will requeue",
			"requeue", result.Requeue,
			"requeueAfter", result.RequeueAfter)
		return result, nil
	}

	// Cleanup fully completed - remove finalizer
	log.Info("Removing finalizer after successful cleanup")
	controllerutil.RemoveFinalizer(cr, FinalizerName)
	if err := r.Update(ctx, cr); err != nil {
		log.Error(err, "Failed to remove finalizer")
		return ctrl.Result{}, err
	}

	log.Info("Finalizer removed, DPFHCPBridge will be deleted")
	return ctrl.Result{}, nil
}
