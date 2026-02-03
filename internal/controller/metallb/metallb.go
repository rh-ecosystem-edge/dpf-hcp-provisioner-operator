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
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/tools/record"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

// MetalLBManager handles MetalLB resource management for DPFHCPProvisioner
type MetalLBManager struct {
	client   client.Client
	recorder record.EventRecorder
}

// NewMetalLBManager creates a new MetalLB manager
func NewMetalLBManager(client client.Client, recorder record.EventRecorder) *MetalLBManager {
	return &MetalLBManager{
		client:   client,
		recorder: recorder,
	}
}

// ConfigureMetalLB orchestrates MetalLB resource configuration for a DPFHCPProvisioner
// It creates and maintains IPAddressPool and L2Advertisement resources when LoadBalancer exposure is needed.
func (m *MetalLBManager) ConfigureMetalLB(ctx context.Context, provisioner *provisioningv1alpha1.DPFHCPProvisioner) (ctrl.Result, error) {
	log := logf.FromContext(ctx).WithValues("provisioner", client.ObjectKeyFromObject(provisioner))

	// Check if MetalLB configuration is needed
	if !provisioner.ShouldExposeThroughLoadBalancer() {
		log.V(1).Info("ShouldExposeThroughLoadBalancer is false, skipping MetalLB configuration")
		return ctrl.Result{}, nil
	}

	log.Info("Starting MetalLB configuration")

	// Configure IPAddressPool
	log.V(1).Info("Configuring IPAddressPool")
	if err := m.ensureIPAddressPool(ctx, provisioner); err != nil {
		log.Error(err, "Failed to configure IPAddressPool")

		if condErr := m.setCondition(ctx, provisioner, metav1.ConditionFalse, "CreatingIPAddressPool",
			fmt.Sprintf("Failed to create/update IPAddressPool: %v", err)); condErr != nil {
			log.Error(condErr, "Failed to update MetalLBConfigured condition")
		}

		return ctrl.Result{}, err
	}
	log.Info("IPAddressPool configured successfully", "name", provisioner.Name, "namespace", common.OpenshiftOperatorsNamespace)

	// Configure L2Advertisement
	log.V(1).Info("Configuring L2Advertisement")
	if err := m.ensureL2Advertisement(ctx, provisioner); err != nil {
		log.Error(err, "Failed to configure L2Advertisement")

		if condErr := m.setCondition(ctx, provisioner, metav1.ConditionFalse, "L2AdvertisementFailed",
			fmt.Sprintf("Failed to create/update L2Advertisement: %v", err)); condErr != nil {
			log.Error(condErr, "Failed to update MetalLBConfigured condition")
		}

		return ctrl.Result{}, err
	}
	log.Info("L2Advertisement configured successfully", "name", fmt.Sprintf("advertise-%s", provisioner.Name), "namespace", common.OpenshiftOperatorsNamespace)

	// Update condition to True - both resources successfully configured
	if err := m.setCondition(ctx, provisioner, metav1.ConditionTrue, "MetalLBReady",
		"MetalLB configured successfully"); err != nil {
		log.Error(err, "Failed to update MetalLBConfigured condition")
		return ctrl.Result{}, err
	}

	log.Info("MetalLB configuration complete")
	return ctrl.Result{}, nil
}

// setCondition updates the MetalLBConfigured condition on the DPFHCPProvisioner CR
// and emits events when the condition changes to avoid event spam
func (m *MetalLBManager) setCondition(ctx context.Context, provisioner *provisioningv1alpha1.DPFHCPProvisioner, status metav1.ConditionStatus, reason, message string) error {
	log := logf.FromContext(ctx)

	condition := metav1.Condition{
		Type:               provisioningv1alpha1.MetalLBConfigured,
		Status:             status,
		Reason:             reason,
		Message:            message,
		LastTransitionTime: metav1.Now(),
		ObservedGeneration: provisioner.Generation,
	}

	// Only update if condition actually changed
	if changed := meta.SetStatusCondition(&provisioner.Status.Conditions, condition); changed {
		log.V(1).Info("Updating MetalLBConfigured condition",
			"status", status,
			"reason", reason,
			"message", message)

		// Emit event only when condition status/reason changed (avoid spam)
		eventType := "Normal"
		eventReason := "MetalLBConfigured"
		if status == metav1.ConditionFalse {
			eventType = "Warning"
			eventReason = "MetalLBConfigurationFailed"
		}
		m.recorder.Event(provisioner, eventType, eventReason, message)

		if err := m.client.Status().Update(ctx, provisioner); err != nil {
			return fmt.Errorf("updating MetalLBConfigured condition: %w", err)
		}
	}

	return nil
}

