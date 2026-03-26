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

const (
	ciNamespace       = "dpf-hcp-provisioner-system"
	dpuClusterNS      = "dpf-e2e-dpucluster"
	provisionerName   = "e2e-test-provisioner"
	dpuClusterName    = "e2e-dpucluster"
	sshKeySecretName  = "e2e-ssh-key"
	pullSecretName    = "e2e-pull-secret"
	dpuDeploymentName = "e2e-dpudeployment"
	dpuFlavorName     = "e2e-dpuflavor"

	hostedClusterReadyTimeout = 25 * time.Minute
	crReadyTimeout            = 35 * time.Minute
	cleanupTimeout            = 10 * time.Minute
	csrApprovalTimeout        = 2 * time.Minute
	pollingInterval           = 10 * time.Second
)

// detectOCPReleaseImage gets the OCP release image from the management cluster.
func detectOCPReleaseImage() string {
	image := os.Getenv("OCP_RELEASE_IMAGE")
	if image != "" {
		return image
	}
	cmd := exec.Command("oc", "get", "clusterversion", "version",
		"-o", "jsonpath={.status.desired.image}")
	output, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to detect OCP release image")
	return strings.TrimSpace(output)
}

// getBaseDomain returns the base domain for the HostedCluster.
// In CI, BASE_DOMAIN is set in the workflow config.
// For local testing, export it manually.
func getBaseDomain() string {
	domain := os.Getenv("BASE_DOMAIN")
	if domain == "" {
		Fail("BASE_DOMAIN must be set. " +
			"In CI this is configured in the workflow. " +
			"For local testing, export it with your cluster's base domain.")
	}
	return domain
}

// getControlPlaneAvailability returns the control plane availability policy.
func getControlPlaneAvailability() string {
	policy := os.Getenv("CONTROL_PLANE_AVAILABILITY")
	if policy == "" {
		policy = "SingleReplica"
	}
	return policy
}

// getMachineOSURL returns the machine OS URL if set.
func getMachineOSURL() string {
	return os.Getenv("MACHINE_OS_URL")
}

// getEtcdStorageClass returns the storage class for etcd PVCs.
// Auto-detects the default StorageClass if not set via env var.
// Fails the test if no StorageClass is available (required for HostedCluster etcd).
func getEtcdStorageClass() string {
	sc := os.Getenv("ETCD_STORAGE_CLASS")
	if sc != "" {
		return sc
	}
	// Try to detect default storage class
	jsonpath := "{.items[?(@.metadata.annotations.storageclass\\.kubernetes\\.io/" +
		"is-default-class==\"true\")].metadata.name}"
	cmd := exec.Command("kubectl", "get", "storageclass", "-o", "jsonpath="+jsonpath)
	output, err := utils.Run(cmd)
	if err == nil && strings.TrimSpace(output) != "" {
		return strings.TrimSpace(strings.Split(output, " ")[0])
	}
	Fail("No default StorageClass found and ETCD_STORAGE_CLASS not set. " +
		"A StorageClass is required for HostedCluster etcd PVCs. " +
		"On AWS IPI clusters this is available by default. " +
		"For local clusters, deploy a storage provider first.")
	return ""
}

// createNamespace creates a Kubernetes namespace, waiting for it to be fully deleted first if terminating.
func createNamespace(ns string) {
	// Wait for namespace to be fully gone if it's terminating from a previous run
	Eventually(func(g Gomega) {
		cmd := exec.Command("kubectl", "get", "ns", ns, "-o", "jsonpath={.status.phase}")
		output, err := utils.Run(cmd)
		if err != nil {
			return // Namespace doesn't exist, good
		}
		g.Expect(strings.TrimSpace(output)).NotTo(Equal("Terminating"),
			"Namespace %s is still terminating", ns)
	}, 2*time.Minute, 5*time.Second).Should(Succeed())

	cmd := exec.Command("kubectl", "create", "ns", ns, "--dry-run=client", "-o", "yaml")
	yaml, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred())
	cmd = exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create namespace %s", ns)
}

