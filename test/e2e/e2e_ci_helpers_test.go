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
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"time"

	dpuservicev1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	operatorv1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	certificatesv1 "k8s.io/api/certificates/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/apis/meta/v1/unstructured"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
	"k8s.io/client-go/tools/clientcmd"
	"sigs.k8s.io/controller-runtime/pkg/client"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
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
	ctx := context.Background()
	clusterVersion := &unstructured.Unstructured{}
	clusterVersion.SetGroupVersionKind(schema.GroupVersionKind{
		Group:   "config.openshift.io",
		Version: "v1",
		Kind:    "ClusterVersion",
	})
	err := k8sClient.Get(ctx, types.NamespacedName{Name: "version"}, clusterVersion)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get ClusterVersion")

	releaseImage, found, err := unstructured.NestedString(clusterVersion.Object, "status", "desired", "image")
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to extract release image")
	ExpectWithOffset(1, found).To(BeTrue(), "Release image not found in ClusterVersion status")
	return releaseImage
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
	ctx := context.Background()
	scList := &unstructured.UnstructuredList{}
	scList.SetGroupVersionKind(schema.GroupVersionKind{
		Group:   "storage.k8s.io",
		Version: "v1",
		Kind:    "StorageClassList",
	})
	err := k8sClient.List(ctx, scList)
	if err == nil {
		for _, item := range scList.Items {
			annotations := item.GetAnnotations()
			if annotations["storageclass.kubernetes.io/is-default-class"] == "true" {
				return item.GetName()
			}
		}
	}
	Fail("No default StorageClass found and ETCD_STORAGE_CLASS not set. " +
		"A StorageClass is required for HostedCluster etcd PVCs. " +
		"On AWS IPI clusters this is available by default. " +
		"For local clusters, deploy a storage provider first.")
	return ""
}

// createNamespace creates a Kubernetes namespace, waiting for it to be fully deleted first if terminating.
func createNamespace(ns string) {
	ctx := context.Background()
	// Wait for namespace to be fully gone if it's terminating from a previous run
	Eventually(func(g Gomega) {
		namespace := &corev1.Namespace{}
		err := k8sClient.Get(ctx, types.NamespacedName{Name: ns}, namespace)
		if apierrors.IsNotFound(err) {
			return // Namespace doesn't exist, good
		}
		g.Expect(err).NotTo(HaveOccurred())
		g.Expect(namespace.Status.Phase).NotTo(Equal(corev1.NamespaceTerminating),
			"Namespace %s is still terminating", ns)
	}, 2*time.Minute, 5*time.Second).Should(Succeed())

	namespace := &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{
			Name: ns,
		},
	}
	err := k8sClient.Create(ctx, namespace)
	if err != nil && !apierrors.IsAlreadyExists(err) {
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create namespace %s", ns)
	}
}

// createDPUClusterStub creates a minimal DPUCluster CR for testing.
func createDPUClusterStub(ns, name string) {
	ctx := context.Background()
	dpuCluster := &dpuprovisioningv1.DPUCluster{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
		Spec: dpuprovisioningv1.DPUClusterSpec{
			Type: string(dpuprovisioningv1.StaticCluster),
		},
	}
	err := k8sClient.Create(ctx, dpuCluster)
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
		_ = os.Remove(keyFile)
		_ = os.Remove(keyFile + ".pub")
	}()

	pubKeyData, err := os.ReadFile(keyFile + ".pub")
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to read public key")

	ctx := context.Background()
	secret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
		Data: map[string][]byte{
			"id_rsa.pub": pubKeyData,
		},
	}
	err = k8sClient.Create(ctx, secret)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create SSH key secret")
}

