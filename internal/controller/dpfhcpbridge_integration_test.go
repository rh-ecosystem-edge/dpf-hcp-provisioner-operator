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
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

var _ = Describe("DPFHCPBridge Integration Tests with envtest", func() {
	ctx := context.Background()

	AfterEach(func() {
		// Clean up all resources
		bridgeList := &provisioningv1alpha1.DPFHCPBridgeList{}
		_ = k8sClient.List(ctx, bridgeList)
		for _, bridge := range bridgeList.Items {
			_ = k8sClient.Delete(ctx, &bridge)
		}
	})

	Context("Complete CR Lifecycle", func() {
		It("should successfully create, read, update, and delete a DPFHCPBridge", func() {
			bridgeName := "lifecycle-test"

			// CREATE
			By("Creating a new DPFHCPBridge")
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      bridgeName,
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:       "lifecycle.example.com",
					OCPReleaseImage:  "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:  corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:    corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass: "test-storage-class",
					VirtualIP:        "192.168.1.100", // Required for HighlyAvailable default
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			// READ
			By("Reading the created DPFHCPBridge")
			created := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())
			Expect(created.Spec.BaseDomain).To(Equal("lifecycle.example.com"))
			Expect(created.Spec.DPUClusterRef.Name).To(Equal("test-dpu"))
			Expect(created.Spec.DPUClusterRef.Namespace).To(Equal("dpu-system"))

			// Verify defaults were applied
			Expect(created.Spec.ControlPlaneAvailabilityPolicy).To(Equal(hyperv1.HighlyAvailable))

			// UPDATE
			By("Updating the DPFHCPBridge spec")
			// Get a fresh copy for update
			toUpdate := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, toUpdate)
			Expect(err).NotTo(HaveOccurred())

			initialGeneration := toUpdate.Generation
			// Update only mutable fields
			toUpdate.Spec.OCPReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi"
			toUpdate.Spec.VirtualIP = "192.168.1.100"

			err = k8sClient.Update(ctx, toUpdate)
			Expect(err).NotTo(HaveOccurred())

			// Verify update
			updated := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, updated)
			Expect(err).NotTo(HaveOccurred())
			Expect(updated.Spec.OCPReleaseImage).To(Equal("quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi"))
			Expect(updated.Spec.VirtualIP).To(Equal("192.168.1.100"))
			Expect(updated.Generation).To(BeNumerically(">", initialGeneration))

			// DELETE
			By("Deleting the DPFHCPBridge")
			err = k8sClient.Delete(ctx, updated)
			Expect(err).NotTo(HaveOccurred())

			// Verify deletion
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, &provisioningv1alpha1.DPFHCPBridge{})
				return errors.IsNotFound(err)
			}, time.Second*5, time.Millisecond*500).Should(BeTrue())
		})

		It("should handle status updates independently of spec", func() {
			bridgeName := "status-update-test"

			// Create resource
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      bridgeName,
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "status.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			// Get the created resource
			created := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			initialGeneration := created.Generation

			// Update status
			By("Updating status fields")
			created.Status.Phase = provisioningv1alpha1.PhaseProvisioning

			err = k8sClient.Status().Update(ctx, created)
			Expect(err).NotTo(HaveOccurred())

			// Verify status was updated but generation didn't change
			updated := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, updated)
			Expect(err).NotTo(HaveOccurred())

			Expect(updated.Status.Phase).To(Equal(provisioningv1alpha1.PhaseProvisioning))
			Expect(updated.Generation).To(Equal(initialGeneration), "Status update should not increment generation")

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})

		It("should support full status population", func() {
			bridgeName := "full-status-test"

			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      bridgeName,
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "full-status.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			created := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			// Populate all status fields
			By("Setting all status fields")
			created.Status = provisioningv1alpha1.DPFHCPBridgeStatus{
				Phase: provisioningv1alpha1.PhaseReady,
				Conditions: []metav1.Condition{
					{
						Type:               "Ready",
						Status:             metav1.ConditionTrue,
						Reason:             "BridgeReady",
						Message:            "All components operational",
						LastTransitionTime: metav1.Now(),
					},
				},
				HostedClusterRef: &corev1.ObjectReference{
					APIVersion: "hypershift.openshift.io/v1beta1",
					Kind:       "HostedCluster",
					Name:       bridgeName,
					Namespace:  "default",
				},
				KubeConfigSecretRef: &corev1.LocalObjectReference{
					Name: bridgeName + "-kubeconfig",
				},
			}

			err = k8sClient.Status().Update(ctx, created)
			Expect(err).NotTo(HaveOccurred())

			// Verify all status fields
			retrieved := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridgeName, Namespace: "default"}, retrieved)
			Expect(err).NotTo(HaveOccurred())

			Expect(retrieved.Status.Phase).To(Equal(provisioningv1alpha1.PhaseReady))
			Expect(retrieved.Status.Conditions).To(HaveLen(1))
			Expect(retrieved.Status.HostedClusterRef).NotTo(BeNil())
			Expect(retrieved.Status.KubeConfigSecretRef).NotTo(BeNil())

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})
	})

	Context("CRD Validation with envtest", func() {
		It("should reject CR with invalid baseDomain at API server level", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "invalid-domain",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "INVALID.DOMAIN.COM", // Uppercase not allowed
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).To(HaveOccurred(), "Invalid baseDomain should be rejected by CRD validation")
		})

		It("should reject CR missing required fields at API server level", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "missing-required",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					// Missing BaseDomain (required)
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).To(HaveOccurred(), "Missing required field should be rejected by CRD validation")
		})

		It("should apply defaults when optional fields are omitted", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-defaults-applied",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:       "defaults.example.com",
					OCPReleaseImage:  "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:  corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:    corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass: "test-storage-class",
					VirtualIP:        "192.168.1.100", // Required for HighlyAvailable default
					// Optional fields omitted - defaults should be applied
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			created := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: "test-defaults-applied", Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			// Verify defaults
			Expect(created.Spec.ControlPlaneAvailabilityPolicy).To(Equal(hyperv1.HighlyAvailable))

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})
	})

	Context("List Operations", func() {
		It("should list multiple DPFHCPBridge resources", func() {
			// Create multiple bridges
			bridgeNames := []string{"bridge-1", "bridge-2", "bridge-3"}

			for _, name := range bridgeNames {
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      name,
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu-" + name,
							Namespace: "dpu-system",
						},
						BaseDomain:                     name + ".example.com",
						OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
						SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
						PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
						EtcdStorageClass:               "test-storage-class",
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					},
				}

				err := k8sClient.Create(ctx, bridge)
				Expect(err).NotTo(HaveOccurred())
			}

			// List all bridges
			bridgeList := &provisioningv1alpha1.DPFHCPBridgeList{}
			err := k8sClient.List(ctx, bridgeList)
			Expect(err).NotTo(HaveOccurred())
			Expect(bridgeList.Items).To(HaveLen(len(bridgeNames)))

			// Verify each bridge
			foundNames := make(map[string]bool)
			for _, bridge := range bridgeList.Items {
				foundNames[bridge.Name] = true
			}

			for _, name := range bridgeNames {
				Expect(foundNames[name]).To(BeTrue(), "Bridge %q should be in the list", name)
			}

			// Clean up
			for _, bridge := range bridgeList.Items {
				_ = k8sClient.Delete(ctx, &bridge)
			}
		})
	})

	Context("Resource Metadata", func() {
		It("should preserve labels and annotations", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-metadata",
					Namespace: "default",
					Labels: map[string]string{
						"environment": "test",
						"team":        "platform",
					},
					Annotations: map[string]string{
						"description": "Test bridge for metadata preservation",
					},
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					BaseDomain:                     "metadata.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			created := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: "test-metadata", Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			// Verify labels and annotations
			Expect(created.Labels).To(HaveKeyWithValue("environment", "test"))
			Expect(created.Labels).To(HaveKeyWithValue("team", "platform"))
			Expect(created.Annotations).To(HaveKeyWithValue("description", "Test bridge for metadata preservation"))

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})
	})
})
