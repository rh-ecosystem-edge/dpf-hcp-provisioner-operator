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
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/test/utils"
)

var _ = Describe("DPFHCPProvisioner E2E", Ordered, func() {
	var kubeconfigFile string

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
		By("deleting DPFHCPProvisioner CR")
		cmd := exec.Command("kubectl", "delete", "dpfhcpprovisioner", provisionerName,
			"-n", ciNamespace, "--ignore-not-found=true", "--timeout=20m")
		_, _ = utils.Run(cmd)

		By("cleaning up resources in DPUCluster namespace")
		dpfResources := []string{
			"dpucluster", "dpudeployment", "dpuflavor",
			"dpfoperatorconfig", "dpu", "configmap",
		}
		for _, resource := range dpfResources {
			cmd = exec.Command("kubectl", "delete", resource, "--all",
				"-n", dpuClusterNS, "--ignore-not-found=true", "--timeout=1m")
			_, _ = utils.Run(cmd)
		}

		By("cleaning up secrets in operator namespace")
		cmd = exec.Command("kubectl", "delete", "secret", sshKeySecretName,
			"-n", ciNamespace, "--ignore-not-found=true")
		_, _ = utils.Run(cmd)
		cmd = exec.Command("kubectl", "delete", "secret", pullSecretName,
			"-n", ciNamespace, "--ignore-not-found=true")
		_, _ = utils.Run(cmd)

		By("deleting DPUCluster namespace")
		cmd = exec.Command("kubectl", "delete", "ns", dpuClusterNS,
			"--ignore-not-found=true", "--timeout=2m")
		_, _ = utils.Run(cmd)

		if kubeconfigFile != "" {
			os.Remove(kubeconfigFile)
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
				cmd := exec.Command("kubectl", "get",
					"dpfhcpprovisioner", provisionerName,
					"-n", ciNamespace,
					"-o", "jsonpath={.status.hostedClusterRef.name}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				hcName = strings.TrimSpace(output)
				g.Expect(hcName).NotTo(BeEmpty(),
					"HostedCluster not yet created")
			}, 2*time.Minute, pollingInterval).Should(Succeed())

			By("waiting for HostedCluster to become available")
			Eventually(func(g Gomega) {
				cmd := exec.Command("kubectl", "get",
					"hostedcluster", hcName,
					"-n", ciNamespace,
					"-o", "jsonpath={.status.conditions[?(@.type==\"Available\")].status}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(strings.TrimSpace(output)).To(Equal("True"),
					"HostedCluster not yet available")
			}, hostedClusterReadyTimeout, pollingInterval).Should(Succeed())

			By("waiting for CR to reach Ready state")
			waitForCRPhase(provisionerName, "Ready", crReadyTimeout)

			By("verifying status fields are populated")
			cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", provisionerName,
				"-n", ciNamespace, "-o", "jsonpath={.status.hostedClusterRef.name}")
			output, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())
			Expect(strings.TrimSpace(output)).NotTo(BeEmpty(), "hostedClusterRef should be set")

			cmd = exec.Command("kubectl", "get", "dpfhcpprovisioner", provisionerName,
				"-n", ciNamespace, "-o", "jsonpath={.status.kubeConfigSecretRef.name}")
			output, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())
			Expect(strings.TrimSpace(output)).NotTo(BeEmpty(), "kubeConfigSecretRef should be set")
		})

		It("should have generated valid ignition", func() {
			ignitionCMName := fmt.Sprintf("bfcfg-%s.cfg", dpuClusterName)

			By("checking ignition ConfigMap exists in DPUCluster namespace")
			Eventually(func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "configmap", ignitionCMName,
					"-n", dpuClusterNS,
					"-o", "jsonpath={.metadata.name}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(strings.TrimSpace(output)).To(Equal(ignitionCMName),
					"Ignition ConfigMap not found")
			}, 2*time.Minute, pollingInterval).Should(Succeed())

			By("verifying ignition content is valid JSON")
			cmd := exec.Command("kubectl", "get", "configmap", ignitionCMName,
				"-n", dpuClusterNS,
				"-o", "jsonpath={.data.BF_CFG_TEMPLATE}")
			output, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())
			ignitionData := strings.TrimSpace(output)
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
			kubeconfigSecretName := fmt.Sprintf("%s-admin-kubeconfig", provisionerName)

			By("verifying kubeconfig secret exists in DPUCluster namespace")
			cmd := exec.Command("kubectl", "get", "secret", kubeconfigSecretName,
				"-n", dpuClusterNS,
				"-o", "jsonpath={.metadata.name}")
			output, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())
			Expect(strings.TrimSpace(output)).NotTo(BeEmpty(),
				"Kubeconfig secret not found in DPUCluster namespace")

			By("verifying DPUCluster spec.kubeconfig is updated")
			cmd = exec.Command("kubectl", "get", "dpucluster", dpuClusterName,
				"-n", dpuClusterNS, "-o", "jsonpath={.spec.kubeconfig}")
			output, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())
			Expect(strings.TrimSpace(output)).NotTo(BeEmpty(),
				"DPUCluster spec.kubeconfig should be set")
		})

		It("should have correct status conditions", func() {
			conditions := getCRConditions(ciNamespace, provisionerName)
			Expect(conditions).NotTo(BeEmpty(), "No conditions found")

			condMap := make(map[string]string)
			for _, c := range conditions {
				condMap[c.Type] = c.Status
			}

			Expect(condMap["HostedClusterAvailable"]).To(Equal("True"),
				"HostedClusterAvailable should be True")
			Expect(condMap["KubeConfigInjected"]).To(Equal("True"),
				"KubeConfigInjected should be True")
			Expect(condMap["SecretsValid"]).To(Equal("True"),
				"SecretsValid should be True")
			Expect(condMap["ClusterTypeValid"]).To(Equal("True"),
				"ClusterTypeValid should be True")
			Expect(condMap["Ready"]).To(Equal("True"),
				"Ready should be True")
			Expect(condMap["DPUClusterMissing"]).To(Equal("False"),
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
			}
		})

		AfterEach(func() {
			// Clean up DPU stubs and CSRs after each test
			deleteDPUStub(dpuClusterNS, testDPUHostname)
			if kubeconfigFile != "" {
				deleteNodeInHostedCluster(kubeconfigFile, testDPUHostname)
			}
		})

		It("should approve bootstrap CSRs for valid DPU nodes", func() {
			By("creating DPU stub with phase 'DPU Cluster Config'")
			createDPUStub(dpuClusterNS, testDPUHostname, "DPU Cluster Config")

			By("creating bootstrap CSR in HostedCluster")
			csrName := createBootstrapCSRInHostedCluster(kubeconfigFile, testDPUHostname)

			By("waiting for CSR to be approved")
			waitForCSRApproval(kubeconfigFile, csrName, csrApprovalTimeout)

			By("cleaning up CSR")
			deleteCSRInHostedCluster(kubeconfigFile, csrName)
		})

		It("should approve serving CSRs for valid DPU nodes", func() {
			By("creating DPU stub with phase 'Ready'")
			createDPUStub(dpuClusterNS, testDPUHostname, "Ready")

			By("creating Node in HostedCluster matching DPU hostname")
			createNodeInHostedCluster(kubeconfigFile, testDPUHostname)

			By("creating serving CSR in HostedCluster")
			csrName := createServingCSRInHostedCluster(kubeconfigFile, testDPUHostname)

			By("waiting for CSR to be approved")
			waitForCSRApproval(kubeconfigFile, csrName, csrApprovalTimeout)

			By("cleaning up")
			deleteCSRInHostedCluster(kubeconfigFile, csrName)
			deleteNodeInHostedCluster(kubeconfigFile, testDPUHostname)
		})

		It("should not approve CSRs for unknown hostnames", func() {
			unknownHostname := "unknown-dpu-node"

			By("creating bootstrap CSR with unknown hostname")
			csrName := createBootstrapCSRInHostedCluster(kubeconfigFile, unknownHostname)

			By("verifying CSR stays pending for 60 seconds")
			verifyCSRNotApproved(kubeconfigFile, csrName, 60*time.Second)

			By("cleaning up CSR")
			deleteCSRInHostedCluster(kubeconfigFile, csrName)
		})
	})

	Context("Cleanup Verification", func() {
		It("should clean up all resources on CR deletion", func() {
			if getCRPhase(ciNamespace, provisionerName) == "" {
				Skip("Skipping cleanup test - CR does not exist")
			}

			By("deleting the DPFHCPProvisioner CR")
			cmd := exec.Command("kubectl", "delete", "dpfhcpprovisioner", provisionerName,
				"-n", ciNamespace, "--timeout=5m")
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to delete DPFHCPProvisioner")

			By("verifying CR is fully deleted")
			Eventually(func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", provisionerName,
					"-n", ciNamespace)
				_, err := utils.Run(cmd)
				g.Expect(err).To(HaveOccurred(), "CR should be deleted")
			}, cleanupTimeout, pollingInterval).Should(Succeed())

			By("verifying HostedCluster is deleted")
			Eventually(func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "hostedcluster",
					"-n", ciNamespace)
				output, err := utils.Run(cmd)
				// Either the namespace doesn't exist or no hostedclusters found
				if err == nil {
					g.Expect(output).To(ContainSubstring("No resources found"),
						"HostedCluster should be deleted")
				}
			}, cleanupTimeout, pollingInterval).Should(Succeed())
		})
	})

	Context("Validation Error Cases", func() {
		const errorTestName = "e2e-error-test"

		AfterEach(func() {
			cmd := exec.Command("kubectl", "delete", "dpfhcpprovisioner", errorTestName,
				"-n", ciNamespace, "--ignore-not-found=true", "--timeout=2m")
			_, _ = utils.Run(cmd)
		})

		It("should fail with missing DPUCluster", func() {
			By("creating CR referencing non-existent DPUCluster")
			releaseImage := detectOCPReleaseImage()
			yaml := fmt.Sprintf(`apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPProvisioner
metadata:
  name: %s
  namespace: %s
spec:
  dpuClusterRef:
    name: non-existent-cluster
    namespace: non-existent-ns
  baseDomain: test.example.com
  ocpReleaseImage: %s
  sshKeySecretRef:
    name: %s
  pullSecretRef:
    name: %s
  controlPlaneAvailabilityPolicy: SingleReplica
  dpuDeploymentRef:
    name: non-existent-deployment
    namespace: non-existent-ns
`, errorTestName, ciNamespace, releaseImage, sshKeySecretName, pullSecretName)

			cmd := exec.Command("kubectl", "apply", "-f", "-")
			cmd.Stdin = strings.NewReader(yaml)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("verifying CR reaches Failed phase")
			waitForCRPhase(errorTestName, "Failed", 2*time.Minute)

			By("verifying DPUClusterMissing condition")
			Eventually(func(g Gomega) {
				status := getConditionStatus(ciNamespace, errorTestName, "DPUClusterMissing")
				g.Expect(status).To(Equal("True"), "DPUClusterMissing should be True")
			}, 30*time.Second, 5*time.Second).Should(Succeed())
		})

		It("should fail with invalid secrets", func() {
			By("creating CR with non-existent secret references")
			releaseImage := detectOCPReleaseImage()
			yaml := fmt.Sprintf(`apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPProvisioner
metadata:
  name: %s
  namespace: %s
spec:
  dpuClusterRef:
    name: %s
    namespace: %s
  baseDomain: test.example.com
  ocpReleaseImage: %s
  sshKeySecretRef:
    name: non-existent-ssh-key
  pullSecretRef:
    name: non-existent-pull-secret
  controlPlaneAvailabilityPolicy: SingleReplica
  dpuDeploymentRef:
    name: %s
    namespace: %s
`, errorTestName, ciNamespace, dpuClusterName, dpuClusterNS, releaseImage,
				dpuDeploymentName, dpuClusterNS)

			cmd := exec.Command("kubectl", "apply", "-f", "-")
			cmd.Stdin = strings.NewReader(yaml)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("verifying CR reaches Failed phase")
			waitForCRPhase(errorTestName, "Failed", 2*time.Minute)

			By("verifying SecretsValid condition is False")
			Eventually(func(g Gomega) {
				status := getConditionStatus(ciNamespace, errorTestName, "SecretsValid")
				g.Expect(status).To(Equal("False"), "SecretsValid should be False")
			}, 30*time.Second, 5*time.Second).Should(Succeed())
		})
	})
})