// copyPullSecret copies the cluster pull secret to the target namespace.
func copyPullSecret(ns, name string) {
	By("copying pull secret from openshift-config")
	ctx := context.Background()

	sourceSecret := &corev1.Secret{}
	err := k8sClient.Get(ctx, types.NamespacedName{
		Namespace: "openshift-config",
		Name:      "pull-secret",
	}, sourceSecret)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get cluster pull secret")

	dockerConfigJSON := sourceSecret.Data[".dockerconfigjson"]
	ExpectWithOffset(1, dockerConfigJSON).NotTo(BeNil(), "Pull secret missing .dockerconfigjson")

	targetSecret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
		Type: corev1.SecretTypeDockerConfigJson,
		Data: map[string][]byte{
			".dockerconfigjson": dockerConfigJSON,
		},
	}
	err = k8sClient.Create(ctx, targetSecret)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create pull secret")
}

// createDPUDeploymentStub creates a minimal DPUDeployment CR that references a DPUFlavor.
func createDPUDeploymentStub(ns, name, flavorName string) {
	ctx := context.Background()
	dpuDeployment := &dpuservicev1.DPUDeployment{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
		Spec: dpuservicev1.DPUDeploymentSpec{
			DPUs: dpuservicev1.DPUs{
				BFB:    "e2e-bfb",
				Flavor: flavorName,
			},
			Services: map[string]dpuservicev1.DPUDeploymentServiceConfiguration{},
			ServiceChains: dpuservicev1.ServiceChains{
				UpgradePolicy: dpuservicev1.UpgradePolicy{},
				Switches: []dpuservicev1.DPUDeploymentSwitch{
					{
						Ports: []dpuservicev1.DPUDeploymentPort{
							{
								ServiceInterface: &dpuservicev1.ServiceIfc{
									MatchLabels: map[string]string{
										"e2e-test": "true",
									},
								},
							},
						},
					},
				},
			},
		},
	}
	err := k8sClient.Create(ctx, dpuDeployment)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPUDeployment stub")
}

// createDPUFlavorStub creates a minimal DPUFlavor CR.
func createDPUFlavorStub(ns, name string) {
	ctx := context.Background()
	dpuFlavor := &dpuprovisioningv1.DPUFlavor{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
		Spec: dpuprovisioningv1.DPUFlavorSpec{
			OVS: dpuprovisioningv1.DPUFlavorOVS{
				RawConfigScript: "#!/bin/bash\necho \"e2e test OVS config\"\n",
			},
		},
	}
	err := k8sClient.Create(ctx, dpuFlavor)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPUFlavor stub")
}

// createDPFOperatorConfig creates a DPFOperatorConfig in the DPUCluster namespace
// (needed by the ignition generator for controlPlaneMTU and BFCFGTemplateConfigMap).
func createDPFOperatorConfig(ns string) {
	ctx := context.Background()
	mtu := 1500
	disable := true
	dpfOperatorConfig := &operatorv1.DPFOperatorConfig{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "dpfoperatorconfig",
			Namespace: ns,
		},
		Spec: operatorv1.DPFOperatorConfigSpec{
			ProvisioningController: operatorv1.ProvisioningControllerConfiguration{
				BFBPersistentVolumeClaimName: "e2e-bfb-pvc",
				Disable:                      &disable,
			},
			Networking: &operatorv1.Networking{
				ControlPlaneMTU: &mtu,
			},
		},
	}
	err := k8sClient.Create(ctx, dpfOperatorConfig)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPFOperatorConfig")
}

// createDPFHCPProvisioner creates a DPFHCPProvisioner CR.
func createDPFHCPProvisioner(ns, name string) {
	ctx := context.Background()
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

	availabilityPolicy := hyperv1.AvailabilityPolicy(availability)

	provisioner := &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
				Name:      dpuClusterName,
				Namespace: dpuClusterNS,
			},
			BaseDomain:      baseDomain,
			OCPReleaseImage: releaseImage,
			SSHKeySecretRef: corev1.LocalObjectReference{
				Name: sshKeySecretName,
			},
			PullSecretRef: corev1.LocalObjectReference{
				Name: pullSecretName,
			},
			ControlPlaneAvailabilityPolicy: availabilityPolicy,
			DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
				Name:      dpuDeploymentName,
				Namespace: dpuClusterNS,
			},
		},
	}

	if machineOSURL != "" {
		provisioner.Spec.MachineOSURL = machineOSURL
	}
	if etcdSC != "" {
		provisioner.Spec.EtcdStorageClass = etcdSC
	}

	err := k8sClient.Create(ctx, provisioner)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPFHCPProvisioner")
}

