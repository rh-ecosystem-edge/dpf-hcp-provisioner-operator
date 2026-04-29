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
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
)

var _ = Describe("HostedCluster and NodePool Creation", func() {
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
		baseDomain       string
		ocpReleaseImage  string
		etcdStorageClass string
	)

	BeforeEach(func() {
		ctx = context.Background()
		testNamespace = "default"
		dpuClusterName = "test-dpucluster-hc-creation"
		provisionerName = "test-provisioner-hc-" + time.Now().Format("150405")
		pullSecretName = "test-pull-secret-hc-creation"
		sshKeySecretName = "test-ssh-key-hc-creation"
		baseDomain = "hc-test.example.com"
		ocpReleaseImage = "quay.io/openshift-release-dev/ocp-release:4.17.0-x86_64"
		etcdStorageClass = "standard"

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
		// Clean up provisioner
		provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:      provisionerName,
				Namespace: testNamespace,
			},
		}
		_ = k8sClient.Delete(ctx, provisioner)

		// Clean up DPUCluster
		_ = k8sClient.Delete(ctx, &dpuprovisioningv1alpha1.DPUCluster{ObjectMeta: metav1.ObjectMeta{Name: dpuClusterName, Namespace: testNamespace}})

		// Clean up secrets
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: pullSecretName, Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: sshKeySecretName, Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: provisionerName + "-pull-secret", Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: provisionerName + "-ssh-key", Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: provisionerName + "-etcd-encryption-key", Namespace: testNamespace}})

		// Clean up HostedCluster and NodePool
		_ = k8sClient.Delete(ctx, &hyperv1.HostedCluster{ObjectMeta: metav1.ObjectMeta{Name: provisionerName, Namespace: testNamespace}})
		_ = k8sClient.Delete(ctx, &hyperv1.NodePool{ObjectMeta: metav1.ObjectMeta{Name: provisionerName, Namespace: testNamespace}})

		// Clean up MetalLB resources
		_ = k8sClient.Delete(ctx, &metallbv1beta1.IPAddressPool{ObjectMeta: metav1.ObjectMeta{Name: provisionerName, Namespace: common.OpenshiftOperatorsNamespace}})
		_ = k8sClient.Delete(ctx, &metallbv1beta1.L2Advertisement{ObjectMeta: metav1.ObjectMeta{Name: fmt.Sprintf("advertise-%s", provisionerName), Namespace: common.OpenshiftOperatorsNamespace}})
	})

	// Use HighlyAvailable with VIP to avoid NodePort mode (which requires real nodes in envtest)
	createProvisioner := func() *provisioningv1alpha1.DPFHCPProvisioner {
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
				BaseDomain:                     baseDomain,
				OCPReleaseImage:                ocpReleaseImage,
				SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshKeySecretName},
				PullSecretRef:                  corev1.LocalObjectReference{Name: pullSecretName},
				EtcdStorageClass:               etcdStorageClass,
				ControlPlaneAvailabilityPolicy: hyperv1.HighlyAvailable,
				VirtualIP:                      "192.168.1.200",
			},
		}
		Expect(k8sClient.Create(ctx, provisioner)).To(Succeed())
		return provisioner
	}

	Context("HostedCluster Creation", func() {
		It("should create HostedCluster with owner reference", func() {
			provisioner := createProvisioner()

			// Wait for HostedCluster to be created
			hc := &hyperv1.HostedCluster{}
			Eventually(func() error {
				return k8sClient.Get(ctx, types.NamespacedName{
					Name:      provisionerName,
					Namespace: testNamespace,
				}, hc)
			}, timeout, interval).Should(Succeed())

			// Verify owner reference
			Expect(hc.OwnerReferences).To(HaveLen(1))
			Expect(hc.OwnerReferences[0].Name).To(Equal(provisionerName))
			Expect(hc.OwnerReferences[0].Kind).To(Equal("DPFHCPProvisioner"))
			Expect(*hc.OwnerReferences[0].Controller).To(BeTrue())

			// Verify HostedCluster spec matches CR spec
			Expect(hc.Spec.DNS.BaseDomain).To(Equal(baseDomain))
			Expect(hc.Spec.Release.Image).To(Equal(ocpReleaseImage))
			Expect(hc.Spec.Etcd.Managed.Storage.PersistentVolume.StorageClassName).To(PointTo(Equal(etcdStorageClass)))

			// Verify pull-secret and ssh-key references
			Expect(hc.Spec.PullSecret.Name).To(Equal(provisionerName + "-pull-secret"))
			Expect(hc.Spec.SSHKey.Name).To(Equal(provisionerName + "-ssh-key"))

			// Verify HostedClusterRef is set in CR status
			Eventually(func() bool {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: testNamespace}, provisioner)
				if err != nil {
					return false
				}
				return provisioner.Status.HostedClusterRef != nil
			}, timeout, interval).Should(BeTrue())

			Expect(provisioner.Status.HostedClusterRef.Name).To(Equal(provisionerName))
			Expect(provisioner.Status.HostedClusterRef.Namespace).To(Equal(testNamespace))
		})

		It("should create NodePool with correct spec", func() {
			createProvisioner()

			// Wait for NodePool to be created
			np := &hyperv1.NodePool{}
			Eventually(func() error {
				return k8sClient.Get(ctx, types.NamespacedName{
					Name:      provisionerName,
					Namespace: testNamespace,
				}, np)
			}, timeout, interval).Should(Succeed())

			// Verify owner reference
			Expect(np.OwnerReferences).To(HaveLen(1))
			Expect(np.OwnerReferences[0].Name).To(Equal(provisionerName))
			Expect(np.OwnerReferences[0].Kind).To(Equal("DPFHCPProvisioner"))
			Expect(*np.OwnerReferences[0].Controller).To(BeTrue())

			// Verify release image matches
			Expect(np.Spec.Release.Image).To(Equal(ocpReleaseImage))

			// Verify replicas are 0 (DPU workers join via CSR approval)
			Expect(np.Spec.Replicas).To(PointTo(Equal(int32(0))))
		})

		It("should transition phase from Pending to Provisioning after HC creation", func() {
			provisioner := createProvisioner()

			// Verify phase transitions to Provisioning
			Eventually(func() provisioningv1alpha1.DPFHCPProvisionerPhase {
				err := k8sClient.Get(ctx, types.NamespacedName{Name: provisionerName, Namespace: testNamespace}, provisioner)
				if err != nil {
					return ""
				}
				return provisioner.Status.Phase
			}, timeout, interval).Should(Equal(provisioningv1alpha1.PhaseProvisioning))
		})
	})
})
