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
	configv1 "github.com/openshift/api/config/v1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	ignitiongenerator "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ignitiongenerator"
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
	)

	BeforeEach(func() {
		ctx = context.Background()
		testNamespace = "default"
		dpuClusterName = "test-dpucluster-phase"
		pullSecretName = "test-pull-secret-phase"
		sshKeySecretName = "test-ssh-key-phase"
		ocpReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64"

		// Create DPUCluster
		dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      dpuClusterName,
				Namespace: testNamespace,
			},
			Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
				Type: string(dpuprovisioningv1alpha1.StaticCluster),
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "test-dpu-deployment",
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

				// 5. BlueField OCP layer image should be found (BlueFieldOCPLayerImageFound should be True)
				imageCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.BlueFieldOCPLayerImageFound)
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "test-dpu-deployment",
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "test-dpu-deployment",
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "test-dpu-deployment",
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
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhaseWaitingForControlPlane))
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
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "test-dpu-deployment",
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
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhaseWaitingForControlPlane))

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
			}, time.Second*3, interval).Should(Equal(provisioningv1alpha1.PhaseWaitingForControlPlane))

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

		It("should show WaitingForControlPlane when upgrading and control plane not ready", func() {
			scheme := runtime.NewScheme()
			Expect(hyperv1.AddToScheme(scheme)).To(Succeed())
			hc := &hyperv1.HostedCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-unit-upgrading",
					Namespace: testNamespace,
				},
			}
			fakeClient := fake.NewClientBuilder().WithScheme(scheme).WithObjects(hc).Build()
			reconciler := &DPFHCPProvisionerReconciler{}
			reconciler.Client = fakeClient

			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "phase-unit-upgrading",
					Namespace:  testNamespace,
					Generation: 2,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "phase-unit-upgrading",
						Namespace: testNamespace,
					},
				},
			}

			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterUpgrading,
				Status: metav1.ConditionTrue,
				Reason: provisioningv1alpha1.ReasonUpgradeInProgress,
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.KubeConfigInjected,
				Status: metav1.ConditionTrue,
				Reason: provisioningv1alpha1.ReasonKubeConfigInjected,
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.IgnitionConfigured,
				Status:             metav1.ConditionFalse,
				Reason:             provisioningv1alpha1.ReasonReleaseImageUpdated,
				ObservedGeneration: 2,
			})

			reconciler.updatePhaseFromConditions(context.Background(), provisioner)
			Expect(provisioner.Status.Phase).To(Equal(provisioningv1alpha1.PhaseWaitingForControlPlane),
				"should be WaitingForControlPlane when upgrading and control plane has not rolled out target version")
		})

		It("should transition to GeneratingIgnition when HostedClusterUpgrading is False", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "phase-unit-upgrade-done",
					Namespace:  testNamespace,
					Generation: 2,
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "phase-unit-upgrade-done",
						Namespace: testNamespace,
					},
				},
			}

			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterUpgrading,
				Status: metav1.ConditionFalse,
				Reason: provisioningv1alpha1.ReasonUpgradeComplete,
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.KubeConfigInjected,
				Status: metav1.ConditionTrue,
				Reason: provisioningv1alpha1.ReasonKubeConfigInjected,
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.IgnitionConfigured,
				Status:             metav1.ConditionFalse,
				Reason:             provisioningv1alpha1.ReasonReleaseImageUpdated,
				ObservedGeneration: 2,
			})

			reconciler.updatePhaseFromConditions(context.Background(), provisioner)
			Expect(provisioner.Status.Phase).To(Equal(provisioningv1alpha1.PhaseGeneratingIgnition),
				"should transition to GeneratingIgnition when upgrade is complete")
		})

		It("should show GeneratingIgnition when upgrading and control plane is ready", func() {
			scheme := runtime.NewScheme()
			Expect(hyperv1.AddToScheme(scheme)).To(Succeed())
			hc := &hyperv1.HostedCluster{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-unit-upgrading-phase",
					Namespace: testNamespace,
				},
				Status: hyperv1.HostedClusterStatus{
					ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{
						History: []hyperv1.ControlPlaneUpdateHistory{
							{
								State:   configv1.CompletedUpdate,
								Version: "4.19.0",
								Image:   "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
							},
						},
					},
				},
			}
			fakeClient := fake.NewClientBuilder().WithScheme(scheme).WithObjects(hc).Build()
			reconciler := &DPFHCPProvisionerReconciler{}
			reconciler.Client = fakeClient

			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:       "phase-unit-upgrading-phase",
					Namespace:  testNamespace,
					Generation: 3,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
				},
				Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
					HostedClusterRef: &corev1.ObjectReference{
						Name:      "phase-unit-upgrading-phase",
						Namespace: testNamespace,
					},
				},
			}

			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterAvailable,
				Status: metav1.ConditionTrue,
				Reason: "Available",
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.KubeConfigInjected,
				Status: metav1.ConditionTrue,
				Reason: provisioningv1alpha1.ReasonKubeConfigInjected,
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterUpgrading,
				Status: metav1.ConditionTrue,
				Reason: provisioningv1alpha1.ReasonUpgradeInProgress,
			})
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:               provisioningv1alpha1.IgnitionConfigured,
				Status:             metav1.ConditionFalse,
				Reason:             provisioningv1alpha1.ReasonReleaseImageUpdated,
				ObservedGeneration: 3,
			})

			reconciler.updatePhaseFromConditions(context.Background(), provisioner)
			Expect(provisioner.Status.Phase).To(Equal(provisioningv1alpha1.PhaseGeneratingIgnition),
				"should be GeneratingIgnition when upgrading and control plane has rolled out target version")
		})

		It("isUpgrading should return true when HostedClusterUpgrading=True", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterUpgrading,
				Status: metav1.ConditionTrue,
				Reason: provisioningv1alpha1.ReasonUpgradeInProgress,
			})
			Expect(isUpgrading(provisioner)).To(BeTrue())
		})

		It("isUpgrading should return false when HostedClusterUpgrading=False", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
			meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
				Type:   provisioningv1alpha1.HostedClusterUpgrading,
				Status: metav1.ConditionFalse,
				Reason: provisioningv1alpha1.ReasonUpgradeComplete,
			})
			Expect(isUpgrading(provisioner)).To(BeFalse())
		})

		It("isUpgrading should return false when condition is not set", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
			Expect(isUpgrading(provisioner)).To(BeFalse())
		})

		It("isHostedClusterVersionReady should return true when version matches and state is Completed", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			hc := &hyperv1.HostedCluster{
				Status: hyperv1.HostedClusterStatus{
					ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{
						History: []hyperv1.ControlPlaneUpdateHistory{
							{Version: "4.22.1", State: configv1.CompletedUpdate},
						},
					},
				},
			}
			Expect(reconciler.isHostedClusterVersionReady(hc, "quay.io/openshift-release-dev/ocp-release:4.22.1-multi")).To(BeTrue())
		})

		It("isHostedClusterVersionReady should return false when version matches but state is Partial", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			hc := &hyperv1.HostedCluster{
				Status: hyperv1.HostedClusterStatus{
					ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{
						History: []hyperv1.ControlPlaneUpdateHistory{
							{Version: "4.22.1", State: configv1.PartialUpdate},
						},
					},
				},
			}
			Expect(reconciler.isHostedClusterVersionReady(hc, "quay.io/openshift-release-dev/ocp-release:4.22.1-multi")).To(BeFalse())
		})

		It("isHostedClusterVersionReady should return false when version does not match", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			hc := &hyperv1.HostedCluster{
				Status: hyperv1.HostedClusterStatus{
					ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{
						History: []hyperv1.ControlPlaneUpdateHistory{
							{Version: "4.22.0", State: configv1.CompletedUpdate},
						},
					},
				},
			}
			Expect(reconciler.isHostedClusterVersionReady(hc, "quay.io/openshift-release-dev/ocp-release:4.22.1-multi")).To(BeFalse())
		})

		It("isHostedClusterVersionReady should return false when version history is empty", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			hc := &hyperv1.HostedCluster{
				Status: hyperv1.HostedClusterStatus{
					ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{},
				},
			}
			Expect(reconciler.isHostedClusterVersionReady(hc, "quay.io/openshift-release-dev/ocp-release:4.22.1-multi")).To(BeFalse())
		})

		It("isHostedClusterVersionReady should return false when ControlPlaneVersion has no history", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			hc := &hyperv1.HostedCluster{}
			Expect(reconciler.isHostedClusterVersionReady(hc, "quay.io/openshift-release-dev/ocp-release:4.22.1-multi")).To(BeFalse())
		})

		It("isHostedClusterVersionReady should return true for digest images when image matches and Completed", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			digestImage := "registry.ci.openshift.org/ci-op/release@sha256:abc123def456"
			hc := &hyperv1.HostedCluster{
				Status: hyperv1.HostedClusterStatus{
					ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{
						History: []hyperv1.ControlPlaneUpdateHistory{
							{Version: "4.22.0", Image: digestImage, State: configv1.CompletedUpdate},
						},
					},
				},
			}
			Expect(reconciler.isHostedClusterVersionReady(hc, digestImage)).To(BeTrue())
		})

		It("isHostedClusterVersionReady should return false for digest images when image does not match", func() {
			reconciler := &DPFHCPProvisionerReconciler{}
			hc := &hyperv1.HostedCluster{
				Status: hyperv1.HostedClusterStatus{
					ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{
						History: []hyperv1.ControlPlaneUpdateHistory{
							{Version: "4.22.0", Image: "registry.ci/release@sha256:old", State: configv1.CompletedUpdate},
						},
					},
				},
			}
			Expect(reconciler.isHostedClusterVersionReady(hc, "registry.ci/release@sha256:new")).To(BeFalse())
		})

		It("should transition to Failed when ignition generation fails (IgnitionConfigured=False)", func() {
			// Create DPFHCPProvisioner
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      "phase-test-ign-fail",
					Namespace: testNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      dpuClusterName,
						Namespace: testNamespace,
					},
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "test-dpu-deployment",
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
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-ign-fail", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhasePending))

			// Simulate: set HostedClusterRef + HC available + kubeconfig injected + IgnitionConfigured=False with failure
			Eventually(func() error {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-ign-fail", Namespace: testNamespace}, provisioner)
				if err != nil {
					return err
				}
				provisioner.Status.HostedClusterRef = &corev1.ObjectReference{
					Name:      "phase-test-ign-fail",
					Namespace: testNamespace,
				}
				meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
					Type:               provisioningv1alpha1.HostedClusterAvailable,
					Status:             metav1.ConditionTrue,
					Reason:             "Available",
					Message:            "HostedCluster is available",
					LastTransitionTime: metav1.Now(),
				})
				meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
					Type:               provisioningv1alpha1.KubeConfigInjected,
					Status:             metav1.ConditionTrue,
					Reason:             provisioningv1alpha1.ReasonKubeConfigInjected,
					Message:            "Kubeconfig injected",
					LastTransitionTime: metav1.Now(),
				})
				meta.SetStatusCondition(&provisioner.Status.Conditions, metav1.Condition{
					Type:               provisioningv1alpha1.IgnitionConfigured,
					Status:             metav1.ConditionFalse,
					Reason:             provisioningv1alpha1.ReasonIgnitionGenerationFailed,
					Message:            "Failed to generate ignition: some error",
					ObservedGeneration: provisioner.Generation,
					LastTransitionTime: metav1.Now(),
				})
				return k8sClient.Status().Update(ctx, provisioner)
			}, timeout, interval).Should(Succeed())

			// Controller should transition to Failed
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-ign-fail", Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhaseFailed))

			// Verify IgnitionConfigured condition shows failure
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: "phase-test-ign-fail", Namespace: testNamespace}, provisioner)
				if err != nil {
					return false
				}
				ignCond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
				return ignCond != nil &&
					ignCond.Status == metav1.ConditionFalse &&
					ignCond.Reason == provisioningv1alpha1.ReasonIgnitionGenerationFailed
			}, timeout, interval).Should(BeTrue())
		})
	})
})

