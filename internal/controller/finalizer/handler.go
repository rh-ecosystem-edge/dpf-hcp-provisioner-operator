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

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// CleanupHandler defines the interface for cleanup handlers that are executed
// during finalizer cleanup when a DPFHCPProvisioner CR is deleted.
//
// Handlers are executed in the order they are registered with the Manager.
// Each handler should clean up resources it created and return an error if
// cleanup should be retried.
type CleanupHandler interface {
	// Name returns the handler name for logging and identification purposes.
	// Should be a short, descriptive name like "hostedcluster" or "kubeconfig-injection".
	Name() string

	// Cleanup performs the cleanup logic for this handler.
	// It should clean up all resources created by the corresponding feature.
	//
	// Returns:
	// - nil if cleanup succeeded or resources are already gone
	// - error if cleanup failed and should be retried
	//
	// The handler should be idempotent - calling Cleanup multiple times should
	// be safe and result in the same final state.
	Cleanup(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) error
}
