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

package e2e

import (
	"context"

	operatorv1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	. "github.com/onsi/gomega"
	configv1 "github.com/openshift/api/config/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	"sigs.k8s.io/controller-runtime/pkg/client"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

const (
	stubOCPVersion   = "4.22.6"
	stubReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.22.6-multi"
	stubOVNImage     = "quay.io/openshift-release-dev/ocp-v4.0-art-dev:ovn-kubernetes-4.22.6"
	stubDPFVersion   = "v26.4.0"
)

// enableManageDPUServiceTemplates patches the DPFHCPProvisionerConfig to
// enable template management.
func enableManageDPUServiceTemplates() {
	ctx := context.Background()
	config := &provisioningv1alpha1.DPFHCPProvisionerConfig{}
	err := k8sClient.Get(ctx, types.NamespacedName{Name: "default"}, config)
	if apierrors.IsNotFound(err) {
		config = &provisioningv1alpha1.DPFHCPProvisionerConfig{
			ObjectMeta: metav1.ObjectMeta{
				Name: "default",
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerConfigSpec{
				ManageDPUServiceTemplates: true,
			},
		}
		err = k8sClient.Create(ctx, config)
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPFHCPProvisionerConfig")
		return
	}
	ExpectWithOffset(1, err).NotTo(HaveOccurred())
	config.Spec.ManageDPUServiceTemplates = true
	err = k8sClient.Update(ctx, config)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to enable ManageDPUServiceTemplates")
}

// setDPFOperatorConfigVersion creates the DPFOperatorConfig singleton and sets status.version.
func setDPFOperatorConfigVersion(version string) {
	ctx := context.Background()

	config := &operatorv1.DPFOperatorConfig{}
	err := k8sClient.Get(ctx, types.NamespacedName{
		Name: "dpfoperatorconfig", Namespace: dpfOperatorConfigNamespace,
	}, config)

	if apierrors.IsNotFound(err) {
		mtu := 1500
		bfbPVCName := "e2e-bfb-pvc"
		config = &operatorv1.DPFOperatorConfig{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "dpfoperatorconfig",
				Namespace: dpfOperatorConfigNamespace,
			},
			Spec: operatorv1.DPFOperatorConfigSpec{
				ProvisioningController: &operatorv1.ProvisioningControllerConfiguration{
					BFBPersistentVolumeClaimName: &bfbPVCName,
				},
				Networking: &operatorv1.Networking{
					ControlPlaneMTU: &mtu,
				},
			},
		}
		err = k8sClient.Create(ctx, config)
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPFOperatorConfig")
	} else {
		ExpectWithOffset(1, err).NotTo(HaveOccurred())
	}

	// Patch status.version
	config.Status.Version = ptr.To(version)
	err = k8sClient.Status().Update(ctx, config)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to set DPFOperatorConfig version")
}

// cleanupTemplateTestResources removes DPUServiceTemplate test resources.
func cleanupTemplateTestResources() {
	ctx := context.Background()

	// Delete provisioner (force-remove finalizer if stuck)
	forceDeleteProvisioner(ciNamespace, provisionerName)

	// Delete DPFOperatorConfig
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &operatorv1.DPFOperatorConfig{},
		client.InNamespace(dpfOperatorConfigNamespace))); err != nil {
		warnError("failed to cleanup DPFOperatorConfig: %v", err)
	}

	// Delete ClusterVersion (only if we created the stub on Kind)
	if !testingOnOCP {
		cv := &configv1.ClusterVersion{ObjectMeta: metav1.ObjectMeta{Name: "version"}}
		if err := client.IgnoreNotFound(k8sClient.Delete(ctx, cv)); err != nil {
			warnError("failed to cleanup ClusterVersion: %v", err)
		}
	}

	// Delete the DPUCluster namespace (cascades DPUCluster, DPUFlavor, DPUDeployment, templates)
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: dpuClusterNS}}
	if err := client.IgnoreNotFound(k8sClient.Delete(ctx, ns)); err != nil {
		warnError("failed to cleanup DPUCluster namespace: %v", err)
	}
}
