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
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/event"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ignitiongenerator"
)

var _ = Describe("ConfigMapName", func() {
	It("should return the expected name format", func() {
		Expect(ignitiongenerator.ConfigMapName("my-cluster")).To(Equal("bfcfg-my-cluster.cfg"))
	})

	It("should handle cluster names with special characters", func() {
		Expect(ignitiongenerator.ConfigMapName("cluster-with-dashes")).To(Equal("bfcfg-cluster-with-dashes.cfg"))
	})
})

var _ = Describe("Ignition ConfigMap Watch", func() {
	var scheme *runtime.Scheme

	BeforeEach(func() {
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(corev1.AddToScheme(scheme)).To(Succeed())
	})

	Describe("ignitionConfigMapPredicate", func() {
		pred := ignitionConfigMapPredicate()

		It("should accept Delete events for bfcfg-template ConfigMaps", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-test-cluster.cfg",
					Namespace: "dpu-system",
					Labels: map[string]string{
						ignitiongenerator.BfcfgTemplateLabel: "true",
					},
				},
			}
			Expect(pred.Delete(event.DeleteEvent{Object: cm})).To(BeTrue())
		})

		It("should reject Delete events for ConfigMaps with nil labels", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-test-cluster.cfg",
					Namespace: "dpu-system",
				},
			}
			Expect(pred.Delete(event.DeleteEvent{Object: cm})).To(BeFalse())
		})

		It("should reject Delete events for non-bfcfg ConfigMaps", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "some-other-configmap",
					Namespace: "dpu-system",
				},
			}
			Expect(pred.Delete(event.DeleteEvent{Object: cm})).To(BeFalse())
		})

		It("should reject Delete events when label value is not 'true'", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-test-cluster.cfg",
					Namespace: "dpu-system",
					Labels: map[string]string{
						ignitiongenerator.BfcfgTemplateLabel: "false",
					},
				},
			}
			Expect(pred.Delete(event.DeleteEvent{Object: cm})).To(BeFalse())
		})

		It("should reject Create events", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name: "bfcfg-test-cluster.cfg",
					Labels: map[string]string{
						ignitiongenerator.BfcfgTemplateLabel: "true",
					},
				},
			}
			Expect(pred.Create(event.CreateEvent{Object: cm})).To(BeFalse())
		})

		It("should reject Update events", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name: "bfcfg-test-cluster.cfg",
					Labels: map[string]string{
						ignitiongenerator.BfcfgTemplateLabel: "true",
					},
				},
			}
			Expect(pred.Update(event.UpdateEvent{ObjectNew: cm})).To(BeFalse())
		})

		It("should reject Generic events", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name: "bfcfg-test-cluster.cfg",
					Labels: map[string]string{
						ignitiongenerator.BfcfgTemplateLabel: "true",
					},
				},
			}
			Expect(pred.Generic(event.GenericEvent{Object: cm})).To(BeFalse())
		})
	})

	Describe("ignitionConfigMapToRequests", func() {
		It("should map to the DPFHCPProvisioner referencing the DPUCluster", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "my-dpu-cluster",
						Namespace: "dpu-system",
					},
				},
			}

			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-my-dpu-cluster.cfg",
					Namespace: "dpu-system",
					Labels: map[string]string{
						ignitiongenerator.BfcfgTemplateLabel: "true",
					},
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateClusterNameAnnotation:      "my-dpu-cluster",
						ignitiongenerator.BfcfgTemplateClusterNamespaceAnnotation: "dpu-system",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.ignitionConfigMapToRequests(context.TODO(), cm)

			Expect(requests).To(HaveLen(1))
			Expect(requests[0].NamespacedName).To(Equal(types.NamespacedName{
				Name:      "test-provisioner",
				Namespace: "default",
			}))
		})

		It("should return empty when no matching provisioner exists", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-orphan-cluster.cfg",
					Namespace: "dpu-system",
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateClusterNameAnnotation:      "orphan-cluster",
						ignitiongenerator.BfcfgTemplateClusterNamespaceAnnotation: "dpu-system",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.ignitionConfigMapToRequests(context.TODO(), cm)

			Expect(requests).To(BeEmpty())
		})

		It("should return empty when annotations are missing", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-no-annotations.cfg",
					Namespace: "dpu-system",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.ignitionConfigMapToRequests(context.TODO(), cm)

			Expect(requests).To(BeEmpty())
		})

		It("should return empty when only cluster name annotation is present", func() {
			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-partial.cfg",
					Namespace: "dpu-system",
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateClusterNameAnnotation: "my-cluster",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.ignitionConfigMapToRequests(context.TODO(), cm)

			Expect(requests).To(BeEmpty())
		})

		It("should only match the provisioner with the correct DPUClusterRef", func() {
			matching := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "matching",
					Namespace: "ns-a",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "target-cluster",
						Namespace: "dpu-system",
					},
				},
			}
			other := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "other",
					Namespace: "ns-b",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "different-cluster",
						Namespace: "dpu-system",
					},
				},
			}

			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-target-cluster.cfg",
					Namespace: "dpu-system",
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateClusterNameAnnotation:      "target-cluster",
						ignitiongenerator.BfcfgTemplateClusterNamespaceAnnotation: "dpu-system",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(matching, other).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.ignitionConfigMapToRequests(context.TODO(), cm)

			Expect(requests).To(HaveLen(1))
			Expect(requests[0].Name).To(Equal("matching"))
			Expect(requests[0].Namespace).To(Equal("ns-a"))
		})

		It("should not match when cluster namespace differs", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "provisioner",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "my-cluster",
						Namespace: "dpu-system-a",
					},
				},
			}

			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "bfcfg-my-cluster.cfg",
					Namespace: "dpu-system-b",
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateClusterNameAnnotation:      "my-cluster",
						ignitiongenerator.BfcfgTemplateClusterNamespaceAnnotation: "dpu-system-b",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.ignitionConfigMapToRequests(context.TODO(), cm)

			Expect(requests).To(BeEmpty())
		})
	})

	Describe("verifyIgnitionConfigMap", func() {
		It("should return false when no conditions exist", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
			r := &DPFHCPProvisionerReconciler{
				Client:   fakeClient,
				Recorder: record.NewFakeRecorder(10),
			}
			deleted := r.verifyIgnitionConfigMap(context.TODO(), provisioner)
			Expect(deleted).To(BeFalse())

			cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
			Expect(cond).To(BeNil())
		})

		It("should return false when IgnitionConfigured is False", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionFalse,
							Reason:             "IgnitionGenerationFailed",
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
			r := &DPFHCPProvisionerReconciler{
				Client:   fakeClient,
				Recorder: record.NewFakeRecorder(10),
			}
			deleted := r.verifyIgnitionConfigMap(context.TODO(), provisioner)
			Expect(deleted).To(BeFalse())

			cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("IgnitionGenerationFailed"))
		})

		It("should return false when ConfigMap exists", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             "IgnitionGenerated",
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      ignitiongenerator.ConfigMapName("cluster"),
					Namespace: "dpu-system",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(cm).
				Build()
			r := &DPFHCPProvisionerReconciler{
				Client:   fakeClient,
				Recorder: record.NewFakeRecorder(10),
			}
			deleted := r.verifyIgnitionConfigMap(context.TODO(), provisioner)
			Expect(deleted).To(BeFalse())

			cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionTrue))
		})

		It("should clear IgnitionConfigured and return true when ConfigMap is missing", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "test",
					Namespace:  "default",
					Generation: 2,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             "IgnitionGenerated",
							ObservedGeneration: 2,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				Build()
			recorder := record.NewFakeRecorder(10)
			r := &DPFHCPProvisionerReconciler{
				Client:   fakeClient,
				Recorder: recorder,
			}
			deleted := r.verifyIgnitionConfigMap(context.TODO(), provisioner)
			Expect(deleted).To(BeTrue())

			cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.Reason).To(Equal("ConfigMapDeleted"))
			Expect(cond.ObservedGeneration).To(Equal(int64(2)))

			// Verify an event was recorded
			Eventually(recorder.Events).Should(Receive(ContainSubstring("IgnitionConfigMapDeleted")))

			// Verify condition message references the ConfigMap name
			Expect(cond.Message).To(ContainSubstring(ignitiongenerator.ConfigMapName("cluster")))
		})

		It("should preserve ObservedGeneration from the CR when clearing condition", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "test",
					Namespace:  "default",
					Generation: 5,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             "IgnitionGenerated",
							ObservedGeneration: 3,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
			r := &DPFHCPProvisionerReconciler{
				Client:   fakeClient,
				Recorder: record.NewFakeRecorder(10),
			}
			deleted := r.verifyIgnitionConfigMap(context.TODO(), provisioner)
			Expect(deleted).To(BeTrue())

			cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
			Expect(cond).NotTo(BeNil())
			Expect(cond.Status).To(Equal(metav1.ConditionFalse))
			Expect(cond.ObservedGeneration).To(Equal(int64(5)),
				"ObservedGeneration should match the current CR generation, not the old condition's")
		})
	})
})
