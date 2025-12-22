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
	"fmt"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

var _ = Describe("DPFHCPBridge Status Field Tests", func() {
	ctx := context.Background()

	AfterEach(func() {
		// Clean up any resources that might have been created
		bridgeList := &provisioningv1alpha1.DPFHCPBridgeList{}
		_ = k8sClient.List(ctx, bridgeList)
		for _, bridge := range bridgeList.Items {
			_ = k8sClient.Delete(ctx, &bridge)
		}
	})

	Context("Phase Field", func() {
		It("should accept all valid phase enum values", func() {
			validPhases := []provisioningv1alpha1.DPFHCPBridgePhase{
				provisioningv1alpha1.PhasePending,
				provisioningv1alpha1.PhaseProvisioning,
				provisioningv1alpha1.PhaseReady,
				provisioningv1alpha1.PhaseFailed,
				provisioningv1alpha1.PhaseDeleting,
			}

			for i, phase := range validPhases {
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      fmt.Sprintf("test-phase-%d", i),
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "default",
						},
						BaseDomain:                     "test.example.com",
						OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
						SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
						PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
						EtcdStorageClass:               "test-storage-class",
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					},
					Status: provisioningv1alpha1.DPFHCPBridgeStatus{
						Phase: phase,
					},
				}

				err := k8sClient.Create(ctx, bridge)
				Expect(err).NotTo(HaveOccurred(), "Phase %q should be valid", phase)

				// Verify we can update the status with this phase
				created := &provisioningv1alpha1.DPFHCPBridge{}
				err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, created)
				Expect(err).NotTo(HaveOccurred())

				created.Status.Phase = phase
				err = k8sClient.Status().Update(ctx, created)
				Expect(err).NotTo(HaveOccurred())

				// Clean up
				_ = k8sClient.Delete(ctx, bridge)
			}
		})

		It("should track phase transitions in status", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-phase-transitions",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			// Simulate phase transitions
			phases := []provisioningv1alpha1.DPFHCPBridgePhase{
				provisioningv1alpha1.PhasePending,
				provisioningv1alpha1.PhaseProvisioning,
				provisioningv1alpha1.PhaseReady,
			}

			for _, phase := range phases {
				updated := &provisioningv1alpha1.DPFHCPBridge{}
				err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, updated)
				Expect(err).NotTo(HaveOccurred())

				updated.Status.Phase = phase
				err = k8sClient.Status().Update(ctx, updated)
				Expect(err).NotTo(HaveOccurred())

				// Verify update
				retrieved := &provisioningv1alpha1.DPFHCPBridge{}
				err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, retrieved)
				Expect(err).NotTo(HaveOccurred())
				Expect(retrieved.Status.Phase).To(Equal(phase))
			}

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})
	})

	Context("Conditions Field", func() {
		It("should allow setting and updating conditions", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-conditions",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			// Set initial Ready condition
			updated := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, updated)
			Expect(err).NotTo(HaveOccurred())

			meta.SetStatusCondition(&updated.Status.Conditions, metav1.Condition{
				Type:    "Ready",
				Status:  metav1.ConditionFalse,
				Reason:  "Pending",
				Message: "Initial validation in progress",
			})

			err = k8sClient.Status().Update(ctx, updated)
			Expect(err).NotTo(HaveOccurred())

			// Verify condition was set
			retrieved := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, retrieved)
			Expect(err).NotTo(HaveOccurred())
			Expect(retrieved.Status.Conditions).To(HaveLen(1))

			readyCond := meta.FindStatusCondition(retrieved.Status.Conditions, "Ready")
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionFalse))
			Expect(readyCond.Reason).To(Equal("Pending"))

			// Update condition to Ready
			meta.SetStatusCondition(&retrieved.Status.Conditions, metav1.Condition{
				Type:    "Ready",
				Status:  metav1.ConditionTrue,
				Reason:  "BridgeReady",
				Message: "All components operational",
			})

			err = k8sClient.Status().Update(ctx, retrieved)
			Expect(err).NotTo(HaveOccurred())

			// Verify condition was updated
			final := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, final)
			Expect(err).NotTo(HaveOccurred())

			readyCond = meta.FindStatusCondition(final.Status.Conditions, "Ready")
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(readyCond.Reason).To(Equal("BridgeReady"))

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})

		It("should support multiple condition types", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-multiple-conditions",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			updated := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, updated)
			Expect(err).NotTo(HaveOccurred())

			// Set multiple conditions
			conditionTypes := []string{"Ready", "Progressing", "DPUClusterAvailable", "BlueFieldImageResolved"}
			for _, condType := range conditionTypes {
				meta.SetStatusCondition(&updated.Status.Conditions, metav1.Condition{
					Type:    condType,
					Status:  metav1.ConditionTrue,
					Reason:  "TestReason",
					Message: "Test message for " + condType,
				})
			}

			err = k8sClient.Status().Update(ctx, updated)
			Expect(err).NotTo(HaveOccurred())

			// Verify all conditions were set
			retrieved := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, retrieved)
			Expect(err).NotTo(HaveOccurred())
			Expect(retrieved.Status.Conditions).To(HaveLen(len(conditionTypes)))

			for _, condType := range conditionTypes {
				cond := meta.FindStatusCondition(retrieved.Status.Conditions, condType)
				Expect(cond).NotTo(BeNil(), "Condition %q should exist", condType)
				Expect(cond.Status).To(Equal(metav1.ConditionTrue))
			}

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})
	})

	Context("Status Pointer Fields", func() {
		It("should allow setting HostedClusterRef", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-hostedcluster-ref",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			updated := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, updated)
			Expect(err).NotTo(HaveOccurred())

			// Set HostedClusterRef
			updated.Status.HostedClusterRef = &corev1.ObjectReference{
				APIVersion: "hypershift.openshift.io/v1beta1",
				Kind:       "HostedCluster",
				Name:       bridge.Name,
				Namespace:  "default",
			}
			updated.Status.KubeConfigSecretRef = &corev1.LocalObjectReference{
				Name: bridge.Name + "-kubeconfig",
			}

			err = k8sClient.Status().Update(ctx, updated)
			Expect(err).NotTo(HaveOccurred())

			// Verify references were set
			retrieved := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: bridge.Name, Namespace: "default"}, retrieved)
			Expect(err).NotTo(HaveOccurred())

			Expect(retrieved.Status.HostedClusterRef).NotTo(BeNil())
			Expect(retrieved.Status.HostedClusterRef.Name).To(Equal(bridge.Name))
			Expect(retrieved.Status.HostedClusterRef.Kind).To(Equal("HostedCluster"))

			Expect(retrieved.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(retrieved.Status.KubeConfigSecretRef.Name).To(Equal(bridge.Name + "-kubeconfig"))

			// Clean up
			_ = k8sClient.Delete(ctx, bridge)
		})
	})
})