// createDPUClusterStub creates a minimal DPUCluster CR for testing.
func createDPUClusterStub(ns, name string) {
	yaml := fmt.Sprintf(`apiVersion: provisioning.dpu.nvidia.com/v1alpha1
kind: DPUCluster
metadata:
  name: %s
  namespace: %s
spec:
  type: static
`, name, ns)
	cmd := exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPUCluster stub")
}

// generateSSHKeySecret creates a secret with a generated SSH public key.
func generateSSHKeySecret(ns, name string) {
	By("generating SSH key pair")
	keyFile := fmt.Sprintf("/tmp/e2e-ssh-key-%d", time.Now().UnixNano())
	cmd := exec.Command("ssh-keygen", "-t", "rsa", "-b", "4096", "-f", keyFile, "-N", "")
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to generate SSH key")
	defer func() {
		os.Remove(keyFile)
		os.Remove(keyFile + ".pub")
	}()

	cmd = exec.Command("kubectl", "create", "secret", "generic", name,
		"--from-file=id_rsa.pub="+keyFile+".pub",
		"-n", ns, "--dry-run=client", "-o", "yaml")
	yaml, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred())
	cmd = exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create SSH key secret")
}

// copyPullSecret copies the cluster pull secret to the target namespace.
func copyPullSecret(ns, name string) {
	By("copying pull secret from openshift-config")
	cmd := exec.Command("oc", "get", "secret", "pull-secret", "-n", "openshift-config",
		"-o", "jsonpath={.data.\\.dockerconfigjson}")
	b64Data, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get cluster pull secret")

	yaml := fmt.Sprintf(`apiVersion: v1
kind: Secret
metadata:
  name: %s
  namespace: %s
type: kubernetes.io/dockerconfigjson
data:
  .dockerconfigjson: %s
`, name, ns, strings.TrimSpace(b64Data))
	cmd = exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create pull secret")
}

// createDPUDeploymentStub creates a minimal DPUDeployment CR that references a DPUFlavor.
func createDPUDeploymentStub(ns, name, flavorName string) {
	yaml := fmt.Sprintf(`apiVersion: svc.dpu.nvidia.com/v1alpha1
kind: DPUDeployment
metadata:
  name: %s
  namespace: %s
spec:
  dpus:
    bfb: e2e-bfb
    flavor: %s
  services: {}
  serviceChains:
    upgradePolicy: {}
    switches:
      - ports:
          - serviceInterface:
              matchLabels:
                e2e-test: "true"
`, name, ns, flavorName)
	cmd := exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPUDeployment stub")
}

// createDPUFlavorStub creates a minimal DPUFlavor CR.
func createDPUFlavorStub(ns, name string) {
	yaml := fmt.Sprintf(`apiVersion: provisioning.dpu.nvidia.com/v1alpha1
kind: DPUFlavor
metadata:
  name: %s
  namespace: %s
spec:
  ovs:
    rawConfigScript: |
      #!/bin/bash
      echo "e2e test OVS config"
`, name, ns)
	cmd := exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPUFlavor stub")
}

// createDPFOperatorConfig creates a DPFOperatorConfig in the DPUCluster namespace
// (needed by the ignition generator for controlPlaneMTU and BFCFGTemplateConfigMap).
func createDPFOperatorConfig(ns string) {
	yaml := fmt.Sprintf(`apiVersion: operator.dpu.nvidia.com/v1alpha1
kind: DPFOperatorConfig
metadata:
  name: dpfoperatorconfig
  namespace: %s
spec:
  provisioningController:
    bfbPVCName: e2e-bfb-pvc
    disable: true
  networking:
    controlPlaneMTU: 1500
`, ns)
	cmd := exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPFOperatorConfig")
}