// waitForCRPhase waits for a DPFHCPProvisioner to reach a specific phase.
func waitForCRPhase(name, phase string, timeout time.Duration) {
	EventuallyWithOffset(1, func(g Gomega) {
		currentPhase := getCRPhase(ciNamespace, name)
		g.Expect(currentPhase).To(Equal(phase),
			"CR phase is %s, expected %s", currentPhase, phase)
	}, timeout, pollingInterval).Should(Succeed(), "Timed out waiting for phase %s", phase)
}

// getCRPhase gets the current phase of a DPFHCPProvisioner.
func getCRPhase(ns, name string) string {
	ctx := context.Background()
	provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
	err := k8sClient.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, provisioner)
	if err != nil {
		return ""
	}
	return string(provisioner.Status.Phase)
}

// getCRConditions retrieves all conditions from the CR status.
func getCRConditions(ns, name string) []metav1.Condition {
	ctx := context.Background()
	provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
	err := k8sClient.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, provisioner)
	if err != nil {
		return nil
	}
	return provisioner.Status.Conditions
}

// getConditionStatus returns the status of a specific condition type.
func getConditionStatus(ns, name, conditionType string) string {
	conditions := getCRConditions(ns, name)
	for _, c := range conditions {
		if c.Type == conditionType {
			return string(c.Status)
		}
	}
	return ""
}

// createDPUStub creates a minimal DPU CR for CSR auto-approval testing.
func createDPUStub(ns, name, phase string) {
	ctx := context.Background()
	dpu := &dpuprovisioningv1.DPU{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
		Spec: dpuprovisioningv1.DPUSpec{
			DPUNodeName:   name,
			DPUDeviceName: "bf3-device",
			BFB:           "e2e-bfb",
			SerialNumber:  "E2E0000000001",
		},
	}
	err := k8sClient.Create(ctx, dpu)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create DPU stub")

	// Re-fetch to get the latest state before updating status
	err = k8sClient.Get(ctx, types.NamespacedName{Name: name, Namespace: ns}, dpu)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get DPU after creation")

	// Update status subresource
	dpu.Status.Phase = dpuprovisioningv1.DPUPhase(phase)
	err = k8sClient.Status().Update(ctx, dpu)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to update DPU status")
}

// deleteDPUStub deletes a DPU CR.
func deleteDPUStub(ns, name string) {
	ctx := context.Background()
	dpu := &dpuprovisioningv1.DPU{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
		},
	}
	// Equivalent to kubectl delete --ignore-not-found=true
	if err := client.IgnoreNotFound(k8sClient.Delete(ctx, dpu)); err != nil {
		warnError("failed to delete DPU %s/%s: %v", ns, name, err)
	}
}

// getHostedClusterKubeconfig retrieves the kubeconfig for the HostedCluster.
func getHostedClusterKubeconfig(provisionerNS, provisionerName string) string {
	ctx := context.Background()
	provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
	err := k8sClient.Get(ctx, types.NamespacedName{Namespace: provisionerNS, Name: provisionerName}, provisioner)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get DPFHCPProvisioner")

	secretName := provisioner.Status.KubeConfigSecretRef.Name
	ExpectWithOffset(1, secretName).NotTo(BeEmpty(), "Kubeconfig secret name is empty")

	// The kubeconfig secret is in the DPUCluster namespace (injected by controller)
	secret := &corev1.Secret{}
	err = k8sClient.Get(ctx, types.NamespacedName{
		Namespace: dpuClusterNS,
		Name:      secretName,
	}, secret)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to get kubeconfig secret")

	b64Kubeconfig := secret.Data["super-admin.conf"]
	ExpectWithOffset(1, b64Kubeconfig).NotTo(BeNil(), "super-admin.conf not found in secret")
	return base64.StdEncoding.EncodeToString(b64Kubeconfig)
}

