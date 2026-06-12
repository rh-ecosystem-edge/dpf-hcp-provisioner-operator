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

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ignitiongenerator"
)

var _ = Describe("DPUDeployment Watch", func() {
	var scheme *runtime.Scheme

	BeforeEach(func() {
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(corev1.AddToScheme(scheme)).To(Succeed())
		Expect(dpuservicev1alpha1.AddToScheme(scheme)).To(Succeed())
	})

	Describe("dpuDeploymentToRequests", func() {
		It("should map to the DPFHCPProvisioner referencing the DPUDeployment", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "dpf-operator-system",
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.dpuDeploymentToRequests(context.TODO(), dd)

			Expect(requests).To(HaveLen(1))
			Expect(requests[0].NamespacedName).To(Equal(types.NamespacedName{
				Name:      "test-provisioner",
				Namespace: "default",
			}))
		})

		It("should return empty when no provisioner references the DPUDeployment", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "other-deployment",
						Namespace: "dpf-operator-system",
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.dpuDeploymentToRequests(context.TODO(), dd)

			Expect(requests).To(BeEmpty())
		})

		It("should return empty when provisioner has nil DPUDeploymentRef", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.dpuDeploymentToRequests(context.TODO(), dd)

			Expect(requests).To(BeEmpty())
		})

		It("should not match when DPUDeployment namespace differs", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "namespace-a",
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "namespace-b",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			requests := r.dpuDeploymentToRequests(context.TODO(), dd)

			Expect(requests).To(BeEmpty())
		})
	})

	Describe("dpuDeploymentChanged", func() {
		It("should return false when IgnitionConfigured is not True", func() {
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "dpf-operator-system",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeFalse())
		})

		It("should return false when DPUDeploymentRef is nil", func() {
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
							Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeFalse())
		})

		It("should return false when BFB and Flavor match ConfigMap annotations", func() {
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "dpf-operator-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
				Spec: dpuservicev1alpha1.DPUDeploymentSpec{
					DPUs: dpuservicev1alpha1.DPUs{
						BFB:    "my-bfb",
						Flavor: "my-flavor",
					},
				},
			}

			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      ignitiongenerator.ConfigMapName("cluster"),
					Namespace: "dpu-system",
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateBFBNameAnnotation:       "my-bfb",
						ignitiongenerator.BfcfgTemplateDPUFlavorNameAnnotation: "my-flavor",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(dd, cm).
				Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeFalse())
		})

		It("should return true when BFB changed", func() {
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "dpf-operator-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
				Spec: dpuservicev1alpha1.DPUDeploymentSpec{
					DPUs: dpuservicev1alpha1.DPUs{
						BFB:    "new-bfb",
						Flavor: "my-flavor",
					},
				},
			}

			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      ignitiongenerator.ConfigMapName("cluster"),
					Namespace: "dpu-system",
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateBFBNameAnnotation:       "old-bfb",
						ignitiongenerator.BfcfgTemplateDPUFlavorNameAnnotation: "my-flavor",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(dd, cm).
				Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeTrue())
		})

		It("should return true when Flavor changed", func() {
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "dpf-operator-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
				Spec: dpuservicev1alpha1.DPUDeploymentSpec{
					DPUs: dpuservicev1alpha1.DPUs{
						BFB:    "my-bfb",
						Flavor: "new-flavor",
					},
				},
			}

			cm := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      ignitiongenerator.ConfigMapName("cluster"),
					Namespace: "dpu-system",
					Annotations: map[string]string{
						ignitiongenerator.BfcfgTemplateBFBNameAnnotation:       "my-bfb",
						ignitiongenerator.BfcfgTemplateDPUFlavorNameAnnotation: "old-flavor",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(dd, cm).
				Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeTrue())
		})

		It("should return true when ConfigMap has nil annotations", func() {
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "dpf-operator-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
				Spec: dpuservicev1alpha1.DPUDeploymentSpec{
					DPUs: dpuservicev1alpha1.DPUs{
						BFB:    "my-bfb",
						Flavor: "my-flavor",
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
				WithObjects(dd, cm).
				Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeTrue())
		})

		It("should return false when DPUDeployment is not found", func() {
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "missing-deployment",
						Namespace: "dpf-operator-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeFalse())
		})

		It("should return false when ConfigMap is not found", func() {
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "my-deployment",
						Namespace: "dpf-operator-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionTrue,
							Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			dd := &dpuservicev1alpha1.DPUDeployment{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
				Spec: dpuservicev1alpha1.DPUDeploymentSpec{
					DPUs: dpuservicev1alpha1.DPUs{
						BFB:    "my-bfb",
						Flavor: "my-flavor",
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(dd).
				Build()
			r := &DPFHCPProvisionerReconciler{Client: fakeClient}
			Expect(r.dpuDeploymentChanged(context.TODO(), provisioner)).To(BeFalse())
		})
	})

	Describe("DPUDeploymentChanged reason in phase computation", func() {
		It("should transition to IgnitionGenerating (not Failed) when reason is DPUDeploymentChanged", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "test",
					Namespace:  "default",
					Generation: 1,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "cluster",
						Namespace: "dpu-system",
					},
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test",
						Namespace: "default",
					},
					Conditions: []metav1.Condition{
						{
							Type:               provisioningv1alpha1.HostedClusterAvailable,
							Status:             metav1.ConditionTrue,
							Reason:             "AsExpected",
							LastTransitionTime: metav1.Now(),
						},
						{
							Type:               provisioningv1alpha1.KubeConfigInjected,
							Status:             metav1.ConditionTrue,
							Reason:             provisioningv1alpha1.ReasonKubeConfigInjected,
							LastTransitionTime: metav1.Now(),
						},
						{
							Type:               provisioningv1alpha1.IgnitionConfigured,
							Status:             metav1.ConditionFalse,
							Reason:             provisioningv1alpha1.ReasonDPUDeploymentChanged,
							Message:            "DPUDeployment BFB or Flavor changed, regenerating ignition configuration",
							LastTransitionTime: metav1.Now(),
						},
					},
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				Build()
			r := &DPFHCPProvisionerReconciler{
				Client:   fakeClient,
				Recorder: record.NewFakeRecorder(10),
			}
			r.updatePhaseFromConditions(provisioner)
			Expect(provisioner.Status.Phase).To(Equal(provisioningv1alpha1.PhaseIgnitionGenerating))
		})
	})
})

