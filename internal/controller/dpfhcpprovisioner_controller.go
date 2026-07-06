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
	"fmt"
	"strings"
	"time"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
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

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
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
	ImageLookup          *bfocplookup.ImageLookup
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

	ignitionReconciled bool
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
// +kubebuilder:rbac:groups="",resources=services,verbs=get;list;watch
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
// +kubebuilder:rbac:groups="",resources=configmaps,verbs=get;list;watch;create;update;patch;delete

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

	// Load operator configuration from Config CR.
	// If the Config CR does not exist, the operator does nothing.
	operatorConfig, err := common.LoadOperatorConfigFromCR(ctx, r.Client)
	if err != nil {
		log.Error(err, "Failed to load operator configuration")
		return ctrl.Result{}, err
	}
	if operatorConfig == nil {
		log.Info("DPFHCPProvisionerConfig CR not found, operator is idle until config CR is created")
		return ctrl.Result{}, nil
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

	// Feature: BlueField OCP Layer Image Lookup
	if result, err := r.lookupBlueFieldOCPLayerImage(ctx, &cr, operatorConfig); err != nil || result.RequeueAfter > 0 {
		return result, err
	}

	// Recompute phase after validations to ensure HostedCluster creation only proceeds if all validations pass
	r.updatePhaseFromConditions(&cr)

	// Feature: MetalLB Configuration
	// Configure MetalLB resources (IPAddressPool and L2Advertisement) when LoadBalancer exposure is needed
	log.V(1).Info("Running MetalLB configuration feature")
	if result, err := r.MetalLBManager.ConfigureMetalLB(ctx, &cr, operatorConfig); err != nil || result.RequeueAfter > 0 {
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

	// Feature: Upgrade Detection and Handling
	// Detects release image changes, sets HostedClusterUpgrading condition,
	// deletes stale ignition, and manages the upgrade lifecycle.
	// This MUST run BEFORE reconcileHostedClusterAndNodePool so conditions are set
	// before the HC/NP images are updated.
	if result, err := r.handleUpgrade(ctx, &cr); err != nil || result.RequeueAfter > 0 {
		return result, err
	}

	// Feature: HostedCluster & NodePool Creation and Upgrade
	if result, err := r.reconcileHostedClusterAndNodePool(ctx, &cr); err != nil || result.RequeueAfter > 0 {
		return result, err
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

	// Detect DPUDeployment Flavor changes and invalidate ignition if needed.
	if changed, result, err := r.handleDPUDeploymentChange(ctx, &cr); changed || err != nil {
		return result, err
	}

	// Feature: Ignition lifecycle (verify + generate)
	// Skipped while upgrade is in progress — CM was deleted by handleUpgrade
	// and will be regenerated after upgrade completes.
	if result, err := r.reconcileIgnition(ctx, &cr); err != nil || result.RequeueAfter > 0 {
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
		Watches(
			&corev1.ConfigMap{},
			handler.EnqueueRequestsFromMapFunc(r.ignitionConfigMapToRequests),
			builder.WithPredicates(ignitionConfigMapPredicate()),
		).
		Watches(
			&dpuservicev1alpha1.DPUDeployment{},
			handler.EnqueueRequestsFromMapFunc(r.dpuDeploymentToRequests),
			builder.WithPredicates(predicate.GenerationChangedPredicate{}),
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

// ignitionConfigMapPredicate filters ConfigMap events to only react to ignition bfcfg template ConfigMaps.
// Only Delete events are processed since the reconciler re-creates missing ConfigMaps.
func ignitionConfigMapPredicate() predicate.Predicate {
	isBfcfgTemplate := func(labels map[string]string) bool {
		return labels != nil && labels[ignitiongenerator.BfcfgTemplateLabel] == "true"
	}
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			return false
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			return false
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			return isBfcfgTemplate(e.Object.GetLabels())
		},
		GenericFunc: func(e event.GenericEvent) bool {
			return false
		},
	}
}

// ignitionConfigMapToRequests maps ignition ConfigMap delete events to reconcile requests
// for the DPFHCPProvisioner CR that owns the ConfigMap.
func (r *DPFHCPProvisionerReconciler) ignitionConfigMapToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	annotations := obj.GetAnnotations()
	if annotations == nil {
		return nil
	}

	dpuClusterName := annotations[ignitiongenerator.BfcfgTemplateClusterNameAnnotation]
	dpuClusterNamespace := annotations[ignitiongenerator.BfcfgTemplateClusterNamespaceAnnotation]
	if dpuClusterName == "" || dpuClusterNamespace == "" {
		return nil
	}

	var provisionerList provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisionerList); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioner CRs for ignition ConfigMap watch")
		return nil
	}

	for _, provisioner := range provisionerList.Items {
		if provisioner.Spec.DPUClusterRef.Name == dpuClusterName &&
			provisioner.Spec.DPUClusterRef.Namespace == dpuClusterNamespace {
			log.Info("Ignition ConfigMap deleted, reconciling DPFHCPProvisioner",
				"configmap", obj.GetName(),
				"configmapNamespace", obj.GetNamespace(),
				"provisioner", provisioner.Name,
				"provisionerNamespace", provisioner.Namespace)
			return []reconcile.Request{{
				NamespacedName: types.NamespacedName{
					Name:      provisioner.Name,
					Namespace: provisioner.Namespace,
				},
			}}
		}
	}

	return nil
}

