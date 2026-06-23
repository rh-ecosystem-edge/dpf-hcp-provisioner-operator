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
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

var _ = Describe("NodePool Builder", func() {
	var (
		npm *NodePoolManager
		cr  *provisioningv1alpha1.DPFHCPProvisioner
	)

	BeforeEach(func() {
		npm = &NodePoolManager{}
		cr = &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: "default",
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
			},
		}
	})

	Context("Basic NodePool Fields", func() {
		It("should set correct metadata", func() {
			np := npm.buildNodePool(cr)

			Expect(np.Name).To(Equal("test-provisioner"))
			Expect(np.Namespace).To(Equal("default"))
		})

		It("should set cluster name to match HostedCluster", func() {
			np := npm.buildNodePool(cr)

			Expect(np.Spec.ClusterName).To(Equal("test-provisioner"))
		})

		It("should set replicas to 0", func() {
			np := npm.buildNodePool(cr)

			Expect(np.Spec.Replicas).ToNot(BeNil())
			Expect(*np.Spec.Replicas).To(Equal(int32(0)))
		})

		It("should set platform to None", func() {
			np := npm.buildNodePool(cr)

			Expect(np.Spec.Platform.Type).To(Equal(hyperv1.NonePlatform))
		})

		It("should set release image from DPFHCPProvisioner spec", func() {
			np := npm.buildNodePool(cr)

			Expect(np.Spec.Release.Image).To(Equal(cr.Spec.OCPReleaseImage))
		})
	})

	Context("Management Configuration", func() {
		It("should set upgrade type to Replace", func() {
			np := npm.buildNodePool(cr)

			Expect(np.Spec.Management.UpgradeType).To(Equal(hyperv1.UpgradeTypeReplace))
		})
	})
})

var _ = Describe("NodePool Upgrade", func() {
	const (
		oldImage = "quay.io/openshift-release-dev/ocp-release:4.18.0-multi"
		newImage = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi"
	)

	var (
		ctx    context.Context
		scheme *runtime.Scheme
	)

	BeforeEach(func() {
		ctx = context.Background()
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(hyperv1.AddToScheme(scheme)).To(Succeed())
	})

	It("should update NodePool release image when spec changes", func() {
		cr := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: "default",
				UID:       "test-uid",
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				OCPReleaseImage: newImage,
			},
		}

		existingNP := &hyperv1.NodePool{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: "default",
				OwnerReferences: []metav1.OwnerReference{{
					APIVersion:         "provisioning.dpu.hcp.io/v1alpha1",
					Kind:               "DPFHCPProvisioner",
					Name:               "test-provisioner",
					UID:                "test-uid",
					Controller:         ptr.To(true),
					BlockOwnerDeletion: ptr.To(true),
				}},
			},
			Spec: hyperv1.NodePoolSpec{
				Release: hyperv1.Release{Image: oldImage},
			},
		}

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithObjects(existingNP).
			Build()

		npm := NewNodePoolManager(fakeClient, scheme)
		result, err := npm.CreateNodePool(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		updatedNP := &hyperv1.NodePool{}
		Expect(fakeClient.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: "default"}, updatedNP)).To(Succeed())
		Expect(updatedNP.Spec.Release.Image).To(Equal(newImage))
	})

	It("should not update NodePool when release image is unchanged", func() {
		cr := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: "default",
				UID:       "test-uid",
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				OCPReleaseImage: oldImage,
			},
		}

		existingNP := &hyperv1.NodePool{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: "default",
				OwnerReferences: []metav1.OwnerReference{{
					APIVersion:         "provisioning.dpu.hcp.io/v1alpha1",
					Kind:               "DPFHCPProvisioner",
					Name:               "test-provisioner",
					UID:                "test-uid",
					Controller:         ptr.To(true),
					BlockOwnerDeletion: ptr.To(true),
				}},
			},
			Spec: hyperv1.NodePoolSpec{
				Release: hyperv1.Release{Image: oldImage},
			},
		}

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithObjects(existingNP).
			Build()

		npm := NewNodePoolManager(fakeClient, scheme)
		result, err := npm.CreateNodePool(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		unchangedNP := &hyperv1.NodePool{}
		Expect(fakeClient.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: "default"}, unchangedNP)).To(Succeed())
		Expect(unchangedNP.Spec.Release.Image).To(Equal(oldImage))
	})
})
