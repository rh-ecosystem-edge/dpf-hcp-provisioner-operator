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

package kubeconfiginjection

import (
	"context"
	"strings"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// IsHostedClusterKubeconfigSecretPredicate returns a predicate that filters for HC kubeconfig secrets
// Only watches secrets with name pattern "*-admin-kubeconfig"
func IsHostedClusterKubeconfigSecretPredicate() predicate.Predicate {
	return predicate.Funcs{
		CreateFunc: func(e event.CreateEvent) bool {
			return isHostedClusterKubeconfigSecret(e.Object)
		},
		UpdateFunc: func(e event.UpdateEvent) bool {
			return isHostedClusterKubeconfigSecret(e.ObjectNew)
		},
		DeleteFunc: func(e event.DeleteEvent) bool {
			return isHostedClusterKubeconfigSecret(e.Object)
		},
		GenericFunc: func(e event.GenericEvent) bool {
			// Don't watch generic events for secrets
			return false
		},
	}
}

// isHostedClusterKubeconfigSecret checks if a secret is an HC admin kubeconfig secret
// Returns true if secret name ends with "-admin-kubeconfig"
func isHostedClusterKubeconfigSecret(obj client.Object) bool {
	secret, ok := obj.(*corev1.Secret)
	if !ok {
		return false
	}
	return strings.HasSuffix(secret.Name, KubeconfigSecretSuffix)
}

// FindProvisionerForKubeconfigSecret maps HC kubeconfig secret to DPFHCPProvisioner CR
// Extracts HC name from secret name and finds corresponding DPFHCPProvisioner
// Returns reconcile requests for the matching DPFHCPProvisioner
func FindProvisionerForKubeconfigSecret(ctx context.Context, c client.Client, obj client.Object) []reconcile.Request {
	log := logf.FromContext(ctx)

	secret, ok := obj.(*corev1.Secret)
	if !ok {
		log.Error(nil, "Failed to convert object to Secret", "object", obj)
		return []reconcile.Request{}
	}

	// Extract HC name from secret name (remove "-admin-kubeconfig" suffix)
	hcName := strings.TrimSuffix(secret.Name, KubeconfigSecretSuffix)

	// Find corresponding DPFHCPProvisioner
	provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
	err := c.Get(ctx, types.NamespacedName{
		Name:      hcName,
		Namespace: secret.Namespace,
	}, provisioner)

	if err != nil {
		// Provisioner not found - this is normal if the secret belongs to a different HC
		log.V(1).Info("No DPFHCPProvisioner found for HC kubeconfig secret",
			"secretName", secret.Name,
			"secretNamespace", secret.Namespace,
			"extractedHCName", hcName,
			"error", err)
		return []reconcile.Request{}
	}

	log.Info("HC kubeconfig secret changed, triggering reconciliation",
		"secretName", secret.Name,
		"secretNamespace", secret.Namespace,
		"provisioner", provisioner.Name,
		"provisionerNamespace", provisioner.Namespace)

	// Trigger reconciliation for this provisioner
	return []reconcile.Request{
		{
			NamespacedName: types.NamespacedName{
				Name:      provisioner.Name,
				Namespace: provisioner.Namespace,
			},
		},
	}
}
