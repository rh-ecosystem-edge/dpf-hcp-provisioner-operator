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
	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

var _ = Describe("MetalLB Integration", func() {
	const (
		timeout  = time.Second * 30
		interval = time.Second * 1
	)

	var (
		ctx              context.Context
		testNamespace    string
		dpuClusterName   string
		provisionerName  string
		pullSecretName   string
		sshKeySecretName string
	)

	BeforeEach(func() {
		ctx = context.Background()
		testNamespace = "default"
		dpuClusterName = "test-dpucluster-metallb"
		provisionerName = "test-provisioner-metallb-" + time.Now().Format("150405")
		pullSecretName = "test-pull-secret-metallb"
		sshKeySecretName = "test-ssh-key-metallb"

		// Create DPUCluster
		dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      dpuClusterName,
				Namespace: testNamespace,
			},
			Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
				Type: "static",
			},
		}
		Expect(k8sClient.Create(ctx, dpuCluster)).To(Succeed())

		// Set DPUCluster phase to Ready
		dpuCluster.Status.Phase = dpuprovisioningv1alpha1.PhaseReady
		Expect(k8sClient.Status().Update(ctx, dpuCluster)).To(Succeed())

		// Create pull-secret
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

		// Create ssh-key
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
		// Clean up DPFHCPProvisioner
		provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      provisionerName,
				Namespace: testNamespace,
			},
		}
		_ = k8sClient.Delete(ctx, provisioner)

		// Clean up DPUCluster
		dpuCluster := &dpuprovisioningv1alpha1.DPUCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      dpuClusterName,
				Namespace: testNamespace,
			},
		}
		_ = k8sClient.Delete(ctx, dpuCluster)

		// Clean up secrets
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: pullSecretName, Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: sshKeySecretName, Namespace: testNamespace}})

		// Clean up MetalLB resources
		_ = k8sClient.Delete(ctx, &metallbv1beta1.IPAddressPool{ObjectMeta: metav1.ObjectMeta{Name: provisionerName, Namespace: common.OpenshiftOperatorsNamespace}})
		_ = k8sClient.Delete(ctx, &metallbv1beta1.L2Advertisement{ObjectMeta: metav1.ObjectMeta{Name: fmt.Sprintf("advertise-%s", provisionerName), Namespace: common.OpenshiftOperatorsNamespace}})

		// Clean up copied secrets and HostedCluster/NodePool
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: provisionerName + "-pull-secret", Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: provisionerName + "-ssh-key", Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: provisionerName + "-etcd-encryption-key", Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &hyperv1.HostedCluster{ObjectMeta: metav1.ObjectMeta{Name: provisionerName, Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &hyperv1.NodePool{ObjectMeta: metav1.ObjectMeta{Name: provisionerName, Namespace: testNamespace}})
	})

	Context("MetalLB Resource Creation for HighlyAvailable Provisioner", func() {
		It("should create IPAddressPool and L2Advertisement with correct spec", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      provisionerName,
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
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					VirtualIP:                      "192.168.1.100",
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Verify IPAddressPool is created
			ipPool := &metallbv1beta1.IPAddressPool{}
			Eventually(func() error {
				return k8sClient.Get(ctx, types.NamespacedName{
					Name:      provisionerName,
					Namespace: common.OpenshiftOperatorsNamespace,
				}, ipPool)
			}, timeout, interval).Should(Succeed())

			// Verify IPAddressPool spec
			Expect(ipPool.Spec.Addresses).To(ConsistOf("192.168.1.100/32"))
			Expect(ipPool.Spec.AllocateTo).NotTo(BeNil())
			Expect(ipPool.Spec.AllocateTo.Namespaces).To(ConsistOf(
				fmt.Sprintf("%s-%s", testNamespace, provisionerName),
			))

			// Verify ownership labels
			Expect(ipPool.Labels).To(HaveKeyWithValue(common.LabelDPFHCPProvisionerName, provisionerName))
			Expect(ipPool.Labels).To(HaveKeyWithValue(common.LabelDPFHCPProvisionerNamespace, testNamespace))

			// Verify L2Advertisement is created
			l2Advert := &metallbv1beta1.L2Advertisement{}
			Eventually(func() error {
				return k8sClient.Get(ctx, types.NamespacedName{
					Name:      fmt.Sprintf("advertise-%s", provisionerName),
					Namespace: common.OpenshiftOperatorsNamespace,
				}, l2Advert)
			}, timeout, interval).Should(Succeed())

			// Verify L2Advertisement spec
			Expect(l2Advert.Spec.IPAddressPools).To(ConsistOf(provisionerName))

			// Verify ownership labels
			Expect(l2Advert.Labels).To(HaveKeyWithValue(common.LabelDPFHCPProvisionerName, provisionerName))
			Expect(l2Advert.Labels).To(HaveKeyWithValue(common.LabelDPFHCPProvisionerNamespace, testNamespace))
		})

		It("should set MetalLBConfigured condition to True", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      provisionerName,
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
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					VirtualIP:                      "192.168.1.100",
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Verify MetalLBConfigured condition is True
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: testNamespace}, provisioner)
				if err != nil {
					return false
				}
				cond := meta.FindStatusCondition(provisioner.Status.Conditions, provisioningv1alpha1.MetalLBConfigured)
				return cond != nil && cond.Status == metav1.ConditionTrue
			}, timeout, interval).Should(BeTrue())
		})
	})

	Context("MetalLB Skipped for SingleReplica without VIP", func() {
		It("should not create MetalLB resources", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      provisionerName,
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
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Wait for reconciliation to process (finalizer should be added)
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: testNamespace}, provisioner)
				if err != nil {
					return false
				}
				return len(provisioner.Finalizers) > 0
			}, timeout, interval).Should(BeTrue())

			// Verify no MetalLB resources were created
			Consistently(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{
					Name:      provisionerName,
					Namespace: common.OpenshiftOperatorsNamespace,
				}, &metallbv1beta1.IPAddressPool{})
				return apierrors.IsNotFound(err)
			}, time.Second*5, interval).Should(BeTrue())
		})
	})

	Context("MetalLB Cleanup on CR Deletion", func() {
		It("should delete MetalLB resources when provisioner is deleted", func() {
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      provisionerName,
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
					OCPReleaseImage:                "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64",
					SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
					PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
					EtcdStorageClass:               "standard",
					ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
					VirtualIP:                      "192.168.1.100",
				},
			}
			Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())

			// Wait for MetalLB resources to be created
			Eventually(func() error {
				return k8sClient.Get(ctx, types.NamespacedName{
					Name:      provisionerName,
					Namespace: common.OpenshiftOperatorsNamespace,
				}, &metallbv1beta1.IPAddressPool{})
			}, timeout, interval).Should(Succeed())

			// Delete the provisioner
			Expect(k8sClient.Delete(ctx, provisioner)).To(Succeed())

			// Verify MetalLB resources are cleaned up
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{
					Name:      provisionerName,
					Namespace: common.OpenshiftOperatorsNamespace,
				}, &metallbv1beta1.IPAddressPool{})
				return apierrors.IsNotFound(err)
			}, timeout, interval).Should(BeTrue())

			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{
					Name:      fmt.Sprintf("advertise-%s", provisionerName),
					Namespace: common.OpenshiftOperatorsNamespace,
				}, &metallbv1beta1.L2Advertisement{})
				return apierrors.IsNotFound(err)
			}, timeout, interval).Should(BeTrue())
		})
	})
})
