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

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

// DPUServiceTemplateReconciler manages DPUServiceTemplate resources based on
// DPFHCPProvisioner presence. Templates are created when at least one active
// provisioner references a DPUCluster namespace, and deleted when none remain.
type DPUServiceTemplateReconciler struct {
	client.Client
	Scheme   *runtime.Scheme
	Recorder record.EventRecorder
	Manager  *DPUServiceTemplateManager
}

// +kubebuilder:rbac:groups=svc.dpu.nvidia.com,resources=dpuservicetemplates,verbs=get;list;watch;create;update;patch;delete
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

	hasActive := false
	for i := range allProvisioners.Items {
		// index-based loop to avoid copying provisioner structs
		p := &allProvisioners.Items[i]
		if p.DeletionTimestamp.IsZero() && p.Spec.DPUClusterRef.Namespace == dpuClusterNS {
			hasActive = true
			break
		}
	}

	if hasActive {
		if err := r.Manager.EnsureTemplates(ctx, dpuClusterNS); err != nil {
			log.Error(err, "Failed to ensure DPUServiceTemplates")
			return ctrl.Result{}, err
		}
		return ctrl.Result{}, nil
	}

	// No active provisioners reference this namespace, ensure templates are deleted - they're
	// not used by any provisioner in the namespace.
	if err := r.Manager.DeleteTemplates(ctx, dpuClusterNS); err != nil {
		log.Error(err, "Failed to delete DPUServiceTemplates")
		return ctrl.Result{}, err
	}
	return ctrl.Result{}, nil
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
			&appsv1.DaemonSet{},
			handler.EnqueueRequestsFromMapFunc(r.ovnDaemonSetToNamespaces),
			// We only care about the OVN DaemonSet
			builder.WithPredicates(ovnDaemonSetPredicate()),
		).
		// TODO: Remove this when we remove the flag which turns off DPUServiceTemplate management. Currently We need to watch the config to trigger reconciles when the flag is toggled.
		Watches(
			&provisioningv1alpha1.DPFHCPProvisionerConfig{},
			handler.EnqueueRequestsFromMapFunc(r.configToNamespaces),
			// Avoids status update reconciliations
			builder.WithPredicates(predicate.GenerationChangedPredicate{}),
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

// ovnDaemonSetToNamespaces enqueues reconcile requests for all active DPUCluster namespaces
// when the OVN DaemonSet changes.
func (r *DPUServiceTemplateReconciler) ovnDaemonSetToNamespaces(ctx context.Context, _ client.Object) []reconcile.Request {
	return r.allActiveNamespaces(ctx)
}

// configToNamespaces enqueues reconcile requests for all active DPUCluster namespaces
// when the operator config changes.
// TODO: Remove this when we remove the flag which turns off DPUServiceTemplate management. Currently We need to watch the config to trigger reconciles when the flag is toggled.
func (r *DPUServiceTemplateReconciler) configToNamespaces(ctx context.Context, _ client.Object) []reconcile.Request {
	return r.allActiveNamespaces(ctx)
}

func (r *DPUServiceTemplateReconciler) allActiveNamespaces(ctx context.Context) []reconcile.Request {
	log := logf.FromContext(ctx)

	var list provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &list); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioners")
		return nil
	}

	seen := make(map[string]struct{})
	var requests []reconcile.Request
	for i := range list.Items {
		ns := list.Items[i].Spec.DPUClusterRef.Namespace
		if _, ok := seen[ns]; ok {
			continue
		}
		seen[ns] = struct{}{}
		requests = append(requests, reconcile.Request{
			NamespacedName: types.NamespacedName{Name: ns},
		})
	}

	return requests
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
