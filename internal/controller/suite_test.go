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
	"os"
	"path/filepath"
	"testing"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes/scheme"
	"k8s.io/client-go/rest"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpucluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/finalizer"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/hostedcluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/kubeconfiginjection"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/metallb"
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

// fakeImageChecker implements bfocplookup.ImageChecker for testing
// to avoid hitting real container registries in unit/integration tests.
type fakeImageChecker struct{}

func (f *fakeImageChecker) CheckTag(_ context.Context, _ name.Reference, _ authn.Keychain) error {
	return nil
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

	err = hyperv1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	err = metallbv1beta1.AddToScheme(scheme.Scheme)
	Expect(err).NotTo(HaveOccurred())

	// +kubebuilder:scaffold:scheme

	By("bootstrapping test environment")
	testEnv = &envtest.Environment{
		CRDDirectoryPaths: []string{
			filepath.Join("..", "..", "config", "crd", "bases"),
			filepath.Join("testdata", "crds"), // External CRDs for testing (DPUCluster, etc.)
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

	By("creating openshift-operators namespace")
	openshiftOperatorsNs := &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{
			Name: "openshift-operators",
		},
	}
	err = k8sClient.Create(ctx, openshiftOperatorsNs)
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
	finalizerManager.RegisterHandler(metallb.NewCleanupHandler(k8sManager.GetClient(), k8sManager.GetEventRecorderFor(common.ProvisionerControllerName)))
	finalizerManager.RegisterHandler(hostedcluster.NewCleanupHandler(k8sManager.GetClient(), k8sManager.GetEventRecorderFor(common.ProvisionerControllerName)))

	reconciler := &DPFHCPProvisionerReconciler{
		Client:   k8sManager.GetClient(),
		Scheme:   k8sManager.GetScheme(),
		Recorder: k8sManager.GetEventRecorderFor(common.ProvisionerControllerName),
		ImageLookup: func() *bfocplookup.ImageLookup {
			lookup := bfocplookup.NewImageLookup(k8sManager.GetClient(), k8sManager.GetEventRecorderFor("bluefield-ocp-layer-lookup"))
			lookup.ImageChecker = &fakeImageChecker{}
			return lookup
		}(),
		DPUClusterValidator:  dpucluster.NewValidator(k8sManager.GetClient(), k8sManager.GetEventRecorderFor("dpucluster-validator")),
		SecretsValidator:     secrets.NewValidator(k8sManager.GetClient(), k8sManager.GetEventRecorderFor("secrets-validator")),
		SecretManager:        hostedcluster.NewSecretManager(k8sManager.GetClient(), k8sManager.GetScheme()),
		MetalLBManager:       metallb.NewMetalLBManager(k8sManager.GetClient(), k8sManager.GetEventRecorderFor("metallb-manager")),
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
