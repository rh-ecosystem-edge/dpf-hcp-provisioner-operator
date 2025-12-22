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

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/api/v1alpha1"
)

var _ = Describe("DPFHCPBridge CRD Schema Validation Tests", func() {
	ctx := context.Background()

	AfterEach(func() {
		// Clean up any resources that might have been created
		bridgeList := &provisioningv1alpha1.DPFHCPBridgeList{}
		_ = k8sClient.List(ctx, bridgeList)
		for _, bridge := range bridgeList.Items {
			_ = k8sClient.Delete(ctx, &bridge)
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
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      fmt.Sprintf("valid-domain-test-%d", i),
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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
				err := k8sClient.Create(ctx, bridge)
				Expect(err).NotTo(HaveOccurred(), "Domain %q should be valid", domain)
				_ = k8sClient.Delete(ctx, bridge)
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
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      "invalid-domain-test",
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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
				err := k8sClient.Create(ctx, bridge)
				Expect(err).To(HaveOccurred(), "Domain %q should be invalid", domain)
				_ = k8sClient.Delete(ctx, bridge)
			}
		})

		It("should reject baseDomain exceeding max length (253 chars)", func() {
			// Create a domain name with >253 characters
			longDomain := "very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.very-long-subdomain-name-that-exceeds-the-maximum-allowed-length.example.com"

			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "long-domain-test",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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
			err := k8sClient.Create(ctx, bridge)
			Expect(err).To(HaveOccurred())
		})
	})

	Context("Enum Validation", func() {
		It("should accept valid controlPlaneAvailabilityPolicy values", func() {
			validPolicies := []hyperv1.AvailabilityPolicy{hyperv1.SingleReplica, hyperv1.HighlyAvailable}

			for i, policy := range validPolicies {
				bridge := &provisioningv1alpha1.DPFHCPBridge{
					ObjectMeta: metav1.ObjectMeta{
						Name:      fmt.Sprintf("valid-cp-policy-%d", i),
						Namespace: "default",
					},
					Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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
					bridge.Spec.VirtualIP = "192.168.1.100"
				}
				err := k8sClient.Create(ctx, bridge)
				Expect(err).NotTo(HaveOccurred(), "Policy %q should be valid", policy)
				_ = k8sClient.Delete(ctx, bridge)
			}
		})

		It("should reject invalid controlPlaneAvailabilityPolicy values", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "invalid-cp-policy",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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
			err := k8sClient.Create(ctx, bridge)
			Expect(err).To(HaveOccurred())
		})

	})

	Context("Default Values Application", func() {
		It("should apply default values when fields are omitted", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "test-defaults",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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

			err := k8sClient.Create(ctx, bridge)
			Expect(err).To(HaveOccurred(), "Should reject HighlyAvailable (default) without VIP")

			// Try again with VIP for HighlyAvailable default
			bridge.Name = "test-defaults-with-vip"
			bridge.Spec.VirtualIP = "192.168.1.100"
			err = k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())

			// Retrieve the created resource to verify defaults were applied
			created := &provisioningv1alpha1.DPFHCPBridge{}
			err = k8sClient.Get(ctx, types.NamespacedName{Name: "test-defaults-with-vip", Namespace: "default"}, created)
			Expect(err).NotTo(HaveOccurred())

			// Verify defaults were applied by the API server
			Expect(created.Spec.ControlPlaneAvailabilityPolicy).To(Equal(hyperv1.HighlyAvailable))

			_ = k8sClient.Delete(ctx, bridge)
		})
	})

	Context("VIP Requirement based on ControlPlaneAvailabilityPolicy", func() {
		It("should reject HighlyAvailable without VIP", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "ha-without-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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

			err := k8sClient.Create(ctx, bridge)
			Expect(err).To(HaveOccurred(), "Should reject HighlyAvailable without VIP")
			Expect(err.Error()).To(ContainSubstring("virtualIP is required"))
		})

		It("should accept HighlyAvailable with VIP", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "ha-with-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred(), "Should accept HighlyAvailable with VIP")
			_ = k8sClient.Delete(ctx, bridge)
		})

		It("should accept SingleReplica without VIP", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "single-without-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred(), "Should accept SingleReplica without VIP")
			_ = k8sClient.Delete(ctx, bridge)
		})

		It("should accept SingleReplica with VIP", func() {
			bridge := &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "single-with-vip",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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

			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred(), "Should accept SingleReplica with VIP")
			_ = k8sClient.Delete(ctx, bridge)
		})
	})

	Context("Field Immutability Validation", func() {
		var bridge *provisioningv1alpha1.DPFHCPBridge

		BeforeEach(func() {
			bridge = &provisioningv1alpha1.DPFHCPBridge{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "immutability-test",
					Namespace: "default",
				},
				Spec: provisioningv1alpha1.DPFHCPBridgeSpec{
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
			err := k8sClient.Create(ctx, bridge)
			Expect(err).NotTo(HaveOccurred())
		})

		It("should reject updates to dpuClusterRef", func() {
			updated := bridge.DeepCopy()
			updated.Spec.DPUClusterRef.Name = "different-dpu"

			err := k8sClient.Update(ctx, updated)
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("dpuClusterRef is immutable"))
		})

		It("should reject updates to baseDomain", func() {
			updated := bridge.DeepCopy()
			updated.Spec.BaseDomain = "updated.example.com"

			err := k8sClient.Update(ctx, updated)
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("baseDomain is immutable"))
		})

		It("should reject updates to sshKeySecretRef", func() {
			updated := bridge.DeepCopy()
			updated.Spec.SSHKeySecretRef.Name = "different-ssh-key"

			err := k8sClient.Update(ctx, updated)
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("sshKeySecretRef is immutable"))
		})

		It("should reject updates to pullSecretRef", func() {
			updated := bridge.DeepCopy()
			updated.Spec.PullSecretRef.Name = "different-pull-secret"

			err := k8sClient.Update(ctx, updated)
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("pullSecretRef is immutable"))
		})

		It("should reject updates to etcdStorageClass", func() {
			updated := bridge.DeepCopy()
			updated.Spec.EtcdStorageClass = "different-storage-class"

			err := k8sClient.Update(ctx, updated)
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("etcdStorageClass is immutable"))
		})

		It("should reject updates to controlPlaneAvailabilityPolicy", func() {
			updated := bridge.DeepCopy()
			updated.Spec.ControlPlaneAvailabilityPolicy = hyperv1.HighlyAvailable
			updated.Spec.VirtualIP = "192.168.1.100" // Required for HighlyAvailable

			err := k8sClient.Update(ctx, updated)
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("controlPlaneAvailabilityPolicy is immutable"))
		})

		It("should reject updates to virtualIP", func() {
			// First set the VIP
			updated := bridge.DeepCopy()
			updated.Spec.VirtualIP = "192.168.1.100"
			err := k8sClient.Update(ctx, updated)
			Expect(err).NotTo(HaveOccurred())

			// Now try to change it
			updated2 := updated.DeepCopy()
			updated2.Spec.VirtualIP = "192.168.1.101"
			err = k8sClient.Update(ctx, updated2)
			Expect(err).To(HaveOccurred())
			Expect(err.Error()).To(ContainSubstring("virtualIP is immutable"))
		})

		It("should allow updates to ocpReleaseImage (mutable)", func() {
			updated := bridge.DeepCopy()
			updated.Spec.OCPReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.6-multi"

			err := k8sClient.Update(ctx, updated)
			Expect(err).NotTo(HaveOccurred())
		})
	})
})