// dpuDeploymentToRequests maps DPUDeployment events to reconcile requests for DPFHCPProvisioner CRs
// that reference the affected DPUDeployment via spec.dpuDeploymentRef.
func (r *DPFHCPProvisionerReconciler) dpuDeploymentToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	dpuDeployment, ok := obj.(*dpuservicev1alpha1.DPUDeployment)
	if !ok {
		log.Error(nil, "Failed to convert object to DPUDeployment", "object", obj)
		return nil
	}

	var provisionerList provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisionerList); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioner CRs for DPUDeployment watch")
		return nil
	}

	var requests []reconcile.Request
	for _, provisioner := range provisionerList.Items {
		if provisioner.Spec.DPUDeploymentRef != nil &&
			provisioner.Spec.DPUDeploymentRef.Name == dpuDeployment.Name &&
			provisioner.Spec.DPUDeploymentRef.Namespace == dpuDeployment.Namespace {
			requests = append(requests, reconcile.Request{
				NamespacedName: types.NamespacedName{
					Name:      provisioner.Name,
					Namespace: provisioner.Namespace,
				},
			})
		}
	}

	if len(requests) > 0 {
		log.Info("DPUDeployment changed, reconciling DPFHCPProvisioner CRs",
			"dpuDeployment", dpuDeployment.Name,
			"dpuDeploymentNamespace", dpuDeployment.Namespace,
			"affectedCRs", len(requests))
	}

	return requests
}