// createDPFHCPProvisioner creates a DPFHCPProvisioner CR.
func createDPFHCPProvisioner(ns, name string) {
	releaseImage := detectOCPReleaseImage()
	baseDomain := getBaseDomain()
	availability := getControlPlaneAvailability()
	machineOSURL := getMachineOSURL()
	etcdSC := getEtcdStorageClass()

	_, _ = fmt.Fprintf(GinkgoWriter,
		"Creating DPFHCPProvisioner with:\n"+
			"  releaseImage=%s\n  baseDomain=%s\n"+
			"  availability=%s\n  machineOSURL=%s\n"+
			"  etcdStorageClass=%s\n",
		releaseImage, baseDomain, availability, machineOSURL, etcdSC)

	optionalFields := ""
	if machineOSURL != "" {
		optionalFields += fmt.Sprintf("  machineOSURL: %q\n", machineOSURL)
	}
	if etcdSC != "" {
		optionalFields += fmt.Sprintf("  etcdStorageClass: %q\n", etcdSC)
	}

	yaml := fmt.Sprintf(`apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPProvisioner
metadata:
  name: %s
  namespace: %s
spec:
  dpuClusterRef:
    name: %s
    namespace: %s
  baseDomain: %s
  ocpReleaseImage: %s
  sshKeySecretRef:
    name: %s
  pullSecretRef:
    name: %s
  controlPlaneAvailabilityPolicy: %s
  dpuDeploymentRef:
    name: %s
    namespace: %s
%s`, name, ns, dpuClusterName, dpuClusterNS, baseDomain, releaseImage,
		sshKeySecretName, pullSecretName, availability,
		dpuDeploymentName, dpuClusterNS, optionalFields)

	cmd := exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPFHCPProvisioner")
}

// waitForCRPhase waits for a DPFHCPProvisioner to reach a specific phase.
func waitForCRPhase(name, phase string, timeout time.Duration) {
	EventuallyWithOffset(1, func(g Gomega) {
		cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", name,
			"-n", ciNamespace, "-o", "jsonpath={.status.phase}")
		output, err := utils.Run(cmd)
		g.Expect(err).NotTo(HaveOccurred())
		g.Expect(strings.TrimSpace(output)).To(Equal(phase),
			"CR phase is %s, expected %s", strings.TrimSpace(output), phase)
	}, timeout, pollingInterval).Should(Succeed(), "Timed out waiting for phase %s", phase)
}

// getCRPhase gets the current phase of a DPFHCPProvisioner.
func getCRPhase(ns, name string) string {
	cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", name,
		"-n", ns, "-o", "jsonpath={.status.phase}")
	output, err := utils.Run(cmd)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(output)
}

// conditionStatus represents a parsed condition from the CR status.
type conditionStatus struct {
	Type    string `json:"type"`
	Status  string `json:"status"`
	Reason  string `json:"reason"`
	Message string `json:"message"`
}

// getCRConditions retrieves all conditions from the CR status.
func getCRConditions(ns, name string) []conditionStatus {
	cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", name,
		"-n", ns, "-o", "jsonpath={.status.conditions}")
	output, err := utils.Run(cmd)
	if err != nil {
		return nil
	}
	trimmed := strings.TrimSpace(output)
	if trimmed == "" {
		return nil
	}
	var conditions []conditionStatus
	if err := json.Unmarshal([]byte(trimmed), &conditions); err != nil {
		return nil
	}
	return conditions
}

// getConditionStatus returns the status of a specific condition type.
func getConditionStatus(ns, name, conditionType string) string {
	conditions := getCRConditions(ns, name)
	for _, c := range conditions {
		if c.Type == conditionType {
			return c.Status
		}
	}
	return ""
}

// createDPUStub creates a minimal DPU CR for CSR auto-approval testing.
func createDPUStub(ns, name, phase string) {
	yaml := fmt.Sprintf(`apiVersion: provisioning.dpu.nvidia.com/v1alpha1
kind: DPU
metadata:
  name: %s
  namespace: %s
spec:
  dpuNodeName: %s
  dpuDeviceName: bf3-device
  bfb: e2e-bfb
  serialNumber: "E2E0000000001"
status:
  phase: "%s"
`, name, ns, name, phase)
	cmd := exec.Command("kubectl", "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPU stub")

	// Update status subresource
	statusPatch := fmt.Sprintf(`{"status":{"phase":"%s"}}`, phase)
	cmd = exec.Command("kubectl", "patch", "dpu", name, "-n", ns,
		"--type=merge", "--subresource=status", "-p", statusPatch)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to patch DPU status")
}

