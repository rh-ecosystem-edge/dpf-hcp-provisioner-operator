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
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// Ready Condition Tests
// These tests directly call computeReadyCondition and updatePhaseFromConditions
// to verify condition aggregation logic without requiring the full controller loop.
var _ = Describe("Ready Condition Computation", func() {
	var (
		reconciler *DPFHCPProvisionerReconciler
		ctx        context.Context
	)

	BeforeEach(func() {
		ctx = context.Background()
		reconciler = &DPFHCPProvisionerReconciler{
			Client: k8sClient,
		}
	})

	Context("computeReadyCondition", func() {
		It("should set Ready=False when HostedClusterAvailable is not True", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "ready-test-hc-not-available",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			reconciler.computeReadyCondition(ctx, cr)

			readyCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.Ready)
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionFalse))
			Expect(readyCond.Reason).To(Equal(provisioningv1alpha1.ReasonHostedClusterNotReady))
		})

		It("should set Ready=False when KubeConfigInjected is not True", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "ready-test-kc-not-injected",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			// Set HC available
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})

			reconciler.computeReadyCondition(ctx, cr)

			readyCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.Ready)
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionFalse))
			Expect(readyCond.Reason).To(Equal(provisioningv1alpha1.ReasonKubeConfigNotInjected))
		})

		It("should set Ready=False when IgnitionConfigured is not True", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "ready-test-ign-not-configured",
					Namespace:  "default",
					Generation: 1,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.KubeConfigInjected,
				Status: metav1.ConditionTrue,
				Reason: "Injected",
			})

			reconciler.computeReadyCondition(ctx, cr)

			readyCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.Ready)
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionFalse))
			Expect(readyCond.Reason).To(Equal("IgnitionNotConfigured"))
		})

		It("should set Ready=False when MetalLBConfigured is not True for HA provisioner", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "ready-test-metallb-not-configured",
					Namespace:  "default",
					Generation: 1,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					VirtualIP:                      "192.168.1.100",
				},
			}

			reconciler.computeReadyCondition(ctx, cr)

			readyCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.Ready)
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionFalse))
			Expect(readyCond.Reason).To(Equal("MetalLBNotConfigured"))
		})

		It("should set Ready=True when all conditions are met", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "ready-test-all-met",
					Namespace:  "default",
					Generation: 1,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.KubeConfigInjected,
				Status: metav1.ConditionTrue,
				Reason: "Injected",
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.IgnitionConfigured,
				Status:             metav1.ConditionTrue,
				Reason:             "Configured",
				ObservedGeneration: 1,
			})

			reconciler.computeReadyCondition(ctx, cr)

			readyCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.Ready)
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(readyCond.Reason).To(Equal(provisioningv1alpha1.ReasonAllComponentsOperational))
		})

		It("should set Ready=True for HA provisioner when all conditions including MetalLB are met", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "ready-test-ha-all-met",
					Namespace:  "default",
					Generation: 1,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					VirtualIP:                      "192.168.1.100",
				},
			}
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.MetalLBConfigured,
				Status: metav1.ConditionTrue,
				Reason: "MetalLBReady",
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.KubeConfigInjected,
				Status: metav1.ConditionTrue,
				Reason: "Injected",
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.IgnitionConfigured,
				Status:             metav1.ConditionTrue,
				Reason:             "Configured",
				ObservedGeneration: 1,
			})

			reconciler.computeReadyCondition(ctx, cr)

			readyCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.Ready)
			Expect(readyCond).NotTo(BeNil())
			Expect(readyCond.Status).To(Equal(metav1.ConditionTrue))
			Expect(readyCond.Reason).To(Equal(provisioningv1alpha1.ReasonAllComponentsOperational))
		})
	})

	Context("updatePhaseFromConditions", func() {
		It("should set phase to IgnitionGenerating when HC available and kubeconfig injected but no ignition", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "phase-ign-gen",
					Namespace:  "default",
					Generation: 1,
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{Name: "test"},
				},
			}
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.KubeConfigInjected,
				Status: metav1.ConditionTrue,
				Reason: "Injected",
			})

			reconciler.updatePhaseFromConditions(cr)

			Expect(cr.Status.Phase).To(Equal(provisioningv1alpha1.PhaseIgnitionGenerating))
		})

		It("should set phase to Ready when all conditions are True including Ready", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "phase-ready",
					Namespace:  "default",
					Generation: 1,
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{Name: "test"},
				},
			}
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.Ready,
				Status: metav1.ConditionTrue,
				Reason: provisioningv1alpha1.ReasonAllComponentsOperational,
			})

			reconciler.updatePhaseFromConditions(cr)

			Expect(cr.Status.Phase).To(Equal(provisioningv1alpha1.PhaseReady))
		})

		It("should set phase to Provisioning when hostedClusterRef is set but Ready is not True", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-provisioning",
					Namespace: "default",
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{Name: "test"},
				},
			}

			reconciler.updatePhaseFromConditions(cr)

			Expect(cr.Status.Phase).To(Equal(provisioningv1alpha1.PhaseProvisioning))
		})

		It("should set phase to Pending when no hostedClusterRef and no failures", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-pending",
					Namespace: "default",
				},
			}

			reconciler.updatePhaseFromConditions(cr)

			Expect(cr.Status.Phase).To(Equal(provisioningv1alpha1.PhasePending))
		})

		It("should set phase to Failed when DPUClusterMissing is True", func() {
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-failed",
					Namespace: "default",
				},
			}
			meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.DPUClusterMissing,
				Status: metav1.ConditionTrue,
				Reason: "Missing",
			})

			reconciler.updatePhaseFromConditions(cr)

			Expect(cr.Status.Phase).To(Equal(provisioningv1alpha1.PhaseFailed))
		})

		It("should set phase to Deleting when DeletionTimestamp is set", func() {
			now := metav1.Now()
			cr := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:              "phase-deleting",
					Namespace:         "default",
					DeletionTimestamp: &now,
				},
			}

			reconciler.updatePhaseFromConditions(cr)

			Expect(cr.Status.Phase).To(Equal(provisioningv1alpha1.PhaseDeleting))
		})
	})
})