var _ = Describe("updatePhaseFromConditions with DPUDeploymentChanged", func() {
	It("should not set Failed phase for DPUDeploymentChanged reason", func() {
		provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:       "test",
				Namespace:  "default",
				Generation: 1,
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
					Name:      "cluster",
					Namespace: "dpu-system",
				},
			},
			Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
				HostedClusterRef: &corev1.ObjectReference{
					Name:      "test",
					Namespace: "default",
				},
				Conditions: []metav1.Condition{
					{
						Type:               provisioningv1alpha1.HostedClusterAvailable,
						Status:             metav1.ConditionTrue,
						Reason:             "AsExpected",
						LastTransitionTime: metav1.Now(),
					},
					{
						Type:               provisioningv1alpha1.KubeConfigInjected,
						Status:             metav1.ConditionTrue,
						Reason:             provisioningv1alpha1.ReasonKubeConfigInjected,
						LastTransitionTime: metav1.Now(),
					},
					{
						Type:               provisioningv1alpha1.IgnitionConfigured,
						Status:             metav1.ConditionFalse,
						Reason:             provisioningv1alpha1.ReasonDPUDeploymentChanged,
						LastTransitionTime: metav1.Now(),
					},
				},
			},
		}

		r := &DPFHCPProvisionerReconciler{
			Recorder: record.NewFakeRecorder(10),
		}
		r.updatePhaseFromConditions(provisioner)

		Expect(provisioner.Status.Phase).NotTo(Equal(provisioningv1alpha1.PhaseFailed))
		Expect(provisioner.Status.Phase).To(Equal(provisioningv1alpha1.PhaseIgnitionGenerating))
	})

	It("should set Failed phase for IgnitionGenerationFailed reason", func() {
		provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:       "test",
				Namespace:  "default",
				Generation: 1,
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
					Name:      "cluster",
					Namespace: "dpu-system",
				},
			},
			Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
				HostedClusterRef: &corev1.ObjectReference{
					Name:      "test",
					Namespace: "default",
				},
				Conditions: []metav1.Condition{
					{
						Type:               provisioningv1alpha1.HostedClusterAvailable,
						Status:             metav1.ConditionTrue,
						Reason:             "AsExpected",
						LastTransitionTime: metav1.Now(),
					},
					{
						Type:               provisioningv1alpha1.KubeConfigInjected,
						Status:             metav1.ConditionTrue,
						Reason:             provisioningv1alpha1.ReasonKubeConfigInjected,
						LastTransitionTime: metav1.Now(),
					},
					{
						Type:               provisioningv1alpha1.IgnitionConfigured,
						Status:             metav1.ConditionFalse,
						Reason:             provisioningv1alpha1.ReasonIgnitionGenerationFailed,
						LastTransitionTime: metav1.Now(),
					},
				},
			},
		}

		r := &DPFHCPProvisionerReconciler{
			Recorder: record.NewFakeRecorder(10),
		}
		r.updatePhaseFromConditions(provisioner)

		Expect(provisioner.Status.Phase).To(Equal(provisioningv1alpha1.PhaseFailed))
	})
})

var _ = Describe("computeReadyCondition with DPUDeploymentChanged", func() {
	It("should set Ready=False when IgnitionConfigured is False due to DPUDeploymentChanged", func() {
		provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:       "test",
				Namespace:  "default",
				Generation: 1,
			},
			Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
				Conditions: []metav1.Condition{
					{
						Type:               provisioningv1alpha1.HostedClusterAvailable,
						Status:             metav1.ConditionTrue,
						Reason:             "AsExpected",
						LastTransitionTime: metav1.Now(),
					},
					{
						Type:               provisioningv1alpha1.KubeConfigInjected,
						Status:             metav1.ConditionTrue,
						Reason:             provisioningv1alpha1.ReasonKubeConfigInjected,
						LastTransitionTime: metav1.Now(),
					},
					{
						Type:               provisioningv1alpha1.IgnitionConfigured,
						Status:             metav1.ConditionFalse,
						Reason:             provisioningv1alpha1.ReasonDPUDeploymentChanged,
						LastTransitionTime: metav1.Now(),
					},
				},
			},
		}

		scheme := runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		fakeClient := fake.NewClientBuilder().WithScheme(scheme).Build()
		r := &DPFHCPProvisionerReconciler{
			Client:   fakeClient,
			Recorder: record.NewFakeRecorder(10),
		}
		r.computeReadyCondition(context.TODO(), provisioner)

		readyCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.Ready)
		Expect(readyCond).NotTo(BeNil())
		Expect(readyCond.Status).To(Equal(metav1.ConditionFalse))
		Expect(readyCond.Reason).To(Equal("IgnitionNotConfigured"))
	})
})
