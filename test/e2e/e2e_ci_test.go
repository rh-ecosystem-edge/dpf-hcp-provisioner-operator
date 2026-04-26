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

package e2e

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"time"

	dpuprovisioningv1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/rest"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

var _ = Describe("DPFHCPProvisioner E2E", Ordered, func() {
	var (
		kubeconfigFile string
		hcConfig       *rest.Config
	)

	BeforeAll(func() {
		By("cleaning up any stale resources from previous runs")
		cleanupStaleResources()

		By("creating DPUCluster namespace")
		createNamespace(dpuClusterNS)

		By("creating DPUCluster stub CR")
		createDPUClusterStub(dpuClusterNS, dpuClusterName)

		By("creating DPFOperatorConfig in DPUCluster namespace")
		createDPFOperatorConfig(dpuClusterNS)

		By("creating DPUFlavor stub")
		createDPUFlavorStub(dpuClusterNS, dpuFlavorName)

		By("creating DPUDeployment stub")
		createDPUDeploymentStub(dpuClusterNS, dpuDeploymentName, dpuFlavorName)

		By("generating SSH key secret")
		generateSSHKeySecret(ciNamespace, sshKeySecretName)

		By("copying pull secret from cluster")
		copyPullSecret(ciNamespace, pullSecretName)
	})

	AfterAll(func() {
		// Skip cleanup to preserve resources for must-gather collection.
		// The AWS cluster will be deprovisioned anyway, cleaning up all resources.
		// Only clean up temporary files from the local filesystem.
		By("skipping resource cleanup to preserve state for must-gather")

		if kubeconfigFile != "" {
			_ = os.Remove(kubeconfigFile)
		}
	})

	AfterEach(func() {
		if CurrentSpecReport().Failed() {
			dumpProvisionerStatus(ciNamespace, provisionerName)
		}
	})

	Context("HostedCluster Lifecycle", func() {
		It("should create DPFHCPProvisioner and reach Ready state", func() {
			By("creating the DPFHCPProvisioner CR")
			createDPFHCPProvisioner(ciNamespace, provisionerName)

			By("waiting for CR to pass validation and begin provisioning")
			waitForCRPhase(provisionerName, "Provisioning", 5*time.Minute)

			By("verifying HostedCluster was created")
			var hcName string
			Eventually(func(g Gomega) {
				ctx := context.Background()
				provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
				err := k8sClient.Get(ctx, types.NamespacedName{
					Namespace: ciNamespace,
					Name:      provisionerName,
				}, provisioner)
				g.Expect(err).NotTo(HaveOccurred())
				hcName = provisioner.Status.HostedClusterRef.Name
				g.Expect(hcName).NotTo(BeEmpty(),
					"HostedCluster not yet created")
			}, 2*time.Minute, pollingInterval).Should(Succeed())

			By("waiting for HostedCluster to become available")
			Eventually(func(g Gomega) {
				ctx := context.Background()
				hc := &hyperv1.HostedCluster{}
				err := k8sClient.Get(ctx, types.NamespacedName{
					Namespace: ciNamespace,
					Name:      hcName,
				}, hc)
				g.Expect(err).NotTo(HaveOccurred())

				available := false
				for _, cond := range hc.Status.Conditions {
					if cond.Type == string(hyperv1.HostedClusterAvailable) && cond.Status == metav1.ConditionTrue {
						available = true
						break
					}
				}
				g.Expect(available).To(BeTrue(), "HostedCluster not yet available")
			}, hostedClusterReadyTimeout, pollingInterval).Should(Succeed())

			By("waiting for CR to reach Ready state")
			waitForCRPhase(provisionerName, "Ready", crReadyTimeout)

			By("verifying status fields are populated")
			ctx := context.Background()
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
			err := k8sClient.Get(ctx, types.NamespacedName{
				Namespace: ciNamespace,
				Name:      provisionerName,
			}, provisioner)
			Expect(err).NotTo(HaveOccurred())

			Expect(provisioner.Status.HostedClusterRef.Name).NotTo(BeEmpty(), "hostedClusterRef should be set")
			Expect(provisioner.Status.KubeConfigSecretRef.Name).NotTo(BeEmpty(), "kubeConfigSecretRef should be set")
		})

		It("should have generated valid ignition", func() {
			ctx := context.Background()
			ignitionCMName := fmt.Sprintf("bfcfg-%s.cfg", dpuClusterName)

			By("checking ignition ConfigMap exists in DPUCluster namespace")
			Eventually(func(g Gomega) {
				cm := &corev1.ConfigMap{}
				err := k8sClient.Get(ctx, types.NamespacedName{
					Namespace: dpuClusterNS,
					Name:      ignitionCMName,
				}, cm)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(cm.Name).To(Equal(ignitionCMName),
					"Ignition ConfigMap not found")
			}, 2*time.Minute, pollingInterval).Should(Succeed())

			By("verifying ignition content is valid JSON")
			cm := &corev1.ConfigMap{}
			err := k8sClient.Get(ctx, types.NamespacedName{
				Namespace: dpuClusterNS,
				Name:      ignitionCMName,
			}, cm)
			Expect(err).NotTo(HaveOccurred())
			ignitionData := cm.Data["BF_CFG_TEMPLATE"]
			Expect(ignitionData).NotTo(BeEmpty(),
				"Ignition ConfigMap data is empty")
			// Verify it's valid JSON (ignition format)
			var ignitionJSON map[string]interface{}
			err = json.Unmarshal([]byte(ignitionData), &ignitionJSON)
			Expect(err).NotTo(HaveOccurred(), "Ignition data should be valid JSON")
			// Verify it has the ignition version field
			Expect(ignitionJSON).To(HaveKey("ignition"),
				"Ignition data should contain 'ignition' key")
		})

		It("should have injected kubeconfig into DPUCluster namespace", func() {
			ctx := context.Background()
			kubeconfigSecretName := fmt.Sprintf("%s-admin-kubeconfig", provisionerName)

			By("verifying kubeconfig secret exists in DPUCluster namespace")
			secret := &corev1.Secret{}
			err := k8sClient.Get(ctx, types.NamespacedName{
				Namespace: dpuClusterNS,
				Name:      kubeconfigSecretName,
			}, secret)
			Expect(err).NotTo(HaveOccurred())
			Expect(secret.Name).NotTo(BeEmpty(),
				"Kubeconfig secret not found in DPUCluster namespace")

			By("verifying DPUCluster spec.kubeconfig is updated")
			dpuCluster := &dpuprovisioningv1.DPUCluster{}
			err = k8sClient.Get(ctx, types.NamespacedName{
				Namespace: dpuClusterNS,
				Name:      dpuClusterName,
			}, dpuCluster)
			Expect(err).NotTo(HaveOccurred())

			Expect(dpuCluster.Spec.Kubeconfig).NotTo(BeEmpty(),
				"DPUCluster spec.kubeconfig should be set")
		})

		It("should have correct status conditions", func() {
			conditions := getCRConditions(ciNamespace, provisionerName)
			Expect(conditions).NotTo(BeEmpty(), "No conditions found")

			condMap := make(map[string]metav1.ConditionStatus)
			for _, c := range conditions {
				condMap[c.Type] = c.Status
			}

			Expect(condMap["HostedClusterAvailable"]).To(Equal(metav1.ConditionTrue),
				"HostedClusterAvailable should be True")
			Expect(condMap["KubeConfigInjected"]).To(Equal(metav1.ConditionTrue),
				"KubeConfigInjected should be True")
			Expect(condMap["SecretsValid"]).To(Equal(metav1.ConditionTrue),
				"SecretsValid should be True")
			Expect(condMap["ClusterTypeValid"]).To(Equal(metav1.ConditionTrue),
				"ClusterTypeValid should be True")
			Expect(condMap["Ready"]).To(Equal(metav1.ConditionTrue),
				"Ready should be True")
			Expect(condMap["DPUClusterMissing"]).To(Equal(metav1.ConditionFalse),
				"DPUClusterMissing should be False")
		})
	})

	Context("CSR Auto-Approval", func() {
		const testDPUHostname = "e2e-test-dpu-node"

		BeforeEach(func() {
			if getCRPhase(ciNamespace, provisionerName) != "Ready" {
				Skip("Skipping CSR tests - CR not in Ready state")
			}
			if kubeconfigFile == "" {
				By("getting HostedCluster kubeconfig")
				b64Kubeconfig := getHostedClusterKubeconfig(ciNamespace, provisionerName)
				kubeconfigFile = writeKubeconfigToFile(b64Kubeconfig)
				hcConfig = loadHCConfig(kubeconfigFile)
			}
		})

		AfterEach(func() {
			// Clean up DPU stubs and CSRs after each test
			deleteDPUStub(dpuClusterNS, testDPUHostname)
			if hcConfig != nil {
				deleteNodeInHostedCluster(hcConfig, testDPUHostname)
			}
		})

		It("should approve bootstrap CSRs for valid DPU nodes", func() {
			By("creating DPU stub with phase 'DPU Cluster Config'")
			createDPUStub(dpuClusterNS, testDPUHostname, "DPU Cluster Config")

			By("creating bootstrap CSR in HostedCluster")
			csrName := createBootstrapCSRInHostedCluster(hcConfig, testDPUHostname)

			By("waiting for CSR to be approved")
			waitForCSRApproval(hcConfig, csrName, csrApprovalTimeout)

			By("cleaning up CSR")
			deleteCSRInHostedCluster(hcConfig, csrName)
		})

		It("should approve serving CSRs for valid DPU nodes", func() {
			By("creating DPU stub with phase 'Ready'")
			createDPUStub(dpuClusterNS, testDPUHostname, "Ready")

			By("creating Node in HostedCluster matching DPU hostname")
			createNodeInHostedCluster(hcConfig, testDPUHostname)

			By("creating serving CSR in HostedCluster")
			csrName := createServingCSRInHostedCluster(hcConfig, testDPUHostname)

			By("waiting for CSR to be approved")
			waitForCSRApproval(hcConfig, csrName, csrApprovalTimeout)

			By("cleaning up")
			deleteCSRInHostedCluster(hcConfig, csrName)
			deleteNodeInHostedCluster(hcConfig, testDPUHostname)
		})

		It("should not approve CSRs for unknown hostnames", func() {
			unknownHostname := "unknown-dpu-node"

			By("creating bootstrap CSR with unknown hostname")
			csrName := createBootstrapCSRInHostedCluster(hcConfig, unknownHostname)

			By("verifying CSR stays pending for 60 seconds")
			verifyCSRNotApproved(hcConfig, csrName, 60*time.Second)

			By("cleaning up CSR")
			deleteCSRInHostedCluster(hcConfig, csrName)
		})
	})

	Context("Cleanup Verification", func() {
		const (
			cleanupTestName       = "e2e-cleanup-test"
			cleanupDPUClusterNS   = "dpf-e2e-cleanup-dpucluster"
			cleanupDPUClusterName = "cleanup-dpucluster" // Max 20 chars for NVIDIA validation
			cleanupDPUFlavorName  = "cleanup-dpuflavor"  // Max 20 chars for NVIDIA validation
			cleanupDPUDeployName  = "cleanup-dpudeploy"  // Max 20 chars for NVIDIA validation
		)

		AfterEach(func() {
			// Force delete the cleanup test CR if it still exists
			forceDeleteProvisioner(ciNamespace, cleanupTestName)

			// Clean up the cleanup test's DPUCluster namespace and all resources
			ctx := context.Background()
			ns := &corev1.Namespace{}
			err := k8sClient.Get(ctx, types.NamespacedName{Name: cleanupDPUClusterNS}, ns)
			if err == nil {
				_ = k8sClient.Delete(ctx, ns)
			}
		})

		It("should clean up all resources on CR deletion", func() {
			ctx := context.Background()

			By("creating cleanup test DPUCluster namespace")
			createNamespace(cleanupDPUClusterNS)

			By("creating cleanup test DPUCluster stub")
			createDPUClusterStub(cleanupDPUClusterNS, cleanupDPUClusterName)

			By("creating DPFOperatorConfig in cleanup test DPUCluster namespace")
			createDPFOperatorConfig(cleanupDPUClusterNS)

			By("creating cleanup test DPUFlavor stub")
			createDPUFlavorStub(cleanupDPUClusterNS, cleanupDPUFlavorName)

			By("creating cleanup test DPUDeployment stub")
			createDPUDeploymentStub(cleanupDPUClusterNS, cleanupDPUDeployName, cleanupDPUFlavorName)

			By("creating a DPFHCPProvisioner CR for cleanup testing")
			createDPFHCPProvisionerWithCluster(
				ciNamespace, cleanupTestName,
				cleanupDPUClusterName, cleanupDPUClusterNS,
				cleanupDPUDeployName,
			)

			By("waiting for cleanup test CR to reach Ready state")
			waitForCRPhase(cleanupTestName, "Ready", crReadyTimeout)

			By("getting the HostedCluster name for cleanup test CR")
			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
			err := k8sClient.Get(ctx, types.NamespacedName{
				Namespace: ciNamespace,
				Name:      cleanupTestName,
			}, provisioner)
			Expect(err).NotTo(HaveOccurred())
			cleanupHCName := provisioner.Status.HostedClusterRef.Name

			By("deleting the cleanup test DPFHCPProvisioner CR")
			err = k8sClient.Delete(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred(), "Failed to delete DPFHCPProvisioner")

			By("verifying cleanup test CR is fully deleted")
			Eventually(func(g Gomega) {
				err := k8sClient.Get(ctx, types.NamespacedName{
					Namespace: ciNamespace,
					Name:      cleanupTestName,
				}, provisioner)
				g.Expect(apierrors.IsNotFound(err)).To(BeTrue(), "CR should be deleted")
			}, cleanupTimeout, pollingInterval).Should(Succeed())

			By("verifying cleanup test HostedCluster is deleted")
			Eventually(func(g Gomega) {
				hc := &hyperv1.HostedCluster{}
				err := k8sClient.Get(ctx, types.NamespacedName{
					Namespace: ciNamespace,
					Name:      cleanupHCName,
				}, hc)
				g.Expect(apierrors.IsNotFound(err)).To(BeTrue(),
					"HostedCluster should be deleted")
			}, cleanupTimeout, pollingInterval).Should(Succeed())
		})
	})

	Context("Validation Error Cases", func() {
		const errorTestName = "e2e-error-test"

		AfterEach(func() {
			// Use forceDeleteProvisioner to ensure it's fully deleted before next test
			forceDeleteProvisioner(ciNamespace, errorTestName)
		})

		It("should fail with missing DPUCluster", func() {
			By("creating CR referencing non-existent DPUCluster")
			ctx := context.Background()
			releaseImage := detectOCPReleaseImage()

			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      errorTestName,
					Namespace: ciNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      "non-existent-cluster",
						Namespace: "non-existent-ns",
					},
					BaseDomain:      "test.example.com",
					OCPReleaseImage: releaseImage,
					SSHKeySecretRef: corev1.LocalObjectReference{
						Name: sshKeySecretName,
					},
					PullSecretRef: corev1.LocalObjectReference{
						Name: pullSecretName,
					},
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      "non-existent-deployment",
						Namespace: "non-existent-ns",
					},
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())

			By("verifying CR reaches Failed phase")
			waitForCRPhase(errorTestName, "Failed", 2*time.Minute)

			By("verifying DPUClusterMissing condition")
			Eventually(func(g Gomega) {
				status := getConditionStatus(ciNamespace, errorTestName, "DPUClusterMissing")
				g.Expect(status).To(Equal(string(metav1.ConditionTrue)), "DPUClusterMissing should be True")
			}, 30*time.Second, 5*time.Second).Should(Succeed())
		})

		It("should fail with invalid secrets", func() {
			By("creating CR with non-existent secret references")
			ctx := context.Background()
			releaseImage := detectOCPReleaseImage()

			provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
				ObjectMeta: metav1.ObjectMeta{
					Name:      errorTestName,
					Namespace: ciNamespace,
				},
				Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
					DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
						Name:      dpuClusterName,
						Namespace: dpuClusterNS,
					},
					BaseDomain:      "test.example.com",
					OCPReleaseImage: releaseImage,
					SSHKeySecretRef: corev1.LocalObjectReference{
						Name: "non-existent-ssh-key",
					},
					PullSecretRef: corev1.LocalObjectReference{
						Name: "non-existent-pull-secret",
					},
					ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
					DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
						Name:      dpuDeploymentName,
						Namespace: dpuClusterNS,
					},
				},
			}

			err := k8sClient.Create(ctx, provisioner)
			Expect(err).NotTo(HaveOccurred())

			By("verifying CR reaches Failed phase")
			waitForCRPhase(errorTestName, "Failed", 2*time.Minute)

			By("verifying SecretsValid condition is False")
			Eventually(func(g Gomega) {
				status := getConditionStatus(ciNamespace, errorTestName, "SecretsValid")
				g.Expect(status).To(Equal(string(metav1.ConditionFalse)), "SecretsValid should be False")
			}, 30*time.Second, 5*time.Second).Should(Succeed())
		})
	})
})