// writeKubeconfigToFile writes base64-encoded kubeconfig to a temp file and returns the path.
// The file is cleaned up in AfterAll when the entire test suite completes.
func writeKubeconfigToFile(b64Kubeconfig string) string {
	kubeconfig, err := base64.StdEncoding.DecodeString(b64Kubeconfig)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to decode kubeconfig")

	f, err := os.CreateTemp("", "e2e-hc-kubeconfig-*")
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create temp kubeconfig file")

	_, err = f.Write(kubeconfig)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to write kubeconfig file")
	err = f.Close()
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to close kubeconfig file")
	return f.Name()
}

// warnError logs a warning message for cleanup errors that are non-critical.
// These warnings appear in test output but don't fail the test.
func warnError(format string, args ...interface{}) {
	_, _ = fmt.Fprintf(GinkgoWriter, "Warning: "+format+"\n", args...)
}

// loadHCConfig loads the rest.Config from a kubeconfig file path.
// This should be called once and the config reused.
func loadHCConfig(kubeconfigFile string) *rest.Config {
	config, err := clientcmd.BuildConfigFromFlags("", kubeconfigFile)
	ExpectWithOffset(2, err).NotTo(HaveOccurred(), "Failed to build config from kubeconfig file")
	return config
}

// getHCClient creates a Kubernetes client from a rest.Config.
func getHCClient(config *rest.Config) (client.Client, *kubernetes.Clientset) {
	hcClient, err := client.New(config, client.Options{Scheme: runtimeScheme})
	ExpectWithOffset(2, err).NotTo(HaveOccurred(), "Failed to create HC client")

	hcClientset, err := kubernetes.NewForConfig(config)
	ExpectWithOffset(2, err).NotTo(HaveOccurred(), "Failed to create HC clientset")

	return hcClient, hcClientset
}

// getHCClientWithImpersonation creates a client for the HostedCluster with user impersonation.
// This allows creating resources (like CSRs) that appear to come from a specific user/group.
func getHCClientWithImpersonation(baseConfig *rest.Config, username string, groups []string) client.Client {
	// Clone the config to avoid mutating the shared instance
	config := rest.CopyConfig(baseConfig)

	// Set impersonation to mimic the desired user identity
	config.Impersonate = rest.ImpersonationConfig{
		UserName: username,
		Groups:   groups,
	}

	hcClient, err := client.New(config, client.Options{Scheme: runtimeScheme})
	ExpectWithOffset(2, err).NotTo(HaveOccurred(), "Failed to create HC client with impersonation")

	return hcClient
}

