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
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bluefield"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpucluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/finalizer"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/hostedcluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ignitiongenerator"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/kubeconfiginjection"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/metallb"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/secrets"
)

// DPFHCPProvisionerReconciler reconciles a DPFHCPProvisioner object
type DPFHCPProvisionerReconciler struct {
	client.Client
	Scheme               *runtime.Scheme
	Recorder             record.EventRecorder
	ImageResolver        *bluefield.ImageResolver
	DPUClusterValidator  *dpucluster.Validator
	SecretsValidator     *secrets.Validator
	SecretManager        *hostedcluster.SecretManager
	MetalLBManager       *metallb.MetalLBManager
	HostedClusterManager *hostedcluster.HostedClusterManager
	NodePoolManager      *hostedcluster.NodePoolManager
	FinalizerManager     *finalizer.Manager
	StatusSyncer         *hostedcluster.StatusSyncer
	KubeconfigInjector   *kubeconfiginjection.KubeconfigInjector
	IgnitionGenerator    *ignitiongenerator.IgnitionGenerator
}

const (
	// FinalizerName is the finalizer added to DPFHCPProvisioner resources
	FinalizerName = "dpfhcpprovisioner.provisioning.dpu.hcp.io/finalizer"
	// OperatorNamespace is the namespace where the operator is deployed
	OperatorNamespace = "dpf-hcp-provisioner-system"
)

// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisioners,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisioners/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisioners/finalizers,verbs=update
// +kubebuilder:rbac:groups=coordination.k8s.io,resources=leases,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups="",resources=events,verbs=create;patch
// +kubebuilder:rbac:groups=provisioning.dpu.nvidia.com,resources=dpuclusters,verbs=get;list;watch;update;patch
// +kubebuilder:rbac:groups="",resources=secrets,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups="",resources=namespaces,verbs=get;list;watch;create
// +kubebuilder:rbac:groups="",resources=nodes,verbs=get;list;watch
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=hostedclusters,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=hostedclusters/status,verbs=get
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=nodepools,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=hypershift.openshift.io,resources=nodepools/status,verbs=get
// +kubebuilder:rbac:groups=metallb.io,resources=ipaddresspools,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=metallb.io,resources=l2advertisements,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=svc.dpu.nvidia.com,resources=dpudeployments,verbs=get;list;watch
// +kubebuilder:rbac:groups=provisioning.dpu.nvidia.com,resources=dpuflavors,verbs=get;list;watch
// +kubebuilder:rbac:groups=operator.dpu.nvidia.com,resources=dpfoperatorconfigs,verbs=get;list;watch;patch
// +kubebuilder:rbac:groups=provisioning.dpu.hcp.io,resources=dpfhcpprovisionerconfigs,verbs=get;list;watch
// +kubebuilder:rbac:groups="",resources=configmaps,verbs=get;list;watch;create;update;patch

