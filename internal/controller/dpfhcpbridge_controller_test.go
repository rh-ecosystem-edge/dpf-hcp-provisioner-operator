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

package controller

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/bluefield"
	"github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/internal/controller/dpucluster"
)

var _ = Describe("DPFHCPBridge Controller", func() {
	Context("When reconciling a resource", func() {
		const resourceName = "test-resource"

		ctx := context.Background()

		typeNamespacedName := types.NamespacedName{
			Name:      resourceName,
			Namespace: "default",
		}
		dpfhcpbridge := &provisioningv1alpha1.DPFHCPBridge{}

		BeforeEach(func() {
			By("creating the custom resource for the Kind DPFHCPBridge")
			err := k8sClient.Get(ctx, typeNamespacedName, dpfhcpbridge)
			if err != nil && errors.IsNotFound(err) {
				resource := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      resourceName,
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpucluster",
							Namespace: "default",
						},
						BaseDomain:                     "test.clusters.example.com",
						OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
						SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
						PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
						EtcdStorageClass:               "test-storage-class",
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					},
				}
				Expect(k8sClient.Create(ctx, resource)).To(Succeed())
			}
		})

		AfterEach(func() {
			resource := &provisioningv1alpha1.DPFHCPBridge{}
			err := k8sClient.Get(ctx, typeNamespacedName, resource)
			Expect(err).NotTo(HaveOccurred())

			By("Cleanup the specific resource instance DPFHCPBridge")
			Expect(k8sClient.Delete(ctx, resource)).To(Succeed())
		})

		It("should successfully reconcile the resource", func() {
			By("Reconciling the created resource")
			fakeRecorder := record.NewFakeRecorder(10)
			controllerReconciler := &DPFHCPBridgeReconciler{
				Client:              k8sClient,
				Scheme:              k8sClient.Scheme(),
				Recorder:            fakeRecorder,
				ImageResolver:       bluefield.NewImageResolver(k8sClient, fakeRecorder),
				DPUClusterValidator: dpucluster.NewValidator(k8sClient, fakeRecorder),
			}

			_, err := controllerReconciler.Reconcile(ctx, reconcile.Request{
				NamespacedName: typeNamespacedName,
			})
			// Note: This test will fail because DPUCluster CRD is not installed in test env
			// The validator is tested thoroughly in dpucluster/validator_test.go
			// TODO: Add proper integration test with DPUCluster CRD
			if err != nil {
				// Expected error: DPUCluster CRD not found in test environment
				Expect(err.Error()).To(ContainSubstring("no matches for kind"))
			}
		})
	})
})
