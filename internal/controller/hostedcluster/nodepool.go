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

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// NodePoolManager manages NodePool resources
type NodePoolManager struct {
	client.Client
	Scheme *runtime.Scheme
}

// NewNodePoolManager creates a new NodePoolManager
func NewNodePoolManager(c client.Client, scheme *runtime.Scheme) *NodePoolManager {
	return &NodePoolManager{
		Client: c,
		Scheme: scheme,
	}
}

// CreateNodePool creates the NodePool resource
// Returns ctrl.Result and error for reconciliation flow
//
// NodePool is created with:
// - replicas=0 (DPU workers will join manually via CSR approval)
// - None platform type
// - Matching release image from DPFHCPProvisioner
// - Upgrade type: Replace (as per spec)
func (nm *NodePoolManager) CreateNodePool(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	npName := cr.Name
	npNamespace := cr.Namespace

	// Check if NodePool already exists (idempotency)
	existingNP := &hyperv1.NodePool{}
	npKey := types.NamespacedName{Name: npName, Namespace: npNamespace}
	err := nm.Get(ctx, npKey, existingNP)

	if err == nil {
		// NodePool exists - verify ownership via OwnerReference
		if metav1.IsControlledBy(existingNP, cr) {
			log.V(1).Info("NodePool already exists and is owned by this DPFHCPProvisioner",
				"nodePool", npName,
				"namespace", npNamespace)
			return ctrl.Result{}, nil
		}

		// Name conflict - NP exists but owned by different DPFHCPProvisioner
		return ctrl.Result{}, fmt.Errorf("nodePool %s exists in %s but is owned by different DPFHCPProvisioner", npName, npNamespace)
	}

	if !apierrors.IsNotFound(err) {
		return ctrl.Result{}, fmt.Errorf("failed to check existing NodePool: %w", err)
	}

	// NodePool doesn't exist - create it
	log.Info("Creating NodePool",
		"nodePool", npName,
		"namespace", npNamespace,
		"replicas", 0)

	np := nm.buildNodePool(cr)

	// Set owner reference for automatic garbage collection
	if err := controllerutil.SetControllerReference(cr, np, nm.Scheme); err != nil {
		log.Error(err, "Failed to set owner reference on NodePool")
		return ctrl.Result{}, fmt.Errorf("failed to set owner reference on NodePool: %w", err)
	}

	if err := nm.Create(ctx, np); err != nil {
		log.Error(err, "Failed to create NodePool",
			"nodePool", npName,
			"namespace", npNamespace)
		return ctrl.Result{}, fmt.Errorf("failed to create NodePool: %w", err)
	}

	log.Info("NodePool created successfully",
		"nodePool", npName,
		"namespace", npNamespace)

	return ctrl.Result{}, nil
}

// buildNodePool constructs the NodePool spec
func (nm *NodePoolManager) buildNodePool(cr *provisioningv1alpha1.DPFHCPProvisioner) *hyperv1.NodePool {
	np := &hyperv1.NodePool{
		ObjectMeta: metav1.ObjectMeta{
			Name:      cr.Name,
			Namespace: cr.Namespace,
		},
		Spec: hyperv1.NodePoolSpec{
			// ClusterName links this NodePool to the HostedCluster
			ClusterName: cr.Name,

			// Replicas=0 - DPU workers will be added manually
			Replicas: ptr.To(int32(0)),

			// Management settings
			Management: hyperv1.NodePoolManagement{
				UpgradeType: hyperv1.UpgradeTypeReplace,
			},

			// Platform: None (DPU environment)
			Platform: hyperv1.NodePoolPlatform{
				Type: hyperv1.NonePlatform,
			},

			// Release image matches HostedCluster
			Release: hyperv1.Release{
				Image: cr.Spec.OCPReleaseImage,
			},
		},
	}

	return np
}
