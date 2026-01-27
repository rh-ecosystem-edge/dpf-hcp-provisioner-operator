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

package kubeconfiginjection

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

var _ = Describe("Kubeconfig Injection Reconciler", func() {
	var (
		ctx        context.Context
		fakeClient client.Client
		recorder   *record.FakeRecorder
		injector   *KubeconfigInjector
		scheme     *runtime.Scheme
	)

	BeforeEach(func() {
		ctx = context.TODO()

		// Setup scheme
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(corev1.AddToScheme(scheme)).To(Succeed())
		Expect(dpuprovisioningv1alpha1.AddToScheme(scheme)).To(Succeed())

		recorder = record.NewFakeRecorder(100)
	})

	Describe("Happy Path - Full Injection Flow", func() {
		It("should successfully inject kubeconfig when all prerequisites are met", func() {
			// Given: DPFHCPBridge with HostedCluster created
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPBridgeStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-bridge",
						Namespace: "test-ns",
					},
				},
			}

			// HC kubeconfig secret exists
			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("fake-kubeconfig-data"),
				},
			}

			// DPUCluster exists
			dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-dpu",
					Namespace: "dpu-ns",
				},
				Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
					Type: "bf3",
				},
			}

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(bridge, hcSecret, dpuCluster).
				WithStatusSubresource(bridge, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state - nothing should be set up yet
			beforeCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(beforeCond).To(BeNil(), "Condition should not be set initially")

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, bridge)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify events were emitted for secret creation and DPUCluster update
			Eventually(recorder.Events).Should(Receive(ContainSubstring("KubeConfigInjected")))
			Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterUpdated")))

			// Secret created in DPUCluster namespace
			destSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-bridge-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, destSecret)
			Expect(err).NotTo(HaveOccurred())
			Expect(destSecret.Data["kubeconfig"]).To(Equal([]byte("fake-kubeconfig-data")))
			Expect(destSecret.Labels[LabelOwnedBy]).To(Equal("test-bridge"))
			Expect(destSecret.Labels[LabelNamespace]).To(Equal("test-ns"))

			// DPUCluster updated
			updatedDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, updatedDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(updatedDPU.Spec.Kubeconfig).To(Equal("test-bridge-admin-kubeconfig"))

			// Status updated
			Expect(bridge.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(bridge.Status.KubeConfigSecretRef.Name).To(Equal("test-bridge-admin-kubeconfig"))

			// Condition set to True
			afterCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))
			Expect(afterCond.Message).To(ContainSubstring("successfully"))
		})
	})

	Describe("Secret Not Ready Scenario", func() {
		It("should set condition to pending when HC kubeconfig secret doesn't exist", func() {
			// Given: DPFHCPBridge but no HC kubeconfig secret yet
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPBridgeStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-bridge",
						Namespace: "test-ns",
					},
				},
			}

			dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-dpu",
					Namespace: "dpu-ns",
				},
			}

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(bridge, dpuCluster).
				WithStatusSubresource(bridge).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, bridge)

			// Then: Requeue without error
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Condition set to pending
			cond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigPending))
		})
	})

	Describe("Idempotency - Scenario A: Drift Detection", func() {
		It("should detect and correct content drift between source and destination secrets", func() {
			// Given: Both secrets exist but with different content (drift scenario)
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPBridgeStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-bridge",
						Namespace: "test-ns",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-bridge-admin-kubeconfig",
					},
				},
			}

			// Set initial condition to True (was previously injected)
			meta.SetStatusCondition(&bridge.Status.Conditions, metav1.Condition{
				Type:    provisioningv1alpha1.KubeConfigInjected,
				Status:  metav1.ConditionTrue,
				Reason:  provisioningv1alpha1.ReasonKubeConfigInjected,
				Message: "Previously injected",
			})

			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("new-rotated-kubeconfig-data"),
				},
			}

			destSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge-admin-kubeconfig",
					Namespace: "dpu-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("old-kubeconfig-data"), // DRIFT!
				},
			}

			dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-dpu",
					Namespace: "dpu-ns",
				},
				Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
					Kubeconfig: "test-bridge-admin-kubeconfig",
				},
			}

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(bridge, hcSecret, destSecret, dpuCluster).
				WithStatusSubresource(bridge, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state
			beforeCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(beforeCond).NotTo(BeNil())
			Expect(beforeCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(beforeCond.Message).To(Equal("Previously injected"))

			// Verify drift exists before reconciliation
			beforeSecret := &corev1.Secret{}
			err := fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-bridge-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, beforeSecret)
			Expect(err).NotTo(HaveOccurred())
			Expect(beforeSecret.Data["kubeconfig"]).To(Equal([]byte("old-kubeconfig-data")))

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, bridge)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify drift was DETECTED - check for DriftCorrected event
			Eventually(recorder.Events).Should(Receive(ContainSubstring("DriftCorrected")))

			// Destination secret updated to match source
			updatedSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-bridge-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, updatedSecret)
			Expect(err).NotTo(HaveOccurred())
			Expect(updatedSecret.Data["kubeconfig"]).To(Equal([]byte("new-rotated-kubeconfig-data")))

			// Condition remains True (stayed healthy through drift correction)
			afterCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))
			Expect(afterCond.Message).To(ContainSubstring("successfully"))

			// Verify status was updated
			Expect(bridge.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(bridge.Status.KubeConfigSecretRef.Name).To(Equal("test-bridge-admin-kubeconfig"))
		})
	})

	Describe("Idempotency - Scenario B: Secret Exists, DPUCluster Not Updated", func() {
		It("should update DPUCluster without recreating secret", func() {
			// Given: Secret exists but DPUCluster not updated (partial completion scenario)
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPBridgeStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-bridge",
						Namespace: "test-ns",
					},
				},
			}

			// No initial condition set (injection was interrupted)

			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("kubeconfig-data"),
				},
			}

			destSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge-admin-kubeconfig",
					Namespace: "dpu-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("kubeconfig-data"),
				},
			}

			dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-dpu",
					Namespace: "dpu-ns",
				},
				Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
					// Kubeconfig field is empty - NOT UPDATED YET
				},
			}

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(bridge, hcSecret, destSecret, dpuCluster).
				WithStatusSubresource(bridge, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state
			beforeCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(beforeCond).To(BeNil(), "Condition should not be set yet")

			beforeDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err := fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, beforeDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(beforeDPU.Spec.Kubeconfig).To(BeEmpty(), "DPUCluster should not be updated yet")

			// Secret should already exist
			existingSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-bridge-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, existingSecret)
			Expect(err).NotTo(HaveOccurred(), "Secret should already exist")
			originalResourceVersion := existingSecret.ResourceVersion

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, bridge)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify NO secret recreation - secret should not have been recreated
			afterSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-bridge-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, afterSecret)
			Expect(err).NotTo(HaveOccurred())
			Expect(afterSecret.ResourceVersion).To(Equal(originalResourceVersion),
				"Secret should not have been recreated/updated")

			// DPUCluster SHOULD be updated
			updatedDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, updatedDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(updatedDPU.Spec.Kubeconfig).To(Equal("test-bridge-admin-kubeconfig"))

			// Condition set to True (injection completed)
			afterCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))

			// Status updated
			Expect(bridge.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(bridge.Status.KubeConfigSecretRef.Name).To(Equal("test-bridge-admin-kubeconfig"))
		})
	})

	Describe("Idempotency - Scenario C: Secret Missing, DPUCluster Updated", func() {
		It("should recreate secret without modifying DPUCluster", func() {
			// Given: DPUCluster updated but secret missing (secret was deleted scenario)
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPBridgeStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-bridge",
						Namespace: "test-ns",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-bridge-admin-kubeconfig",
					},
				},
			}

			// Set initial condition to True (was previously injected but secret got deleted)
			meta.SetStatusCondition(&bridge.Status.Conditions, metav1.Condition{
				Type:    provisioningv1alpha1.KubeConfigInjected,
				Status:  metav1.ConditionTrue,
				Reason:  provisioningv1alpha1.ReasonKubeConfigInjected,
				Message: "Previously injected",
			})

			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("kubeconfig-data"),
				},
			}

			dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-dpu",
					Namespace: "dpu-ns",
				},
				Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
					Kubeconfig: "test-bridge-admin-kubeconfig", // Already updated
				},
			}

			// NOTE: destSecret is NOT created - it's missing!

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(bridge, hcSecret, dpuCluster). // No destSecret
				WithStatusSubresource(bridge, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state
			beforeCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(beforeCond).NotTo(BeNil())
			Expect(beforeCond.Status).To(Equal(metav1.ConditionTrue))

			beforeDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err := fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, beforeDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(beforeDPU.Spec.Kubeconfig).To(Equal("test-bridge-admin-kubeconfig"),
				"DPUCluster should already be updated")
			originalDPUResourceVersion := beforeDPU.ResourceVersion

			// Verify secret is missing
			missingSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-bridge-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, missingSecret)
			Expect(err).To(HaveOccurred())
			Expect(apierrors.IsNotFound(err)).To(BeTrue(), "Secret should be missing")

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, bridge)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Secret SHOULD be recreated
			recreatedSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-bridge-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, recreatedSecret)
			Expect(err).NotTo(HaveOccurred(), "Secret should be recreated")
			Expect(recreatedSecret.Data["kubeconfig"]).To(Equal([]byte("kubeconfig-data")))
			Expect(recreatedSecret.Labels[LabelOwnedBy]).To(Equal("test-bridge"))
			Expect(recreatedSecret.Labels[LabelNamespace]).To(Equal("test-ns"))

			// DPUCluster should NOT be modified (already updated)
			afterDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, afterDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(afterDPU.Spec.Kubeconfig).To(Equal("test-bridge-admin-kubeconfig"))
			Expect(afterDPU.ResourceVersion).To(Equal(originalDPUResourceVersion),
				"DPUCluster should not have been modified")

			// Condition remains True (stayed healthy through secret recreation)
			afterCond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))

			// Status still references the secret
			Expect(bridge.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(bridge.Status.KubeConfigSecretRef.Name).To(Equal("test-bridge-admin-kubeconfig"))
		})
	})

	Describe("Skip When HostedCluster Not Created", func() {
		It("should skip injection when HostedClusterRef is nil", func() {
			// Given: DPFHCPBridge without HostedCluster created yet
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				// HostedClusterRef is nil
			}

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(bridge).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, bridge)

			// Then: Skip without error
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// No condition set
			cond := findCondition(bridge.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(cond).To(BeNil())
		})
	})
})

// Helper function to find a condition
func findCondition(conditions []metav1.Condition, condType string) *metav1.Condition {
	for i := range conditions {
		if conditions[i].Type == condType {
			return &conditions[i]
		}
	}
	return nil
}