// handleDPUDeploymentChange detects DPUDeployment Flavor changes and invalidates
// the ignition ConfigMap. Deletes the stale ConfigMap immediately so the DPF
// provisioning controller does not use outdated configuration while we regenerate.
// Returns (true, result, err) if a change was handled, (false, _, nil) otherwise.
func (r *DPFHCPProvisionerReconciler) handleDPUDeploymentChange(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (bool, ctrl.Result, error) {
	log := logf.FromContext(ctx)

	ignConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
	if ignConfigured == nil || ignConfigured.Status != metav1.ConditionTrue {
		return false, ctrl.Result{}, nil
	}
	if cr.Spec.DPUDeploymentRef == nil {
		return false, ctrl.Result{}, nil
	}

	dpuDeployment := &dpuservicev1alpha1.DPUDeployment{}
	key := types.NamespacedName{
		Name:      cr.Spec.DPUDeploymentRef.Name,
		Namespace: cr.Spec.DPUDeploymentRef.Namespace,
	}
	if err := r.Get(ctx, key, dpuDeployment); err != nil {
		log.V(1).Info("Cannot fetch DPUDeployment for change detection, skipping", "error", err)
		return false, ctrl.Result{}, nil
	}

	cm, err := r.getIgnitionConfigMap(ctx, cr)
	if err != nil {
		return false, ctrl.Result{}, err
	}
	if cm == nil {
		return false, ctrl.Result{}, nil
	}

	if cm.Annotations == nil {
		cm.Annotations = map[string]string{}
	}
	annotations := cm.Annotations

	currentFlavor := dpuDeployment.Spec.DPUs.Flavor
	storedFlavor := annotations[ignitiongenerator.BfcfgTemplateDPUFlavorNameAnnotation]
	currentBFB := dpuDeployment.Spec.DPUs.BFB
	storedBFB := annotations[ignitiongenerator.BfcfgTemplateBFBNameAnnotation]

	// Flavor changed → delete CM and regenerate ignition (content depends on flavor)
	if currentFlavor != storedFlavor {
		log.Info("DPUDeployment Flavor changed, deleting stale ignition ConfigMap and regenerating",
			"currentFlavor", currentFlavor, "storedFlavor", storedFlavor)

		if _, err := r.deleteIgnitionConfigMap(ctx, cr); err != nil {
			return true, ctrl.Result{}, err
		}

		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:               provisioningv1alpha1.IgnitionConfigured,
			Status:             metav1.ConditionFalse,
			Reason:             provisioningv1alpha1.ReasonDPUDeploymentChanged,
			Message:            "DPUDeployment Flavor changed, regenerating ignition configuration",
			ObservedGeneration: cr.Generation,
		})
		r.Recorder.Event(cr, corev1.EventTypeNormal, "DPUDeploymentChanged",
			"DPUDeployment Flavor changed, triggering ignition regeneration")
		r.computeReadyCondition(ctx, cr)
		r.updatePhaseFromConditions(cr)
		if err := r.Status().Update(ctx, cr); err != nil {
			log.Error(err, "Failed to update status after DPUDeployment Flavor change")
			return true, ctrl.Result{}, err
		}
		return true, ctrl.Result{RequeueAfter: 1 * time.Second}, nil
	}

	// BFB changed → update CM annotation only (ignition content doesn't depend on BFB name)
	if currentBFB != storedBFB {
		log.Info("DPUDeployment BFB changed, updating ConfigMap annotation",
			"currentBFB", currentBFB, "storedBFB", storedBFB)

		cm.Annotations[ignitiongenerator.BfcfgTemplateBFBNameAnnotation] = currentBFB
		if err := r.Update(ctx, cm); err != nil {
			log.Error(err, "Failed to update ignition ConfigMap BFB annotation")
			return true, ctrl.Result{}, err
		}
		r.Recorder.Event(cr, corev1.EventTypeNormal, "BFBAnnotationUpdated",
			fmt.Sprintf("Ignition ConfigMap BFB annotation updated to %s", currentBFB))
		return true, ctrl.Result{}, nil
	}

	return false, ctrl.Result{}, nil
}

