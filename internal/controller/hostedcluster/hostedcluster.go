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
	"github.com/openshift/hypershift/api/util/ipnet"
	"github.com/openshift/hypershift/support/infraid"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
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

// HostedClusterManager manages HostedCluster resources
type HostedClusterManager struct {
	client.Client
	Scheme *runtime.Scheme
}

// NewHostedClusterManager creates a new HostedClusterManager
func NewHostedClusterManager(c client.Client, scheme *runtime.Scheme) *HostedClusterManager {
	return &HostedClusterManager{
		Client: c,
		Scheme: scheme,
	}
}

// CreateOrUpdateHostedCluster creates or updates the HostedCluster resource
// Returns ctrl.Result and error for reconciliation flow
//
// This function:
// - Checks if HostedCluster already exists with matching OwnerReference (idempotency)
// - Creates new HostedCluster if it doesn't exist
// - Handles name conflicts (HC exists with different owner)
// - Uses infraid.New() for consistent infraID generation
func (hm *HostedClusterManager) CreateOrUpdateHostedCluster(ctx context.Context, cr *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx)

	hcName := cr.Name
	hcNamespace := cr.Namespace

	// Check if HostedCluster already exists
	existingHC := &hyperv1.HostedCluster{}
	hcKey := types.NamespacedName{Name: hcName, Namespace: hcNamespace}
	err := hm.Get(ctx, hcKey, existingHC)

	if err == nil {
		// HostedCluster exists - verify ownership via OwnerReference
		if metav1.IsControlledBy(existingHC, cr) {
			log.V(1).Info("HostedCluster already exists and is owned by this DPFHCPProvisioner, adopting",
				"hostedCluster", hcName,
				"namespace", hcNamespace)
			// TODO: If the DPFHCPProvisioner is updated, check whether the hc spec needs to be updated here
			return ctrl.Result{}, nil
		}

		// Name conflict - HC exists but owned by different DPFHCPProvisioner
		return ctrl.Result{}, fmt.Errorf("hostedCluster %s exists in %s but is owned by different DPFHCPProvisioner", hcName, hcNamespace)
	}

	if !apierrors.IsNotFound(err) {
		return ctrl.Result{}, fmt.Errorf("failed to check existing HostedCluster: %w", err)
	}

	// HostedCluster doesn't exist - create it
	exposeThroughLB := cr.ShouldExposeThroughLoadBalancer()
	log.Info("Creating HostedCluster",
		"hostedCluster", hcName,
		"namespace", hcNamespace,
		"releaseImage", cr.Spec.OCPReleaseImage,
		"exposeThroughLoadBalancer", exposeThroughLB)

	// Detect node address if using NodePort mode
	var nodeAddress string
	if !exposeThroughLB {
		log.V(1).Info("Detecting node address for NodePort mode")
		addr, err := detectNodeAddress(ctx, hm.Client)
		if err != nil {
			log.Error(err, "Failed to detect node address")
			return ctrl.Result{}, fmt.Errorf("failed to detect node address: %w", err)
		}
		nodeAddress = addr
		log.Info("Detected node address", "address", nodeAddress)
	}

	hc := hm.buildHostedCluster(cr, nodeAddress)

	// Set owner reference for automatic garbage collection
	if err := controllerutil.SetControllerReference(cr, hc, hm.Scheme); err != nil {
		log.Error(err, "Failed to set owner reference on HostedCluster")
		return ctrl.Result{}, fmt.Errorf("failed to set owner reference on HostedCluster: %w", err)
	}

	if err := hm.Create(ctx, hc); err != nil {
		log.Error(err, "Failed to create HostedCluster",
			"hostedCluster", hcName,
			"namespace", hcNamespace)
		return ctrl.Result{}, fmt.Errorf("failed to create HostedCluster: %w", err)
	}

	log.Info("HostedCluster created successfully",
		"hostedCluster", hcName,
		"namespace", hcNamespace)

	return ctrl.Result{}, nil
}