// Reconcile is part of the main kubernetes reconciliation loop which aims to
// move the current state of the cluster closer to the desired state.
//
// For more details, check Reconcile and its Result here:
// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.21.0/pkg/reconcile
func (r *DPFHCPProvisionerReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := logf.FromContext(ctx)
	log.Info("Reconciling DPFHCPProvisioner", "namespace", req.Namespace, "name", req.Name)

	// Fetch the DPFHCPProvisioner CR
	var cr provisioningv1alpha1.DPFHCPProvisioner
	if err := r.Get(ctx, req.NamespacedName, &cr); err != nil {
		// CR not found - likely deleted
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Load operator configuration from Config CR (or defaults if CR doesn't exist)
	operatorConfig, err := common.LoadOperatorConfigFromCR(ctx, r.Client)
	if err != nil {
		log.Error(err, "Failed to load operator configuration")
		return ctrl.Result{}, err
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
		log.Info("Adding finalizer to DPFHCPProvisioner", "finalizer", FinalizerName)
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
	if result, err := r.DPUClusterValidator.ValidateDPUCluster(ctx, &cr); err != nil || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "DPUCluster validation failed")
		}
		return result, err
	}

	// Feature: Secrets Validation
	log.V(1).Info("Running secrets validation feature")
	if result, err := r.SecretsValidator.ValidateSecrets(ctx, &cr); err != nil || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "Secrets validation failed")
		}
		return result, err
	}

	// Feature: Resolve BlueField Image
	// Validate image during initial creation/retry and ignition generation phases.
	// Skip during Provisioning/Ready to avoid false failures when old OCP versions are removed from registry.
	if operatorConfig.EnableBlueFieldValidation {
		r.ImageResolver.Repository = operatorConfig.BlueFieldOCPRepo
		if cr.Status.Phase == provisioningv1alpha1.PhasePending || cr.Status.Phase == provisioningv1alpha1.PhaseFailed || cr.Status.Phase == provisioningv1alpha1.PhaseIgnitionGenerating {
			log.V(1).Info("Running BlueField image resolution feature")
			if result, err := r.ImageResolver.ResolveBlueFieldImage(ctx, &cr); err != nil || result.RequeueAfter > 0 {
				return result, err
			}
		} else {
			log.V(1).Info("Skipping BlueField image resolution - cluster already provisioned or being deleted", "phase", cr.Status.Phase)
		}
	} else {
		log.V(1).Info("Skipping BlueField image resolution - feature disabled via operator config")
		// Set BlueFieldImageResolved condition to True when feature is disabled
		// This prevents old False conditions from blocking phase progression
		condition := metav1.Condition{
			Type:               provisioningv1alpha1.BlueFieldImageResolved,
			Status:             metav1.ConditionTrue,
			Reason:             "ValidationDisabled",
			Message:            "BlueField image validation is disabled via operator config",
			LastTransitionTime: metav1.Now(),
			ObservedGeneration: cr.Generation,
		}
		if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
			if err := r.Status().Update(ctx, &cr); err != nil {
				log.Error(err, "Failed to update BlueFieldImageResolved condition when feature is disabled")
				return ctrl.Result{}, err
			}
		}
	}

	// Recompute phase after validations to ensure HostedCluster creation only proceeds if all validations pass
	r.updatePhaseFromConditions(&cr)

	// Feature: MetalLB Configuration
	// Configure MetalLB resources (IPAddressPool and L2Advertisement) when LoadBalancer exposure is needed
	log.V(1).Info("Configuring MetalLB resources")
	if result, err := r.MetalLBManager.ConfigureMetalLB(ctx, &cr); err != nil || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "MetalLB configuration failed")
		}
		return result, err
	}

	// Feature: Copy Secrets to clusters namespace
	// Only run during Pending phase (all validations must pass first)
	// Note: We only check for Pending (not Failed) to prevent secret operations when validations fail
	if cr.Status.Phase == provisioningv1alpha1.PhasePending {
		log.V(1).Info("Copying secrets to clusters namespace")
		if result, err := r.SecretManager.CopySecrets(ctx, &cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "Secret copying failed")
			}
			return result, err
		}

		// Generate ETCD encryption key
		log.V(1).Info("Generating ETCD encryption key")
		if result, err := r.SecretManager.GenerateETCDEncryptionKey(ctx, &cr); err != nil || result.RequeueAfter > 0 {
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
		if result, err := r.HostedClusterManager.CreateOrUpdateHostedCluster(ctx, &cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "HostedCluster creation failed")
			}
			return result, err
		}

		// Create NodePool
		if result, err := r.NodePoolManager.CreateNodePool(ctx, &cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "NodePool creation failed")
			}
			return result, err
		}
	} else {
		log.V(1).Info("Skipping HostedCluster/NodePool creation - cluster already provisioned or being deleted", "phase", cr.Status.Phase)
	}

	// Set hostedClusterRef if HostedCluster exists and is owned by this CR
	// This ensures the ref is always set when the HostedCluster exists, regardless of phase
	if cr.Status.HostedClusterRef == nil {
		hc := &hyperv1.HostedCluster{}
		hcKey := types.NamespacedName{Name: cr.Name, Namespace: cr.Namespace}
		if err := r.Get(ctx, hcKey, hc); err == nil {
			// HostedCluster exists - verify ownership and set ref
			if metav1.IsControlledBy(hc, &cr) {
				log.V(1).Info("Setting hostedClusterRef for existing HostedCluster")
				cr.Status.HostedClusterRef = &corev1.ObjectReference{
					Name:       cr.Name,
					Namespace:  cr.Namespace,
					Kind:       "HostedCluster",
					APIVersion: "hypershift.openshift.io/v1beta1",
				}
			}
		}
	}

	// Feature: HostedCluster Status Mirroring
	// Sync status from HostedCluster to DPFHCPProvisioner
	// This runs in all phases (Pending, Provisioning, Ready) to keep status up-to-date
	// Only syncs if hostedClusterRef is set (after HostedCluster creation)
	log.V(1).Info("Syncing status from HostedCluster")
	if result, err := r.StatusSyncer.SyncStatusFromHostedCluster(ctx, &cr); err != nil || result.RequeueAfter > 0 {
		if err != nil {
			log.Error(err, "Status sync failed")
		}
		return result, err
	}

	// Feature: Kubeconfig Injection
	// Inject HostedCluster kubeconfig into DPUCluster namespace and update DPUCluster CR
	// Only runs after HostedCluster creation (hostedClusterRef is set)
	if cr.Status.HostedClusterRef != nil {
		log.V(1).Info("Running kubeconfig injection feature")
		if result, err := r.KubeconfigInjector.InjectKubeconfig(ctx, &cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "Kubeconfig injection failed")
			}
			return result, err
		}
	} else {
		log.V(1).Info("Skipping kubeconfig injection - HostedCluster not created yet")
	}

	// Feature: Ignition Generation
	if result, err := r.generateIgnition(ctx, &cr); err != nil || result.RequeueAfter > 0 {
		return result, err
	}

	// Compute Ready condition based on all operational requirements
	// This must run AFTER all features have updated their conditions
	// (HostedClusterAvailable, KubeConfigInjected, etc.)
	r.computeReadyCondition(ctx, &cr)

	// Compute final phase from all conditions after features have updated them
	// This must run AFTER computeReadyCondition since it checks the Ready condition
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
func (r *DPFHCPProvisionerReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&provisioningv1alpha1.DPFHCPProvisioner{}).
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
			handler.EnqueueRequestForOwner(
				mgr.GetScheme(),
				mgr.GetRESTMapper(),
				&provisioningv1alpha1.DPFHCPProvisioner{},
				handler.OnlyControllerOwner(),
			),
			builder.WithPredicates(hostedClusterPredicate()),
		).
		Watches(
			&corev1.Secret{},
			handler.EnqueueRequestsFromMapFunc(r.kubeconfigSecretToRequests),
			builder.WithPredicates(kubeconfiginjection.IsHostedClusterKubeconfigSecretPredicate()),
		).
		Watches(
			&provisioningv1alpha1.DPFHCPProvisionerConfig{},
			handler.EnqueueRequestsFromMapFunc(r.configToRequests),
			builder.WithPredicates(configPredicate()),
		).
		Named("dpfhcpprovisioner").
		Complete(r)
}

