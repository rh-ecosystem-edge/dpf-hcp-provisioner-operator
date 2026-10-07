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

package dpuservicetemplate

import (
	"context"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

// DPUServiceTemplateReconciler manages DPUServiceTemplate resources based on
// DPFHCPProvisioner and DPUDeployment presence. Templates are created when an
// active provisioner references a non-deleting DPUDeployment in the DPUCluster
// namespace, and deleted when no provisioner or DPUDeployment remains.
type DPUServiceTemplateReconciler struct {
	client.Client
	Scheme  *runtime.Scheme
	Manager *DPUServiceTemplateManager
}

// +kubebuilder:rbac:groups=svc.dpu.nvidia.com,resources=dpuservicetemplates,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=svc.dpu.nvidia.com,resources=dpudeployments,verbs=get;list;watch
// +kubebuilder:rbac:groups=apps,resources=daemonsets,verbs=get;list;watch
// +kubebuilder:rbac:groups=config.openshift.io,resources=clusterversions,verbs=get;list;watch
// +kubebuilder:rbac:groups="",resources=secrets,verbs=get;list;watch,namespace=openshift-config

// Reconcile ensures DPUServiceTemplates are in sync for a given DPUCluster namespace.
func (r *DPUServiceTemplateReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	// The reconcile request's Name is the DPUCluster namespace (not a real object key).
	// See SetupWithManager mappings for more info
	dpuClusterNS := req.Name
	log := logf.FromContext(ctx).WithValues("dpuClusterNamespace", dpuClusterNS)

	// TODO: Remove this when we remove the flag which turns off DPUServiceTemplate management.
	operatorConfig, err := common.LoadOperatorConfigFromCR(ctx, r.Client)
	if err != nil {
		log.Error(err, "Failed to load operator configuration")
		return ctrl.Result{}, err
	}
	if operatorConfig == nil || !operatorConfig.ManageDPUServiceTemplates {
		log.V(1).Info("DPUServiceTemplate management is disabled")
		return ctrl.Result{}, nil
	}

	var allProvisioners provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &allProvisioners); err != nil {
		return ctrl.Result{}, err
	}

	deployments, err := r.activeDPUDeployments(ctx, &allProvisioners, dpuClusterNS)
	if err != nil {
		return ctrl.Result{}, err
	}
	if len(deployments) == 0 {
		if err := r.Manager.DeleteTemplates(ctx, dpuClusterNS); err != nil {
			log.Error(err, "Failed to delete DPUServiceTemplates")
			return ctrl.Result{}, err
		}
		return ctrl.Result{}, nil
	}

	if err := r.Manager.EnsureTemplates(ctx, dpuClusterNS, operatorConfig); err != nil {
		log.Error(err, "Failed to ensure DPUServiceTemplates")
		return ctrl.Result{}, err
	}
	return ctrl.Result{}, nil
}

// activeDPUDeployments returns non-deleting DPUDeployments referenced by
// active provisioners for this DPUCluster namespace.
func (r *DPUServiceTemplateReconciler) activeDPUDeployments(ctx context.Context, allProvisioners *provisioningv1alpha1.DPFHCPProvisionerList, dpuClusterNamespace string) ([]dpuservicev1alpha1.DPUDeployment, error) {
	seen := make(map[types.NamespacedName]struct{})
	var deployments []dpuservicev1alpha1.DPUDeployment

	for i := range allProvisioners.Items {
		// index-based loop to avoid copying provisioner structs
		p := &allProvisioners.Items[i]
		if !p.DeletionTimestamp.IsZero() || p.Spec.DPUClusterRef.Namespace != dpuClusterNamespace {
			continue
		}
		if p.Spec.DPUDeploymentRef == nil {
			continue
		}

		key := types.NamespacedName{
			Name:      p.Spec.DPUDeploymentRef.Name,
			Namespace: p.Spec.DPUDeploymentRef.Namespace,
		}
		if _, ok := seen[key]; ok {
			continue
		}

		dpuDeployment := &dpuservicev1alpha1.DPUDeployment{}
		err := r.Get(ctx, key, dpuDeployment)
		if apierrors.IsNotFound(err) {
			continue
		}
		if err != nil {
			return nil, err
		}
		if !dpuDeployment.DeletionTimestamp.IsZero() {
			continue
		}

		seen[key] = struct{}{}
		deployments = append(deployments, *dpuDeployment)
	}

	return deployments, nil
}

// SetupWithManager registers this controller with the manager.
func (r *DPUServiceTemplateReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		Named("dpuservicetemplate").
		Watches(
			&provisioningv1alpha1.DPFHCPProvisioner{},
			handler.EnqueueRequestsFromMapFunc(r.provisionerToNamespace),
		).
		Watches(
			&dpuservicev1alpha1.DPUDeployment{},
			handler.EnqueueRequestsFromMapFunc(r.dpuDeploymentToNamespace),
		).
		Watches(
			&appsv1.DaemonSet{},
			handler.EnqueueRequestsFromMapFunc(r.toAllProvisionerNamespaces),
			// We only care about the OVN DaemonSet
			builder.WithPredicates(ovnDaemonSetPredicate()),
		).
		// TODO: Remove this when we remove the flag which turns off DPUServiceTemplate management. Currently We need to watch the config to trigger reconciles when the flag is toggled.
		Watches(
			&provisioningv1alpha1.DPFHCPProvisionerConfig{},
			handler.EnqueueRequestsFromMapFunc(r.toAllProvisionerNamespaces),
			// Avoids status update reconciliations
			builder.WithPredicates(predicate.GenerationChangedPredicate{}),
		).
		Watches(
			&corev1.ConfigMap{},
			handler.EnqueueRequestsFromMapFunc(r.toAllProvisionerNamespaces),
			builder.WithPredicates(overridesConfigMapPredicate(r.Manager.OperatorNamespace)),
		).
		Watches(
			&dpuservicev1alpha1.DPUServiceTemplate{},
			handler.EnqueueRequestsFromMapFunc(r.templateToNamespace),
		).
		Complete(r)
}

