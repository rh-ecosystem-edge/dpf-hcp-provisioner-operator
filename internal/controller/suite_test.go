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
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes/scheme"
	"k8s.io/client-go/rest"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpucluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/finalizer"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/hostedcluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/kubeconfiginjection"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/secrets"
	// +kubebuilder:scaffold:imports
)

// These tests use Ginkgo (BDD-style Go testing framework). Refer to
// http://onsi.github.io/ginkgo/ to learn more about Ginkgo.

var (
	ctx        context.Context
	cancel     context.CancelFunc
	testEnv    *envtest.Environment
	cfg        *rest.Config
	k8sClient  client.Client
	k8sManager ctrl.Manager
)

// fakeRegistry implements bfocplookup.Registry for testing
// to avoid hitting real container registries in unit/integration tests.
// FetchLabels derives version labels from the image tag so any selected tag verifies.
type fakeRegistry struct {
	tags []string
}

func (f *fakeRegistry) ListTags(_ context.Context, _ name.Repository, _ authn.Keychain) ([]string, error) {
	return f.tags, nil
}

func (f *fakeRegistry) FetchLabels(_ context.Context, image string, _ authn.Keychain) (map[string]string, error) {
	idx := strings.LastIndex(image, ":")
	if idx < 0 {
		return map[string]string{}, nil
	}
	ocp, doca, found := strings.Cut(image[idx+1:], "-doca")
	if !found {
		return map[string]string{"OCP.version": ocp}, nil
	}
	return map[string]string{
		"OCP.version":         ocp,
		"NVIDIA.DOCA.version": doca,
	}, nil
}

func TestControllers(t *testing.T) {
	RegisterFailHandler(Fail)

	RunSpecs(t, "Controller Suite")
}

var _ = BeforeSuite(func() {
	logf.SetLogger(zap.New(zap.WriteTo(GinkgoWriter), zap.UseDevMode(true)))

	ctx, cancel = context.WithCancel(context.TODO())

	var err error
	err = provisioningv1alpha1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	err = dpuprovisioningv1alpha1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	err = dpuservicev1alpha1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	err = hyperv1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	err = dpuservicev1alpha1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	err = operatorv1alpha1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	// +kubebuilder:scaffold:scheme

	docaPlatformDir, err := docaPlatformModuleDir()
	Expect(err).NotTo(HaveOccurred())

	By("bootstrapping test environment")
	testEnv = &envtest.Environment{
		CRDDirectoryPaths: []string{
			filepath.Join("..", "..", "config", "crd", "bases"),
			filepath.Join("testdata", "crds"), // External CRDs for testing (DPUCluster, etc.)
			// DPFOperatorConfig CRD as shipped by the imported doca-platform module
			filepath.Join(docaPlatformDir, "config", "operator-crds", "operator.dpu.nvidia.com_dpfoperatorconfigs.yaml"),
		},
		ErrorIfCRDPathMissing: true,
	}

	// Retrieve the first found binary directory to allow running tests from IDEs
	if getFirstFoundEnvTestBinaryDir() != "" {
		testEnv.BinaryAssetsDirectory = getFirstFoundEnvTestBinaryDir()
	}

	// cfg is defined in this file globally.
	cfg, err = testEnv.Start()
	Expect(err).NotTo(HaveOccurred())
	Expect(cfg).NotTo(BeNil())

	k8sClient, err = client.New(cfg, client.Options{Scheme: scheme.Scheme})
	Expect(err).NotTo(HaveOccurred())
	Expect(k8sClient).NotTo(BeNil())

	By("creating controller manager")
	k8sManager, err = ctrl.NewManager(cfg, ctrl.Options{
		Scheme: scheme.Scheme,
	})
	Expect(err).NotTo(HaveOccurred())

	By("creating clusters namespace")
	clustersNs := &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{
			Name: "clusters",
		},
	}
	err = k8sClient.Create(ctx, clustersNs)
	Expect(err).NotTo(HaveOccurred())

	By("creating dpf-operator-system namespace")
	dpfNs := &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{
			Name: "dpf-operator-system",
		},
	}
	err = k8sClient.Create(ctx, dpfNs)
	Expect(err).NotTo(HaveOccurred())

	By("creating DPFOperatorConfig CR")
	dpfVersion := "v26.4.1"
	dpfConfig := &operatorv1alpha1.DPFOperatorConfig{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "dpfoperatorconfig",
			Namespace: "dpf-operator-system",
		},
		Spec: operatorv1alpha1.DPFOperatorConfigSpec{
			DeploymentMode:         operatorv1alpha1.DeploymentModeHostTrusted,
			ProvisioningController: &operatorv1alpha1.ProvisioningControllerConfiguration{},
		},
	}
	err = k8sClient.Create(ctx, dpfConfig)
	Expect(err).NotTo(HaveOccurred())
	dpfConfig.Status.Version = &dpfVersion
	err = k8sClient.Status().Update(ctx, dpfConfig)
	Expect(err).NotTo(HaveOccurred())

	By("creating DPFHCPProvisionerConfig CR")
	configCR := &provisioningv1alpha1.DPFHCPProvisionerConfig{
		ObjectMeta: metav1.ObjectMeta{
			Name: provisioningv1alpha1.DefaultConfigName,
		},
		Spec: provisioningv1alpha1.DPFHCPProvisionerConfigSpec{},
	}
	err = k8sClient.Create(ctx, configCR)
	Expect(err).NotTo(HaveOccurred())

	By("setting up DPFHCPProvisioner controller")
	kubeconfigInjector := kubeconfiginjection.NewKubeconfigInjector(k8sManager.GetClient(), k8sManager.GetEventRecorderFor(common.ProvisionerControllerName))

	// Initialize Finalizer Manager with pluggable cleanup handlers
	finalizerManager := finalizer.NewManager(k8sManager.GetClient(), k8sManager.GetEventRecorderFor(common.ProvisionerControllerName))
	// Register cleanup handlers in order (dependent resources first)
	finalizerManager.RegisterHandler(kubeconfiginjection.NewCleanupHandler(k8sManager.GetClient(), k8sManager.GetEventRecorderFor(common.ProvisionerControllerName)))
	finalizerManager.RegisterHandler(hostedcluster.NewCleanupHandler(k8sManager.GetClient(), k8sManager.GetEventRecorderFor(common.ProvisionerControllerName)))

	reconciler := &DPFHCPProvisionerReconciler{
		Client:   k8sManager.GetClient(),
		Scheme:   k8sManager.GetScheme(),
		Recorder: k8sManager.GetEventRecorderFor(common.ProvisionerControllerName),
		ImageLookup: func() *bfocplookup.ImageLookup {
			lookup := bfocplookup.NewImageLookup(k8sManager.GetClient(), k8sManager.GetEventRecorderFor("bluefield-ocp-layer-lookup"))
			lookup.Registry = &fakeRegistry{tags: []string{
				"4.17.0", "4.17.0-doca3.4.2",
				"4.19.0", "4.19.0-doca3.4.2",
				"4.19.0-ec.5", "4.19.0-ec.5-doca3.4.2",
				"4.19.0-ec.6", "4.19.0-ec.6-doca3.4.2",
				"4.22.11", "4.22.11-doca3.4.2",
			}}
			return lookup
		}(),
		DPUClusterValidator:  dpucluster.NewValidator(k8sManager.GetClient(), k8sManager.GetEventRecorderFor("dpucluster-validator")),
		SecretsValidator:     secrets.NewValidator(k8sManager.GetClient(), k8sManager.GetEventRecorderFor("secrets-validator")),
		SecretManager:        hostedcluster.NewSecretManager(k8sManager.GetClient(), k8sManager.GetScheme()),
		NodePoolManager:      hostedcluster.NewNodePoolManager(k8sManager.GetClient(), k8sManager.GetScheme()),
		HostedClusterManager: hostedcluster.NewHostedClusterManager(k8sManager.GetClient(), k8sManager.GetScheme()),
		FinalizerManager:     finalizerManager,
		StatusSyncer:         hostedcluster.NewStatusSyncer(k8sManager.GetClient()),
		KubeconfigInjector:   kubeconfigInjector,
	}
	err = reconciler.SetupWithManager(k8sManager)
	Expect(err).NotTo(HaveOccurred())

	By("starting controller manager")
	go func() {
		defer GinkgoRecover()
		err = k8sManager.Start(ctx)
		Expect(err).NotTo(HaveOccurred())
	}()
})