// deleteStaleIgnitionConfigMap deletes the ignition ConfigMap when it was generated for an older
// spec version (e.g., after a release image upgrade). This prevents DPUs from being provisioned
// with stale ignition during an upgrade. The deletion is idempotent -- it runs on every reconcile
// getIgnitionConfigMap returns the ignition ConfigMap for a DPUCluster by name.
// Returns (nil, nil) if not found, or (nil, err) for other errors.
func (r *DPFHCPProvisionerReconciler) getIgnitionConfigMap(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (*corev1.ConfigMap, error) {
	cmName := ignitiongenerator.ConfigMapName(cr.Spec.DPUClusterRef.Name)
	cm := &corev1.ConfigMap{}
	err := r.Get(ctx, types.NamespacedName{Name: cmName, Namespace: cr.Spec.DPUClusterRef.Namespace}, cm)
	if apierrors.IsNotFound(err) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return cm, nil
}

// deleteIgnitionConfigMap deletes the ignition ConfigMap for a DPUCluster.
// Returns true if a ConfigMap was deleted.
func (r *DPFHCPProvisionerReconciler) deleteIgnitionConfigMap(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (bool, error) {
	cm, err := r.getIgnitionConfigMap(ctx, cr)
	if err != nil {
		return false, err
	}
	if cm == nil {
		return false, nil
	}
	if err := r.Delete(ctx, cm); err != nil {
		return false, fmt.Errorf("failed to delete ignition ConfigMap %s/%s: %w", cm.Namespace, cm.Name, err)
	}
	return true, nil
}

// isUpgrading returns true if a HostedCluster upgrade is currently in progress.
func isUpgrading(cr *provisioningv1alpha1.DPFHCPProvisioner) bool {
	cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterUpgrading)
	return cond != nil && cond.Status == metav1.ConditionTrue
}

// reconcileIgnition handles the ignition ConfigMap lifecycle: verification and generation.
// Skipped entirely while HostedClusterUpgrading=True — the CM was deleted by handleUpgrade
// and must not be regenerated until the upgrade completes.
func (r *DPFHCPProvisionerReconciler) reconcileIgnition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	if isUpgrading(cr) {
		log.V(1).Info("Skipping ignition operations - upgrade in progress")
		return ctrl.Result{}, nil
	}

	if configMapDeleted := r.verifyIgnitionConfigMap(ctx, cr); configMapDeleted {
		r.computeReadyCondition(ctx, cr)
		r.updatePhaseFromConditions(cr)
		if err := r.Status().Update(ctx, cr); err != nil {
			log.Error(err, "Failed to update status after ignition ConfigMap deletion")
			return ctrl.Result{}, err
		}
		return ctrl.Result{RequeueAfter: 1 * time.Second}, nil
	}

	if result, err := r.generateIgnition(ctx, cr); err != nil || result.RequeueAfter > 0 {
		return result, err
	}

	r.ignitionReconciled = true
	return ctrl.Result{}, nil
}

// verifyIgnitionConfigMap checks the ignition ConfigMap when IgnitionConfigured=True.
// Clears IgnitionConfigured to trigger regeneration if the ConfigMap was deleted or
// if this is the first reconcile after operator startup (ensures bug fixes in embedded
// ignition content are applied when the operator image is updated).
// Returns true if regeneration was triggered.
func (r *DPFHCPProvisionerReconciler) verifyIgnitionConfigMap(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) bool {
	log := logf.FromContext(ctx)

	ignConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
	if ignConfigured == nil || ignConfigured.Status != metav1.ConditionTrue {
		return false
	}

	cm, err := r.getIgnitionConfigMap(ctx, cr)
	if err != nil {
		log.Error(err, "Failed to verify ignition ConfigMap existence")
		return false
	}

	reason := ""
	message := ""
	eventType := corev1.EventTypeNormal
	eventReason := ""

	if cm == nil {
		reason = "ConfigMapDeleted"
		message = "Ignition ConfigMap was deleted, will regenerate"
		eventType = corev1.EventTypeWarning
		eventReason = "IgnitionConfigMapDeleted"
	} else if !r.ignitionReconciled {
		reason = "OperatorRestarted"
		message = "Operator restarted, regenerating ignition to apply any updates"
		eventReason = "IgnitionRegeneration"
	}

	if reason != "" {
		log.Info("Triggering ignition regeneration", "reason", reason)
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:               provisioningv1alpha1.IgnitionConfigured,
			Status:             metav1.ConditionFalse,
			Reason:             reason,
			Message:            message,
			ObservedGeneration: cr.Generation,
		})
		r.Recorder.Event(cr, eventType, eventReason, message)
		r.ignitionReconciled = true
		return true
	}

	r.ignitionReconciled = true
	return false
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

