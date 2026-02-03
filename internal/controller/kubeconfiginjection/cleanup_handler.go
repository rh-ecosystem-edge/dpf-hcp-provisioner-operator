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
	"fmt"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

// CleanupHandler handles cleanup of kubeconfig secrets created in DPUCluster namespace
// when a DPFHCPProvisioner CR is deleted.
//
// This handler is responsible for:
// 1. Finding kubeconfig secrets by labels (owned by this DPFHCPProvisioner)
// 2. Deleting all found kubeconfig secrets
type CleanupHandler struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewCleanupHandler creates a new kubeconfig cleanup handler
func NewCleanupHandler(client client.Client, recorder record.EventRecorder) *CleanupHandler {
	return &CleanupHandler{
		client:   client,
		recorder: recorder,
	}
}

// Name returns the handler name for logging
func (h *CleanupHandler) Name() string {
	return "kubeconfig-injection"
}

// Cleanup deletes the kubeconfig secret created in DPUCluster namespace.
// This is called during finalizer cleanup when the DPFHCPProvisioner is deleted.
//
// The cleanup process:
// 1. List all secrets with labels matching this DPFHCPProvisioner
// 2. Delete each found secret
//
// Labels used for finding secrets (defined in internal/common/constants.go):
// - common.LabelDPFHCPProvisionerName: <provisioner-name>
// - common.LabelDPFHCPProvisionerNamespace: <provisioner-namespace>
//
// Returns:
// - nil if cleanup succeeded or secrets are already gone
// - error if cleanup failed and should be retried
func (h *CleanupHandler) Cleanup(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx).WithValues(
		"handler", h.Name(),
		common.DPFHCPProvisionerName, fmt.Sprintf("%s/%s", cr.Namespace, cr.Name),
	)

	log.Info("Cleaning up kubeconfig secrets")

	// Find kubeconfig secret by labels
	secretList := &corev1.SecretList{}
	err := h.client.List(ctx, secretList,
		client.MatchingLabels{
			common.LabelDPFHCPProvisionerName:      cr.Name,
			common.LabelDPFHCPProvisionerNamespace: cr.Namespace,
		})
	if err != nil {
		log.Error(err, "Failed to list kubeconfig secrets")
		return fmt.Errorf("failed to list kubeconfig secrets: %w", err)
	}

	if len(secretList.Items) == 0 {
		log.Info("No kubeconfig secrets found, nothing to clean up")
		return nil
	}

	// Delete found secrets
	deletedCount := 0
	for i := range secretList.Items {
		secret := &secretList.Items[i]
		log.Info("Deleting kubeconfig secret",
			"secretName", secret.Name,
			"namespace", secret.Namespace)

		if err := h.client.Delete(ctx, secret); err != nil {
			if apierrors.IsNotFound(err) {
				// Already deleted (race condition)
				log.V(1).Info("Kubeconfig secret already deleted",
					"secretName", secret.Name,
					"namespace", secret.Namespace)
				continue
			}
			log.Error(err, "Failed to delete kubeconfig secret",
				"secretName", secret.Name,
				"namespace", secret.Namespace)
			return fmt.Errorf("failed to delete kubeconfig secret %s/%s: %w", secret.Namespace, secret.Name, err)
		}

		deletedCount++
		log.Info("Kubeconfig secret deleted successfully",
			"secretName", secret.Name,
			"namespace", secret.Namespace)
	}

	log.Info("Kubeconfig cleanup completed successfully",
		"deletedCount", deletedCount)
	h.recorder.Eventf(cr, "Normal", "KubeconfigCleanupSucceeded",
		"Deleted %d kubeconfig secret(s)", deletedCount)

	return nil
}
