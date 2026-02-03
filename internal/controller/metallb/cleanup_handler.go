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

package metallb

import (
	"context"
	"fmt"

	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

// CleanupHandler handles cleanup of MetalLB resources (IPAddressPool and L2Advertisement)
// when a DPFHCPProvisioner CR is deleted.
//
// This handler is responsible for:
// 1. Deleting IPAddressPool created for this DPFHCPProvisioner
// 2. Deleting L2Advertisement created for this DPFHCPProvisioner
// 3. Waiting for resources to be fully deleted before completing
type CleanupHandler struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewCleanupHandler creates a new MetalLB cleanup handler
func NewCleanupHandler(client client.Client, recorder record.EventRecorder) *CleanupHandler {
	return &CleanupHandler{
		client:   client,
		recorder: recorder,
	}
}

// Name returns the handler name for logging
func (h *CleanupHandler) Name() string {
	return "metallb"
}

// Cleanup deletes MetalLB resources (IPAddressPool and L2Advertisement) created for this DPFHCPProvisioner.
// This is called during finalizer cleanup when the DPFHCPProvisioner is deleted.
//
// The cleanup process:
// 1. Delete IPAddressPool if it exists
// 2. Wait for IPAddressPool to be fully deleted
// 3. Delete L2Advertisement if it exists
// 4. Wait for L2Advertisement to be fully deleted
func (h *CleanupHandler) Cleanup(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx).WithValues(
		"handler", h.Name(),
		common.DPFHCPProvisionerName, fmt.Sprintf("%s/%s", cr.Namespace, cr.Name),
	)

	log.Info("Cleaning up MetalLB resources")

	// Step 1: Delete IPAddressPool
	pool := &metallbv1beta1.IPAddressPool{}
	err := h.client.Get(ctx, client.ObjectKey{
		Name:      cr.Name,
		Namespace: common.OpenshiftOperatorsNamespace,
	}, pool)

	if err != nil {
		// Check if resource was already deleted (NotFound is success)
		if apierrors.IsNotFound(err) {
			log.V(1).Info("IPAddressPool already deleted or never existed")
		} else {
			// Unexpected error (permission denied, network issue, etc.)
			log.Error(err, "Failed to get IPAddressPool")
			return fmt.Errorf("getting IPAddressPool: %w", err)
		}
	} else {
		// Resource still exists, initiate deletion
		log.Info("Deleting IPAddressPool",
			"name", pool.Name,
			"namespace", pool.Namespace)

		if err := h.client.Delete(ctx, pool); err != nil {
			if !apierrors.IsNotFound(err) {
				// Deletion failed with non-NotFound error
				log.Error(err, "Failed to delete IPAddressPool")
				return fmt.Errorf("deleting IPAddressPool: %w", err)
			}
			// Resource was deleted between GET and DELETE (race condition - this is OK)
		}

		// Resource deletion initiated, wait for it to complete
		log.V(1).Info("Waiting for IPAddressPool deletion to complete")
		return fmt.Errorf("waiting for IPAddressPool deletion")
	}

	// Step 2: Delete L2Advertisement
	advert := &metallbv1beta1.L2Advertisement{}
	err = h.client.Get(ctx, client.ObjectKey{
		Name:      fmt.Sprintf("advertise-%s", cr.Name),
		Namespace: common.OpenshiftOperatorsNamespace,
	}, advert)

	if err != nil {
		// Check if resource was already deleted (NotFound is success)
		if apierrors.IsNotFound(err) {
			log.V(1).Info("L2Advertisement already deleted or never existed")
		} else {
			// Unexpected error (permission denied, network issue, etc.)
			log.Error(err, "Failed to get L2Advertisement")
			return fmt.Errorf("getting L2Advertisement: %w", err)
		}
	} else {
		// Resource still exists, initiate deletion
		log.Info("Deleting L2Advertisement",
			"name", advert.Name,
			"namespace", advert.Namespace)

		if err := h.client.Delete(ctx, advert); err != nil {
			if !apierrors.IsNotFound(err) {
				// Deletion failed with non-NotFound error
				log.Error(err, "Failed to delete L2Advertisement")
				return fmt.Errorf("deleting L2Advertisement: %w", err)
			}
			// Resource was deleted between GET and DELETE (race condition - this is OK)
		}

		// Resource deletion initiated, wait for it to complete
		log.V(1).Info("Waiting for L2Advertisement deletion to complete")
		return fmt.Errorf("waiting for L2Advertisement deletion")
	}

	// All resources cleaned up successfully
	log.Info("MetalLB cleanup completed successfully")
	h.recorder.Event(cr, "Normal", "MetalLBCleanupComplete", "MetalLB resources cleaned up successfully")

	return nil
}