// provisionerToNamespace maps a DPFHCPProvisioner event to a reconcile request
// keyed by the DPUCluster namespace.
func (r *DPUServiceTemplateReconciler) provisionerToNamespace(_ context.Context, obj client.Object) []reconcile.Request {
	provisioner, ok := obj.(*provisioningv1alpha1.DPFHCPProvisioner)
	if !ok {
		return nil
	}
	return []reconcile.Request{
		{NamespacedName: types.NamespacedName{Name: provisioner.Spec.DPUClusterRef.Namespace}},
	}
}

// toAllProvisionerNamespaces wraps allProvisionerDPUClusterNamespaces to match the MapFunc signature.
func (r *DPUServiceTemplateReconciler) toAllProvisionerNamespaces(ctx context.Context, _ client.Object) []reconcile.Request {
	return r.allProvisionerDPUClusterNamespaces(ctx)
}

// allProvisionerDPUClusterNamespaces finds all namespaces managed by all the
// DPFHCPProvisioners and creates a reconcile.Request for each unique
// namespace, then returns the list of the requests.
func (r *DPUServiceTemplateReconciler) allProvisionerDPUClusterNamespaces(ctx context.Context) []reconcile.Request {
	log := logf.FromContext(ctx)

	var provisioners provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisioners); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioners")
		return nil
	}

	// Multiple provisioners might reference the same DPUCluster namespace, so
	// we need to deduplicate the requests with this ad-hoc set.
	seenNamespacesSet := make(map[string]struct{})

	var requests []reconcile.Request
	for provisionerIdx := range provisioners.Items {
		// The namespace of the provisioner itself is not relevant, we care
		// about the namespace of the DPUCluster it references.
		provisionerManagedNamespace := provisioners.Items[provisionerIdx].Spec.DPUClusterRef.Namespace
		if _, ok := seenNamespacesSet[provisionerManagedNamespace]; ok {
			continue
		}

		// Mark set as seen so we don't generate duplicate requests for the same namespace.
		seenNamespacesSet[provisionerManagedNamespace] = struct{}{}

		// Finally generate a request so we reconcile that namespace.
		requests = append(requests, reconcile.Request{
			NamespacedName: types.NamespacedName{Name: provisionerManagedNamespace},
		})
	}

	return requests
}

// templateToNamespace maps a DPUServiceTemplate event to a reconcile request.
// This controller reconciles all templates in a namespace at once, so the reconcile
// key is the namespace name (not an individual template name).
func (r *DPUServiceTemplateReconciler) templateToNamespace(_ context.Context, obj client.Object) []reconcile.Request {
	return []reconcile.Request{
		{NamespacedName: types.NamespacedName{Name: obj.GetNamespace()}},
	}
}

// dpuDeploymentToNamespace maps a DPUDeployment event to reconcile requests for
// DPUCluster namespaces of provisioners that reference the DPUDeployment.
func (r *DPUServiceTemplateReconciler) dpuDeploymentToNamespace(ctx context.Context, obj client.Object) []reconcile.Request {
	dpuDeployment, ok := obj.(*dpuservicev1alpha1.DPUDeployment)
	if !ok {
		return nil
	}

	var provisioners provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisioners); err != nil {
		logf.FromContext(ctx).Error(err, "Failed to list DPFHCPProvisioners for DPUDeployment watch")
		return nil
	}

	seenNamespacesSet := make(map[string]struct{})
	var requests []reconcile.Request
	for i := range provisioners.Items {
		p := &provisioners.Items[i]
		if p.Spec.DPUDeploymentRef == nil ||
			p.Spec.DPUDeploymentRef.Name != dpuDeployment.Name ||
			p.Spec.DPUDeploymentRef.Namespace != dpuDeployment.Namespace {
			continue
		}

		ns := p.Spec.DPUClusterRef.Namespace
		if _, ok := seenNamespacesSet[ns]; ok {
			continue
		}
		seenNamespacesSet[ns] = struct{}{}
		requests = append(requests, reconcile.Request{
			NamespacedName: types.NamespacedName{Name: ns},
		})
	}
	return requests
}

func overridesConfigMapPredicate(operatorNamespace string) predicate.Predicate {
	isOverridesConfigMap := func(obj client.Object) bool {
		return obj.GetName() == overridesConfigMapName &&
			obj.GetNamespace() == operatorNamespace
	}
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			return isOverridesConfigMap(e.Object)
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			return isOverridesConfigMap(e.ObjectNew)
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			return isOverridesConfigMap(e.Object)
		},
	}
}

func ovnDaemonSetPredicate() predicate.Predicate {
	isOVNDaemonSet := func(obj client.Object) bool {
		return obj.GetName() == ovnDPUDaemonSetName &&
			obj.GetNamespace() == ovnDPUDaemonSetNamespace
	}
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			return isOVNDaemonSet(e.Object)
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			return isOVNDaemonSet(e.ObjectNew)
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			return false
		},
	}
}