var _ = Describe("handleUpgrade", func() {
	const (
		oldImage       = "quay.io/openshift-release-dev/ocp-release:4.18.0-multi"
		newImage       = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi"
		testNamespace  = "default"
		dpuClusterName = "test-dpucluster"
		dpuClusterNS   = "test-dpucluster-ns"
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
		Expect(corev1.AddToScheme(scheme)).To(Succeed())
	})

	newCR := func(image string, phase provisioningv1alpha1.DPFHCPProvisionerPhase) *provisioningv1alpha1.DPFHCPProvisioner {
		return &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: testNamespace,
				UID:       "test-uid-123",
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				OCPReleaseImage: image,
				DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
					Name:      dpuClusterName,
					Namespace: dpuClusterNS,
				},
			},
			Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
				Phase: phase,
				HostedClusterRef: &corev1.ObjectReference{
					Name:      "test-provisioner",
					Namespace: testNamespace,
				},
			},
		}
	}

	newHC := func(image string) *hyperv1.HostedCluster {
		return &hyperv1.HostedCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: testNamespace,
			},
			Spec: hyperv1.HostedClusterSpec{
				Release: hyperv1.Release{Image: image},
			},
		}
	}

	newNP := func(image string) *hyperv1.NodePool {
		return &hyperv1.NodePool{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-provisioner",
				Namespace: testNamespace,
			},
			Spec: hyperv1.NodePoolSpec{
				Release: hyperv1.Release{Image: image},
			},
		}
	}

	newIgnitionCM := func() *corev1.ConfigMap {
		return &corev1.ConfigMap{
			ObjectMeta: metav1.ObjectMeta{
				Name:      ignitiongenerator.ConfigMapName(dpuClusterName),
				Namespace: dpuClusterNS,
			},
			Data: map[string]string{"BF_CFG_TEMPLATE": "some-ignition-data"},
		}
	}

	buildReconciler := func(cr *provisioningv1alpha1.DPFHCPProvisioner, objs ...client.Object) *DPFHCPProvisionerReconciler {
		allObjs := append([]client.Object{cr}, objs...)
		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithObjects(allObjs...).
			WithStatusSubresource(cr).
			Build()

		return &DPFHCPProvisionerReconciler{
			Client:   fakeClient,
			Scheme:   scheme,
			Recorder: record.NewFakeRecorder(10),
		}
	}

	It("should recover when HC was updated but NP was not (partial failure)", func() {
		cr := newCR(newImage, provisioningv1alpha1.PhaseReady)
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:   provisioningv1alpha1.HostedClusterAvailable,
			Status: metav1.ConditionTrue,
			Reason: "Available",
		})
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:   provisioningv1alpha1.IgnitionConfigured,
			Status: metav1.ConditionTrue,
			Reason: provisioningv1alpha1.ReasonIgnitionGenerated,
		})

		hc := newHC(newImage) // HC already updated in previous partial run
		np := newNP(oldImage) // NP NOT updated due to crash

		r := buildReconciler(cr, hc, np)

		result, err := r.handleUpgrade(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(Equal(1*time.Second),
			"should requeue to continue monitoring upgrade")

		// NP should be updated to new image
		updatedNP := &hyperv1.NodePool{}
		Expect(r.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: testNamespace}, updatedNP)).To(Succeed())
		Expect(updatedNP.Spec.Release.Image).To(Equal(newImage),
			"NodePool should be updated to new image during recovery")

		// HC should remain unchanged (already matched)
		updatedHC := &hyperv1.HostedCluster{}
		Expect(r.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: testNamespace}, updatedHC)).To(Succeed())
		Expect(updatedHC.Spec.Release.Image).To(Equal(newImage),
			"HostedCluster should still have new image")

		// HostedClusterUpgrading should be True
		upgradingCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterUpgrading)
		Expect(upgradingCond).NotTo(BeNil())
		Expect(upgradingCond.Status).To(Equal(metav1.ConditionTrue))

		// IgnitionConfigured should be False (invalidated)
		ignCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(ignCond).NotTo(BeNil())
		Expect(ignCond.Status).To(Equal(metav1.ConditionFalse))
		Expect(ignCond.Reason).To(Equal(provisioningv1alpha1.ReasonReleaseImageUpdated))
	})

	It("should recover and delete stale ignition ConfigMap during partial failure", func() {
		cr := newCR(newImage, provisioningv1alpha1.PhaseReady)
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:   provisioningv1alpha1.IgnitionConfigured,
			Status: metav1.ConditionTrue,
			Reason: provisioningv1alpha1.ReasonIgnitionGenerated,
		})

		hc := newHC(newImage) // HC already updated
		np := newNP(oldImage) // NP not updated
		cm := newIgnitionCM() // Stale CM still present (crash happened before CM deletion)

		r := buildReconciler(cr, hc, np, cm)

		result, err := r.handleUpgrade(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(Equal(1*time.Second),
			"should requeue to continue monitoring upgrade")

		// Ignition ConfigMap should be deleted
		deletedCM := &corev1.ConfigMap{}
		err = r.Get(ctx, types.NamespacedName{
			Name:      ignitiongenerator.ConfigMapName(dpuClusterName),
			Namespace: dpuClusterNS,
		}, deletedCM)
		Expect(apierrors.IsNotFound(err)).To(BeTrue(),
			"stale ignition ConfigMap should be deleted during recovery")
	})

	It("should detect upgrade and update both HC and NP", func() {
		cr := newCR(newImage, provisioningv1alpha1.PhaseReady)
		hc := newHC(oldImage)
		np := newNP(oldImage)

		r := buildReconciler(cr, hc, np)

		result, err := r.handleUpgrade(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(Equal(1 * time.Second))

		// HC should be updated
		updatedHC := &hyperv1.HostedCluster{}
		Expect(r.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: testNamespace}, updatedHC)).To(Succeed())
		Expect(updatedHC.Spec.Release.Image).To(Equal(newImage))

		// NP should be updated
		updatedNP := &hyperv1.NodePool{}
		Expect(r.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: testNamespace}, updatedNP)).To(Succeed())
		Expect(updatedNP.Spec.Release.Image).To(Equal(newImage))

		// HostedClusterUpgrading should be True
		upgradingCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterUpgrading)
		Expect(upgradingCond).NotTo(BeNil())
		Expect(upgradingCond.Status).To(Equal(metav1.ConditionTrue))
		Expect(upgradingCond.Reason).To(Equal(provisioningv1alpha1.ReasonUpgradeInProgress))
	})

	It("should complete upgrade when version is ready", func() {
		cr := newCR(newImage, provisioningv1alpha1.PhaseWaitingForControlPlane)
		meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{
			Type:   provisioningv1alpha1.HostedClusterUpgrading,
			Status: metav1.ConditionTrue,
			Reason: provisioningv1alpha1.ReasonUpgradeInProgress,
		})

		hc := newHC(newImage)
		hc.Status.ControlPlaneVersion = hyperv1.ControlPlaneVersionStatus{
			History: []hyperv1.ControlPlaneUpdateHistory{{
				State:   configv1.CompletedUpdate,
				Version: "4.19.0",
				Image:   newImage,
			}},
		}
		np := newNP(newImage)

		r := buildReconciler(cr, hc, np)

		result, err := r.handleUpgrade(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		// HostedClusterUpgrading should be set to False
		upgradingCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterUpgrading)
		Expect(upgradingCond).NotTo(BeNil())
		Expect(upgradingCond.Status).To(Equal(metav1.ConditionFalse))
		Expect(upgradingCond.Reason).To(Equal(provisioningv1alpha1.ReasonUpgradeComplete))
	})

	It("should skip upgrade during Pending phase", func() {
		cr := newCR(newImage, provisioningv1alpha1.PhasePending)
		hc := newHC(oldImage)
		np := newNP(oldImage)

		r := buildReconciler(cr, hc, np)

		result, err := r.handleUpgrade(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		updatedHC := &hyperv1.HostedCluster{}
		Expect(r.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: testNamespace}, updatedHC)).To(Succeed())
		Expect(updatedHC.Spec.Release.Image).To(Equal(oldImage),
			"HC should not be updated during Pending phase")

		updatedNP := &hyperv1.NodePool{}
		Expect(r.Get(ctx, types.NamespacedName{Name: "test-provisioner", Namespace: testNamespace}, updatedNP)).To(Succeed())
		Expect(updatedNP.Spec.Release.Image).To(Equal(oldImage),
			"NP should not be updated during Pending phase")
	})

	It("should no-op when images match and version is ready", func() {
		cr := newCR(newImage, provisioningv1alpha1.PhaseReady)
		hc := newHC(newImage)
		hc.Status.ControlPlaneVersion = hyperv1.ControlPlaneVersionStatus{
			History: []hyperv1.ControlPlaneUpdateHistory{{
				State:   configv1.CompletedUpdate,
				Version: "4.19.0",
				Image:   newImage,
			}},
		}
		np := newNP(newImage)

		r := buildReconciler(cr, hc, np)

		result, err := r.handleUpgrade(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		upgradingCond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.HostedClusterUpgrading)
		Expect(upgradingCond).To(BeNil(),
			"HostedClusterUpgrading should not be set when no upgrade is happening")
	})

	It("should skip when HostedClusterRef is nil", func() {
		cr := newCR(newImage, provisioningv1alpha1.PhaseReady)
		cr.Status.HostedClusterRef = nil

		r := buildReconciler(cr)

		result, err := r.handleUpgrade(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())
	})
})