var _ = AfterSuite(func() {
	By("tearing down the test environment")
	cancel()
	err := testEnv.Stop()
	Expect(err).NotTo(HaveOccurred())
})

// docaPlatformModuleDir returns the module cache directory of the imported
// doca-platform module, so envtest can load the CRDs it ships. The module is
// vendored, but vendoring only copies Go packages, not the CRD manifests.
func docaPlatformModuleDir() (string, error) {
	// On failure go still prints the JSON, with the reason in its Error field
	out, runErr := exec.Command("go", "mod", "download", "-json", "github.com/nvidia/doca-platform").Output()
	var mod struct{ Dir, Error string }
	if err := json.Unmarshal(out, &mod); err != nil {
		return "", fmt.Errorf("go mod download github.com/nvidia/doca-platform: %v (output: %q)", errors.Join(runErr, err), out)
	}
	if mod.Error != "" || mod.Dir == "" {
		return "", fmt.Errorf("go mod download github.com/nvidia/doca-platform: %s", mod.Error)
	}
	return mod.Dir, nil
}

// getFirstFoundEnvTestBinaryDir locates the first binary in the specified path.
// ENVTEST-based tests depend on specific binaries, usually located in paths set by
// controller-runtime. When running tests directly (e.g., via an IDE) without using
// Makefile targets, the 'BinaryAssetsDirectory' must be explicitly configured.
//
// This function streamlines the process by finding the required binaries, similar to
// setting the 'KUBEBUILDER_ASSETS' environment variable. To ensure the binaries are
// properly set up, run 'make setup-envtest' beforehand.
func getFirstFoundEnvTestBinaryDir() string {
	basePath := filepath.Join("..", "..", "bin", "k8s")
	entries, err := os.ReadDir(basePath)
	if err != nil {
		logf.Log.Error(err, "Failed to read directory", "path", basePath)
		return ""
	}
	for _, entry := range entries {
		if entry.IsDir() {
			return filepath.Join(basePath, entry.Name())
		}
	}
	return ""
}
