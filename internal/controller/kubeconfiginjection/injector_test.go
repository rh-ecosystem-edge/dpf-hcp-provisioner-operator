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
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
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
			// Given: DPFHCPProvisioner with HostedCluster created
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
				},
			}

			// HC kubeconfig secret exists (created by Hypershift with "kubeconfig" key)
			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					SourceKubeconfigSecretKey: []byte("fake-kubeconfig-data"),
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
				WithObjects(provisioner, hcSecret, dpuCluster).
				WithStatusSubresource(provisioner, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state - nothing should be set up yet
			beforeCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(beforeCond).To(BeNil(), "Condition should not be set initially")

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, provisioner)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify events were emitted for secret creation and DPUCluster update
			Eventually(recorder.Events).Should(Receive(ContainSubstring("KubeConfigInjected")))
			Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterUpdated")))

			// Secret created in DPUCluster namespace
			destSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-provisioner-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, destSecret)
			Expect(err).NotTo(HaveOccurred())
			// Destination secret should have "super-admin.conf" key (DPF operator expects this)
			Expect(destSecret.Data[DestinationKubeconfigSecretKey]).To(Equal([]byte("fake-kubeconfig-data")))
			Expect(destSecret.Labels[common.LabelDPFHCPProvisionerName]).To(Equal("test-provisioner"))
			Expect(destSecret.Labels[common.LabelDPFHCPProvisionerNamespace]).To(Equal("test-ns"))

			// DPUCluster updated
			updatedDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, updatedDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(updatedDPU.Spec.Kubeconfig).To(Equal("test-provisioner-admin-kubeconfig"))

			// Status updated
			Expect(provisioner.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(provisioner.Status.KubeConfigSecretRef.Name).To(Equal("test-provisioner-admin-kubeconfig"))

			// Condition set to True
			afterCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))
			Expect(afterCond.Message).To(ContainSubstring("successfully"))
		})
	})

	Describe("Secret Not Ready Scenario", func() {
		It("should set condition to pending when HC kubeconfig secret doesn't exist", func() {
			// Given: DPFHCPProvisioner but no HC kubeconfig secret yet
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-provisioner",
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
				WithObjects(provisioner, dpuCluster).
				WithStatusSubresource(provisioner).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, provisioner)

			// Then: Requeue without error
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Condition set to pending
			cond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigPending))
		})
	})

	Describe("Idempotency - Scenario A: Drift Detection", func() {
		It("should detect and correct content drift between source and destination secrets", func() {
			// Given: Both secrets exist but with different content (drift scenario)
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-provisioner-admin-kubeconfig",
					},
				},
			}

			// Set initial condition to True (was previously injected)
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:    provisioningv1alpha1.KubeConfigInjected,
				Status:  metav1.ConditionTrue,
				Reason:  provisioningv1alpha1.ReasonKubeConfigInjected,
				Message: "Previously injected",
			})

			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					SourceKubeconfigSecretKey: []byte("new-rotated-kubeconfig-data"),
				},
			}

			destSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "dpu-ns",
				},
				Data: map[string][]byte{
					DestinationKubeconfigSecretKey: []byte("old-kubeconfig-data"), // DRIFT!
				},
			}

			dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-dpu",
					Namespace: "dpu-ns",
				},
				Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
					Kubeconfig: "test-provisioner-admin-kubeconfig",
				},
			}

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner, hcSecret, destSecret, dpuCluster).
				WithStatusSubresource(provisioner, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state
			beforeCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(beforeCond).NotTo(BeNil())
			Expect(beforeCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(beforeCond.Message).To(Equal("Previously injected"))

			// Verify drift exists before reconciliation
			beforeSecret := &corev1.Secret{}
			err := fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-provisioner-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, beforeSecret)
			Expect(err).NotTo(HaveOccurred())
			Expect(beforeSecret.Data[DestinationKubeconfigSecretKey]).To(Equal([]byte("old-kubeconfig-data")))

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, provisioner)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify drift was DETECTED - check for DriftCorrected event
			Eventually(recorder.Events).Should(Receive(ContainSubstring("DriftCorrected")))

			// Destination secret updated to match source
			updatedSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-provisioner-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, updatedSecret)
			Expect(err).NotTo(HaveOccurred())
			Expect(updatedSecret.Data[DestinationKubeconfigSecretKey]).To(Equal([]byte("new-rotated-kubeconfig-data")))

			// Condition remains True (stayed healthy through drift correction)
			afterCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))
			Expect(afterCond.Message).To(ContainSubstring("successfully"))

			// Verify status was updated
			Expect(provisioner.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(provisioner.Status.KubeConfigSecretRef.Name).To(Equal("test-provisioner-admin-kubeconfig"))
		})
	})

	Describe("Idempotency - Scenario B: Secret Exists, DPUCluster Not Updated", func() {
		It("should update DPUCluster without recreating secret", func() {
			// Given: Secret exists but DPUCluster not updated (partial completion scenario)
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
				},
			}

			// No initial condition set (injection was interrupted)

			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					SourceKubeconfigSecretKey: []byte("kubeconfig-data"),
				},
			}

			destSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "dpu-ns",
				},
				Data: map[string][]byte{
					DestinationKubeconfigSecretKey: []byte("kubeconfig-data"),
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
				WithObjects(provisioner, hcSecret, destSecret, dpuCluster).
				WithStatusSubresource(provisioner, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state
			beforeCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
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
				Name:      "test-provisioner-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, existingSecret)
			Expect(err).NotTo(HaveOccurred(), "Secret should already exist")
			originalResourceVersion := existingSecret.ResourceVersion

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, provisioner)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify NO secret recreation - secret should not have been recreated
			afterSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-provisioner-admin-kubeconfig",
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
			Expect(updatedDPU.Spec.Kubeconfig).To(Equal("test-provisioner-admin-kubeconfig"))

			// Condition set to True (injection completed)
			afterCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))

			// Status updated
			Expect(provisioner.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(provisioner.Status.KubeConfigSecretRef.Name).To(Equal("test-provisioner-admin-kubeconfig"))
		})
	})

	Describe("Idempotency - Scenario C: Secret Missing, DPUCluster Updated", func() {
		It("should recreate secret without modifying DPUCluster", func() {
			// Given: DPUCluster updated but secret missing (secret was deleted scenario)
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					KubeConfigSecretRef: &corev1.LocalObjectReference{
						Name: "test-provisioner-admin-kubeconfig",
					},
				},
			}

			// Set initial condition to True (was previously injected but secret got deleted)
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:    provisioningv1alpha1.KubeConfigInjected,
				Status:  metav1.ConditionTrue,
				Reason:  provisioningv1alpha1.ReasonKubeConfigInjected,
				Message: "Previously injected",
			})

			hcSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					SourceKubeconfigSecretKey: []byte("kubeconfig-data"),
				},
			}

			dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-dpu",
					Namespace: "dpu-ns",
				},
				Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
					Kubeconfig: "test-provisioner-admin-kubeconfig", // Already updated
				},
			}

			// NOTE: destSecret is NOT created - it's missing!

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner, hcSecret, dpuCluster). // No destSecret
				WithStatusSubresource(provisioner, dpuCluster).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// Verify BEFORE state
			beforeCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(beforeCond).NotTo(BeNil())
			Expect(beforeCond.Status).To(Equal(metav1.ConditionTrue))

			beforeDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err := fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, beforeDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(beforeDPU.Spec.Kubeconfig).To(Equal("test-provisioner-admin-kubeconfig"),
				"DPUCluster should already be updated")
			originalDPUResourceVersion := beforeDPU.ResourceVersion

			// Verify secret is missing
			missingSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-provisioner-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, missingSecret)
			Expect(err).To(HaveOccurred())
			Expect(apierrors.IsNotFound(err)).To(BeTrue(), "Secret should be missing")

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, provisioner)

			// Then: Success
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Secret SHOULD be recreated with "super-admin.conf" key
			recreatedSecret := &corev1.Secret{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-provisioner-admin-kubeconfig",
				Namespace: "dpu-ns",
			}, recreatedSecret)
			Expect(err).NotTo(HaveOccurred(), "Secret should be recreated")
			Expect(recreatedSecret.Data[DestinationKubeconfigSecretKey]).To(Equal([]byte("kubeconfig-data")))
			Expect(recreatedSecret.Labels[common.LabelDPFHCPProvisionerName]).To(Equal("test-provisioner"))
			Expect(recreatedSecret.Labels[common.LabelDPFHCPProvisionerNamespace]).To(Equal("test-ns"))

			// DPUCluster should NOT be modified (already updated)
			afterDPU := &dpuprovisioningv1alpha1.DPUCluster{}
			err = fakeClient.Get(ctx, types.NamespacedName{
				Name:      "test-dpu",
				Namespace: "dpu-ns",
			}, afterDPU)
			Expect(err).NotTo(HaveOccurred())
			Expect(afterDPU.Spec.Kubeconfig).To(Equal("test-provisioner-admin-kubeconfig"))
			Expect(afterDPU.ResourceVersion).To(Equal(originalDPUResourceVersion),
				"DPUCluster should not have been modified")

			// Condition remains True (stayed healthy through secret recreation)
			afterCond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
			Expect(afterCond).NotTo(BeNil())
			Expect(afterCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(afterCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigInjected))

			// Status still references the secret
			Expect(provisioner.Status.KubeConfigSecretRef).NotTo(BeNil())
			Expect(provisioner.Status.KubeConfigSecretRef.Name).To(Equal("test-provisioner-admin-kubeconfig"))
		})
	})

	Describe("Skip When HostedCluster Not Created", func() {
		It("should skip injection when HostedClusterRef is nil", func() {
			// Given: DPFHCPProvisioner without HostedCluster created yet
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-ns",
					},
				},
				// HostedClusterRef is nil
			}

			fakeClient = fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			injector = NewKubeconfigInjector(fakeClient, recorder)

			// When: Reconciliation runs
			result, err := injector.InjectKubeconfig(ctx, provisioner)

			// Then: Skip without error
			Expect(err).NotTo(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// No condition set
			cond := findCondition(provisioner.Status.Conditions, provisioningv1alpha1.KubeConfigInjected)
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
