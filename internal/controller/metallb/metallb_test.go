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

package metallb_test

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/metallb"
)

var _ = Describe("MetalLB Manager", func() {
	var (
		ctx        context.Context
		fakeClient client.Client
		recorder   *record.FakeRecorder
		manager    *metallb.MetalLBManager
		scheme     *runtime.Scheme
	)

	BeforeEach(func() {
		ctx = context.TODO()

		// Setup scheme
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(metallbv1beta1.AddToScheme(scheme)).To(Succeed())
		Expect(hyperv1.AddToScheme(scheme)).To(Succeed())

		recorder = record.NewFakeRecorder(100)
	})

	Describe("ConfigureMetalLB", func() {
		Context("when ShouldExposeThroughLoadBalancer is false", func() {
			It("should skip MetalLB configuration", func() {
				// Given: DPFHCPProvisioner with SingleReplica and no VirtualIP
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
						VirtualIP:                      "", // No VIP provided
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				result, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: No error, no requeue
				Expect(err).NotTo(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// No MetalLB resources should be created
				poolList := &metallbv1beta1.IPAddressPoolList{}
				err = fakeClient.List(ctx, poolList)
				Expect(err).NotTo(HaveOccurred())
				Expect(poolList.Items).To(BeEmpty())
			})
		})

		Context("when MetalLB resources do not exist", func() {
			It("should create IPAddressPool and L2Advertisement", func() {
				// Given: DPFHCPProvisioner with HighlyAvailable and VirtualIP
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				result, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: Success
				Expect(err).NotTo(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// IPAddressPool created
				pool := &metallbv1beta1.IPAddressPool{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, pool)
				Expect(err).NotTo(HaveOccurred())
				Expect(pool.Spec.Addresses).To(Equal([]string{"192.168.1.100/32"}))
				Expect(pool.Spec.AllocateTo.Namespaces).To(Equal([]string{"test-ns-test-provisioner"}))
				Expect(*pool.Spec.AutoAssign).To(BeTrue())
				Expect(pool.Labels[common.LabelDPFHCPProvisionerName]).To(Equal("test-provisioner"))
				Expect(pool.Labels[common.LabelDPFHCPProvisionerNamespace]).To(Equal("test-ns"))

				// L2Advertisement created
				advert := &metallbv1beta1.L2Advertisement{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "advertise-test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, advert)
				Expect(err).NotTo(HaveOccurred())
				Expect(advert.Spec.IPAddressPools).To(Equal([]string{"test-provisioner"}))
				Expect(advert.Labels[common.LabelDPFHCPProvisionerName]).To(Equal("test-provisioner"))
				Expect(advert.Labels[common.LabelDPFHCPProvisionerNamespace]).To(Equal("test-ns"))

				// Condition set to True
				cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.MetalLBConfigured)
				Expect(cond).NotTo(BeNil())
				Expect(cond.Status).To(Equal(metav1.ConditionTrue))
				Expect(cond.Reason).To(Equal("MetalLBReady"))

				// Event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("MetalLBConfigured")))
			})
		})

		Context("when MetalLB resources exist with correct spec", func() {
			It("should not update resources", func() {
				// Given: DPFHCPProvisioner with existing MetalLB resources
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				existingPool := &metallbv1beta1.IPAddressPool{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels: map[string]string{
							common.LabelDPFHCPProvisionerName:      "test-provisioner",
							common.LabelDPFHCPProvisionerNamespace: "test-ns",
						},
					},
					Spec: metallbv1beta1.IPAddressPoolSpec{
						Addresses: []string{"192.168.1.100/32"},
						AllocateTo: &metallbv1beta1.ServiceAllocation{
							Namespaces: []string{"clusters-test-provisioner"},
						},
						AutoAssign: func() *bool { b := true; return &b }(),
					},
				}

				existingAdvert := &metallbv1beta1.L2Advertisement{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "advertise-test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels: map[string]string{
							common.LabelDPFHCPProvisionerName:      "test-provisioner",
							common.LabelDPFHCPProvisionerNamespace: "test-ns",
						},
					},
					Spec: metallbv1beta1.L2AdvertisementSpec{
						IPAddressPools: []string{"test-provisioner"},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, existingPool, existingAdvert).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				result, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: Success, no updates
				Expect(err).NotTo(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// Condition set to True
				cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.MetalLBConfigured)
				Expect(cond).NotTo(BeNil())
				Expect(cond.Status).To(Equal(metav1.ConditionTrue))
			})
		})

		Context("when MetalLB resources exist with incorrect spec (drift)", func() {
			It("should update IPAddressPool to correct drift", func() {
				// Given: DPFHCPProvisioner with drifted IPAddressPool
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				driftedPool := &metallbv1beta1.IPAddressPool{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels: map[string]string{
							common.LabelDPFHCPProvisionerName:      "test-provisioner",
							common.LabelDPFHCPProvisionerNamespace: "test-ns",
						},
					},
					Spec: metallbv1beta1.IPAddressPoolSpec{
						Addresses: []string{"192.168.1.200/32"}, // Wrong IP!
						AllocateTo: &metallbv1beta1.ServiceAllocation{
							Namespaces: []string{"clusters-test-provisioner"},
						},
						AutoAssign: func() *bool { b := true; return &b }(),
					},
				}

				existingAdvert := &metallbv1beta1.L2Advertisement{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "advertise-test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels: map[string]string{
							common.LabelDPFHCPProvisionerName:      "test-provisioner",
							common.LabelDPFHCPProvisionerNamespace: "test-ns",
						},
					},
					Spec: metallbv1beta1.L2AdvertisementSpec{
						IPAddressPools: []string{"test-provisioner"},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, driftedPool, existingAdvert).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				result, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: Success
				Expect(err).NotTo(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// IPAddressPool corrected
				pool := &metallbv1beta1.IPAddressPool{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, pool)
				Expect(err).NotTo(HaveOccurred())
				Expect(pool.Spec.Addresses).To(Equal([]string{"192.168.1.100/32"}))

				// Drift correction event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("MetalLBDriftCorrected")))
			})

			It("should update L2Advertisement to correct drift", func() {
				// Given: DPFHCPProvisioner with drifted L2Advertisement
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				existingPool := &metallbv1beta1.IPAddressPool{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels: map[string]string{
							common.LabelDPFHCPProvisionerName:      "test-provisioner",
							common.LabelDPFHCPProvisionerNamespace: "test-ns",
						},
					},
					Spec: metallbv1beta1.IPAddressPoolSpec{
						Addresses: []string{"192.168.1.100/32"},
						AllocateTo: &metallbv1beta1.ServiceAllocation{
							Namespaces: []string{"clusters-test-provisioner"},
						},
						AutoAssign: func() *bool { b := true; return &b }(),
					},
				}

				driftedAdvert := &metallbv1beta1.L2Advertisement{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "advertise-test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels: map[string]string{
							common.LabelDPFHCPProvisionerName:      "test-provisioner",
							common.LabelDPFHCPProvisionerNamespace: "test-ns",
						},
					},
					Spec: metallbv1beta1.L2AdvertisementSpec{
						IPAddressPools: []string{"wrong-pool"}, // Wrong pool reference!
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, existingPool, driftedAdvert).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				result, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: Success
				Expect(err).NotTo(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				// L2Advertisement corrected
				advert := &metallbv1beta1.L2Advertisement{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "advertise-test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, advert)
				Expect(err).NotTo(HaveOccurred())
				Expect(advert.Spec.IPAddressPools).To(Equal([]string{"test-provisioner"}))

				// Drift correction event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("MetalLBDriftCorrected")))
			})
		})
	})

	Describe("Cleanup Handler", func() {
		Context("when both MetalLB resources exist", func() {
			It("should delete IPAddressPool and L2Advertisement", func() {
				// Given: DPFHCPProvisioner with existing MetalLB resources
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				existingPool := &metallbv1beta1.IPAddressPool{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
					},
					Spec: metallbv1beta1.IPAddressPoolSpec{
						Addresses: []string{"192.168.1.100/32"},
					},
				}

				existingAdvert := &metallbv1beta1.L2Advertisement{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "advertise-test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
					},
					Spec: metallbv1beta1.L2AdvertisementSpec{
						IPAddressPools: []string{"test-provisioner"},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, existingPool, existingAdvert).
					Build()

				handler := metallb.NewCleanupHandler(fakeClient, recorder)

				// When: Cleanup is called first time
				err := handler.Cleanup(ctx, provisioner)

				// Then: Returns error because it initiated IPAddressPool deletion
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("waiting for IPAddressPool deletion"))

				// When: Cleanup is called second time (IPAddressPool deleted, now deletes L2Advertisement)
				err = handler.Cleanup(ctx, provisioner)

				// Then: Returns error because it initiated L2Advertisement deletion
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("waiting for L2Advertisement deletion"))

				// When: Cleanup is called third time (all resources deleted)
				err = handler.Cleanup(ctx, provisioner)

				// Then: Success - all resources gone
				Expect(err).NotTo(HaveOccurred())

				// Event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("MetalLBCleanupComplete")))
			})
		})

		Context("when MetalLB resources don't exist", func() {
			It("should return nil (idempotent)", func() {
				// Given: DPFHCPProvisioner with no MetalLB resources
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					Build()

				handler := metallb.NewCleanupHandler(fakeClient, recorder)

				// When: Cleanup is called
				err := handler.Cleanup(ctx, provisioner)

				// Then: Success (idempotent)
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("when only IPAddressPool exists", func() {
			It("should delete IPAddressPool and return success", func() {
				// Given: DPFHCPProvisioner with only IPAddressPool
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
				}

				existingPool := &metallbv1beta1.IPAddressPool{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, existingPool).
					Build()

				handler := metallb.NewCleanupHandler(fakeClient, recorder)

				// When: Cleanup is called first time
				err := handler.Cleanup(ctx, provisioner)

				// Then: Returns error (waiting for deletion)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("waiting for IPAddressPool deletion"))

				// When: Called again (fake client already deleted the resource)
				err = handler.Cleanup(ctx, provisioner)

				// Then: Success
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("when only L2Advertisement exists", func() {
			It("should delete L2Advertisement and return success", func() {
				// Given: DPFHCPProvisioner with only L2Advertisement
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
				}

				existingAdvert := &metallbv1beta1.L2Advertisement{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "advertise-test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, existingAdvert).
					Build()

				handler := metallb.NewCleanupHandler(fakeClient, recorder)

				// When: Cleanup is called first time
				err := handler.Cleanup(ctx, provisioner)

				// Then: Returns error (waiting for deletion)
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("waiting for L2Advertisement deletion"))

				// When: Called again (fake client already deleted the resource)
				err = handler.Cleanup(ctx, provisioner)

				// Then: Success
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("when cleanup is called multiple times (idempotency)", func() {
			It("should succeed on subsequent calls", func() {
				// Given: DPFHCPProvisioner with no resources
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					Build()

				handler := metallb.NewCleanupHandler(fakeClient, recorder)

				// When: Cleanup is called multiple times
				err := handler.Cleanup(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				err = handler.Cleanup(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				err = handler.Cleanup(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				// Then: All succeed (idempotent)
			})
		})
	})

	Describe("Helper Functions", func() {
		Context("buildIPAddressPool", func() {
			It("should build correct IPAddressPool spec", func() {
				// Given: DPFHCPProvisioner
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						VirtualIP: "10.0.0.100",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: We would call buildIPAddressPool (tested indirectly through ConfigureMetalLB)
				// Create the resource and verify
				provisioner.Spec.ControlPlaneAvailabilityPolicy = hyperv1.HighlyAvailable
				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)
				_, err := manager.ConfigureMetalLB(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				// Then: Verify the created pool matches expected spec
				pool := &metallbv1beta1.IPAddressPool{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, pool)
				Expect(err).NotTo(HaveOccurred())
				Expect(pool.Spec.Addresses).To(Equal([]string{"10.0.0.100/32"}))
				Expect(pool.Spec.AllocateTo.Namespaces).To(Equal([]string{"test-ns-test-provisioner"}))
				Expect(*pool.Spec.AutoAssign).To(BeTrue())
			})
		})

		Context("buildL2Advertisement", func() {
			It("should build correct L2Advertisement spec", func() {
				// Given: DPFHCPProvisioner
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "10.0.0.100",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB creates the advertisement
				_, err := manager.ConfigureMetalLB(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				// Then: Verify the created advertisement matches expected spec
				advert := &metallbv1beta1.L2Advertisement{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "advertise-test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, advert)
				Expect(err).NotTo(HaveOccurred())
				Expect(advert.Spec.IPAddressPools).To(Equal([]string{"test-provisioner"}))
			})
		})
	})

	Describe("Edge Cases", func() {
		Context("when DPFHCPProvisioner has SingleReplica with VirtualIP", func() {
			It("should configure MetalLB", func() {
				// Given: SingleReplica with VirtualIP provided
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
						VirtualIP:                      "192.168.1.100",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				result, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: MetalLB resources created
				Expect(err).NotTo(HaveOccurred())
				Expect(result.Requeue).To(BeFalse())

				pool := &metallbv1beta1.IPAddressPool{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, pool)
				Expect(err).NotTo(HaveOccurred())
			})
		})

		Context("when IPAddressPool exists without ownership labels (ownership conflict)", func() {
			It("should return error and not take ownership", func() {
				// Given: IPAddressPool exists but lacks ownership labels (created by user/other operator)
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				unownedPool := &metallbv1beta1.IPAddressPool{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels:    map[string]string{}, // Missing ownership labels!
					},
					Spec: metallbv1beta1.IPAddressPoolSpec{
						Addresses: []string{"192.168.1.100/32"},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, unownedPool).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				_, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: Error returned - won't take ownership
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("not owned by DPFHCPProvisioner"))
				Expect(err.Error()).To(ContainSubstring("missing ownership labels"))

				// Pool remains unchanged (labels not added)
				pool := &metallbv1beta1.IPAddressPool{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, pool)
				Expect(err).NotTo(HaveOccurred())
				Expect(pool.Labels).To(BeEmpty()) // Labels still empty - didn't claim ownership
			})
		})

		Context("when L2Advertisement exists without ownership labels (ownership conflict)", func() {
			It("should return error and not take ownership", func() {
				// Given: L2Advertisement exists but lacks ownership labels
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				// IPAddressPool with correct ownership (so we get past that check)
				ownedPool := &metallbv1beta1.IPAddressPool{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels: map[string]string{
							common.LabelDPFHCPProvisionerName:      "test-provisioner",
							common.LabelDPFHCPProvisionerNamespace: "test-ns",
						},
					},
					Spec: metallbv1beta1.IPAddressPoolSpec{
						Addresses: []string{"192.168.1.100/32"},
						AllocateTo: &metallbv1beta1.ServiceAllocation{
							Namespaces: []string{"clusters-test-provisioner"},
						},
						AutoAssign: func() *bool { b := true; return &b }(),
					},
				}

				// L2Advertisement WITHOUT ownership labels
				unownedAdvert := &metallbv1beta1.L2Advertisement{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "advertise-test-provisioner",
						Namespace: common.OpenshiftOperatorsNamespace,
						Labels:    map[string]string{}, // Missing ownership labels!
					},
					Spec: metallbv1beta1.L2AdvertisementSpec{
						IPAddressPools: []string{"test-provisioner"},
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner, ownedPool, unownedAdvert).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called
				_, err := manager.ConfigureMetalLB(ctx, provisioner)

				// Then: Error returned - won't take ownership
				Expect(err).To(HaveOccurred())
				Expect(err.Error()).To(ContainSubstring("not owned by DPFHCPProvisioner"))
				Expect(err.Error()).To(ContainSubstring("missing ownership labels"))

				// Advertisement remains unchanged
				advert := &metallbv1beta1.L2Advertisement{}
				err = fakeClient.Get(ctx, types.NamespacedName{
					Name:      "advertise-test-provisioner",
					Namespace: common.OpenshiftOperatorsNamespace,
				}, advert)
				Expect(err).NotTo(HaveOccurred())
				Expect(advert.Labels).To(BeEmpty()) // Labels still empty
			})
		})
	})

	Describe("Status Condition Management", func() {
		Context("when MetalLB configuration succeeds", func() {
			It("should set MetalLBConfigured condition to True with correct reason", func() {
				// Given: Successful MetalLB configuration
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB succeeds
				_, err := manager.ConfigureMetalLB(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				// Then: Condition is True with MetalLBReady reason
				cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.MetalLBConfigured)
				Expect(cond).NotTo(BeNil())
				Expect(cond.Status).To(Equal(metav1.ConditionTrue))
				Expect(cond.Reason).To(Equal("MetalLBReady"))
				Expect(cond.Message).To(Equal("MetalLB configured successfully"))
				Expect(cond.ObservedGeneration).To(Equal(provisioner.Generation))
			})
		})

		Context("when condition changes", func() {
			It("should emit event only when condition actually changes", func() {
				// Given: DPFHCPProvisioner
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
						VirtualIP:                      "192.168.1.100",
					},
				}

				fakeClient = fake.NewClientBuilder().
					WithScheme(scheme).
					WithObjects(provisioner).
					WithStatusSubresource(provisioner).
					Build()

				manager = metallb.NewMetalLBManager(fakeClient, recorder)

				// When: ConfigureMetalLB is called first time
				_, err := manager.ConfigureMetalLB(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				// Then: Event emitted
				Eventually(recorder.Events).Should(Receive(ContainSubstring("MetalLBConfigured")))

				// When: ConfigureMetalLB is called again (no change)
				_, err = manager.ConfigureMetalLB(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred())

				// Then: No additional event (condition didn't change)
				Consistently(recorder.Events).ShouldNot(Receive())
			})
		})
	})
})
