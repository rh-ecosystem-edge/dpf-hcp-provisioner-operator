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
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
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

		It("should return false when Flavor matches ConfigMap annotation", func() {
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

	Describe("Phase computation with condition reasons", func() {
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

var _ = Describe("DPUDeployment flavor change integration (envtest)", func() {
	const (
		provisionerName = "test-provisioner-dpudeploy"
		provisionerNS   = "clusters"
		dpuClusterNS    = "dpudeployment-integ-dpucluster"
		dpuClusterName  = "integ-dpucluster"
		deploymentName  = "integ-deployment"
	)

	It("should detect flavor change and set IgnitionConfigured=False via controller reconcile", func() {
		By("creating DPUCluster namespace")
		Expect(k8sClient.Create(ctx, &corev1.Namespace{
			ObjectMeta: metav1.ObjectMeta{Name: dpuClusterNS},
		})).To(Succeed())

		By("creating DPUDeployment stub with old-flavor")
		dd := &dpuservicev1alpha1.DPUDeployment{
			ObjectMeta: metav1.ObjectMeta{
				Name:      deploymentName,
				Namespace: dpuClusterNS,
			},
			Spec: dpuservicev1alpha1.DPUDeploymentSpec{
				DPUs: dpuservicev1alpha1.DPUs{
					BFB:            "test-bfb",
					Flavor:         "old-flavor",
					NodeEffect:     dpuprovisioningv1alpha1.Action{},
					DPUSetStrategy: dpuprovisioningv1alpha1.DPUSetStrategy{Type: "RollingUpdate"},
				},
				Services: map[string]dpuservicev1alpha1.DPUDeploymentServiceConfiguration{
					"stub-svc": {ServiceTemplate: "stub-tpl", ServiceConfiguration: "stub-cfg"},
				},
				ServiceChains: &dpuservicev1alpha1.ServiceChains{
					Switches: []dpuservicev1alpha1.DPUDeploymentSwitch{
						{Ports: []dpuservicev1alpha1.DPUDeploymentPort{
							{ServiceInterface: &dpuservicev1alpha1.ServiceIfc{MatchLabels: map[string]string{"test": "true"}}},
						}},
					},
				},
			},
		}
		Expect(k8sClient.Create(ctx, dd)).To(Succeed())

		By("creating ignition ConfigMap with old-flavor annotation")
		cmName := ignitiongenerator.ConfigMapName(dpuClusterName)
		cm := &corev1.ConfigMap{
			ObjectMeta: metav1.ObjectMeta{
				Name:      cmName,
				Namespace: dpuClusterNS,
				Labels: map[string]string{
					ignitiongenerator.BfcfgTemplateLabel: "true",
				},
				Annotations: map[string]string{
					ignitiongenerator.BfcfgTemplateClusterNameAnnotation:      dpuClusterName,
					ignitiongenerator.BfcfgTemplateClusterNamespaceAnnotation: dpuClusterNS,
					ignitiongenerator.BfcfgTemplateDPUFlavorNameAnnotation:    "old-flavor",
				},
			},
			Data: map[string]string{
				"BF_CFG_TEMPLATE": `{"ignition":{"version":"3.4.0"}}`,
			},
		}
		Expect(k8sClient.Create(ctx, cm)).To(Succeed())

		By("creating DPFHCPProvisioner CR with IgnitionConfigured=True (simulating Ready state)")
		provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      provisionerName,
				Namespace: provisionerNS,
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
					Name:      dpuClusterName,
					Namespace: dpuClusterNS,
				},
				DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
					Name:      deploymentName,
					Namespace: dpuClusterNS,
				},
				BaseDomain:                     "test.example.com",
				OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64",
				SSHKeySecretRef:                corev1.LocalObjectReference{Name: "dummy-ssh"},
				PullSecretRef:                  corev1.LocalObjectReference{Name: "dummy-pull"},
				ControlPlaneAvailabilityPolicy: "SingleReplica",
			},
		}
		Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

		By("setting IgnitionConfigured=True on the CR status (simulating post-ignition state)")
		Eventually(func(g Gomega) {
			fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: provisionerNS}, fresh)).To(Succeed())
			meta.SetStatusCondition(&fresh.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.IgnitionConfigured,
				Status:             metav1.ConditionTrue,
				Reason:             provisioningv1alpha1.ReasonIgnitionGenerated,
				LastTransitionTime: metav1.Now(),
			})
			g.Expect(k8sClient.Status().Update(ctx, fresh)).To(Succeed())
		}, "10s", "1s").Should(Succeed())

		By("updating DPUDeployment flavor from old-flavor to new-flavor")
		Eventually(func(g Gomega) {
			freshDD := &dpuservicev1alpha1.DPUDeployment{}
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{Name: deploymentName, Namespace: dpuClusterNS}, freshDD)).To(Succeed())
			freshDD.Spec.DPUs.Flavor = "new-flavor"
			g.Expect(k8sClient.Update(ctx, freshDD)).To(Succeed())
		}, "10s", "1s").Should(Succeed())

		By("verifying the controller detects the flavor change and sets IgnitionConfigured=False")
		Eventually(func(g Gomega) {
			fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Name:      provisionerName,
				Namespace: provisionerNS,
			}, fresh)).To(Succeed())
			cond := meta.FindStatusCondition(fresh.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
			g.Expect(cond).NotTo(BeNil(), "IgnitionConfigured condition should exist")
			g.Expect(cond.Status).To(Equal(metav1.ConditionFalse), "IgnitionConfigured should be False after flavor change")
			g.Expect(cond.Reason).To(Equal(provisioningv1alpha1.ReasonDPUDeploymentChanged))
		}, "30s", "1s").Should(Succeed())
	})
})