// buildHostedCluster constructs the HostedCluster spec from DPFHCPProvisioner fields
// nodeAddress is only used when exposeThroughLoadBalancer=false (NodePort mode)
func (hm *HostedClusterManager) buildHostedCluster(cr *provisioningv1alpha1.DPFHCPProvisioner, nodeAddress string) *hyperv1.HostedCluster {
	// Build etcd storage spec
	// Only set StorageClassName if explicitly provided (matches HyperShift CLI behavior)
	// If not set, Kubernetes will use the default StorageClass
	etcdStorage := &hyperv1.PersistentVolumeEtcdStorageSpec{
		Size: ptr.To(resource.MustParse("8Gi")),
	}
	if cr.Spec.EtcdStorageClass != "" {
		etcdStorage.StorageClassName = ptr.To(cr.Spec.EtcdStorageClass)
	}

	hc := &hyperv1.HostedCluster{
		ObjectMeta: metav1.ObjectMeta{
			Name:      cr.Name,
			Namespace: cr.Namespace,
		},
		Spec: hyperv1.HostedClusterSpec{
			// Release image
			Release: hyperv1.Release{
				Image: cr.Spec.OCPReleaseImage,
			},

			// Pull secret reference (copied to clusters namespace)
			PullSecret: corev1.LocalObjectReference{
				Name: fmt.Sprintf("%s-pull-secret", cr.Name),
			},

			// SSH key reference (copied to clusters namespace)
			SSHKey: corev1.LocalObjectReference{
				Name: fmt.Sprintf("%s-ssh-key", cr.Name),
			},

			// DNS configuration
			DNS: hyperv1.DNSSpec{
				BaseDomain: cr.Spec.BaseDomain,
			},

			// ETCD configuration with managed storage
			Etcd: hyperv1.EtcdSpec{
				ManagementType: hyperv1.Managed,
				Managed: &hyperv1.ManagedEtcdSpec{
					Storage: hyperv1.ManagedEtcdStorageSpec{
						Type:             hyperv1.PersistentVolumeEtcdStorage,
						PersistentVolume: etcdStorage,
					},
				},
			},

			// Networking configuration with Other network type
			// Default CIDRs
			Networking: hyperv1.ClusterNetworking{
				NetworkType: hyperv1.Other,
				ServiceNetwork: []hyperv1.ServiceNetworkEntry{
					{CIDR: *ipnet.MustParseCIDR("172.31.0.0/16")},
				},
				ClusterNetwork: []hyperv1.ClusterNetworkEntry{
					{CIDR: *ipnet.MustParseCIDR("10.132.0.0/14")},
				},
				MachineNetwork: []hyperv1.MachineNetworkEntry{},
			},

			// Platform: None (for DPU environments)
			Platform: hyperv1.PlatformSpec{
				Type: hyperv1.NonePlatform,
			},

			// Availability policy from DPFHCPProvisioner spec
			ControllerAvailabilityPolicy: cr.Spec.ControlPlaneAvailabilityPolicy,

			// InfraID: Generate deterministically from cluster name
			InfraID: infraid.New(cr.Name),

			// Secret encryption with AESCBC
			SecretEncryption: &hyperv1.SecretEncryptionSpec{
				Type: hyperv1.AESCBC,
				AESCBC: &hyperv1.AESCBCSpec{
					ActiveKey: corev1.LocalObjectReference{
						Name: fmt.Sprintf("%s-etcd-encryption-key", cr.Name),
					},
				},
			},

			// Service publishing strategy (LoadBalancer or NodePort mode)
			Services: BuildServicePublishingStrategy(cr.ShouldExposeThroughLoadBalancer(), nodeAddress),

			// Capabilities: Disable optional cluster capabilities
			// These capabilities are disabled to reduce resource consumption in DPU environments
			Capabilities: &hyperv1.Capabilities{
				Disabled: []hyperv1.OptionalCapability{
					"ImageRegistry",
					"Insights",
					"Console",
					"openshift-samples",
					"Ingress",
					"NodeTuning",
				},
			},

			// NodeSelector: Schedule control plane pods based on user preference
			// Default: control-plane nodes
			NodeSelector: getNodeSelector(cr),
		},
	}

	return hc
}

// getNodeSelector returns the NodeSelector from DPFHCPProvisioner spec or the default if not specified
func getNodeSelector(cr *provisioningv1alpha1.DPFHCPProvisioner) map[string]string {
	if cr.Spec.NodeSelector != nil && len(cr.Spec.NodeSelector) > 0 {
		return cr.Spec.NodeSelector
	}
	// Default: Schedule control plane pods only on control-plane nodes
	return map[string]string{
		"node-role.kubernetes.io/control-plane": "",
	}
}

// detectNodeAddress auto-detects the management cluster node address for NodePort publishing
// Priority: ExternalDNS > ExternalIP > InternalIP
// This matches the HyperShift CLI pattern (GetAPIServerAddressByNode)
func detectNodeAddress(ctx context.Context, c client.Client) (string, error) {
	nodes := &corev1.NodeList{}
	if err := c.List(ctx, nodes); err != nil {
		return "", fmt.Errorf("failed to list nodes: %w", err)
	}

	if len(nodes.Items) == 0 {
		return "", fmt.Errorf("no nodes found in cluster")
	}

	// Use first node and check addresses in priority order
	node := nodes.Items[0]

	// Priority order: ExternalDNS > ExternalIP > InternalIP
	addressTypes := []corev1.NodeAddressType{
		corev1.NodeExternalDNS,
		corev1.NodeExternalIP,
		corev1.NodeInternalIP,
	}

	for _, addrType := range addressTypes {
		for _, addr := range node.Status.Addresses {
			if addr.Type == addrType {
				return addr.Address, nil
			}
		}
	}

	return "", fmt.Errorf("no valid address found on node %s", node.Name)
}
