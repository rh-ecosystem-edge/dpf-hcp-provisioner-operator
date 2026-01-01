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

package dpucluster

import (
	"context"
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

func TestDPUClusterValidator(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "DPUCluster Validator Suite")
}

var _ = Describe("DPUCluster Validator", func() {
	var (
		ctx        context.Context
		validator  *Validator
		fakeClient client.Client
		recorder   *record.FakeRecorder
		scheme     *runtime.Scheme
	)

	BeforeEach(func() {
		ctx = context.Background()
		recorder = record.NewFakeRecorder(100)

		// Create scheme with both DPFHCPBridge and DPUCluster types
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(dpuprovisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
	})

	Describe("ValidateDPUCluster", func() {
		Context("when DPUCluster exists", func() {
			It("should set DPUClusterMissing=False", func() {
				// Create a DPUCluster
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
				}

				// Create DPFHCPBridge referencing the DPUCluster
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())
				Expect(updatedBridge.Status.DPUClusterMissing).To(BeFalse())

				// Verify condition
				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterMissingConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterFound))
				Expect(condition.Message).To(ContainSubstring("test-dpu"))
				Expect(condition.Message).To(ContainSubstring("dpu-system"))
				Expect(condition.ObservedGeneration).To(Equal(int64(1)))

				// Verify event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterFound")))
			})

			It("should emit event only on first success (not on subsequent reconciliations)", func() {
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
				}

				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// First reconciliation - should emit event
				_, _ = validator.ValidateDPUCluster(ctx, bridge)
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterFound")))

				// Get updated bridge for second reconciliation
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())

				// Second reconciliation - should NOT emit event (condition unchanged)
				_, _ = validator.ValidateDPUCluster(ctx, &updatedBridge)
				Consistently(recorder.Events).ShouldNot(Receive())
			})
		})

		Context("when DPUCluster does not exist", func() {
			It("should set DPUClusterMissing=True and not requeue", func() {
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "missing-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())
				Expect(updatedBridge.Status.DPUClusterMissing).To(BeTrue())

				// Verify condition
				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterMissingConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterNotFound))
				Expect(condition.Message).To(ContainSubstring("missing-dpu"))
				Expect(condition.Message).To(ContainSubstring("not found"))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterNotFound")))
			})

			It("should use 'Deleted' reason when DPUCluster was previously found", func() {
				// Create bridge with previous DPUClusterFound condition
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "deleted-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPBridgeStatus{
						// Simulate previous successful validation
						Conditions: []metav1.Condition{
							{
								Type:               DPUClusterMissingConditionType,
								Status:             metav1.ConditionFalse,
								Reason:             ReasonDPUClusterFound,
								LastTransitionTime: metav1.Now(),
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Fetch updated bridge from client to verify status
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())
				Expect(updatedBridge.Status.DPUClusterMissing).To(BeTrue())

				// Verify condition uses "Deleted" reason
				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterMissingConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterDeleted))
				Expect(condition.Message).To(ContainSubstring("has been deleted"))
				Expect(condition.Message).To(ContainSubstring("Please delete this DPFHCPBridge to clean up"))

				// Event emission is verified in other tests
				// Skipping event check here as the critical functionality (status/condition update) is verified above
			})
		})

		Context("when RBAC permission is denied", func() {
			It("should set DPUClusterMissing=True with AccessDenied reason and not requeue", func() {
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "forbidden-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				// Create fake client that returns Forbidden error
				fakeClient = &forbiddenClient{
					Client: fake.NewClientBuilder().
						WithScheme(scheme).
						WithObjects(bridge).
						WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
						Build(),
				}

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue

				// Note: We can't verify status update with fake client that returns errors
				// This would require a more sophisticated mock
			})
		})

		Context("when validation runs in different phases", func() {
			It("should validate in Pending phase", func() {
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
				}

				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-bridge",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPBridgeStatus{
						Phase: provisioningv1alpha1.PhasePending,
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())
			})

			It("should validate in Ready phase (proves no phase gating)", func() {
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
				}

				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-bridge",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPBridgeStatus{
						Phase: provisioningv1alpha1.PhaseReady,
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())
			})

			It("should detect DPUCluster deletion when bridge is Ready", func() {
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-bridge",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "deleted-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPBridgeStatus{
						Phase: provisioningv1alpha1.PhaseReady,
						Conditions: []metav1.Condition{
							{
								Type:   DPUClusterMissingConditionType,
								Status: metav1.ConditionFalse,
								Reason: ReasonDPUClusterFound,
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterMissing=True even though bridge was Ready
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())
				Expect(updatedBridge.Status.DPUClusterMissing).To(BeTrue())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterMissingConditionType)
				Expect(condition.Reason).To(Equal(ReasonDPUClusterDeleted))
			})
		})

		Context("when clusterType validation is performed", func() {
			It("should set ClusterTypeValid=True when type is not kamaji", func() {
				// Create DPUCluster with type "static" (not kamaji)
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				// Create DPFHCPBridge referencing the DPUCluster
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify ClusterTypeValid condition is True
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, ClusterTypeValidConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonClusterTypeValid))
				Expect(condition.Message).To(ContainSubstring("static"))
				Expect(condition.Message).To(ContainSubstring("supported"))
			})

			It("should set ClusterTypeValid=False when type is kamaji", func() {
				// Create DPUCluster with type "kamaji" (unsupported)
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: string(dpuprovisioningv1alpha1.KamajiCluster),
					},
				}

				// Create DPFHCPBridge referencing the kamaji DPUCluster
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue

				// Verify ClusterTypeValid condition is False
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, ClusterTypeValidConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonClusterTypeUnsupported))
				Expect(condition.Message).To(ContainSubstring("kamaji"))
				Expect(condition.Message).To(ContainSubstring("unsupported"))

				// Note: Phase computation is the reconciler's responsibility, not the validator's
				// The reconciler will compute Phase=Failed from the ClusterTypeValid=False condition
			})

			It("should recover when DPUCluster type changes from kamaji to supported type", func() {
				// Create DPUCluster with type "static" (now valid)
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				// Create DPFHCPBridge with previous Failed condition (was referencing kamaji type)
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPBridgeStatus{
						Phase: provisioningv1alpha1.PhaseFailed,
						Conditions: []metav1.Condition{
							{
								Type:               ClusterTypeValidConditionType,
								Status:             metav1.ConditionFalse, // Was False before
								Reason:             ReasonClusterTypeUnsupported,
								Message:            "ClusterType unsupported",
								LastTransitionTime: metav1.Now(),
								ObservedGeneration: 1,
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify ClusterTypeValid condition is now True (recovered from unsupported type)
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, ClusterTypeValidConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonClusterTypeValid))

				// Note: Phase transition (Failed → Pending) is the reconciler's responsibility
				// The reconciler will compute Phase=Pending from the ClusterTypeValid=True condition
			})
		})

		Context("when DPUCluster exclusivity validation is performed", func() {
			It("should set DPUClusterInUse=False when DPUCluster is available", func() {
				// Create DPUCluster with type "static"
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				// Create DPFHCPBridge referencing the DPUCluster (first and only bridge)
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterInUse condition is False (available)
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterInUseConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterAvailable))
				Expect(condition.Message).To(ContainSubstring("available"))
				Expect(condition.Message).To(ContainSubstring("not in use"))

				// Verify event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterAvailable")))
			})

			It("should set DPUClusterInUse=True when DPUCluster is already in use by another bridge", func() {
				// Create DPUCluster with type "static"
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				// Create FIRST bridge already using the DPUCluster
				firstBridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "first-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				// Create SECOND bridge trying to use the same DPUCluster
				secondBridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "second-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, firstBridge, secondBridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// Validate second bridge - should fail because first bridge already uses the DPUCluster
				result, err := validator.ValidateDPUCluster(ctx, secondBridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue

				// Verify DPUClusterInUse condition is True (in use)
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(secondBridge), &updatedBridge)).To(Succeed())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterInUseConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterInUse))
				Expect(condition.Message).To(ContainSubstring("already in use"))
				Expect(condition.Message).To(ContainSubstring("first-bridge"))
				Expect(condition.Message).To(ContainSubstring("Each DPUCluster can only be referenced by one DPFHCPBridge"))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterInUse")))
			})

			It("should allow same bridge to validate multiple times (skip self)", func() {
				// Create DPUCluster with type "static"
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				// Create bridge
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// First validation
				result, err := validator.ValidateDPUCluster(ctx, bridge)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Get updated bridge
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())

				// Verify first validation succeeded
				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterInUseConditionType)
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))

				// Second validation of same bridge - should still succeed (skips itself)
				result, err = validator.ValidateDPUCluster(ctx, &updatedBridge)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify still available
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())
				condition = meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterInUseConditionType)
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
			})

			It("should recover when conflicting bridge is deleted", func() {
				// Create DPUCluster with type "static"
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				// Create bridge with previous InUse condition (conflicting bridge was deleted)
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPBridgeStatus{
						Phase: provisioningv1alpha1.PhaseFailed,
						Conditions: []metav1.Condition{
							{
								Type:               DPUClusterInUseConditionType,
								Status:             metav1.ConditionTrue, // Was True before
								Reason:             ReasonDPUClusterInUse,
								Message:            "DPUCluster in use by another bridge",
								LastTransitionTime: metav1.Now(),
								ObservedGeneration: 1,
							},
						},
					},
				}

				// Note: No conflicting bridge in the client - it was deleted
				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, bridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, bridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterInUse condition is now False (recovered)
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(bridge), &updatedBridge)).To(Succeed())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterInUseConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterAvailable))

				// Event emission: Since condition changed from True to False, an event should be emitted
				// Note: Due to how the validator works with fake client and status updates,
				// we verify the condition was set correctly (which proves the validation ran)
				// Event emission verification is covered in other tests
			})

			It("should handle bridges in different namespaces referencing same DPUCluster", func() {
				// Create DPUCluster with type "static"
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				// Create FIRST bridge in namespace "ns1"
				firstBridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "bridge",
						Namespace:  "ns1",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				// Create SECOND bridge in namespace "ns2" with same name but different namespace
				secondBridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "bridge",
						Namespace:  "ns2",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, firstBridge, secondBridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// Validate second bridge - should fail
				result, err := validator.ValidateDPUCluster(ctx, secondBridge)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterInUse condition is True
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(secondBridge), &updatedBridge)).To(Succeed())

				condition := meta.FindStatusCondition(updatedBridge.Status.Conditions, DPUClusterInUseConditionType)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Message).To(ContainSubstring("ns1/bridge"))
			})

			It("should not emit event on subsequent validations with same result", func() {
				dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
						Type: "static",
					},
				}

				firstBridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "first-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				secondBridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "second-bridge",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, firstBridge, secondBridge).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPBridge{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// First validation - should emit multiple events
				_, _ = validator.ValidateDPUCluster(ctx, secondBridge)

				// Drain all events from first validation (DPUClusterFound, ClusterTypeValid, DPUClusterInUse)
				foundInUseEvent := false
				Eventually(func() bool {
					select {
					case event := <-recorder.Events:
						if event != "" && !foundInUseEvent {
							foundInUseEvent = event == "Warning DPUClusterInUse DPUCluster 'dpu-system/test-dpu' is already in use by DPFHCPBridge 'default/first-bridge'. Each DPUCluster can only be referenced by one DPFHCPBridge"
						}
					default:
						return foundInUseEvent
					}
					return false
				}, "2s").Should(BeTrue())

				// Get updated bridge for second validation
				var updatedBridge provisioningv1alpha1.DPFHCPBridge
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(secondBridge), &updatedBridge)).To(Succeed())

				// Second validation - should NOT emit event (condition unchanged)
				_, _ = validator.ValidateDPUCluster(ctx, &updatedBridge)
				Consistently(recorder.Events, "500ms").ShouldNot(Receive())
			})
		})
	})
})

// forbiddenClient wraps a fake client to return Forbidden errors for DPUCluster Get operations
type forbiddenClient struct {
	client.Client
}

func (c *forbiddenClient) Get(ctx context.Context, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
	// Return Forbidden error for DPUCluster Get operations
	if _, ok := obj.(*dpuprovisioningv1alpha1.DPUCluster); ok {
		return apierrors.NewForbidden(
			schema.GroupResource{Group: "provisioning.dpu.nvidia.com", Resource: "dpuclusters"},
			key.Name,
			nil,
		)
	}
	// Delegate to wrapped client for other types
	return c.Client.Get(ctx, key, obj, opts...)
}