// lookupBlueFieldOCPLayerImage handles BlueField OCP layer image lookup.
// Skips lookup if machineOSURL is provided in the CR spec.
// Runs lookup during initial creation/retry and ignition generation phases.
// Skips during Provisioning/Ready to avoid false failures when old OCP versions are removed from registry.
func (r *DPFHCPProvisionerReconciler) lookupBlueFieldOCPLayerImage(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner, operatorConfig *common.OperatorConfig) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	// Skip lookup if machineOSURL is provided directly
	if cr.Spec.MachineOSURL != "" {
		log.V(1).Info("Skipping BlueField OCP layer lookup - machineOSURL provided in spec")
		condition := metav1.Condition{
			Type:               provisioningv1alpha1.BlueFieldOCPLayerImageFound,
			Status:             metav1.ConditionTrue,
			Reason:             "LookupSkipped",
			Message:            "BlueField OCP layer lookup skipped - machineOSURL provided in spec",
			ObservedGeneration: cr.Generation,
		}
		if changed := meta.SetStatusCondition(&cr.Status.Conditions, condition); changed {
			if err := r.Status().Update(ctx, cr); err != nil {
				log.Error(err, "Failed to update BlueFieldOCPLayerImageFound condition")
				return ctrl.Result{}, err
			}
		}
		return ctrl.Result{}, nil
	}

	// Run lookup (phase-gated)
	r.ImageLookup.Repository = operatorConfig.BlueFieldOCPLayerRepo
	if cr.Status.Phase == provisioningv1alpha1.PhasePending || cr.Status.Phase == provisioningv1alpha1.PhaseFailed || cr.Status.Phase == provisioningv1alpha1.PhaseIgnitionGenerating {
		log.V(1).Info("Running BlueField OCP layer image lookup")
		if result, err := r.ImageLookup.LookupBlueFieldOCPLayerImage(ctx, cr); err != nil || result.RequeueAfter > 0 {
			return result, err
		}
	} else {
		log.V(1).Info("Skipping BlueField OCP layer lookup - not in applicable phase", "phase", cr.Status.Phase)
	}

	return ctrl.Result{}, nil
}