// dpuClusterPredicate filters DPUCluster events to watch for deletion and updates
func dpuClusterPredicate() predicate.Predicate {
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			// Watch creation - reconcile affected DPFHCPProvisioner CR
			return true
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			// Watch updates - reconcile affected DPFHCPProvisioner CR
			return true
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			// CRITICAL: Watch deletion to alert users
			return true
		},
	}
}

// dpuClusterToRequests maps DPUCluster events to reconcile requests for DPFHCPProvisioner CRs
// that reference the affected DPUCluster.
// Note: the relationship is 1:1 (one DPFHCPProvisioner per DPUCluster), but we still
// iterate to find the matching CR since we don't know the CR name from the DPUCluster.
func (r *DPFHCPProvisionerReconciler) dpuClusterToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	dpuCluster, ok := obj.(*dpuprovisioningv1alpha1.DPUCluster)
	if !ok {
		log.Error(nil, "Failed to convert object to DPUCluster", "object", obj)
		return []reconcile.Request{}
	}

	// List all DPFHCPProvisioner CRs cluster-wide
	var provisionerList provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisionerList); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioner CRs for DPUCluster watch")
		return []reconcile.Request{}
	}

	// Find the DPFHCPProvisioner CR that references this DPUCluster (should be at most one per 1:1 relationship)
	requests := make([]reconcile.Request, 0, 1)
	for _, provisioner := range provisionerList.Items {
		if provisioner.Spec.DPUClusterRef.Name == dpuCluster.Name &&
			provisioner.Spec.DPUClusterRef.Namespace == dpuCluster.Namespace {
			requests = append(requests, reconcile.Request{
				NamespacedName: types.NamespacedName{
					Name:      provisioner.Name,
					Namespace: provisioner.Namespace,
				},
			})

			// there should only be one DPFHCPProvisioner per DPUCluster
			// If we find multiple, log a warning but still reconcile all of them
			if len(requests) > 1 {
				log.Info("WARNING: Multiple DPFHCPProvisioner CRs reference the same DPUCluster (violates 1:1 relationship)",
					"dpuCluster", dpuCluster.Name,
					"dpuClusterNamespace", dpuCluster.Namespace,
					"count", len(requests))
			}
		}
	}

	if len(requests) > 0 {
		log.Info("DPUCluster changed, reconciling DPFHCPProvisioner CR",
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
			// Watch creation - reconcile affected DPFHCPProvisioner CRs
			return true
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			// Watch updates - reconcile affected DPFHCPProvisioner CRs
			return true
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			// Watch deletion - reconcile affected DPFHCPProvisioner CRs
			return true
		},
	}
}

