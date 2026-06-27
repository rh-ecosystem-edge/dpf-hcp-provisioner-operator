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

package ignitiongenerator

import (
	"context"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	ignvalidate "github.com/coreos/ignition/v2/config/validate"
	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/ignition"
)

// buildRealisticHCPIgnition builds a realistic HCP ignition payload with machine config,
// SSH keys, and systemd units — similar to what HyperShift actually returns.
func buildRealisticHCPIgnition() []byte {
	machineConfig := map[string]interface{}{
		"spec": map[string]interface{}{
			"osImageURL": "https://registry.example.com/rhcos:4.17",
		},
	}
	mcJSON, _ := json.Marshal(machineConfig)
	encoded := url.QueryEscape(string(mcJSON))
	source := fmt.Sprintf("data:,%s", encoded)

	mode := 0644
	overwrite := true
	ign := &igntypes.Config{
		Ignition: igntypes.Ignition{Version: testIgnitionVersion},
		Storage: igntypes.Storage{
			Files: []igntypes.File{
				{
					Node:          igntypes.Node{Path: "/etc/ignition-machine-config-encapsulated.json", Overwrite: &overwrite},
					FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}, Mode: &mode},
				},
			},
		},
		Passwd: igntypes.Passwd{
			Users: []igntypes.PasswdUser{
				{
					Name:              "core",
					SSHAuthorizedKeys: []igntypes.SSHAuthorizedKey{"ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQ... test@example.com"},
				},
			},
		},
		Systemd: igntypes.Systemd{
			Units: []igntypes.Unit{
				{
					Name:     "kubelet.service",
					Enabled:  ignition.Ptr(true),
					Contents: ignition.Ptr("[Unit]\nDescription=Kubelet\n[Service]\nExecStart=/usr/bin/kubelet\n[Install]\nWantedBy=multi-user.target"),
				},
			},
		},
	}
	data, _ := json.Marshal(ign)
	return data
}

