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
	"fmt"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// CRD Schema Validation Tests
// These tests verify API-level validation (CEL rules, regex patterns, etc.)
// They run with the controller but test CRD validation which happens at the API server level
var _ = Describe("DPFHCPProvisioner CRD Schema Validation Tests", func() {
	ctx := context.Background()

	AfterEach(func() {
		// Clean up any resources created during tests
		provisionerList := &provisioningv1alpha1.DPFHCPProvisionerList{}
		_ = k8sClient.List(ctx, provisionerList)
		for _, provisioner := range provisionerList.Items {
			_ = k8sClient.Delete(ctx, &provisioner)
		}
	})

	Context("BaseDomain Validation", func() {
		It("should accept valid baseDomain patterns", func() {
			validDomains := []string{
				"clusters.example.com",
				"test.example.com",
				"sub.domain.example.com",
				"a-b.example.com",
				"test.co",
				"long-subdomain-name.example.org",
			}

			for i, domain := range validDomains {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      fmt.Sprintf("valid-domain-test-%d", i),
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "default",
						},
						BaseDomain:                     domain,
						OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
						SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
						PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
						EtcdStorageClass:               "test-storage-class",
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					},
				}
				err := k8sClient.Create(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred(), "Domain %q should be valid", domain)
				_ = k8sClient.Delete(ctx, provisioner)
			}
		})

		It("should reject invalid baseDomain patterns", func() {
			invalidDomains := []string{
				"UPPERCASE.example.com", // Uppercase not allowed
				"example",               // Missing TLD
				"example.",              // Trailing dot
				".example.com",          // Leading dot
				"example..com",          // Consecutive dots
				"example .com",          // Space not allowed
				"-example.com",          // Leading hyphen
				"example-.com",          // Trailing hyphen
				"192.168.1.1",           // IP address not allowed
				"example.com-",          // Trailing hyphen in label
				"test_domain.com",       // Underscore not allowed
				"ex",                    // Too short (less than 4 chars)
			}

			for _, domain := range invalidDomains {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "invalid-domain-test",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "default",
						},
						BaseDomain:                     domain,
						OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
						SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
						PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
						EtcdStorageClass:               "test-storage-class",
						ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					},
				}
				err := k8sClient.Create(ctx, provisioner)
				Expect(err).To(HaveOccurred(), "Domain %q should be invalid", domain)
				_ = k8sClient.Delete(ctx, provisioner)
			}
		})

		It("should reject baseDomain exceeding max length (253 chars)", func() {
			// Create a domain name with >253 characters
			longDomain := "very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.example.com"

			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "long-domain-test",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:       longDomain,
					OCPReleaseImage:  "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:  corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:    corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass: "test-storage-class",
				},
			}
			err := k8sClient.Create(ctx, provisioner)
			Expect(err).To(HaveOccurred())
		})

		It("should reject CR missing required baseDomain field", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "missing-required",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "dpu-system",
					},
					// BaseDomain missing (required field)
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).To(HaveOccurred(), "Missing required field should be rejected by CRD validation")
		})
	})

	Context("Enum Validation", func() {
		It("should accept valid controlPlaneAvailabilityPolicy values", func() {
			validPolicies := []hyperv1.AvailabilityPolicy{hyperv1.SingleReplica, hyperv1.HighlyAvailable}

			for i, policy := range validPolicies {
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
					ObjectMeta: metav1.ObjectMeta{
						Name:      fmt.Sprintf("valid-cp-policy-%d", i),
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
						DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
							Name:      "test-dpu",
							Namespace: "default",
						},
						BaseDomain:                     "test.example.com",
						OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
						SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
						PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
						EtcdStorageClass:               "test-storage-class",
						ControlPlaneAvailabilityPolicy: policy,
					},
				}
				// Add VIP if HighlyAvailable
				if policy == hyperv1.HighlyAvailable {
					provisioner.Spec.VirtualIP = "192.168.1.100"
				}
				err := k8sClient.Create(ctx, provisioner)
				Expect(err).NotTo(HaveOccurred(), "Policy %q should be valid", policy)
				_ = k8sClient.Delete(ctx, provisioner)
			}
		})

		It("should reject invalid controlPlaneAvailabilityPolicy values", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "invalid-cp-policy",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: "InvalidValue",
				},
			}
			err := k8sClient.Create(ctx, provisioner)
			Expect(err).To(HaveOccurred())
		})

	})

	Context("Default Values Application", func() {
		It("should apply default values when fields are omitted", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-defaults",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:      "test.example.com",
					OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef: corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:   corev1.LocalObjectReference{Name: "test-pull-secret"},
					// Optional fields omitted - should get defaults
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).To(HaveOccurred(), "Should reject HighlyAvailable (default) without VIP")

			// Try again with VIP for HighlyAvailable default
			provisioner.Name = "test-defaults-with-vip"
			provisioner.Spec.VirtualIP = "192.168.1.100"
			err = k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())

			// Retrieve the created resource to verify defaults were applied
			created := &provisioningv1alpha1.DPFHCPProvisioner{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: "test-defaults-with-vip", Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			// Verify defaults were applied by the API server
			Expect(created.Spec.ControlPlaneAvailabilityPolicy).To(Equal(hyperv1.HighlyAvailable))

			_ = k8sClient.Delete(ctx, provisioner)
		})
	})

	Context("VIP Requirement based on ControlPlaneAvailabilityPolicy", func() {
		It("should reject HighlyAvailable without VIP", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "ha-without-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					// VirtualIP not set - should be rejected
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).To(HaveOccurred(), "Should reject HighlyAvailable without VIP")
			Expect(err.Error()).To(ContainSubstring("virtualIP is required"))
		})

		It("should accept HighlyAvailable with VIP", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "ha-with-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					VirtualIP:                      "192.168.1.100",
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred(), "Should accept HighlyAvailable with VIP")
			_ = k8sClient.Delete(ctx, provisioner)
		})

		It("should accept SingleReplica without VIP", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "single-without-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					// VirtualIP not set - should be allowed for SingleReplica
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred(), "Should accept SingleReplica without VIP")
			_ = k8sClient.Delete(ctx, provisioner)
		})

		It("should accept SingleReplica with VIP", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "single-with-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					VirtualIP:                      "192.168.1.100",
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred(), "Should accept SingleReplica with VIP")
			_ = k8sClient.Delete(ctx, provisioner)
		})
	})

	Context("Field Immutability Validation", func() {
		var provisioner *provisioningv1alpha1.DPFHCPProvisioner

		BeforeEach(func() {
			provisioner = &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "immutability-test",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "test-dpu",
						Namespace: "default",
					},
					BaseDomain:                     "test.example.com",
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: "test-ssh-key"},
					PullSecretRef:                  corev1.LocalObjectReference{Name: "test-pull-secret"},
					EtcdStorageClass:               "test-storage-class",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())
		})

		AfterEach(func() {
			// Clean up the provisioner created in BeforeEach
			// Must delete and wait for finalizer cleanup before next test
			if provisioner != nil {
				_ = k8sClient.Delete(ctx, provisioner)
				// Wait for deletion to complete (finalizer processing)
				Eventually(func() bool {
					err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, provisioner)
					return err != nil
				}, time.Second*10, time.Millisecond*500).Should(BeTrue())
			}
		})

		It("should reject updates to dpuClusterRef", func() {
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.DPUClusterRef.Name = "different-dpu"
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(MatchError(ContainSubstring("dpuClusterRef is immutable")))
		})

		It("should reject updates to baseDomain", func() {
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.BaseDomain = "updated.example.com"
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(MatchError(ContainSubstring("baseDomain is immutable")))
		})

		It("should reject updates to sshKeySecretRef", func() {
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.SSHKeySecretRef.Name = "different-ssh-key"
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(MatchError(ContainSubstring("sshKeySecretRef is immutable")))
		})

		It("should reject updates to pullSecretRef", func() {
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.PullSecretRef.Name = "different-pull-secret"
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(MatchError(ContainSubstring("pullSecretRef is immutable")))
		})

		It("should reject updates to etcdStorageClass", func() {
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.EtcdStorageClass = "different-storage-class"
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(MatchError(ContainSubstring("etcdStorageClass is immutable")))
		})

		It("should reject updates to controlPlaneAvailabilityPolicy", func() {
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.ControlPlaneAvailabilityPolicy = hyperv1.HighlyAvailable
				updated.Spec.VirtualIP = "192.168.1.100" // Required for HighlyAvailable
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(MatchError(ContainSubstring("controlPlaneAvailabilityPolicy is immutable")))
		})

		It("should reject updates to virtualIP", func() {
			// First set the VIP - wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.VirtualIP = "192.168.1.100"
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(Succeed())

			// Now try to change it - should fail with immutability error
			Eventually(func() error {
				fresh2 := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh2); err != nil {
					return err
				}
				updated2 := fresh2.DeepCopy()
				updated2.Spec.VirtualIP = "192.168.1.101"
				return k8sClient.Update(ctx, updated2)
			}, time.Second*5, time.Millisecond*100).Should(MatchError(ContainSubstring("virtualIP is immutable")))
		})

		It("should allow updates to ocpReleaseImage (mutable)", func() {
			// Wrap in Eventually to handle race with controller
			Eventually(func() error {
				fresh := &provisioningv1alpha1.DPFHCPProvisioner{}
				if err := k8sClient.Get(ctx, types.NamespacedName{Name: "immutability-test", Namespace: "default"}, fresh); err != nil {
					return err
				}
				updated := fresh.DeepCopy()
				updated.Spec.OCPReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi"
				return k8sClient.Update(ctx, updated)
			}, time.Second*5, time.Millisecond*100).Should(Succeed())
		})
	})
})
