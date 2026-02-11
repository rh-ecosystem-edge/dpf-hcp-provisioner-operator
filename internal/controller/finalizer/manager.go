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

package finalizer

import (
	"context"

	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// Manager manages the finalizer cleanup process for DPFHCPProvisioner resources.
// It maintains a list of cleanup handlers that are executed in order during
// the finalizer cleanup phase.
//
// The Manager uses a single finalizer (dpfhcpprovisioner.provisioning.dpu.hcp.io/finalizer)
// and executes all registered handlers sequentially. This ensures:
// - Explicit cleanup ordering (handlers execute in registration order)
// - Atomicity (all cleanup succeeds or all cleanup is retried)
// - Single blocking point (simpler debugging than multiple finalizers)
type Manager struct {
	client   client.Client
	recorder record.EventRecorder
	handlers []CleanupHandler
}

// NewManager creates a new finalizer Manager with no handlers registered.
// Handlers must be registered using RegisterHandler before the Manager can perform cleanup.
func NewManager(client client.Client, recorder record.EventRecorder) *Manager {
	return &Manager{
		client:   client,
		recorder: recorder,
		handlers: make([]CleanupHandler, 0),
	}
}

// RegisterHandler adds a cleanup handler to the manager.
// Handlers are executed in the order they are registered.
//
// IMPORTANT: Order matters! Register dependent resources before their dependencies.
// Example: Register kubeconfig cleanup before HostedCluster cleanup, since kubeconfig
// is a dependent resource.
func (m *Manager) RegisterHandler(handler CleanupHandler) {
	m.handlers = append(m.handlers, handler)
}

// HandleFinalizerCleanup executes all registered cleanup handlers in order.
// It is called when a DPFHCPProvisioner CR is deleted and the finalizer needs to run.
//
// Returns:
// - ctrl.Result{}: Cleanup completed, finalizer can be removed
// - ctrl.Result{Requeue: true} or ctrl.Result{RequeueAfter: duration}: Cleanup in progress, retry needed
// - error: Cleanup failed, will be retried with exponential backoff
func (m *Manager) HandleFinalizerCleanup(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)
	log.Info("Starting finalizer cleanup with registered handlers", "handlerCount", len(m.handlers))

	// Execute all handlers in registration order
	for i, handler := range m.handlers {
		handlerLog := log.WithValues("handler", handler.Name(), "index", i, "total", len(m.handlers))
		handlerLog.Info("Executing cleanup handler")

		// Execute handler cleanup
		if err := handler.Cleanup(ctx, cr); err != nil {
			handlerLog.Error(err, "Cleanup handler failed")
			m.recorder.Eventf(cr, "Warning", "CleanupHandlerFailed",
				"Cleanup handler '%s' failed: %v", handler.Name(), err)

			// Return error to trigger requeue with exponential backoff
			return ctrl.Result{}, err
		}

		handlerLog.Info("Cleanup handler completed successfully")
	}

	// All handlers succeeded
	log.Info("All cleanup handlers completed successfully")
	m.recorder.Event(cr, "Normal", "CleanupSucceeded", "All resources cleaned up successfully")

	return ctrl.Result{}, nil
}