// generateIgnition runs the ignition generation feature if the CR is in the IgnitionGenerating phase,
// or if it's in the Failed phase due to a previous ignition generation failure (transient retry).
func (r *DPFHCPProvisionerReconciler) generateIgnition(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	if cr.Status.HostedClusterRef == nil {
		log.V(1).Info("Skipping ignition generation - no HostedCluster ref", "phase", cr.Status.Phase)
		return ctrl.Result{}, nil
	}

	// Run ignition generation when:
	// 1. Phase is IgnitionGenerating (normal path), OR
	// 2. Phase is Failed AND the failure was caused by ignition generation (retry path), OR
	// 3. IgnitionConfigured was cleared due to operator restart or ConfigMap deletion
	shouldGenerate := cr.Status.Phase == provisioningv1alpha1.PhaseIgnitionGenerating
	if !shouldGenerate {
		ignConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		if ignConfigured != nil && ignConfigured.Status == metav1.ConditionFalse {
			switch ignConfigured.Reason {
			case provisioningv1alpha1.ReasonIgnitionGenerationFailed:
				shouldGenerate = true
				log.Info("Retrying ignition generation after transient failure")
			case "OperatorRestarted", "ConfigMapDeleted":
				shouldGenerate = true
				log.Info("Regenerating ignition", "reason", ignConfigured.Reason)
			}
		}
	}
	if !shouldGenerate {
		log.V(1).Info("Skipping ignition generation", "phase", cr.Status.Phase)
		return ctrl.Result{}, nil
	}

	log.V(1).Info("Running ignition generation feature")
	result, err := r.IgnitionGenerator.GenerateIgnition(ctx, cr)
	if err != nil {
		log.Error(err, "Ignition generation failed")
		r.updatePhaseFromConditions(cr)
		return result, err
	}

	// When ignition generation fails, the IgnitionGenerator sets IgnitionConfigured=False
	// and returns RequeueAfter (without error). In that case, recompute the phase so the
	// CR transitions to Failed instead of staying in IgnitionGenerating indefinitely.
	if result.RequeueAfter > 0 {
		ignConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		if ignConfigured != nil && ignConfigured.Status == metav1.ConditionFalse {
			r.updatePhaseFromConditions(cr)
			// The IgnitionGenerator already persisted the IgnitionConfigured condition,
			// but we need to persist the updated phase.
			if statusErr := r.Status().Update(ctx, cr); statusErr != nil {
				log.Error(statusErr, "Failed to update phase after ignition generation failure")
				return ctrl.Result{}, statusErr
			}
		}
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

	// MetalLB must be configured (if required)
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

	// HostedCluster must not be upgrading
	if isUpgrading(cr) {
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:    provisioningv1alpha1.Ready,
			Status:  metav1.ConditionFalse,
			Reason:  provisioningv1alpha1.ReasonHostedClusterNotReady,
			Message: "HostedCluster upgrade in progress",
		})
		log.V(1).Info("Not ready: HostedCluster upgrade in progress")
		return
	}

	// HostedCluster must be available
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

	// Kubeconfig must be injected
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

	// Ignition must be configured
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
	// Check for deletion (highest priority)
	if !cr.DeletionTimestamp.IsZero() {
		cr.Status.Phase = provisioningv1alpha1.PhaseDeleting
		return
	}

	// List of validation conditions that must pass before provisioning
	// Order matters: check critical validations first
	validationChecks := []struct {
		condType string
		negative bool // true if ConditionTrue = bad, false if ConditionFalse = bad
	}{
		{"DPUClusterMissing", true},            // True = cluster missing = bad
		{"ClusterTypeValid", false},            // False = type invalid = bad
		{"DPUClusterInUse", true},              // True = cluster already in use = bad
		{"SecretsValid", false},                // False = secrets invalid = bad
		{"BlueFieldOCPLayerImageFound", false}, // False = OCP layer image not found = bad
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

	// Check for Ready condition (HostedCluster is operational)
	readyCond := meta.FindStatusCondition(cr.Status.Conditions, "Ready")
	if readyCond != nil && readyCond.Status == metav1.ConditionTrue {
		cr.Status.Phase = provisioningv1alpha1.PhaseReady
		return
	}

	// Check if ignition generation failed
	// When IgnitionConfigured is explicitly False with a failure reason, transition to Failed
	// so the user can see the error and take action. The controller will retry on requeue.
	ignConfigured := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
	if ignConfigured != nil && ignConfigured.Status == metav1.ConditionFalse &&
		(ignConfigured.Reason == provisioningv1alpha1.ReasonIgnitionGenerationFailed || ignConfigured.Reason == provisioningv1alpha1.ReasonMachineOSURLMissing) {
		cr.Status.Phase = provisioningv1alpha1.PhaseFailed
		return
	}

	// Check if upgrade is in progress
	if isUpgrading(cr) {
		cr.Status.Phase = provisioningv1alpha1.PhaseUpgrading
		return
	}

	// Check if ignition generation is required.
	// Transition to IgnitionGenerating when HC is available and ignition is stale/missing.
	hcAvailable := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterAvailable)
	kcInjected := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
	if hcAvailable != nil && hcAvailable.Status == metav1.ConditionTrue &&
		kcInjected != nil && kcInjected.Status == metav1.ConditionTrue &&
		(ignConfigured == nil || ignConfigured.Status != metav1.ConditionTrue ||
			ignConfigured.ObservedGeneration != cr.Generation) {
		cr.Status.Phase = provisioningv1alpha1.PhaseIgnitionGenerating
		return
	}

	// Check if HostedCluster provisioning has started
	if cr.Status.HostedClusterRef != nil {
		cr.Status.Phase = provisioningv1alpha1.PhaseProvisioning
		return
	}

	// All validations passed, waiting for provisioning to start
	cr.Status.Phase = provisioningv1alpha1.PhasePending
}

// reconcileHostedClusterAndNodePool creates or updates the HostedCluster and NodePool resources.
// During Pending phase: creates HC and NP for the first time (initial provisioning).
// After provisioning (hostedClusterRef is set): checks for release image changes to support upgrades.
// If an upgrade is triggered, persists status and returns RequeueAfter to let the HC upgrade
// proceed before running ignition generation.
func (r *DPFHCPProvisionerReconciler) reconcileHostedClusterAndNodePool(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	if cr.Status.Phase == provisioningv1alpha1.PhasePending {
		log.V(1).Info("Creating HostedCluster and NodePool")

		if result, err := r.HostedClusterManager.CreateOrUpdateHostedCluster(ctx, cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "HostedCluster creation failed")
			}
			return result, err
		}

		if result, err := r.NodePoolManager.CreateNodePool(ctx, cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "NodePool creation failed")
			}
			return result, err
		}

		return ctrl.Result{}, nil
	}

	if cr.Status.HostedClusterRef != nil {
		log.V(1).Info("Checking HostedCluster and NodePool for release image updates")

		if result, err := r.HostedClusterManager.CreateOrUpdateHostedCluster(ctx, cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "HostedCluster update failed")
			}
			return result, err
		}

		if result, err := r.NodePoolManager.CreateNodePool(ctx, cr); err != nil || result.RequeueAfter > 0 {
			if err != nil {
				log.Error(err, "NodePool update failed")
			}
			return result, err
		}
	} else {
		log.V(1).Info("Skipping HostedCluster/NodePool operations - not in applicable phase", "phase", cr.Status.Phase)
	}

	return ctrl.Result{}, nil
}