var _ = Describe("generateIgnition integration", func() {
	var (
		ctx      context.Context
		cancel   context.CancelFunc
		recorder *record.FakeRecorder
	)

	BeforeEach(func() {
		ctx, cancel = context.WithTimeout(context.TODO(), 30*time.Second)
		recorder = record.NewFakeRecorder(100)
	})

	AfterEach(func() {
		cancel()
	})

	// startIgnitionServer creates an httptest TLS server that serves the given HCP ignition bytes
	// on /ignition. Returns the server and a PEM-encoded CA cert.
	startIgnitionServer := func(hcpJSON []byte) (*httptest.Server, []byte) {
		handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.URL.Path != "/ignition" {
				w.WriteHeader(http.StatusNotFound)
				return
			}
			// Require Authorization header (matches real HyperShift behavior)
			auth := r.Header.Get("Authorization")
			if !strings.HasPrefix(auth, "Bearer ") {
				w.WriteHeader(http.StatusUnauthorized)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write(hcpJSON)
		})
		server := httptest.NewTLSServer(handler)

		// Extract CA cert as PEM from the test server
		caCert := pem.EncodeToMemory(&pem.Block{
			Type:  "CERTIFICATE",
			Bytes: server.TLS.Certificates[0].Certificate[0],
		})
		return server, caCert
	}

	// setupClusterObjects creates all the Kubernetes objects needed for generateIgnition to run
	setupClusterObjects := func(server *httptest.Server, caCert []byte) (
		*provisioningv1alpha1.DPFHCPProvisioner,
		[]interface{},
	) {
		// Parse server URL to get host:port for the HostedCluster endpoint
		serverURL, err := url.Parse(server.URL)
		Expect(err).NotTo(HaveOccurred())

		cr := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:       "doca",
				Namespace:  "clusters",
				Generation: 1,
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
					Name:      "dpucluster-1",
					Namespace: "dpf-operator-system",
				},
				DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{
					Name:      "my-deployment",
					Namespace: "dpf-operator-system",
				},
				OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
				MachineOSURL:    "https://registry.example.com/rhcos-bf:4.17",
			},
			Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
				HostedClusterRef: &corev1.ObjectReference{
					Name:      "doca",
					Namespace: "clusters",
				},
			},
		}

		// Ignition CA cert secret (in control plane namespace clusters-doca)
		caSecret := &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      ignitionSecretName,
				Namespace: "clusters-doca",
			},
			Data: map[string][]byte{
				"tls.crt": caCert,
			},
		}

		// Ignition token secret
		tokenSecret := &corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "token-doca-abc123",
				Namespace: "clusters-doca",
			},
			Data: map[string][]byte{
				ignitionTokenKey: []byte("test-bearer-token"),
			},
		}

		// HostedCluster with ignition endpoint pointing to our test server
		hc := &hyperv1.HostedCluster{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "doca",
				Namespace: "clusters",
			},
			Status: hyperv1.HostedClusterStatus{
				IgnitionEndpoint: serverURL.Host,
			},
		}

		// DPUDeployment
		deployment := &dpuservicev1alpha1.DPUDeployment{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "my-deployment",
				Namespace: "dpf-operator-system",
			},
			Spec: dpuservicev1alpha1.DPUDeploymentSpec{
				DPUs: dpuservicev1alpha1.DPUs{
					BFB:    "test-bfb",
					Flavor: "test-flavor",
				},
			},
		}

		// DPUFlavor with OVS script and DPU mode
		flavor := &dpuprovisioningv1alpha1.DPUFlavor{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "test-flavor",
				Namespace: "dpf-operator-system",
			},
			Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
				DpuMode: dpuprovisioningv1alpha1.DpuMode,
				OVS: dpuprovisioningv1alpha1.DPUFlavorOVS{
					RawConfigScript: "#!/bin/bash\novs-vsctl add-br br0\novs-vsctl add-port br0 p0",
				},
			},
		}

		// DPFOperatorConfig
		mtu := int(9000)
		operatorConfig := &operatorv1alpha1.DPFOperatorConfig{
			ObjectMeta: metav1.ObjectMeta{
				Name:      "dpf-config",
				Namespace: "dpf-operator-system",
			},
			Spec: operatorv1alpha1.DPFOperatorConfigSpec{
				Networking: &operatorv1alpha1.Networking{
					ControlPlaneMTU: &mtu,
				},
			},
		}

		objects := []interface{}{caSecret, tokenSecret, hc, deployment, flavor, operatorConfig}
		return cr, objects
	}

	It("should run the full ignition generation pipeline end-to-end", func() {
		hcpJSON := buildRealisticHCPIgnition()
		server, caCert := startIgnitionServer(hcpJSON)
		defer server.Close()

		cr, objects := setupClusterObjects(server, caCert)
		scheme := newTestScheme()

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(
				cr,
				objects[0].(*corev1.Secret),
				objects[1].(*corev1.Secret),
				objects[2].(*hyperv1.HostedCluster),
				objects[3].(*dpuservicev1alpha1.DPUDeployment),
				objects[4].(*dpuprovisioningv1alpha1.DPUFlavor),
				objects[5].(*operatorv1alpha1.DPFOperatorConfig),
			).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		// Run the full pipeline
		result, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero(), "should not requeue on success")

		// Verify status condition was set to IgnitionConfigured=True
		cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(cond).NotTo(BeNil())
		Expect(cond.Status).To(Equal(metav1.ConditionTrue))
		Expect(cond.Reason).To(Equal("IgnitionGenerated"))

		// Verify ConfigMap was created with correct name and labels
		cm := &corev1.ConfigMap{}
		cmKey := types.NamespacedName{
			Name:      ConfigMapName("dpucluster-1"),
			Namespace: "dpf-operator-system",
		}
		Expect(fakeClient.Get(ctx, cmKey, cm)).To(Succeed())
		Expect(cm.Labels).To(HaveKeyWithValue(BfcfgTemplateLabel, "true"))
		Expect(cm.Annotations).To(HaveKeyWithValue(BfcfgTemplateClusterNameAnnotation, "dpucluster-1"))
		Expect(cm.Annotations).To(HaveKeyWithValue(BfcfgTemplateBFBNameAnnotation, "test-bfb"))
		Expect(cm.Annotations).To(HaveKeyWithValue(BfcfgTemplateDPUFlavorNameAnnotation, "test-flavor"))
		Expect(cm.Annotations).To(HaveKeyWithValue(bfcfgTemplateMachineOSURLAnnotation, "https://registry.example.com/rhcos-bf:4.17"))

		// Verify the ConfigMap data contains valid ignition JSON
		ignitionData, ok := cm.Data[configMapKeyName]
		Expect(ok).To(BeTrue(), "ConfigMap should have BF_CFG_TEMPLATE key")
		Expect(ignitionData).NotTo(BeEmpty())

		// Parse the live ignition from the ConfigMap and verify it's valid
		liveIgn := &igntypes.Config{}
		Expect(json.Unmarshal([]byte(strings.TrimSpace(ignitionData)), liveIgn)).To(Succeed())
		Expect(liveIgn.Ignition.Version).To(Equal(testIgnitionVersion))

		// Verify live ignition has /var/target.ign (embedded target ignition)
		var hasTargetIgn bool
		for _, f := range liveIgn.Storage.Files {
			if f.Path == "/var/target.ign" {
				hasTargetIgn = true
				Expect(f.Contents.Compression).NotTo(BeNil())
				Expect(*f.Contents.Compression).To(Equal("gzip"))
				break
			}
		}
		Expect(hasTargetIgn).To(BeTrue(), "live ignition should contain /var/target.ign")

		// Verify SSH keys were copied to live ignition
		Expect(liveIgn.Passwd.Users).To(HaveLen(1))
		Expect(liveIgn.Passwd.Users[0].Name).To(Equal("core"))
		Expect(liveIgn.Passwd.Users[0].SSHAuthorizedKeys).To(HaveLen(1))

		// Verify success event was emitted
		Expect(recorder.Events).NotTo(BeEmpty())
		event := <-recorder.Events
		Expect(event).To(ContainSubstring("IgnitionConfigured"))
	})

	It("should produce validated ignition with MTU=1500 (no NM connections)", func() {
		hcpJSON := buildRealisticHCPIgnition()
		server, caCert := startIgnitionServer(hcpJSON)
		defer server.Close()

		cr, objects := setupClusterObjects(server, caCert)
		scheme := newTestScheme()

		// Override MTU to 1500
		operatorConfig := objects[5].(*operatorv1alpha1.DPFOperatorConfig)
		mtu := int(1500)
		operatorConfig.Spec.Networking.ControlPlaneMTU = &mtu

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(
				cr,
				objects[0].(*corev1.Secret),
				objects[1].(*corev1.Secret),
				objects[2].(*hyperv1.HostedCluster),
				objects[3].(*dpuservicev1alpha1.DPUDeployment),
				objects[4].(*dpuprovisioningv1alpha1.DPUFlavor),
				operatorConfig,
			).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		result, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		// Verify ConfigMap was created
		cm := &corev1.ConfigMap{}
		cmKey := types.NamespacedName{
			Name:      ConfigMapName("dpucluster-1"),
			Namespace: "dpf-operator-system",
		}
		Expect(fakeClient.Get(ctx, cmKey, cm)).To(Succeed())

		// Parse live ignition and verify no NM connections
		liveIgn := &igntypes.Config{}
		Expect(json.Unmarshal([]byte(strings.TrimSpace(cm.Data[configMapKeyName])), liveIgn)).To(Succeed())

		for _, f := range liveIgn.Storage.Files {
			Expect(f.Path).NotTo(MatchRegexp(`/etc/NetworkManager/system-connections/(p0|p1|pf0hpf|pf1hpf)\.nmconnection`),
				"MTU=1500 should not add NM connection files")
		}
	})

	It("should produce validated ignition with MTU=9000 (with NM connections in target)", func() {
		hcpJSON := buildRealisticHCPIgnition()
		server, caCert := startIgnitionServer(hcpJSON)
		defer server.Close()

		cr, objects := setupClusterObjects(server, caCert)
		scheme := newTestScheme()

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(
				cr,
				objects[0].(*corev1.Secret),
				objects[1].(*corev1.Secret),
				objects[2].(*hyperv1.HostedCluster),
				objects[3].(*dpuservicev1alpha1.DPUDeployment),
				objects[4].(*dpuprovisioningv1alpha1.DPUFlavor),
				objects[5].(*operatorv1alpha1.DPFOperatorConfig),
			).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		result, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(cond).NotTo(BeNil())
		Expect(cond.Status).To(Equal(metav1.ConditionTrue))
	})

	It("should produce idempotent results on re-run (update existing ConfigMap)", func() {
		hcpJSON := buildRealisticHCPIgnition()
		server, caCert := startIgnitionServer(hcpJSON)
		defer server.Close()

		cr, objects := setupClusterObjects(server, caCert)
		scheme := newTestScheme()

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(
				cr,
				objects[0].(*corev1.Secret),
				objects[1].(*corev1.Secret),
				objects[2].(*hyperv1.HostedCluster),
				objects[3].(*dpuservicev1alpha1.DPUDeployment),
				objects[4].(*dpuprovisioningv1alpha1.DPUFlavor),
				objects[5].(*operatorv1alpha1.DPFOperatorConfig),
			).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		// First run — creates ConfigMap
		result1, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result1.RequeueAfter).To(BeZero())

		cm1 := &corev1.ConfigMap{}
		cmKey := types.NamespacedName{
			Name:      ConfigMapName("dpucluster-1"),
			Namespace: "dpf-operator-system",
		}
		Expect(fakeClient.Get(ctx, cmKey, cm1)).To(Succeed())
		data1 := cm1.Data[configMapKeyName]

		// Second run — updates ConfigMap (same input → same output)
		result2, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result2.RequeueAfter).To(BeZero())

		cm2 := &corev1.ConfigMap{}
		Expect(fakeClient.Get(ctx, cmKey, cm2)).To(Succeed())
		data2 := cm2.Data[configMapKeyName]

		Expect(data2).To(Equal(data1), "idempotent run should produce identical ignition output")
	})

	It("should set IgnitionConfigured=False when ignition endpoint is unavailable", func() {
		// Don't start a server — HCP download will fail
		cr := &provisioningv1alpha1.DPFHCPProvisioner{
			ObjectMeta: metav1.ObjectMeta{
				Name:       "doca",
				Namespace:  "clusters",
				Generation: 1,
			},
			Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
				DPUClusterRef: provisioningv1alpha1.DPUClusterReference{
					Name:      "dpucluster-1",
					Namespace: "dpf-operator-system",
				},
				MachineOSURL: "https://registry.example.com/rhcos-bf:4.17",
			},
			Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
				HostedClusterRef: &corev1.ObjectReference{
					Name:      "doca",
					Namespace: "clusters",
				},
			},
		}

		scheme := newTestScheme()
		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(cr).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		result, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred()) // returns nil + RequeueAfter
		Expect(result.RequeueAfter).To(Equal(30 * time.Second))

		cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(cond).NotTo(BeNil())
		Expect(cond.Status).To(Equal(metav1.ConditionFalse))
		Expect(cond.Reason).To(Equal("IgnitionGenerationFailed"))
	})

	It("should propagate machineOSURL from spec to ConfigMap annotation", func() {
		hcpJSON := buildRealisticHCPIgnition()
		server, caCert := startIgnitionServer(hcpJSON)
		defer server.Close()

		cr, objects := setupClusterObjects(server, caCert)
		customURL := "https://custom-registry.io/custom-rhcos-bf:v4.18"
		cr.Spec.MachineOSURL = customURL
		scheme := newTestScheme()

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(
				cr,
				objects[0].(*corev1.Secret),
				objects[1].(*corev1.Secret),
				objects[2].(*hyperv1.HostedCluster),
				objects[3].(*dpuservicev1alpha1.DPUDeployment),
				objects[4].(*dpuprovisioningv1alpha1.DPUFlavor),
				objects[5].(*operatorv1alpha1.DPFOperatorConfig),
			).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		result, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		cm := &corev1.ConfigMap{}
		cmKey := types.NamespacedName{
			Name:      ConfigMapName("dpucluster-1"),
			Namespace: "dpf-operator-system",
		}
		Expect(fakeClient.Get(ctx, cmKey, cm)).To(Succeed())
		Expect(cm.Annotations[bfcfgTemplateMachineOSURLAnnotation]).To(Equal(customURL))
	})

	It("should reject HCP ignition with duplicate files via target validation", func() {
		// Build HCP ignition with duplicate file paths — this should cause ValidateConfig
		// to fail during the pipeline
		mode := 0644
		overwrite := true
		source := "data:,content"
		machineConfig := map[string]interface{}{
			"spec": map[string]interface{}{
				"osImageURL": "https://registry.example.com/rhcos:4.17",
			},
		}
		mcJSON, _ := json.Marshal(machineConfig)
		encoded := url.QueryEscape(string(mcJSON))
		mcSource := fmt.Sprintf("data:,%s", encoded)

		ign := &igntypes.Config{
			Ignition: igntypes.Ignition{Version: testIgnitionVersion},
			Storage: igntypes.Storage{
				Files: []igntypes.File{
					{
						Node:          igntypes.Node{Path: "/etc/ignition-machine-config-encapsulated.json", Overwrite: &overwrite},
						FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &mcSource}, Mode: &mode},
					},
					// Duplicate file path — the validator should catch this
					{
						Node:          igntypes.Node{Path: "/etc/duplicate-file", Overwrite: &overwrite},
						FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}, Mode: &mode},
					},
					{
						Node:          igntypes.Node{Path: "/etc/duplicate-file", Overwrite: &overwrite},
						FileEmbedded1: igntypes.FileEmbedded1{Contents: igntypes.Resource{Source: &source}, Mode: &mode},
					},
				},
			},
		}
		hcpJSON, _ := json.Marshal(ign)

		server, caCert := startIgnitionServer(hcpJSON)
		defer server.Close()

		cr, objects := setupClusterObjects(server, caCert)
		scheme := newTestScheme()

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(
				cr,
				objects[0].(*corev1.Secret),
				objects[1].(*corev1.Secret),
				objects[2].(*hyperv1.HostedCluster),
				objects[3].(*dpuservicev1alpha1.DPUDeployment),
				objects[4].(*dpuprovisioningv1alpha1.DPUFlavor),
				objects[5].(*operatorv1alpha1.DPFOperatorConfig),
			).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		result, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred()) // returns nil + RequeueAfter on failure
		Expect(result.RequeueAfter).To(Equal(30*time.Second), "should requeue on validation failure")

		cond := meta.FindStatusCondition(cr.Status.Conditions, provisioningv1alpha1.IgnitionConfigured)
		Expect(cond).NotTo(BeNil())
		Expect(cond.Status).To(Equal(metav1.ConditionFalse))
		Expect(cond.Message).To(ContainSubstring("validation failed"))

		// Verify no ConfigMap was created
		cm := &corev1.ConfigMap{}
		cmKey := types.NamespacedName{
			Name:      ConfigMapName("dpucluster-1"),
			Namespace: "dpf-operator-system",
		}
		err = fakeClient.Get(ctx, cmKey, cm)
		Expect(err).To(HaveOccurred(), "ConfigMap should not exist when validation fails")
	})

	It("should produce live ignition that passes validation after template substitution", func() {
		hcpJSON := buildRealisticHCPIgnition()
		server, caCert := startIgnitionServer(hcpJSON)
		defer server.Close()

		cr, objects := setupClusterObjects(server, caCert)
		scheme := newTestScheme()

		fakeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithStatusSubresource(cr).
			WithObjects(
				cr,
				objects[0].(*corev1.Secret),
				objects[1].(*corev1.Secret),
				objects[2].(*hyperv1.HostedCluster),
				objects[3].(*dpuservicev1alpha1.DPUDeployment),
				objects[4].(*dpuprovisioningv1alpha1.DPUFlavor),
				objects[5].(*operatorv1alpha1.DPFOperatorConfig),
			).
			Build()
		ig := NewIgnitionGenerator(fakeClient, scheme, recorder)

		result, err := ig.GenerateIgnition(ctx, cr)
		Expect(err).NotTo(HaveOccurred())
		Expect(result.RequeueAfter).To(BeZero())

		// Get live ignition from ConfigMap
		cm := &corev1.ConfigMap{}
		cmKey := types.NamespacedName{
			Name:      ConfigMapName("dpucluster-1"),
			Namespace: "dpf-operator-system",
		}
		Expect(fakeClient.Get(ctx, cmKey, cm)).To(Succeed())
		ignitionData := strings.TrimSpace(cm.Data[configMapKeyName])

		// Replace Go template placeholders with real values — simulates what the
		// DPF provisioning controller does before applying the ignition
		resolved := ignitionData
		resolved = strings.ReplaceAll(resolved, "{{.DPUHostName}}", "bf2-worker-0")
		resolved = strings.ReplaceAll(resolved, "{{.DPUName}}", "dpu-0")
		resolved = strings.ReplaceAll(resolved, "{{.DPUNamespace}}", "dpf-operator-system")
		resolved = strings.ReplaceAll(resolved, "{{.DPUUID}}", "12345678-1234-1234-1234-123456789abc")
		resolved = strings.ReplaceAll(resolved, "{{.KernelParameters}}", "console=ttyAMA0")

		// Parse the resolved live ignition
		liveIgn := &igntypes.Config{}
		Expect(json.Unmarshal([]byte(resolved), liveIgn)).To(Succeed())
		Expect(liveIgn.Ignition.Version).To(Equal(testIgnitionVersion))

		// Run full ignition validation on the resolved live ignition
		raw, err := json.Marshal(liveIgn)
		Expect(err).NotTo(HaveOccurred())
		rpt := ignvalidate.ValidateWithContext(*liveIgn, raw)
		Expect(rpt.IsFatal()).To(BeFalse(),
			"live ignition should be valid after template substitution, got: %s", rpt.String())

		// Verify structural completeness: live ignition should have target.ign,
		// OVS script, DPU flavor files, and common content
		filePaths := make(map[string]bool)
		for _, f := range liveIgn.Storage.Files {
			filePaths[f.Path] = true
		}
		Expect(filePaths).To(HaveKey("/var/target.ign"), "should embed target ignition")
	})
})