// deleteDPUStub deletes a DPU CR.
func deleteDPUStub(ns, name string) {
	cmd := exec.Command("kubectl", "delete", "dpu", name, "-n", ns, "--ignore-not-found=true")
	_, _ = utils.Run(cmd)
}

// getHostedClusterKubeconfig retrieves the kubeconfig for the HostedCluster.
func getHostedClusterKubeconfig(provisionerNS, provisionerName string) string {
	cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", provisionerName,
		"-n", provisionerNS, "-o", "jsonpath={.status.kubeConfigSecretRef.name}")
	secretName, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get kubeconfig secret name")
	secretName = strings.TrimSpace(secretName)
	ExpectWithOffset(1, secretName).NotTo(BeEmpty(), "Kubeconfig secret name is empty")

	// The kubeconfig secret is in the clusters namespace (same namespace as HostedCluster)
	cmd = exec.Command("kubectl", "get", "secret", secretName,
		"-n", dpuClusterNS, "-o", "jsonpath={.data.super-admin\\.conf}")
	b64Kubeconfig, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get kubeconfig data")
	return strings.TrimSpace(b64Kubeconfig)
}

// writeKubeconfigToFile writes base64-encoded kubeconfig to a temp file and returns the path.
func writeKubeconfigToFile(b64Kubeconfig string) string {
	cmd := exec.Command("bash", "-c", fmt.Sprintf("echo '%s' | base64 -d", b64Kubeconfig))
	kubeconfig, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to decode kubeconfig")

	kubeconfigFile := fmt.Sprintf("/tmp/e2e-hc-kubeconfig-%d", time.Now().UnixNano())
	err = os.WriteFile(kubeconfigFile, []byte(kubeconfig), 0600)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to write kubeconfig file")
	return kubeconfigFile
}

// createBootstrapCSRInHostedCluster creates a bootstrap CSR in the HostedCluster.
// Bootstrap CSRs come from the node-bootstrapper service account, not from the node itself.
func createBootstrapCSRInHostedCluster(kubeconfigFile, hostname string) string {
	csrName := fmt.Sprintf("e2e-bootstrap-csr-%s-%d", hostname, time.Now().UnixNano())

	// Generate key and CSR PEM with CN=system:node:<hostname>, O=system:nodes, no SANs
	keyFile := fmt.Sprintf("/tmp/e2e-csr-key-%d", time.Now().UnixNano())
	csrFile := fmt.Sprintf("/tmp/e2e-csr-%d.pem", time.Now().UnixNano())
	defer func() {
		os.Remove(keyFile)
		os.Remove(csrFile)
	}()

	cmd := exec.Command("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
		"-keyout", keyFile, "-out", csrFile,
		"-subj", fmt.Sprintf("/CN=system:node:%s/O=system:nodes", hostname))
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to generate bootstrap CSR")

	b64CSR := base64EncodeFile(csrFile)

	yaml := fmt.Sprintf(`apiVersion: certificates.k8s.io/v1
kind: CertificateSigningRequest
metadata:
  name: %s
spec:
  request: %s
  signerName: kubernetes.io/kube-apiserver-client-kubelet
  usages:
    - digital signature
    - client auth
`, csrName, b64CSR)

	// Use --as to impersonate the node-bootstrapper identity so the API server
	// sets the correct username and groups on the CSR.
	cmd = exec.Command("kubectl", "--kubeconfig", kubeconfigFile,
		"--as", "system:serviceaccount:openshift-machine-config-operator:node-bootstrapper",
		"--as-group", "system:serviceaccounts",
		"--as-group", "system:serviceaccounts:openshift-machine-config-operator",
		"--as-group", "system:authenticated",
		"apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create bootstrap CSR in hosted cluster")

	return csrName
}

