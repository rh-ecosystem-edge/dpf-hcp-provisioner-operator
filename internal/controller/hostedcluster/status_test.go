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

package hostedcluster

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

var _ = Describe("Status Syncer", func() {
	var (
		ctx          context.Context
		syncer       *StatusSyncer
		cr           *provisioningv1alpha1.DPFHCPBridge
		hc           *hyperv1.HostedCluster
		scheme       *runtime.Scheme
		fakeClient   *fake.ClientBuilder
		objectsToAdd []runtime.Object
	)

	BeforeEach(func() {
		ctx = context.Background()
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(hyperv1.AddToScheme(scheme)).To(Succeed())

		// Create test DPFHCPBridge CR
		cr = &provisioningv1alpha1.DPFHCPBridge{
			ObjectMeta: metav1.ObjectMeta{
				Name:       "test-bridge",
				Namespace:  "default",
				Generation: 1,
			},
			Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
				OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
			},
			Status: provisioningv1alpha1.DPFHCPBridgeStatus{
				HostedClusterRef: &corev1.ObjectReference{
					Name:       "test-bridge",
					Namespace:  "default",
					Kind:       "HostedCluster",
					APIVersion: "hypershift.openshift.io/v1beta1",
				},
			},
		}

		// Create test HostedCluster with status
		hc = &hyperv1.HostedCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-bridge",
				Namespace: "default",
			},
			Status: hyperv1.HostedClusterStatus{
				Conditions: []metav1.Condition{
					{
						Type:   string(hyperv1.HostedClusterAvailable),
						Status: metav1.ConditionTrue,
						Reason: "HostedClusterAsExpected",
					},
					{
						Type:   string(hyperv1.HostedClusterProgressing),
						Status: metav1.ConditionFalse,
						Reason: "HostedClusterAsExpected",
					},
					{
						Type:   string(hyperv1.InfrastructureReady),
						Status: metav1.ConditionTrue,
						Reason: "InfrastructureReady",
					},
				},
			},
		}

		objectsToAdd = []runtime.Object{cr, hc}
		fakeClient = fake.NewClientBuilder().WithScheme(scheme).WithRuntimeObjects(objectsToAdd...)
	})

	Context("SyncStatusFromHostedCluster", func() {
		It("should skip sync when hostedClusterRef is not set", func() {
			cr.Status.HostedClusterRef = nil
			client := fakeClient.Build()
			syncer = NewStatusSyncer(client)

			result, err := syncer.SyncStatusFromHostedCluster(ctx, cr)

			Expect(err).ToNot(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())
			Expect(result.RequeueAfter).To(BeZero())
		})

		It("should handle missing HostedCluster gracefully", func() {
			// Create CR without corresponding HostedCluster
			crNoHC := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "test-bridge-no-hc",
					Namespace:  "default",
					Generation: 1,
				},
				Status: provisioningv1alpha1.DPFHCPBridgeStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:       "missing-hc",
						Namespace:  "default",
						Kind:       "HostedCluster",
						APIVersion: "hypershift.openshift.io/v1beta1",
					},
				},
			}
			client := fakeClient.WithRuntimeObjects(crNoHC).Build()
			syncer = NewStatusSyncer(client)

			result, err := syncer.SyncStatusFromHostedCluster(ctx, crNoHC)

			// Should not error, just skip sync
			Expect(err).ToNot(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())
		})

		It("should requeue when HostedCluster status is not populated", func() {
			// Create HostedCluster without status conditions
			hcNoStatus := &hyperv1.HostedCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-bridge-no-status",
					Namespace: "default",
				},
				Status: hyperv1.HostedClusterStatus{
					Conditions: nil, // No conditions yet
				},
			}
			crNoStatus := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "test-bridge-no-status",
					Namespace:  "default",
					Generation: 1,
				},
				Status: provisioningv1alpha1.DPFHCPBridgeStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:       "test-bridge-no-status",
						Namespace:  "default",
						Kind:       "HostedCluster",
						APIVersion: "hypershift.openshift.io/v1beta1",
					},
				},
			}
			client := fakeClient.WithRuntimeObjects(crNoStatus, hcNoStatus).Build()
			syncer = NewStatusSyncer(client)

			result, err := syncer.SyncStatusFromHostedCluster(ctx, crNoStatus)

			Expect(err).ToNot(HaveOccurred())
			Expect(result.RequeueAfter).To(Equal(RequeueDelayStatusPending))
		})

		It("should mirror all 7 HostedCluster conditions to DPFHCPBridge", func() {
			// Add the 7 conditions we mirror from the spec
			hc.Status.Conditions = []metav1.Condition{
				{Type: string(hyperv1.HostedClusterAvailable), Status: metav1.ConditionTrue, Reason: "Test"},
				{Type: string(hyperv1.HostedClusterProgressing), Status: metav1.ConditionFalse, Reason: "Test"},
				{Type: string(hyperv1.HostedClusterDegraded), Status: metav1.ConditionFalse, Reason: "Test"},
				{Type: string(hyperv1.ValidReleaseInfo), Status: metav1.ConditionTrue, Reason: "Test"},
				{Type: string(hyperv1.ValidReleaseImage), Status: metav1.ConditionTrue, Reason: "Test"},
				{Type: string(hyperv1.IgnitionEndpointAvailable), Status: metav1.ConditionTrue, Reason: "Test"},
				{Type: string(hyperv1.IgnitionServerValidReleaseInfo), Status: metav1.ConditionTrue, Reason: "Test"},
			}
			client := fakeClient.Build()
			syncer = NewStatusSyncer(client)

			result, err := syncer.SyncStatusFromHostedCluster(ctx, cr)

			Expect(err).ToNot(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify all 7 conditions were mirrored
			expectedConditions := []string{
				provisioningv1alpha1.HostedClusterAvailable,
				provisioningv1alpha1.HostedClusterProgressing,
				provisioningv1alpha1.HostedClusterDegraded,
				provisioningv1alpha1.ValidReleaseInfo,
				provisioningv1alpha1.ValidReleaseImage,
				provisioningv1alpha1.IgnitionEndpointAvailable,
				provisioningv1alpha1.IgnitionServerValidReleaseInfo,
			}

			for _, condType := range expectedConditions {
				cond := meta.FindStatusCondition(cr.Status.Conditions, condType)
				Expect(cond).ToNot(BeNil(), "Condition %s should be mirrored", condType)
			}
		})

		It("should set ObservedGeneration on mirrored conditions", func() {
			client := fakeClient.Build()
			syncer = NewStatusSyncer(client)

			result, err := syncer.SyncStatusFromHostedCluster(ctx, cr)

			Expect(err).ToNot(HaveOccurred())
			Expect(result.Requeue).To(BeFalse())

			// Verify ObservedGeneration is set on mirrored conditions
			availableCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterAvailable)
			Expect(availableCond).ToNot(BeNil())
			Expect(availableCond.ObservedGeneration).To(Equal(cr.Generation))

			progressingCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterProgressing)
			Expect(progressingCond).ToNot(BeNil())
			Expect(progressingCond.ObservedGeneration).To(Equal(cr.Generation))
		})
	})
})