// createBootstrapCSRInHostedCluster creates a bootstrap CSR in the HostedCluster.
// Bootstrap CSRs come from the node-bootstrapper service account, not from the node itself.
func createBootstrapCSRInHostedCluster(hcConfig *rest.Config, hostname string) string {
	ctx := context.Background()
	csrName := fmt.Sprintf("e2e-bootstrap-csr-%s-%d", hostname, time.Now().UnixNano())

	// Generate key and CSR PEM with CN=system:node:<hostname>, O=system:nodes, no SANs
	keyFile := fmt.Sprintf("/tmp/e2e-csr-key-%d", time.Now().UnixNano())
	csrFile := fmt.Sprintf("/tmp/e2e-csr-%d.pem", time.Now().UnixNano())
	defer func() {
		_ = os.Remove(keyFile)
		_ = os.Remove(csrFile)
	}()

	cmd := exec.Command("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
		"-keyout", keyFile, "-out", csrFile,
		"-subj", fmt.Sprintf("/CN=system:node:%s/O=system:nodes", hostname))
	_, err := utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to generate bootstrap CSR")

	b64CSR := base64EncodeFile(csrFile)
	csrBytes, err := base64.StdEncoding.DecodeString(b64CSR)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to decode CSR")

	// Create client with impersonation to mimic node-bootstrapper service account
	hcClient := getHCClientWithImpersonation(hcConfig,
		"system:serviceaccount:openshift-machine-config-operator:node-bootstrapper",
		[]string{
			"system:serviceaccounts",
			"system:serviceaccounts:openshift-machine-config-operator",
			"system:authenticated",
		})

	csr := &certificatesv1.CertificateSigningRequest{
		ObjectMeta: metav1.ObjectMeta{
			Name: csrName,
		},
		Spec: certificatesv1.CertificateSigningRequestSpec{
			Request:    csrBytes,
			SignerName: "kubernetes.io/kube-apiserver-client-kubelet",
			Usages: []certificatesv1.KeyUsage{
				certificatesv1.UsageDigitalSignature,
				certificatesv1.UsageClientAuth,
			},
			// Note: Username and Groups are server-populated based on the authenticated user.
			// We use impersonation above to set the identity.
		},
	}

	err = hcClient.Create(ctx, csr)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create bootstrap CSR in hosted cluster")

	return csrName
}