// createServingCSRInHostedCluster creates a serving CSR in the HostedCluster.
// Serving CSRs come from the node itself (system:node:<hostname>).
func createServingCSRInHostedCluster(kubeconfigFile, hostname string) string {
	csrName := fmt.Sprintf("e2e-serving-csr-%s-%d", hostname, time.Now().UnixNano())

	// Generate key and CSR PEM with CN=system:node:<hostname>, O=system:nodes, DNS SAN=hostname
	keyFile := fmt.Sprintf("/tmp/e2e-csr-key-%d", time.Now().UnixNano())
	csrFile := fmt.Sprintf("/tmp/e2e-csr-%d.pem", time.Now().UnixNano())
	confFile := fmt.Sprintf("/tmp/e2e-csr-%d.cnf", time.Now().UnixNano())
	defer func() {
		os.Remove(keyFile)
		os.Remove(csrFile)
		os.Remove(confFile)
	}()

	// OpenSSL config to add DNS SAN
	conf := fmt.Sprintf(`[req]
req_extensions = v3_req
distinguished_name = req_distinguished_name
prompt = no

[req_distinguished_name]
CN = system:node:%s
O = system:nodes

[v3_req]
subjectAltName = DNS:%s
`, hostname, hostname)
	err := os.WriteFile(confFile, []byte(conf), 0600)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to write openssl config")

	cmd := exec.Command("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
		"-keyout", keyFile, "-out", csrFile,
		"-config", confFile)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to generate serving CSR")

	b64CSR := base64EncodeFile(csrFile)

	yaml := fmt.Sprintf(`apiVersion: certificates.k8s.io/v1
kind: CertificateSigningRequest
metadata:
  name: %s
spec:
  request: %s
  signerName: kubernetes.io/kubelet-serving
  usages:
    - digital signature
    - server auth
`, csrName, b64CSR)

	// Use --as to impersonate the node identity so the API server
	// sets the correct username and groups on the CSR.
	cmd = exec.Command("kubectl", "--kubeconfig", kubeconfigFile,
		"--as", fmt.Sprintf("system:node:%s", hostname),
		"--as-group", "system:nodes",
		"--as-group", "system:authenticated",
		"apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create serving CSR in hosted cluster")

	return csrName
}

// base64EncodeFile reads a file and returns its base64-encoded content.
func base64EncodeFile(path string) string {
	data, err := os.ReadFile(path)
	ExpectWithOffset(2, err).NotTo(HaveOccurred(), "Failed to read file %s", path)
	cmd := exec.Command("base64", "-w0")
	cmd.Stdin = strings.NewReader(string(data))
	output, err := cmd.CombinedOutput()
	ExpectWithOffset(2, err).NotTo(HaveOccurred(), "Failed to base64 encode")
	return strings.TrimSpace(string(output))
}

// waitForCSRApproval waits for a CSR to be approved in the HostedCluster.
func waitForCSRApproval(kubeconfigFile, csrName string, timeout time.Duration) {
	EventuallyWithOffset(1, func(g Gomega) {
		cmd := exec.Command("kubectl", "--kubeconfig", kubeconfigFile,
			"get", "csr", csrName,
			"-o", "jsonpath={.status.conditions[?(@.type==\"Approved\")].status}")
		output, err := utils.Run(cmd)
		g.Expect(err).NotTo(HaveOccurred())
		g.Expect(strings.TrimSpace(output)).To(Equal("True"), "CSR not yet approved")
	}, timeout, 5*time.Second).Should(Succeed(), "Timed out waiting for CSR %s approval", csrName)
}

// verifyCSRNotApproved verifies a CSR stays pending (not approved) for a duration.
func verifyCSRNotApproved(kubeconfigFile, csrName string, duration time.Duration) {
	ConsistentlyWithOffset(1, func(g Gomega) {
		cmd := exec.Command("kubectl", "--kubeconfig", kubeconfigFile,
			"get", "csr", csrName,
			"-o", "jsonpath={.status.conditions[?(@.type==\"Approved\")].status}")
		output, err := utils.Run(cmd)
		g.Expect(err).NotTo(HaveOccurred())
		g.Expect(strings.TrimSpace(output)).To(BeEmpty(), "CSR should not be approved")
	}, duration, 5*time.Second).Should(Succeed())
}

