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
	"fmt"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
)

// CleanupHandler deletes the custom-bfb.cfg ConfigMap when a DPFHCPProvisioner is deleted.
type CleanupHandler struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewCleanupHandler creates a new ignition cleanup handler.
func NewCleanupHandler(c client.Client, recorder record.EventRecorder) *CleanupHandler {
	return &CleanupHandler{client: c, recorder: recorder}
}

// Name returns the handler name used in log messages.
func (h *CleanupHandler) Name() string {
	return "ignition"
}

// Cleanup deletes the custom-bfb.cfg ConfigMap from dpf-operator-system.
// It is idempotent: a NotFound error is treated as success.
func (h *CleanupHandler) Cleanup(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx).WithValues(
		"handler", h.Name(),
		common.DPFHCPProvisionerName, fmt.Sprintf("%s/%s", cr.Namespace, cr.Name),
	)
	log.Info("Cleaning up ignition output ConfigMap")

	cm := &corev1.ConfigMap{}
	err := h.client.Get(ctx, client.ObjectKey{
		Name:      outputConfigMapName,
		Namespace: DPFOperatorNamespace,
	}, cm)
	if err != nil {
		if apierrors.IsNotFound(err) {
			log.V(1).Info("Output ConfigMap already deleted or never existed")
			return nil
		}
		return fmt.Errorf("getting output ConfigMap: %w", err)
	}

	if err := h.client.Delete(ctx, cm); err != nil && !apierrors.IsNotFound(err) {
		return fmt.Errorf("deleting output ConfigMap: %w", err)
	}

	log.Info("Ignition cleanup completed successfully")
	h.recorder.Event(cr, "Normal", "IgnitionCleanupComplete", "Ignition output ConfigMap deleted")
	return nil
}