// secretToRequests maps Secret events to reconcile requests for DPFHCPProvisioner CRs
// that reference the secret via sshKeySecretRef or pullSecretRef
func (r *DPFHCPProvisionerReconciler) secretToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	secret, ok := obj.(*corev1.Secret)
	if !ok {
		log.Error(nil, "Failed to convert object to Secret", "object", obj)
		return []reconcile.Request{}
	}

	// List all DPFHCPProvisioner CRs cluster-wide
	var provisionerList provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisionerList); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioner CRs for Secret watch")
		return []reconcile.Request{}
	}

	// Find all DPFHCPProvisioner CRs that reference this secret
	requests := make([]reconcile.Request, 0)
	for _, provisioner := range provisionerList.Items {
		// Check if this secret is referenced by sshKeySecretRef or pullSecretRef
		// Note: Secrets are namespace-scoped, so we need to check both name and namespace
		isSSHKeySecret := provisioner.Spec.SSHKeySecretRef.Name == secret.Name &&
			provisioner.Namespace == secret.Namespace
		isPullSecret := provisioner.Spec.PullSecretRef.Name == secret.Name &&
			provisioner.Namespace == secret.Namespace

		if isSSHKeySecret || isPullSecret {
			requests = append(requests, reconcile.Request{
				NamespacedName: types.NamespacedName{
					Name:      provisioner.Name,
					Namespace: provisioner.Namespace,
				},
			})

			log.V(1).Info("Secret referenced by DPFHCPProvisioner CR",
				"secret", secret.Name,
				"secretNamespace", secret.Namespace,
				"provisioner", provisioner.Name,
				"provisionerNamespace", provisioner.Namespace,
				"isSSHKey", isSSHKeySecret,
				"isPullSecret", isPullSecret)
		}
	}

	if len(requests) > 0 {
		log.Info("Secret changed, reconciling DPFHCPProvisioner CRs",
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

// kubeconfigSecretToRequests maps HC kubeconfig secret events to reconcile requests for DPFHCPProvisioner CRs
// Uses the kubeconfiginjection.FindProvisionerForKubeconfigSecret function
func (r *DPFHCPProvisionerReconciler) kubeconfigSecretToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	return kubeconfiginjection.FindProvisionerForKubeconfigSecret(ctx, r.Client, obj)
}

// configPredicate filters DPFHCPProvisionerConfig events to only react to spec changes
func configPredicate() predicate.Predicate {
	return predicate.GenerationChangedPredicate{}
}

// configToRequests enqueues all DPFHCPProvisioner CRs when the Config CR changes
func (r *DPFHCPProvisionerReconciler) configToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	var provisionerList provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisionerList); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioner CRs for Config watch")
		return []reconcile.Request{}
	}

	requests := make([]reconcile.Request, 0, len(provisionerList.Items))
	for _, provisioner := range provisionerList.Items {
		requests = append(requests, reconcile.Request{
			NamespacedName: types.NamespacedName{
				Name:      provisioner.Name,
				Namespace: provisioner.Namespace,
			},
		})
	}

	if len(requests) > 0 {
		log.Info("DPFHCPProvisionerConfig changed, reconciling all DPFHCPProvisioner CRs",
			"affectedCRs", len(requests))
	}

	return requests
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

// generateIgnition runs the ignition generation feature if the CR is in the IgnitionGenerating phase.
func (r *DPFHCPProvisionerReconciler) generateIgnition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	if cr.Status.HostedClusterRef == nil || cr.Status.Phase != provisioningv1alpha1.PhaseIgnitionGenerating {
		log.V(1).Info("Skipping ignition generation", "phase", cr.Status.Phase)
		return ctrl.Result{}, nil
	}

	log.V(1).Info("Running ignition generation feature")
	result, err := r.IgnitionGenerator.GenerateIgnition(ctx, cr)
	if err != nil {
		log.Error(err, "Ignition generation failed")
	}
	return result, err
}