// ensureIPAddressPool creates or updates the IPAddressPool resource
func (m *MetalLBManager) ensureIPAddressPool(ctx context.Context, provisioner *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx)

	pool := &metallbv1beta1.IPAddressPool{
		ObjectMeta: metav1.ObjectMeta{
			Name:      provisioner.Name,
			Namespace: common.OpenshiftOperatorsNamespace,
		},
	}

	op, err := controllerutil.CreateOrUpdate(ctx, m.client, pool, func() error {
		// Verify ownership if resource already exists (ResourceVersion is set by API server on existing resources)
		if pool.ResourceVersion != "" && !m.isOwnedByProvisioner(pool, provisioner) {
			return fmt.Errorf("IPAddressPool %s/%s exists but is not owned by DPFHCPProvisioner %s/%s (missing ownership labels)",
				pool.Namespace, pool.Name, provisioner.Namespace, provisioner.Name)
		}

		// Set ownership labels
		if pool.Labels == nil {
			pool.Labels = make(map[string]string)
		}
		pool.Labels[common.LabelDPFHCPProvisionerName] = provisioner.Name
		pool.Labels[common.LabelDPFHCPProvisionerNamespace] = provisioner.Namespace

		// Set desired spec
		pool.Spec = metallbv1beta1.IPAddressPoolSpec{
			Addresses: []string{
				fmt.Sprintf("%s/32", provisioner.Spec.VirtualIP),
			},
			AllocateTo: &metallbv1beta1.ServiceAllocation{
				Namespaces: []string{
					fmt.Sprintf("%s-%s", provisioner.Namespace, provisioner.Name),
				},
			},
			AutoAssign: ptr.To(true),
		}

		return nil
	})

	if err != nil {
		return fmt.Errorf("creating or updating IPAddressPool: %w", err)
	}

	// Log the operation result
	switch op {
	case controllerutil.OperationResultCreated:
		log.Info("Created IPAddressPool", "name", pool.Name, "namespace", pool.Namespace)
	case controllerutil.OperationResultUpdated:
		log.Info("Updated IPAddressPool (drift corrected)", "name", pool.Name)
		m.recorder.Event(provisioner, "Normal", "MetalLBDriftCorrected",
			fmt.Sprintf("Corrected spec drift in IPAddressPool %s", pool.Name))
	case controllerutil.OperationResultNone:
		log.V(1).Info("IPAddressPool already matches desired state", "name", pool.Name)
	}

	return nil
}

// ensureL2Advertisement creates or updates the L2Advertisement resource
func (m *MetalLBManager) ensureL2Advertisement(ctx context.Context, provisioner *provisioningv1alpha1.DPFHCPProvisioner) error {
	log := logf.FromContext(ctx)

	advert := &metallbv1beta1.L2Advertisement{
		ObjectMeta: metav1.ObjectMeta{
			Name:      fmt.Sprintf("advertise-%s", provisioner.Name),
			Namespace: common.OpenshiftOperatorsNamespace,
		},
	}

	op, err := controllerutil.CreateOrUpdate(ctx, m.client, advert, func() error {
		// Verify ownership if resource already exists (ResourceVersion is set by API server on existing resources)
		if advert.ResourceVersion != "" && !m.isOwnedByProvisioner(advert, provisioner) {
			return fmt.Errorf("L2Advertisement %s/%s exists but is not owned by DPFHCPProvisioner %s/%s (missing ownership labels)",
				advert.Namespace, advert.Name, provisioner.Namespace, provisioner.Name)
		}

		// Set ownership labels
		if advert.Labels == nil {
			advert.Labels = make(map[string]string)
		}
		advert.Labels[common.LabelDPFHCPProvisionerName] = provisioner.Name
		advert.Labels[common.LabelDPFHCPProvisionerNamespace] = provisioner.Namespace

		// Set desired spec
		advert.Spec = metallbv1beta1.L2AdvertisementSpec{
			IPAddressPools: []string{provisioner.Name},
		}

		return nil
	})

	if err != nil {
		return fmt.Errorf("creating or updating L2Advertisement: %w", err)
	}

	// Log the operation result
	switch op {
	case controllerutil.OperationResultCreated:
		log.Info("Created L2Advertisement", "name", advert.Name, "namespace", advert.Namespace)
	case controllerutil.OperationResultUpdated:
		log.Info("Updated L2Advertisement (drift corrected)", "name", advert.Name)
		m.recorder.Event(provisioner, "Normal", "MetalLBDriftCorrected",
			fmt.Sprintf("Corrected spec drift in L2Advertisement %s", advert.Name))
	case controllerutil.OperationResultNone:
		log.V(1).Info("L2Advertisement already matches desired state", "name", advert.Name)
	}

	return nil
}

// isOwnedByProvisioner checks if a resource has the correct ownership labels for the given DPFHCPProvisioner.
// This prevents taking ownership of resources created by other operators or users.
func (m *MetalLBManager) isOwnedByProvisioner(obj client.Object, provisioner *provisioningv1alpha1.DPFHCPProvisioner) bool {
	labels := obj.GetLabels()
	if labels == nil {
		return false
	}

	provisionerName, hasProvisionerName := labels[common.LabelDPFHCPProvisionerName]
	provisionerNamespace, hasProvisionerNamespace := labels[common.LabelDPFHCPProvisionerNamespace]

	return hasProvisionerName && hasProvisionerNamespace &&
		provisionerName == provisioner.Name &&
		provisionerNamespace == provisioner.Namespace
}