// createNodeInHostedCluster creates a Node object in the HostedCluster.
func createNodeInHostedCluster(kubeconfigFile, hostname string) {
	yaml := fmt.Sprintf(`apiVersion: v1
kind: Node
metadata:
  name: %s
`, hostname)
	cmd := exec.Command("kubectl", "--kubeconfig", kubeconfigFile, "apply", "-f", "-")
	cmd.Stdin = strings.NewReader(yaml)
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create Node in hosted cluster")
}

// deleteNodeInHostedCluster deletes a Node object from the HostedCluster.
func deleteNodeInHostedCluster(kubeconfigFile, hostname string) {
	cmd := exec.Command("kubectl", "--kubeconfig", kubeconfigFile,
		"delete", "node", hostname, "--ignore-not-found=true")
	_, _ = utils.Run(cmd)
}

// deleteCSRInHostedCluster deletes a CSR from the HostedCluster.
func deleteCSRInHostedCluster(kubeconfigFile, csrName string) {
	cmd := exec.Command("kubectl", "--kubeconfig", kubeconfigFile,
		"delete", "csr", csrName, "--ignore-not-found=true")
	_, _ = utils.Run(cmd)
}

// forceDeleteProvisioner deletes a DPFHCPProvisioner CR, removing finalizers if stuck.
func forceDeleteProvisioner(ns, name string) {
	// Check if CR exists
	cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", name,
		"-n", ns, "-o", "jsonpath={.metadata.name}")
	output, err := utils.Run(cmd)
	if err != nil || strings.TrimSpace(output) == "" {
		return // Doesn't exist
	}

	// Request deletion
	cmd = exec.Command("kubectl", "delete", "dpfhcpprovisioner", name,
		"-n", ns, "--timeout=5m")
	_, _ = utils.Run(cmd)

	// Check if it's gone
	cmd = exec.Command("kubectl", "get", "dpfhcpprovisioner", name, "-n", ns)
	_, err = utils.Run(cmd)
	if err != nil {
		return // Gone
	}

	// Still exists - remove finalizers
	_, _ = fmt.Fprintf(GinkgoWriter, "CR stuck in Deleting, removing finalizers...\n")
	cmd = exec.Command("kubectl", "patch", "dpfhcpprovisioner", name,
		"-n", ns, "--type=merge", "-p", `{"metadata":{"finalizers":[]}}`)
	_, _ = utils.Run(cmd)

	// Wait for it to be gone
	Eventually(func(g Gomega) {
		cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", name, "-n", ns)
		_, err := utils.Run(cmd)
		g.Expect(err).To(HaveOccurred())
	}, 1*time.Minute, 5*time.Second).Should(Succeed())

	// Clean up orphaned resources
	cmd = exec.Command("kubectl", "delete", "hostedcluster", "--all",
		"-n", ns, "--ignore-not-found=true", "--timeout=2m")
	_, _ = utils.Run(cmd)
	cmd = exec.Command("kubectl", "delete", "nodepool", "--all",
		"-n", ns, "--ignore-not-found=true", "--timeout=2m")
	_, _ = utils.Run(cmd)
}

// cleanupStaleResources removes leftover resources from previous failed test runs.
func cleanupStaleResources() {
	forceDeleteProvisioner(ciNamespace, provisionerName)
}

// dumpProvisionerStatus logs the current CR status for debugging.
func dumpProvisionerStatus(ns, name string) {
	cmd := exec.Command("kubectl", "get", "dpfhcpprovisioner", name,
		"-n", ns, "-o", "yaml")
	output, err := utils.Run(cmd)
	if err == nil {
		_, _ = fmt.Fprintf(GinkgoWriter, "DPFHCPProvisioner status:\n%s\n", output)
	}
}