// computeReadyCondition determines if the DPFHCPProvisioner is fully operational and sets the Ready condition.
//
// Ready state requires ALL of the following currently implemented features:
// 1. HostedCluster is available and healthy (HostedClusterAvailable=True)
// 2. Kubeconfig successfully injected into DPUCluster (KubeConfigInjected=True)
//
// This function should be called AFTER all feature reconciliation completes, so that all
// sub-conditions (HostedClusterAvailable, KubeConfigInjected, etc.) are up-to-date.
//
// TODO: Add additional requirement checks here as new features are implemented
func (r *DPFHCPProvisionerReconciler) computeReadyCondition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) {
	log := logf.FromContext(ctx)

	// Requirement 1: MetalLB must be configured (if required)
	// This is only required when exposing services through LoadBalancer
	// Checked first because MetalLB configuration happens before HostedCluster creation
	if cr.ShouldExposeThroughLoadBalancer() {
		metalLBConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.MetalLBConfigured)
		if metalLBConfigured == nil || metalLBConfigured.Status != metav1.ConditionTrue {
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:    provisioningv1alpha1.Ready,
				Status:  metav1.ConditionFalse,
				Reason:  "MetalLBNotConfigured",
				Message: "Waiting for MetalLB configuration to complete",
			})
			log.V(1).Info("Not ready: MetalLB not configured")
			return
		}
	}

	// Requirement 2: HostedCluster must be available
	// This is set by the StatusSyncer after mirroring HostedCluster status
	hcAvailable := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterAvailable)
	if hcAvailable == nil || hcAvailable.Status != metav1.ConditionTrue {
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:    provisioningv1alpha1.Ready,
			Status:  metav1.ConditionFalse,
			Reason:  provisioningv1alpha1.ReasonHostedClusterNotReady,
			Message: "Waiting for HostedCluster to become available",
		})
		log.V(1).Info("Not ready: HostedCluster not available")
		return
	}

	// Requirement 3: Kubeconfig must be injected
	// This is set by the KubeconfigInjector after successful injection
	kubeconfigInjected := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
	if kubeconfigInjected == nil || kubeconfigInjected.Status != metav1.ConditionTrue {
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:    provisioningv1alpha1.Ready,
			Status:  metav1.ConditionFalse,
			Reason:  provisioningv1alpha1.ReasonKubeConfigNotInjected,
			Message: "Waiting for kubeconfig injection to DPUCluster",
		})
		log.V(1).Info("Not ready: Kubeconfig not injected")
		return
	}

	// Requirement 4: Ignition must be configured
	ignConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
	if ignConfigured == nil || ignConfigured.Status != metav1.ConditionTrue ||
		ignConfigured.ObservedGeneration != cr.Generation {
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:    provisioningv1alpha1.Ready,
			Status:  metav1.ConditionFalse,
			Reason:  "IgnitionNotConfigured",
			Message: "Waiting for ignition configuration to complete",
		})
		log.V(1).Info("Not ready: Ignition not configured")
		return
	}

	// All requirements met - set Ready to True
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:    provisioningv1alpha1.Ready,
		Status:  metav1.ConditionTrue,
		Reason:  provisioningv1alpha1.ReasonAllComponentsOperational,
		Message: "All required components are operational",
	})
	log.Info("DPFHCPProvisioner is ready")
}

// updatePhaseFromConditions computes the phase based on all conditions
func (r *DPFHCPProvisionerReconciler) updatePhaseFromConditions(cr *provisioningv1alpha1.DPFHCPProvisioner) {
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

	// Phase 4: Check if ignition generation is required
	hcAvailable := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterAvailable)
	kcInjected := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
	ignConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
	if hcAvailable != nil && hcAvailable.Status == metav1.ConditionTrue &&
		kcInjected != nil && kcInjected.Status == metav1.ConditionTrue &&
		(ignConfigured == nil || ignConfigured.Status != metav1.ConditionTrue ||
			ignConfigured.ObservedGeneration != cr.Generation) {
		cr.Status.Phase = provisioningv1alpha1.PhaseIgnitionGenerating
		return
	}

	// Phase 5: Check if HostedCluster provisioning has started
	if cr.Status.HostedClusterRef != nil {
		cr.Status.Phase = provisioningv1alpha1.PhaseProvisioning
		return
	}

	// Phase 6: All validations passed, waiting for provisioning to start
	cr.Status.Phase = provisioningv1alpha1.PhasePending
}

// handleDeletion handles the deletion of a DPFHCPProvisioner CR by running finalizer cleanup
func (r *DPFHCPProvisionerReconciler) handleDeletion(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)
	log.Info("DPFHCPProvisioner is being deleted", "namespace", cr.Namespace, "name", cr.Name)

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
	if result.RequeueAfter > 0 {
		log.Info("Cleanup still in progress, will requeue",
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

	log.Info("Finalizer removed, DPFHCPProvisioner will be deleted")
	return ctrl.Result{}, nil
}
