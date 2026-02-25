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

package ignition

import (
	"context"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	corev1 "k8s.io/api/core/v1"
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
)

// IgnitionReconciler reconciles ignition config generation for DPFHCPProvisioner objects.
// It runs as a separate controller alongside the main DPFHCPProvisionerReconciler.
type IgnitionReconciler struct {
	client.Client
	Recorder record.EventRecorder
	Manager  *IgnitionManager
}

// Reconcile fetches the DPFHCPProvisioner and runs ignition config generation.
// +kubebuilder:rbac:groups="",resources=configmaps,verbs=get;list;watch;create;update;patch
func (r *IgnitionReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := logf.FromContext(ctx)
	log.V(1).Info("Reconciling ignition config", "namespace", req.Namespace, "name", req.Name)

	var cr provisioningv1alpha1.DPFHCPProvisioner
	if err := r.Get(ctx, req.NamespacedName, &cr); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	if !cr.DeletionTimestamp.IsZero() {
		return ctrl.Result{}, nil
	}

	return r.Manager.GenerateAndApplyIgnition(ctx, &cr)
}

// SetupWithManager registers the IgnitionReconciler with the controller manager.
// It watches DPFHCPProvisioner resources and the three ignition content ConfigMaps.
func (r *IgnitionReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&provisioningv1alpha1.DPFHCPProvisioner{}).
		Watches(
			&corev1.ConfigMap{},
			handler.EnqueueRequestsFromMapFunc(r.ignitionCMToRequests),
			builder.WithPredicates(ignitionContentCMPredicate()),
		).
		Named("ignition").
		Complete(r)
}

// ignitionContentCMPredicate filters ConfigMap events to the 3 ignition content ConfigMaps.
func ignitionContentCMPredicate() predicate.Predicate {
	ignitionCMNames := map[string]bool{
		commonCMName: true,
		liveCMName:   true,
		targetCMName: true,
	}
	matches := func(name, ns string) bool {
		return ignitionCMNames[name] && ns == DPFHCPProvisionerNamespace
	}
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			return matches(e.Object.GetName(), e.Object.GetNamespace())
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			return matches(e.ObjectNew.GetName(), e.ObjectNew.GetNamespace())
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			return matches(e.Object.GetName(), e.Object.GetNamespace())
		},
	}
}

// ignitionCMToRequests maps ignition content ConfigMap events to reconcile requests for all
// DPFHCPProvisioner CRs (content change requires regeneration for all).
func (r *IgnitionReconciler) ignitionCMToRequests(ctx context.Context, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	var provisionerList provisioningv1alpha1.DPFHCPProvisionerList
	if err := r.List(ctx, &provisionerList); err != nil {
		log.Error(err, "Failed to list DPFHCPProvisioner CRs for ignition ConfigMap watch")
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

	log.Info("Ignition content ConfigMap changed, reconciling all DPFHCPProvisioner CRs",
		"configMap", obj.GetName(),
		"count", len(requests))

	return requests
}