// handleUpgrade detects and manages the HostedCluster upgrade lifecycle.
// When the user updates spec.ocpReleaseImage, this function:
//  1. Detects the upgrade by comparing the HC release image with the CR spec
//  2. Sets HostedClusterUpgrading=True and IgnitionConfigured=False
//  3. Deletes the stale ignition ConfigMap
//  4. Persists status and requeues
//
// On subsequent reconciles while HostedClusterUpgrading=True:
//   - The HostedClusterUpgrading condition blocks Ready (via computeReadyCondition)
//   - The HostedClusterUpgrading condition blocks IgnitionGenerating (via updatePhaseFromConditions)
//   - Phase stays Provisioning until the upgrade is marked complete
//
// The upgrade is marked complete when HostedClusterUpgrading=True but the HC release image
// matches the CR spec (HC was already updated by CreateOrUpdateHostedCluster).
// At that point, HostedClusterUpgrading is set to False, allowing the phase machine
// to transition to IgnitionGenerating → Ready.
func (r *DPFHCPProvisionerReconciler) handleUpgrade(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	if cr.Status.HostedClusterRef == nil {
		return ctrl.Result{}, nil
	}

	// Only run upgrade logic when the CR is Ready or already Upgrading.
	// Skip during initial installation (Pending, Provisioning, IgnitionGenerating, Failed).
	if cr.Status.Phase != provisioningv1alpha1.PhaseReady && cr.Status.Phase != provisioningv1alpha1.PhaseUpgrading {
		return ctrl.Result{}, nil
	}

	// Fetch current HC to compare release images
	hc := &hyperv1.HostedCluster{}
	hcKey := types.NamespacedName{Name: cr.Status.HostedClusterRef.Name, Namespace: cr.Status.HostedClusterRef.Namespace}
	if err := r.Get(ctx, hcKey, hc); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Case 1: Upgrade in progress — check if the HC has finished upgrading
	// by verifying the version history shows the target version as Completed.
	if isUpgrading(cr) {
		if r.isHostedClusterVersionReady(hc, cr.Spec.OCPReleaseImage) {
			log.Info("HostedCluster upgrade completed",
				"releaseImage", cr.Spec.OCPReleaseImage)
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.HostedClusterUpgrading,
				Status:             metav1.ConditionFalse,
				Reason:             provisioningv1alpha1.ReasonUpgradeComplete,
				Message:            fmt.Sprintf("Upgrade to %s complete", cr.Spec.OCPReleaseImage),
				ObservedGeneration: cr.Generation,
			})
			r.Recorder.Event(cr, corev1.EventTypeNormal, provisioningv1alpha1.ReasonUpgradeComplete,
				fmt.Sprintf("HostedCluster upgrade to %s completed", cr.Spec.OCPReleaseImage))
		}
		return ctrl.Result{}, nil
	}

	// Case 2: HC spec already matches — check if an upgrade is still in progress
	// (handles recovery after operator restart or previous image that didn't set the condition).
	// Only triggers if HostedClusterUpgrading condition was previously set.
	upgradingCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterUpgrading)
	if hc.Spec.Release.Image == cr.Spec.OCPReleaseImage {
		if upgradingCond != nil && !r.isHostedClusterVersionReady(hc, cr.Spec.OCPReleaseImage) {
			log.Info("HC spec matches but version not yet ready, performing upgrade invalidation",
				"releaseImage", cr.Spec.OCPReleaseImage)
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.HostedClusterUpgrading,
				Status:             metav1.ConditionTrue,
				Reason:             provisioningv1alpha1.ReasonUpgradeInProgress,
				Message:            fmt.Sprintf("Upgrade to %s in progress", cr.Spec.OCPReleaseImage),
				ObservedGeneration: cr.Generation,
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.IgnitionConfigured,
				Status:             metav1.ConditionFalse,
				Reason:             "ReleaseImageUpdated",
				Message:            "Ignition invalidated due to release image upgrade recovery",
				ObservedGeneration: cr.Generation,
			})
			if deleted, err := r.deleteIgnitionConfigMap(ctx, cr); err != nil {
				return ctrl.Result{}, err
			} else if deleted {
				log.Info("Deleted stale ignition ConfigMap during recovery")
			}
			cr.Status.BlueFieldOCPLayerImage = ""
		}
		return ctrl.Result{}, nil
	}

	// Case 3: Upgrade detected — HC release image differs from CR spec
	log.Info("Upgrade detected",
		"currentImage", hc.Spec.Release.Image,
		"targetImage", cr.Spec.OCPReleaseImage)

	// Set HostedClusterUpgrading=True
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.HostedClusterUpgrading,
		Status:             metav1.ConditionTrue,
		Reason:             provisioningv1alpha1.ReasonUpgradeInProgress,
		Message:            fmt.Sprintf("Upgrading from %s to %s", hc.Spec.Release.Image, cr.Spec.OCPReleaseImage),
		ObservedGeneration: cr.Generation,
	})

	// Set IgnitionConfigured=False (stale for old version)
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.IgnitionConfigured,
		Status:             metav1.ConditionFalse,
		Reason:             "ReleaseImageUpdated",
		Message:            "Ignition invalidated due to release image upgrade, will regenerate after upgrade completes",
		ObservedGeneration: cr.Generation,
	})

	// Delete stale ignition ConfigMap by label+annotation (separate API call, not affected by status conflicts)
	if deleted, err := r.deleteIgnitionConfigMap(ctx, cr); err != nil {
		return ctrl.Result{}, err
	} else if deleted {
		log.Info("Deleted stale ignition ConfigMap")
	}

	// Clear cached BlueField OCP layer image (stale for old version)
	cr.Status.BlueFieldOCPLayerImage = ""

	// Persist status and requeue
	r.computeReadyCondition(ctx, cr)
	r.updatePhaseFromConditions(cr)
	if err := r.Status().Update(ctx, cr); err != nil {
		return ctrl.Result{}, err
	}

	r.Recorder.Event(cr, corev1.EventTypeNormal, provisioningv1alpha1.ReasonUpgradeInProgress,
		fmt.Sprintf("Upgrade started: %s → %s", hc.Spec.Release.Image, cr.Spec.OCPReleaseImage))

	return ctrl.Result{RequeueAfter: 1 * time.Second}, nil
}

// isHostedClusterVersionReady checks if the HostedCluster has accepted the target release image
// by inspecting the version history. Returns true when the most recent history entry
// matches the target (Partial or Completed — both mean the ignition server is serving new content).
func (r *DPFHCPProvisionerReconciler) isHostedClusterVersionReady(hc *hyperv1.HostedCluster, targetReleaseImage string) bool {
	if hc.Status.Version == nil || len(hc.Status.Version.History) == 0 {
		return false
	}

	latest := hc.Status.Version.History[0]

	// Compare by version string if the target has a tag (e.g. :4.22.1-multi → "4.22.1")
	if strings.Contains(targetReleaseImage, ":") && !strings.Contains(targetReleaseImage, "@sha256:") {
		parts := strings.Split(targetReleaseImage, ":")
		tag := parts[len(parts)-1]
		for _, suffix := range []string{"-multi", "-amd64", "-arm64", "-ppc64le", "-s390x", "-x86_64"} {
			tag = strings.TrimSuffix(tag, suffix)
		}
		if tag != "" {
			return latest.Version == tag
		}
	}

	// Digest images: compare full image URL
	return latest.Image == targetReleaseImage
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
