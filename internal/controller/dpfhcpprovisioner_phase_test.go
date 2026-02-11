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
	"encoding/base64"
	"fmt"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

// Phase Transition Tests
// These tests observe controller behavior and verify that the controller
// correctly computes phases based on conditions (NOT manual status manipulation)
var _ = Describe("DPFHCPProvisioner Phase Transitions", func() {
	const (
		timeout  = time.Second * 30
		interval = time.Second * 1
	)

	var (
		ctx              context.Context
		testNamespace    string
		dpuClusterName   string
		pullSecretName   string
		sshKeySecretName string
		ocpReleaseImage  string
		blueFieldImage   string
	)

	BeforeEach(func() {
		ctx = context.Background()
		testNamespace = "default"
		dpuClusterName = "test-dpucluster-phase"
		pullSecretName = "test-pull-secret-phase"
		sshKeySecretName = "test-ssh-key-phase"
		ocpReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64"
		blueFieldImage = "quay.io/example/bluefield:4.17.0"

		// Ensure dpf-hcp-provisioner-system namespace exists (for ConfigMap)
		operatorNs := &corev1.Namespace{
			ObjectMeta: metav1.ObjectMeta{
				Name: "dpf-hcp-provisioner-system",
			},
		}
		err := k8sClient.Create(ctx, operatorNs)
		if err != nil && !apierrors.IsAlreadyExists(err) {
			Fail("Failed to create dpf-hcp-provisioner-system namespace: " + err.Error())
		}

		// Create ocp-bluefield-images ConfigMap for image resolution
		configMap := &corev1.ConfigMap{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "ocp-bluefield-images",
				Namespace: "dpf-hcp-provisioner-system",
			},
			Data: map[string]string{
				"4.17.0": blueFieldImage,
			},
		}
		Expect(k8sClient.Create(ctx, configMap)).To(Succeed())

		// Create DPUCluster
		dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      dpuClusterName,
				Namespace: testNamespace,
			},
			Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
				Type: "bf3",
			},
		}
		Expect(k8sClient.Create(ctx, dpuCluster)).To(Succeed())

		// Set DPUCluster phase to Ready
		dpuCluster.Status.Phase = dpuprovisioningv1alpha1.PhaseReady
		Expect(k8sClient.Status().Update(ctx, dpuCluster)).To(Succeed())

		// Create pull-secret
		// Generate auth at runtime to avoid security scanner false positives
		testAuth := base64.StdEncoding.EncodeToString([]byte("test:test"))
		pullSecretData := fmt.Sprintf(`{"auths":{"quay.io":{"auth":"%s"}}}`, testAuth)
		pullSecret := &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      pullSecretName,
				Namespace: testNamespace,
			},
			Type: corev1.SecretTypeDockerConfigJson,
			Data: map[string][]byte{
				".dockerconfigjson": []byte(pullSecretData),
			},
		}
		Expect(k8sClient.Create(ctx, pullSecret)).To(Succeed())

		// Create ssh-key - must contain "id_rsa.pub" key as expected by validator
		sshKey := &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      sshKeySecretName,
				Namespace: testNamespace,
			},
			Type: corev1.SecretTypeOpaque,
			Data: map[string][]byte{
				"id_rsa.pub": []byte("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ..."),
			},
		}
		Expect(k8sClient.Create(ctx, sshKey)).To(Succeed())
	})

	AfterEach(func() {
		// Clean up all provisioner resources
		provisionerList := &provisioningv1alpha1.DPFHCPProvisionerList{}
		_ = k8sClient.List(ctx, provisionerList)
		for _, provisioner := range provisionerList.Items {
			_ = k8sClient.Delete(ctx, &provisioner)
		}

		// Clean up HostedClusters
		hcList := &hyperv1.HostedClusterList{}
		_ = k8sClient.List(ctx, hcList)
		for _, hc := range hcList.Items {
			_ = k8sClient.Delete(ctx, &hc)
		}

		// Clean up DPUCluster
		dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      dpuClusterName,
				Namespace: testNamespace,
			},
		}
		_ = k8sClient.Delete(ctx, dpuCluster)

		// Clean up secrets
		_ = k8sClient.Delete(ctx, &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      pullSecretName,
				Namespace: testNamespace,
			},
		})
		_ = k8sClient.Delete(ctx, &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      sshKeySecretName,
				Namespace: testNamespace,
			},
		})
		_ = k8sClient.Delete(ctx, &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "phase-test-ready-admin-kubeconfig",
				Namespace: testNamespace,
			},
		})

		// Clean up ConfigMap
		_ = k8sClient.Delete(ctx, &corev1.ConfigMap{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "ocp-bluefield-images",
				Namespace: "dpf-hcp-provisioner-system",
			},
		})

		// Clean up copied secrets in clusters namespace
		_ = k8sClient.Delete(ctx, &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "phase-test-pull-secret",
				Namespace: "clusters",
			},
		})
		_ = k8sClient.Delete(ctx, &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "phase-test-ssh-key",
				Namespace: "clusters",
			},
		})
		_ = k8sClient.Delete(ctx, &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "phase-test-etcd-encryption-key",
				Namespace: "clusters",
			},
		})
	})

	Context("Controller-Driven Phase Computation", func() {
		It("should transition to Pending when all validations pass", func() {
			// Create DPFHCPProvisioner with valid configuration
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-test",
					Namespace: testNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      dpuClusterName,
						Namespace: testNamespace,
					},
					BaseDomain:                     "test-cluster.example.com",
					OCPReleaseImage:                ocpReleaseImage,
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Controller should set phase to Pending after all validations pass
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhasePending))

			// Verify validation conditions are set correctly
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test", Namespace: testNamespace}, provisioner)
				if err != nil {
					return false
				}

				// All validation conditions should be successful for Pending phase
				// 1. DPUCluster should exist (DPUClusterMissing should be False or nil)
				dpuClusterMissingCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.DPUClusterMissing)
				if dpuClusterMissingCond != nil && dpuClusterMissingCond.Status == metav1.ConditionTrue {
					// DPUCluster is missing - validation failed
					return false
				}

				// 2. DPUCluster type should be valid (ClusterTypeValid should be True)
				clusterTypeValidCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.ClusterTypeValid)
				if clusterTypeValidCond == nil || clusterTypeValidCond.Status != metav1.ConditionTrue {
					return false
				}

				// 3. DPUCluster should not be in use (DPUClusterInUse should be False or nil)
				dpuClusterInUseCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.DPUClusterInUse)
				if dpuClusterInUseCond != nil && dpuClusterInUseCond.Status == metav1.ConditionTrue {
					// DPUCluster is already in use - validation failed
					return false
				}

				// 4. Secrets should be valid (SecretsValid should be True)
				secretsCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.SecretsValid)
				if secretsCond == nil || secretsCond.Status != metav1.ConditionTrue {
					return false
				}

				// 5. BlueField image should be resolved (BlueFieldImageResolved should be True)
				imageCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.BlueFieldImageResolved)
				if imageCond == nil || imageCond.Status != metav1.ConditionTrue {
					return false
				}

				return true
			}, timeout, interval).Should(BeTrue())
		})

		It("should transition to Failed when DPUCluster is missing", func() {
			// Create DPFHCPProvisioner referencing non-existent DPUCluster
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-test-missing-dpu",
					Namespace: testNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "non-existent-dpu",
						Namespace: testNamespace,
					},
					BaseDomain:                     "test-cluster.example.com",
					OCPReleaseImage:                ocpReleaseImage,
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Controller should set phase to Failed
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-missing-dpu", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhaseFailed))

			// Verify DPUClusterMissing condition is set
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-missing-dpu", Namespace: testNamespace}, provisioner)
				if err != nil {
					return false
				}

				missingCond := meta.FindStatusCondition(provisioner.Status.Conditions, "DPUClusterMissing")
				return missingCond != nil && missingCond.Status == metav1.ConditionTrue
			}, timeout, interval).Should(BeTrue())
		})

		It("should transition to Deleting when CR is being deleted", func() {
			// Create DPFHCPProvisioner
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-test-deleting",
					Namespace: testNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      dpuClusterName,
						Namespace: testNamespace,
					},
					BaseDomain:                     "test-cluster.example.com",
					OCPReleaseImage:                ocpReleaseImage,
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Wait for initial phase
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-deleting", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhasePending))

			// Delete the CR
			Expect(k8sClient.Delete(ctx, provisioner)).To(Succeed())

			// Controller should set phase to Deleting (if CR still exists during deletion)
			// Note: CR might be deleted very quickly, so we check if either:
			// 1. Phase is Deleting (CR still exists)
			// 2. CR is deleted (which is also acceptable)
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-deleting", Namespace: testNamespace}, provisioner)
				if apierrors.IsNotFound(err) {
					// CR deleted successfully
					return true
				}
				// If CR still exists, phase should be Deleting
				return provisioner.Status.Phase == provisioningv1alpha1.PhaseDeleting
			}, timeout, interval).Should(BeTrue())
		})

		It("should transition to Provisioning when HostedCluster is created", func() {
			// Create DPFHCPProvisioner
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-test-provisioning",
					Namespace: testNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      dpuClusterName,
						Namespace: testNamespace,
					},
					BaseDomain:                     "test-cluster.example.com",
					OCPReleaseImage:                ocpReleaseImage,
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Wait for Pending phase
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-provisioning", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhasePending))

			// Set HostedClusterRef to simulate HostedCluster creation
			Eventually(func() error {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-provisioning", Namespace: testNamespace}, provisioner)
				if err != nil {
					return err
				}
				provisioner.Status.HostedClusterRef = &corev1.ObjectReference{
					Name:      "phase-test-provisioning",
					Namespace: testNamespace,
				}
				return k8sClient.Status().Update(ctx, provisioner)
			}, timeout, interval).Should(Succeed())

			// Controller should transition to Provisioning
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-provisioning", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhaseProvisioning))
		})

		It("should stay in Provisioning when HostedCluster is available BUT kubeconfig is NOT injected", func() {
			// Create DPFHCPProvisioner
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-test-not-ready",
					Namespace: testNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      dpuClusterName,
						Namespace: testNamespace,
					},
					BaseDomain:                     "test-cluster.example.com",
					OCPReleaseImage:                ocpReleaseImage,
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Wait for Pending phase
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-not-ready", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhasePending))

			// Mock: Simulate controller setting HostedCluster created (move to Provisioning)
			Eventually(func() error {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-not-ready", Namespace: testNamespace}, provisioner)
				if err != nil {
					return err
				}
				provisioner.Status.HostedClusterRef = &corev1.ObjectReference{
					Name:      "phase-test-not-ready",
					Namespace: testNamespace,
				}
				return k8sClient.Status().Update(ctx, provisioner)
			}, timeout, interval).Should(Succeed())

			// Wait for Provisioning phase
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-not-ready", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhaseProvisioning))

			// Mock: Simulate HC available but kubeconfig NOT injected
			Eventually(func() error {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-not-ready", Namespace: testNamespace}, provisioner)
				if err != nil {
					return err
				}
				// Set HC available = True, kubeconfig injected = False
				meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
					Type:               provisioningv1alpha1.HostedClusterAvailable,
					Status:             metav1.ConditionTrue,
					Reason:             "Available",
					Message:            "HostedCluster is available",
					LastTransitionTime: metav1.Now(),
				})
				meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
					Type:               provisioningv1alpha1.KubeConfigInjected,
					Status:             metav1.ConditionFalse,
					Reason:             provisioningv1alpha1.ReasonKubeConfigPending,
					Message:            "Waiting for kubeconfig secret",
					LastTransitionTime: metav1.Now(),
				})
				return k8sClient.Status().Update(ctx, provisioner)
			}, timeout, interval).Should(Succeed())

			// Controller should stay in Provisioning (not Ready) since kubeconfig not injected
			Consistently(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-not-ready", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, time.Second*3, interval).Should(Equal(provisioningv1alpha1.PhaseProvisioning))

			// Verify Ready condition is False or not set
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-not-ready", Namespace: testNamespace}, provisioner)
				if err != nil {
					return false
				}

				readyCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.Ready)
				// Ready should either be False or not set yet
				return readyCond == nil || readyCond.Status == metav1.ConditionFalse
			}, timeout, interval).Should(BeTrue())
		})
	})
})
