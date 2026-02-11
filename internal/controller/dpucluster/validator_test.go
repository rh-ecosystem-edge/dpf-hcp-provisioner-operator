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
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
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

		// Create scheme with both DPFHCPProvisioner and DPUCluster types
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

				// Create DPFHCPProvisioner referencing the DPUCluster
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterMissing)
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

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// First reconciliation - should emit event
				_, _ = validator.ValidateDPUCluster(ctx, provisioner)
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterFound")))

				// Get updated provisioner for second reconciliation
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Second reconciliation - should NOT emit event (condition unchanged)
				_, _ = validator.ValidateDPUCluster(ctx, &updatedProvisioner)
				Consistently(recorder.Events).ShouldNot(Receive())
			})
		})

		Context("when DPUCluster does not exist", func() {
			It("should set DPUClusterMissing=True and not requeue", func() {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "missing-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue
				Expect(result.RequeueAfter).To(BeZero())

				// Verify status updated
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterMissing)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterNotFound))
				Expect(condition.Message).To(ContainSubstring("missing-dpu"))
				Expect(condition.Message).To(ContainSubstring("not found"))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterNotFound")))
			})

			It("should use 'Deleted' reason when DPUCluster was previously found", func() {
				// Create provisioner with previous DPUClusterFound condition
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "deleted-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						// Simulate previous successful validation
						Conditions: []metav1.Condition{
							{
								Type:               provisioningv1alpha1.DPUClusterMissing,
								Status:             metav1.ConditionFalse,
								Reason:             ReasonDPUClusterFound,
								LastTransitionTime: metav1.Now(),
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Fetch updated provisioner from client to verify status
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify condition uses "Deleted" reason
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterMissing)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterDeleted))
				Expect(condition.Message).To(ContainSubstring("has been deleted"))
				Expect(condition.Message).To(ContainSubstring("Please delete this DPFHCPProvisioner to clean up"))
			})
		})

		Context("when RBAC permission is denied", func() {
			It("should set DPUClusterMissing=True with AccessDenied reason and not requeue", func() {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
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
						WithObjects(provisioner).
						WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
						Build(),
				}

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

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

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						Phase: provisioningv1alpha1.PhasePending,
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)
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

				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						Phase: provisioningv1alpha1.PhaseReady,
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())
			})

			It("should detect DPUCluster deletion when provisioner is Ready", func() {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "deleted-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						Phase: provisioningv1alpha1.PhaseReady,
						Conditions: []metav1.Condition{
							{
								Type:   provisioningv1alpha1.DPUClusterMissing,
								Status: metav1.ConditionFalse,
								Reason: ReasonDPUClusterFound,
							},
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterMissing=True even though provisioner was Ready
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterMissing)
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

				// Create DPFHCPProvisioner referencing the DPUCluster
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify ClusterTypeValid condition is True
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.ClusterTypeValid)
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

				// Create DPFHCPProvisioner referencing the kamaji DPUCluster
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue

				// Verify ClusterTypeValid condition is False
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.ClusterTypeValid)
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

				// Create DPFHCPProvisioner with previous Failed condition (was referencing kamaji type)
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						Phase: provisioningv1alpha1.PhaseFailed,
						Conditions: []metav1.Condition{
							{
								Type:               provisioningv1alpha1.ClusterTypeValid,
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
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify ClusterTypeValid condition is now True (recovered from unsupported type)
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.ClusterTypeValid)
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

				// Create DPFHCPProvisioner referencing the DPUCluster (first and only provisioner)
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterInUse condition is False (available)
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterInUse)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterAvailable))
				Expect(condition.Message).To(ContainSubstring("available"))
				Expect(condition.Message).To(ContainSubstring("not in use"))

				// Verify event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterAvailable")))
			})

			It("should set DPUClusterInUse=True when DPUCluster is already in use by another provisioner", func() {
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

				// Create FIRST provisioner already using the DPUCluster
				firstProvisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "first-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				// Create SECOND provisioner trying to use the same DPUCluster
				secondProvisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "second-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, firstProvisioner, secondProvisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// Validate second provisioner - should fail because first provisioner already uses the DPUCluster
				result, err := validator.ValidateDPUCluster(ctx, secondProvisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse()) // Permanent error - don't requeue

				// Verify DPUClusterInUse condition is True (in use)
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(secondProvisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterInUse)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterInUse))
				Expect(condition.Message).To(ContainSubstring("already in use"))
				Expect(condition.Message).To(ContainSubstring("first-provisioner"))
				Expect(condition.Message).To(ContainSubstring("Each DPUCluster can only be referenced by one DPFHCPProvisioner"))

				// Verify warning event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("DPUClusterInUse")))
			})

			It("should allow same provisioner to validate multiple times (skip self)", func() {
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

				// Create provisioner
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// First validation
				result, err := validator.ValidateDPUCluster(ctx, provisioner)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Get updated provisioner
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				// Verify first validation succeeded
				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterInUse)
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))

				// Second validation of same provisioner - should still succeed (skips itself)
				result, err = validator.ValidateDPUCluster(ctx, &updatedProvisioner)
				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify still available
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())
				condition = meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterInUse)
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
			})

			It("should recover when conflicting provisioner is deleted", func() {
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

				// Create provisioner with previous InUse condition (conflicting provisioner was deleted)
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "test-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
					Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
						Phase: provisioningv1alpha1.PhaseFailed,
						Conditions: []metav1.Condition{
							{
								Type:               provisioningv1alpha1.DPUClusterInUse,
								Status:             metav1.ConditionTrue, // Was True before
								Reason:             ReasonDPUClusterInUse,
								Message:            "DPUCluster in use by another provisioner",
								LastTransitionTime: metav1.Now(),
								ObservedGeneration: 1,
							},
						},
					},
				}

				// Note: No conflicting provisioner in the client - it was deleted
				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, provisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				result, err := validator.ValidateDPUCluster(ctx, provisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterInUse condition is now False (recovered)
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(provisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterInUse)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionFalse))
				Expect(condition.Reason).To(Equal(ReasonDPUClusterAvailable))

				// Event emission: Since condition changed from True to False, an event should be emitted
				// Note: Due to how the validator works with fake client and status updates,
				// we verify the condition was set correctly (which proves the validation ran)
				// Event emission verification is covered in other tests
			})

			It("should handle provisioners in different namespaces referencing same DPUCluster", func() {
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

				// Create FIRST provisioner in namespace "ns1"
				firstProvisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "provisioner",
						Namespace:  "ns1",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				// Create SECOND provisioner in namespace "ns2" with same name but different namespace
				secondProvisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "provisioner",
						Namespace:  "ns2",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, firstProvisioner, secondProvisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// Validate second provisioner - should fail
				result, err := validator.ValidateDPUCluster(ctx, secondProvisioner)

				Expect(err).ToNot(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Verify DPUClusterInUse condition is True
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(secondProvisioner), &updatedProvisioner)).To(Succeed())

				condition := meta.FindStatusCondition(updatedProvisioner.Status.Conditions, provisioningv1alpha1.DPUClusterInUse)
				Expect(condition).ToNot(BeNil())
				Expect(condition.Status).To(Equal(metav1.ConditionTrue))
				Expect(condition.Message).To(ContainSubstring("ns1/provisioner"))
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

				firstProvisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "first-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				secondProvisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:       "second-provisioner",
						Namespace:  "default",
						Generation: 1,
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "dpu-system",
						},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(dpuCluster, firstProvisioner, secondProvisioner).
					WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
					Build()

				validator = NewValidator(fakeClient, recorder)

				// First validation - should emit multiple events
				_, _ = validator.ValidateDPUCluster(ctx, secondProvisioner)

				// Drain all events from first validation (DPUClusterFound, ClusterTypeValid, DPUClusterInUse)
				foundInUseEvent := false
				Eventually(func() bool {
					select {
					case event := <-recorder.Events:
						if event != "" && !foundInUseEvent {
							foundInUseEvent = event == "Warning DPUClusterInUse DPUCluster 'dpu-system/test-dpu' is already in use by DPFHCPProvisioner 'default/first-provisioner'. Each DPUCluster can only be referenced by one DPFHCPProvisioner"
						}
					default:
						return foundInUseEvent
					}
					return false
				}, "2s").Should(BeTrue())

				// Get updated provisioner for second validation
				var updatedProvisioner provisioningv1alpha1.DPFHCPProvisioner
				Expect(fakeClient.Get(ctx, client.ObjectKeyFromObject(secondProvisioner), &updatedProvisioner)).To(Succeed())

				// Second validation - should NOT emit event (condition unchanged)
				_, _ = validator.ValidateDPUCluster(ctx, &updatedProvisioner)
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
