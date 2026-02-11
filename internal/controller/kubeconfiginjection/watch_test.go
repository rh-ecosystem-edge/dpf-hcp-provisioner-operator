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

package kubeconfiginjection

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/event"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

var _ = Describe("Watch Functions", func() {
	var (
		ctx    context.Context
		scheme *runtime.Scheme
	)

	BeforeEach(func() {
		ctx = context.TODO()
		scheme = runtime.NewScheme()
		Expect(provisioningv1alpha1.AddToScheme(scheme)).To(Succeed())
		Expect(corev1.AddToScheme(scheme)).To(Succeed())
	})

	Describe("isHostedClusterKubeconfigSecret", func() {
		It("should return true for valid HC kubeconfig secret", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-cluster-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			result := isHostedClusterKubeconfigSecret(secret)
			Expect(result).To(BeTrue())
		})

		It("should return false for non-kubeconfig secret", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "random-secret",
					Namespace: "test-ns",
				},
			}

			result := isHostedClusterKubeconfigSecret(secret)
			Expect(result).To(BeFalse())
		})

		It("should return false for secret with partial match", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "admin-kubeconfig-extra",
					Namespace: "test-ns",
				},
			}

			result := isHostedClusterKubeconfigSecret(secret)
			Expect(result).To(BeFalse())
		})

		It("should return false for non-secret object", func() {
			configMap := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			result := isHostedClusterKubeconfigSecret(configMap)
			Expect(result).To(BeFalse())
		})
	})

	Describe("IsHostedClusterKubeconfigSecretPredicate", func() {
		var predicate = IsHostedClusterKubeconfigSecretPredicate()

		It("should accept Create event with valid kubeconfig secret", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-cluster-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			result := predicate.Create(event.CreateEvent{Object: secret})
			Expect(result).To(BeTrue())
		})

		It("should reject Create event with non-kubeconfig secret", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "random-secret",
					Namespace: "test-ns",
				},
			}

			result := predicate.Create(event.CreateEvent{Object: secret})
			Expect(result).To(BeFalse())
		})

		It("should accept Update event with valid kubeconfig secret", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-cluster-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			result := predicate.Update(event.UpdateEvent{ObjectNew: secret})
			Expect(result).To(BeTrue())
		})

		It("should accept Delete event with valid kubeconfig secret", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-cluster-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			result := predicate.Delete(event.DeleteEvent{Object: secret})
			Expect(result).To(BeTrue())
		})

		It("should reject Generic event", func() {
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-cluster-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			result := predicate.Generic(event.GenericEvent{Object: secret})
			Expect(result).To(BeFalse())
		})
	})

	Describe("FindProvisionerForKubeconfigSecret", func() {
		It("should return reconcile request when provisioner exists", func() {
			// Given: DPFHCPProvisioner and its HC kubeconfig secret
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
			}

			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			// When: Mapping function is called
			requests := FindProvisionerForKubeconfigSecret(ctx, fakeClient, secret)

			// Then: Should return reconcile request for the provisioner
			Expect(requests).To(HaveLen(1))
			Expect(requests[0].Name).To(Equal("test-provisioner"))
			Expect(requests[0].Namespace).To(Equal("test-ns"))
		})

		It("should return empty when provisioner not found", func() {
			// Given: HC kubeconfig secret but no corresponding provisioner
			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				Build()

			// When: Mapping function is called
			requests := FindProvisionerForKubeconfigSecret(ctx, fakeClient, secret)

			// Then: Should return empty (no reconciliation needed)
			Expect(requests).To(BeEmpty())
		})

		It("should return empty when provisioner exists but in different namespace", func() {
			// Given: Provisioner in different namespace than secret
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "different-ns",
				},
			}

			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			// When: Mapping function is called
			requests := FindProvisionerForKubeconfigSecret(ctx, fakeClient, secret)

			// Then: Should return empty (namespace mismatch)
			Expect(requests).To(BeEmpty())
		})

		It("should handle name extraction correctly", func() {
			// Given: Provisioner with complex name
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "cluster-with-dashes",
					Namespace: "test-ns",
				},
			}

			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "cluster-with-dashes-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			// When: Mapping function is called
			requests := FindProvisionerForKubeconfigSecret(ctx, fakeClient, secret)

			// Then: Should correctly extract name and find provisioner
			Expect(requests).To(HaveLen(1))
			Expect(requests[0].Name).To(Equal("cluster-with-dashes"))
		})

		It("should return empty for invalid object type", func() {
			// Given: Non-secret object
			configMap := &corev1.ConfigMap{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-admin-kubeconfig",
					Namespace: "test-ns",
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				Build()

			// When: Mapping function is called
			requests := FindProvisionerForKubeconfigSecret(ctx, fakeClient, configMap)

			// Then: Should return empty
			Expect(requests).To(BeEmpty())
		})
	})

	Describe("Watch Integration", func() {
		It("should trigger reconciliation when HC kubeconfig secret is created", func() {
			// Given: DPFHCPProvisioner waiting for kubeconfig
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "test-provisioner",
						Namespace: "test-ns",
					},
				},
			}

			secret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("fake-kubeconfig-data"),
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			predicate := IsHostedClusterKubeconfigSecretPredicate()

			// When: Secret is created
			shouldReconcile := predicate.Create(event.CreateEvent{Object: secret})

			// Then: Predicate should accept
			Expect(shouldReconcile).To(BeTrue())

			// And: Mapping function should return reconcile request
			requests := FindProvisionerForKubeconfigSecret(ctx, fakeClient, secret)
			Expect(requests).To(HaveLen(1))
			Expect(requests[0].NamespacedName).To(Equal(types.NamespacedName{
				Name:      "test-provisioner",
				Namespace: "test-ns",
			}))
		})

		It("should trigger reconciliation when HC kubeconfig secret is updated (drift)", func() {
			// Given: Existing secret and provisioner
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner",
					Namespace: "test-ns",
				},
			}

			oldSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("old-kubeconfig"),
				},
			}

			newSecret := &corev1.Secret{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-provisioner-admin-kubeconfig",
					Namespace: "test-ns",
				},
				Data: map[string][]byte{
					"kubeconfig": []byte("new-rotated-kubeconfig"),
				},
			}

			fakeClient := fake.NewClientBuilder().
				WithScheme(scheme).
				WithObjects(provisioner).
				Build()

			predicate := IsHostedClusterKubeconfigSecretPredicate()

			// When: Secret is updated
			shouldReconcile := predicate.Update(event.UpdateEvent{
				ObjectOld: oldSecret,
				ObjectNew: newSecret,
			})

			// Then: Predicate should accept
			Expect(shouldReconcile).To(BeTrue())

			// And: Mapping function should return reconcile request
			requests := FindProvisionerForKubeconfigSecret(ctx, fakeClient, newSecret)
			Expect(requests).To(HaveLen(1))
			Expect(requests[0].Name).To(Equal("test-provisioner"))
		})
	})
})
