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
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ignitiongenerator"
)

// These tests cover the lifecycle fix allowing spec.dpuDeploymentRef to be left unset
// at creation and populated later (e.g. by the DPF Operator, once the DPUDeployment it
// points to actually exists) instead of being required up front.
var _ = Describe("DPUDeploymentRef optional/deferred lifecycle", func() {
	var (
		scheme *runtime.Scheme
		ctx    context.Context
	)

	BeforeEach(func() {
		ctx = context.Background()
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(corev1.AddToScheme(scheme)).To(Succeed())
	})

	newCR := func() *provisioningv1alpha1.DPFHCPProvisioner {
		cr := &provisioningv1alpha1.DPFHCPProvisioner{
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
			Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
				Phase: provisioningv1alpha1.PhaseGeneratingIgnition,
				HostedClusterRef: &corev1.ObjectReference{
					Name:      "hc",
					Namespace: "clusters-test-provisioner",
				},
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
			Reason: provisioningv1alpha1.ReasonKubeConfigInjected,
		})
		return cr
	}

	buildReconciler := func(cr *provisioningv1alpha1.DPFHCPProvisioner) *DPFHCPProvisionerReconciler {
		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithObjects(cr).
			WithStatusSubresource(cr).
			Build()
		return &DPFHCPProvisionerReconciler{
			Client:            fakeClient,
			Scheme:            scheme,
			Recorder:          record.NewFakeRecorder(10),
			IgnitionGenerator: ignitiongenerator.NewIgnitionGenerator(fakeClient, scheme, record.NewFakeRecorder(10)),
		}
	}

	It("defers ignition generation without panicking when dpuDeploymentRef is nil", func() {
		cr := newCR()
		Expect(cr.Spec.DPUDeploymentRef).To(BeNil())

		r := buildReconciler(cr)

		var result ctrl.Result
		var err error
		Expect(func() {
			result, err = r.generateIgnition(ctx, cr)
		}).NotTo(Panic())

		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		ignCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(ignCond).NotTo(BeNil())
		Expect(ignCond.Status).To(Equal(metav1.ConditionFalse))
		Expect(ignCond.Reason).To(Equal(provisioningv1alpha1.ReasonDPUDeploymentRefNotSet))

		// Status must be persisted, not just mutated in-memory.
		persisted := &provisioningv1alpha1.DPFHCPProvisioner{}
		Expect(r.Get(ctx, types.NamespacedName{Name: cr.Name, Namespace: cr.Namespace}, persisted)).To(Succeed())
		persistedCond := meta.FindStatusCondition(persisted.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(persistedCond).NotTo(BeNil())
		Expect(persistedCond.Reason).To(Equal(provisioningv1alpha1.ReasonDPUDeploymentRefNotSet))
	})

	PIt("proceeds with ignition generation once dpuDeploymentRef is set by a later update (e.g. by the DPF Operator)", func() {
		cr := newCR()
		r := buildReconciler(cr)

		// First pass: ref not set yet, generation deferred.
		_, err := r.generateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		ignCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(ignCond.Reason).To(Equal(provisioningv1alpha1.ReasonDPUDeploymentRefNotSet))

		// Second pass: Simulate the DPF Operator having set the ref by updating the CR in memory
		// (In real reconciliation, the CR would be refetched after the DPF Operator patches it)
		cr.Spec.DPUDeploymentRef = &provisioningv1alpha1.DPUDeploymentReference{
			Name:      "my-deployment",
			Namespace: "dpf-operator-system",
		}

		// Verify the ref is actually set
		Expect(cr.Spec.DPUDeploymentRef).NotTo(BeNil(), "dpuDeploymentRef should be set before second call")

		Expect(func() {
			_, err = r.generateIgnition(ctx, cr)
		}).NotTo(Panic())

		// The deferred-wait condition must have cleared: the nil-guard must not keep
		// blocking generation now that the ref is set.
		ignCond = meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(ignCond).NotTo(BeNil())
		Expect(ignCond.Reason).NotTo(Equal(provisioningv1alpha1.ReasonDPUDeploymentRefNotSet),
			"controller must proceed past nil-guard once dpuDeploymentRef is set")

		// With no real DPUDeployment in this unit test, validateIgnitionDependencies catches
		// the missing dependency before calling the ignition generator.
		Expect(ignCond.Reason).To(Equal(provisioningv1alpha1.ReasonDependencyMissing),
			"should wait for DPUDeployment to exist")
	})
})