// createServingCSRInHostedCluster creates a serving CSR in the HostedCluster.
// Serving CSRs come from the node itself (system:node:<hostname>).
func createServingCSRInHostedCluster(hcConfig *rest.Config, hostname string) string {
	ctx := context.Background()
	csrName := fmt.Sprintf("e2e-serving-csr-%s-%d", hostname, time.Now().UnixNano())

	// Generate key and CSR PEM with CN=system:node:<hostname>, O=system:nodes, DNS SAN=hostname
	keyFile := fmt.Sprintf("/tmp/e2e-csr-key-%d", time.Now().UnixNano())
	csrFile := fmt.Sprintf("/tmp/e2e-csr-%d.pem", time.Now().UnixNano())
	confFile := fmt.Sprintf("/tmp/e2e-csr-%d.cnf", time.Now().UnixNano())
	defer func() {
		_ = os.Remove(keyFile)
		_ = os.Remove(csrFile)
		_ = os.Remove(confFile)
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
	csrBytes, err := base64.StdEncoding.DecodeString(b64CSR)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to decode CSR")

	// Create client with impersonation to mimic the node itself
	hcClient := getHCClientWithImpersonation(hcConfig,
		fmt.Sprintf("system:node:%s", hostname),
		[]string{
			"system:nodes",
			"system:authenticated",
		})

	csr := &certificatesv1.CertificateSigningRequest{
		ObjectMeta: metav1.ObjectMeta{
			Name: csrName,
		},
		Spec: certificatesv1.CertificateSigningRequestSpec{
			Request:    csrBytes,
			SignerName: "kubernetes.io/kubelet-serving",
			Usages: []certificatesv1.KeyUsage{
				certificatesv1.UsageDigitalSignature,
				certificatesv1.UsageServerAuth,
			},
			// Note: Username and Groups are server-populated based on the authenticated user.
			// We use impersonation above to set the identity.
		},
	}

	err = hcClient.Create(ctx, csr)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create serving CSR in hosted cluster")

	return csrName
}

// base64EncodeFile reads a file and returns its base64-encoded content.
func base64EncodeFile(path string) string {
	data, err := os.ReadFile(path)
	ExpectWithOffset(2, err).NotTo(HaveOccurred(), "Failed to read file %s", path)
	return base64.StdEncoding.EncodeToString(data)
}

// waitForCSRApproval waits for a CSR to be approved in the HostedCluster.
func waitForCSRApproval(hcConfig *rest.Config, csrName string, timeout time.Duration) {
	ctx := context.Background()
	hcClient, _ := getHCClient(hcConfig)

	EventuallyWithOffset(1, func(g Gomega) {
		csr := &certificatesv1.CertificateSigningRequest{}
		err := hcClient.Get(ctx, types.NamespacedName{Name: csrName}, csr)
		g.Expect(err).NotTo(HaveOccurred())

		approved := false
		for _, condition := range csr.Status.Conditions {
			if condition.Type == certificatesv1.CertificateApproved {
				approved = true
				break
			}
		}
		g.Expect(approved).To(BeTrue(), "CSR not yet approved")
	}, timeout, 5*time.Second).Should(Succeed(), "Timed out waiting for CSR %s approval", csrName)
}

// verifyCSRNotApproved verifies a CSR stays pending (not approved) for a duration.
func verifyCSRNotApproved(hcConfig *rest.Config, csrName string, duration time.Duration) {
	ctx := context.Background()
	hcClient, _ := getHCClient(hcConfig)

	ConsistentlyWithOffset(1, func(g Gomega) {
		csr := &certificatesv1.CertificateSigningRequest{}
		err := hcClient.Get(ctx, types.NamespacedName{Name: csrName}, csr)
		g.Expect(err).NotTo(HaveOccurred())

		for _, condition := range csr.Status.Conditions {
			if condition.Type == certificatesv1.CertificateApproved {
				g.Expect(false).To(BeTrue(), "CSR should not be approved")
			}
		}
	}, duration, 5*time.Second).Should(Succeed())
}

// createNodeInHostedCluster creates a Node object in the HostedCluster.
func createNodeInHostedCluster(hcConfig *rest.Config, hostname string) {
	ctx := context.Background()
	hcClient, _ := getHCClient(hcConfig)

	node := &corev1.Node{
		ObjectMeta: metav1.ObjectMeta{
			Name: hostname,
		},
	}
	err := hcClient.Create(ctx, node)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to create Node in hosted cluster")
}

// deleteNodeInHostedCluster deletes a Node object from the HostedCluster.
func deleteNodeInHostedCluster(hcConfig *rest.Config, hostname string) {
	ctx := context.Background()
	hcClient, _ := getHCClient(hcConfig)

	node := &corev1.Node{
		ObjectMeta: metav1.ObjectMeta{
			Name: hostname,
		},
	}
	if err := client.IgnoreNotFound(hcClient.Delete(ctx, node)); err != nil {
		warnError("failed to delete Node %s: %v", hostname, err)
	}
}

// deleteCSRInHostedCluster deletes a CSR from the HostedCluster.
func deleteCSRInHostedCluster(hcConfig *rest.Config, csrName string) {
	ctx := context.Background()
	hcClient, _ := getHCClient(hcConfig)

	csr := &certificatesv1.CertificateSigningRequest{
		ObjectMeta: metav1.ObjectMeta{
			Name: csrName,
		},
	}
	if err := client.IgnoreNotFound(hcClient.Delete(ctx, csr)); err != nil {
		warnError("failed to delete CSR %s: %v", csrName, err)
	}
}

// forceDeleteProvisioner deletes a DPFHCPProvisioner CR, removing finalizers if stuck.
func forceDeleteProvisioner(ns, name string) {
	ctx := context.Background()
	provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}

	// Check if CR exists
	err := k8sClient.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, provisioner)
	if apierrors.IsNotFound(err) {
		return // Doesn't exist
	}

	// Request deletion with 5m timeout context
	deleteCtx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	if err := client.IgnoreNotFound(k8sClient.Delete(deleteCtx, provisioner)); err != nil {
		warnError("failed to delete provisioner %s/%s: %v", ns, name, err)
	}

	// Check if it's gone
	err = k8sClient.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, provisioner)
	if apierrors.IsNotFound(err) {
		return // Gone
	}

	// Still exists - remove finalizers
	_, _ = fmt.Fprintf(GinkgoWriter, "CR stuck in Deleting, removing finalizers...\n")
	provisioner.SetFinalizers([]string{})
	if err := k8sClient.Update(ctx, provisioner); err != nil {
		warnError("failed to remove finalizers: %v", err)
	}

	// Wait for it to be gone
	Eventually(func(g Gomega) {
		err := k8sClient.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, provisioner)
		g.Expect(apierrors.IsNotFound(err)).To(BeTrue())
	}, 1*time.Minute, 5*time.Second).Should(Succeed())

	// Clean up orphaned resources
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &hyperv1.HostedCluster{},
		client.InNamespace(ns))); err != nil {
		warnError("failed to delete orphaned HostedClusters: %v", err)
	}
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &hyperv1.NodePool{},
		client.InNamespace(ns))); err != nil {
		warnError("failed to delete orphaned NodePools: %v", err)
	}
}

