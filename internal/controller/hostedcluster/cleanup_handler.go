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

package hostedcluster

import (
	"context"
	"fmt"
	"time"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

const (
	// DeletionTimeout is the maximum time to wait for HostedCluster deletion (30 minutes)
	DeletionTimeout = 30 * time.Minute

	// DeletionRequeueInterval is the interval between deletion status checks (10 seconds)
	DeletionRequeueInterval = 10 * time.Second
)

// CleanupHandler handles cleanup of HostedCluster, NodePool, and related secrets
// when a DPFHCPProvisioner CR is deleted.
//
// This handler is responsible for:
// 1. Deleting HostedCluster CR and waiting for full deletion
// 2. Deleting NodePool CR and waiting for full deletion
// 3. Deleting copied/generated secrets (pull-secret, ssh-key, etcd-encryption-key)
type CleanupHandler struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewCleanupHandler creates a new HostedCluster cleanup handler
func NewCleanupHandler(client client.Client, recorder record.EventRecorder) *CleanupHandler {
	return &CleanupHandler{
		client:   client,
		recorder: recorder,
	}
}

// Name returns the handler name for logging
func (h *CleanupHandler) Name() string {
	return "hostedcluster"
}

// Cleanup performs the cleanup logic for HostedCluster resources.
// This includes:
// 1. Deleting HostedCluster CR in the same namespace as DPFHCPProvisioner
// 2. Waiting for HostedCluster to be fully deleted
// 3. Deleting NodePool CR in the same namespace as DPFHCPProvisioner
// 4. Waiting for NodePool to be fully deleted
// 5. Deleting copied/generated secrets
//
// Returns:
// - nil if cleanup succeeded or resources are already gone
// - error if cleanup failed and should be retried
//
// Note: This handler does NOT enforce timeout. The finalizer manager
// is responsible for timeout handling if needed.
func (h *CleanupHandler) Cleanup(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx).WithValues(
		"handler", h.Name(),
		"dpfhcpprovisioner", fmt.Sprintf("%s/%s", cr.Namespace, cr.Name),
	)

	// Step 1: Delete HostedCluster and wait for it to be fully removed
	log.Info("Deleting HostedCluster")
	hcDeleted, err := h.deleteResource(ctx, cr, &hyperv1.HostedCluster{}, "HostedCluster")
	if err != nil {
		log.Error(err, "Failed to delete HostedCluster")
		return err
	}

	if !hcDeleted {
		// HostedCluster still exists, return error to trigger requeue
		log.Info("HostedCluster deletion in progress, will retry")
		return fmt.Errorf("waiting for HostedCluster deletion")
	}

	// Step 2: Delete NodePool and wait for it to be fully removed
	log.Info("HostedCluster deleted, deleting NodePool")
	npDeleted, err := h.deleteResource(ctx, cr, &hyperv1.NodePool{}, "NodePool")
	if err != nil {
		log.Error(err, "Failed to delete NodePool")
		return err
	}

	if !npDeleted {
		// NodePool still exists, return error to trigger requeue
		log.Info("NodePool deletion in progress, will retry")
		return fmt.Errorf("waiting for NodePool deletion")
	}

	// Step 3: Delete secrets
	log.Info("NodePool deleted, deleting secrets")
	if err := h.deleteSecrets(ctx, cr); err != nil {
		log.Error(err, "Failed to delete secrets")
		return err
	}

	log.Info("HostedCluster cleanup completed successfully")
	h.recorder.Event(cr, "Normal", "HostedClusterCleanupSucceeded",
		"HostedCluster, NodePool, and secrets deleted successfully")

	return nil
}