// cleanupStaleResources removes leftover resources from previous failed test runs.
func cleanupStaleResources() {
	ctx := context.Background()

	// Delete the provisioner first (this should cascade delete HostedCluster and NodePools)
	forceDeleteProvisioner(ciNamespace, provisionerName)

	// Clean up any orphaned HostedClusters and NodePools (best effort)
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &hyperv1.HostedCluster{},
		client.InNamespace(ciNamespace))); err != nil {
		warnError("failed to cleanup orphaned HostedClusters: %v", err)
	}
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &hyperv1.NodePool{},
		client.InNamespace(ciNamespace))); err != nil {
		warnError("failed to cleanup orphaned NodePools: %v", err)
	}

	// Clean up DPU resources in DPUCluster namespace (best effort)
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &dpuprovisioningv1.DPU{},
		client.InNamespace(dpuClusterNS))); err != nil {
		warnError("failed to cleanup DPUs: %v", err)
	}
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &dpuprovisioningv1.DPUCluster{},
		client.InNamespace(dpuClusterNS))); err != nil {
		warnError("failed to cleanup DPUClusters: %v", err)
	}
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &dpuprovisioningv1.DPUFlavor{},
		client.InNamespace(dpuClusterNS))); err != nil {
		warnError("failed to cleanup DPUFlavors: %v", err)
	}
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &dpuservicev1.DPUDeployment{},
		client.InNamespace(dpuClusterNS))); err != nil {
		warnError("failed to cleanup DPUDeployments: %v", err)
	}
	if err := client.IgnoreNotFound(k8sClient.DeleteAllOf(ctx, &operatorv1.DPFOperatorConfig{},
		client.InNamespace(dpuClusterNS))); err != nil {
		warnError("failed to cleanup DPFOperatorConfigs: %v", err)
	}

	// Clean up secrets in operator namespace (best effort)
	secret := &corev1.Secret{}
	secret.SetName(sshKeySecretName)
	secret.SetNamespace(ciNamespace)
	if err := client.IgnoreNotFound(k8sClient.Delete(ctx, secret)); err != nil {
		warnError("failed to cleanup SSH key secret: %v", err)
	}

	secret = &corev1.Secret{}
	secret.SetName(pullSecretName)
	secret.SetNamespace(ciNamespace)
	if err := client.IgnoreNotFound(k8sClient.Delete(ctx, secret)); err != nil {
		warnError("failed to cleanup pull secret: %v", err)
	}

	// Give resources time to clean up
	time.Sleep(2 * time.Second)
}

// dumpProvisionerStatus logs the current CR status for debugging.
func dumpProvisionerStatus(ns, name string) {
	ctx := context.Background()
	provisioner := &provisioningv1alpha1.DPFHCPProvisioner{}
	err := k8sClient.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, provisioner)
	if err == nil {
		output, _ := json.MarshalIndent(provisioner, "", "  ")
		_, _ = fmt.Fprintf(GinkgoWriter, "DPFHCPProvisioner status:\n%s\n", string(output))
	}
}