// deleteResource is a generic function to delete a Kubernetes resource and wait for deletion
// Returns true when resource is fully deleted (NotFound), false if still exists
func (h *CleanupHandler) deleteResource(
	ctx context.Context,
	cr *provisioningv1alpha1.DPFHCPProvisioner,
	obj client.Object,
	resourceKind string,
) (bool, error) {
	log := logf.FromContext(ctx)

	// Construct the resource key from CR name (HostedCluster and NodePool use same name as CR)
	key := types.NamespacedName{
		Name:      cr.Name,
		Namespace: cr.Namespace,
	}

	err := h.client.Get(ctx, key, obj)
	if err != nil {
		if apierrors.IsNotFound(err) {
			// Resource is fully deleted
			log.Info(fmt.Sprintf("%s deleted successfully", resourceKind),
				resourceKind, key.Name,
				"namespace", key.Namespace)
			return true, nil
		}
		// Handle "no matches for kind" error (CRD not installed) as if resource doesn't exist
		if meta.IsNoMatchError(err) {
			log.V(1).Info(fmt.Sprintf("%s CRD not installed, treating as deleted", resourceKind),
				resourceKind, key.Name,
				"namespace", key.Namespace)
			return true, nil
		}
		return false, fmt.Errorf("failed to get %s: %w", resourceKind, err)
	}

	// Resource still exists
	deletionTimestamp := obj.GetDeletionTimestamp()
	if deletionTimestamp == nil {
		// Resource not yet marked for deletion, delete it now
		log.Info(fmt.Sprintf("Deleting %s", resourceKind),
			resourceKind, key.Name,
			"namespace", key.Namespace)

		if err := h.client.Delete(ctx, obj); err != nil {
			if apierrors.IsNotFound(err) {
				// Already deleted (race condition)
				return true, nil
			}
			return false, fmt.Errorf("failed to delete %s: %w", resourceKind, err)
		}
		log.Info(fmt.Sprintf("%s deletion initiated", resourceKind),
			resourceKind, key.Name,
			"namespace", key.Namespace)
	} else {
		// Resource is marked for deletion but still exists (finalizers running)
		elapsedDeletion := time.Since(deletionTimestamp.Time)
		log.V(1).Info(fmt.Sprintf("%s deletion in progress (finalizers running)", resourceKind),
			resourceKind, key.Name,
			"namespace", key.Namespace,
			"deletionElapsed", elapsedDeletion)
	}

	// Resource still exists, need to wait
	return false, nil
}

// deleteSecrets deletes all copied/generated secrets
func (h *CleanupHandler) deleteSecrets(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx)

	secretNames := []string{
		fmt.Sprintf("%s-pull-secret", cr.Name),
		fmt.Sprintf("%s-ssh-key", cr.Name),
		fmt.Sprintf("%s-etcd-encryption-key", cr.Name),
	}

	for _, secretName := range secretNames {
		if err := h.deleteSecret(ctx, cr.Namespace, secretName); err != nil {
			log.Error(err, "Failed to delete secret",
				"secret", secretName,
				"namespace", cr.Namespace)
			return err
		}
	}

	log.Info("All secrets deleted successfully",
		"count", len(secretNames),
		"namespace", cr.Namespace)

	return nil
}

// deleteSecret deletes a single secret
func (h *CleanupHandler) deleteSecret(ctx context.Context, namespace, secretName string) error {
	log := logf.FromContext(ctx)

	secret := &corev1.Secret{}
	secretKey := types.NamespacedName{
		Name:      secretName,
		Namespace: namespace,
	}

	err := h.client.Get(ctx, secretKey, secret)
	if err != nil {
		if apierrors.IsNotFound(err) {
			// Secret already deleted
			log.V(1).Info("Secret already deleted",
				"secret", secretName,
				"namespace", namespace)
			return nil
		}
		return fmt.Errorf("failed to get secret %s: %w", secretName, err)
	}

	// Delete secret
	log.V(1).Info("Deleting secret",
		"secret", secretName,
		"namespace", namespace)

	if err := h.client.Delete(ctx, secret); err != nil {
		if apierrors.IsNotFound(err) {
			// Already deleted (race condition)
			return nil
		}
		return fmt.Errorf("failed to delete secret %s: %w", secretName, err)
	}

	log.Info("Secret deleted successfully",
		"secret", secretName,
		"namespace", namespace)

	return nil
}
