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

// Command dpuservicetemplate generates golden fixtures for the Rust port of
// internal/controller/dpuservicetemplate (crates/ctrl-dpuservicetemplate).
//
// Each scenario drives the Go DPUServiceTemplateManager (EnsureTemplates / DeleteTemplates)
// or DPUServiceTemplateReconciler against a controller-runtime fake client with a fake
// ReleaseImageReader, and records for every step the returned error and the release-reader
// calls, plus the encoding/json bytes of the three DPUServiceTemplates as stored in the fake
// client at the end. Release images live on an httptest "rules" registry (fixed responses per
// method+path, recorded in the fixture so the Rust test can serve the same bytes), which
// decides common.IsMultiArchReleaseImage without touching the network.
//
// A second fixture (release_reader.json) runs RemoteReleaseImageReader.GetComponentImage
// against release payload images served by the rules registry.
//
// Normalization: the registry address is replaced by "REGISTRY" everywhere, and a stored
// template's deletionTimestamp (stamped with metav1.Now() by the fake client on Delete) is set
// to 2025-01-01T00:00:00Z.
//
// Usage: go run -mod=vendor ./hack/golden/dpuservicetemplate crates/ctrl-dpuservicetemplate/tests/fixtures
package main

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/google/go-containerregistry/pkg/authn"
	configv1 "github.com/openshift/api/config/v1"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpuservicetemplate"
)

// ---------------------------------------------------------------------------------------------
// Rules registry
// ---------------------------------------------------------------------------------------------

// Rule is a fixed response for METHOD+path ("" method matches GET and HEAD).
type Rule struct {
	Method  string            `json:"method"`
	Path    string            `json:"path"`
	Status  int               `json:"status"`
	Headers map[string]string `json:"headers"`
	Body    string            `json:"body"` // base64
}

func startRegistry(rules []Rule) *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/v2/" {
			w.WriteHeader(http.StatusOK)
			return
		}
		for _, rule := range rules {
			if rule.Path != r.URL.Path {
				continue
			}
			if rule.Method != "" && rule.Method != r.Method {
				continue
			}
			if rule.Method == "" && r.Method != http.MethodGet && r.Method != http.MethodHead {
				continue
			}
			body, _ := base64.StdEncoding.DecodeString(rule.Body)
			for k, v := range rule.Headers {
				w.Header().Set(k, v)
			}
			w.Header().Set("Content-Length", fmt.Sprint(len(body)))
			w.WriteHeader(rule.Status)
			if r.Method != http.MethodHead {
				_, _ = w.Write(body)
			}
			return
		}
		w.WriteHeader(http.StatusNotFound)
	}))
}

func digestOf(b []byte) string {
	h := sha256.Sum256(b)
	return "sha256:" + hex.EncodeToString(h[:])
}

func manifestRules(repo, ref, mediaType string, body []byte) []Rule {
	return []Rule{{
		Path:   "/v2/" + repo + "/manifests/" + ref,
		Status: 200,
		Headers: map[string]string{
			"Content-Type":          mediaType,
			"Docker-Content-Digest": digestOf(body),
		},
		Body: base64.StdEncoding.EncodeToString(body),
	}}
}

func blobRule(repo string, body []byte) Rule {
	return Rule{
		Path:    "/v2/" + repo + "/blobs/" + digestOf(body),
		Status:  200,
		Headers: map[string]string{"Content-Type": "application/octet-stream"},
		Body:    base64.StdEncoding.EncodeToString(body),
	}
}

const (
	ociIndex    = "application/vnd.oci.image.index.v1+json"
	ociManifest = "application/vnd.oci.image.manifest.v1+json"
)

// indexRules serves an (empty) OCI index for repo:tag, i.e. a multi-arch release image.
func indexRules(repo, tag string) []Rule {
	body := []byte(`{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[]}`)
	return manifestRules(repo, tag, ociIndex, body)
}

type file struct {
	name    string
	content string
}

func layer(files []file) []byte {
	var tbuf bytes.Buffer
	tw := tar.NewWriter(&tbuf)
	for _, f := range files {
		must(tw.WriteHeader(&tar.Header{Name: f.name, Mode: 0o644, Size: int64(len(f.content)), Typeflag: tar.TypeReg, ModTime: time.Unix(0, 0)}))
		_, err := tw.Write([]byte(f.content))
		must(err)
	}
	must(tw.Close())
	var gbuf bytes.Buffer
	gw := gzip.NewWriter(&gbuf)
	_, err := gw.Write(tbuf.Bytes())
	must(err)
	must(gw.Close())
	return gbuf.Bytes()
}

// imageRules serves a single-platform image repo:tag made of the given layers (bottom first).
func imageRules(repo, tag string, layers ...[]byte) []Rule {
	cfg := []byte(`{"architecture":"arm64","os":"linux","rootfs":{"type":"layers","diff_ids":[]},"config":{}}`)
	rules := []Rule{blobRule(repo, cfg)}
	var descs []string
	for _, l := range layers {
		rules = append(rules, blobRule(repo, l))
		descs = append(descs, fmt.Sprintf(`{"mediaType":"application/vnd.oci.image.layer.v1.tar+gzip","size":%d,"digest":%q}`, len(l), digestOf(l)))
	}
	manifest := []byte(fmt.Sprintf(`{"schemaVersion":2,"mediaType":%q,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":%d,"digest":%q},"layers":[%s]}`,
		ociManifest, len(cfg), digestOf(cfg), strings.Join(descs, ",")))
	return append(rules, manifestRules(repo, tag, ociManifest, manifest)...)
}

// ---------------------------------------------------------------------------------------------
// Objects (mirroring dpuservicetemplate_test.go)
// ---------------------------------------------------------------------------------------------

const (
	operatorNamespace = "dpf-hcp-provisioner-system"
	targetNamespace   = "dpf-operator-system"
	x86OVNImage       = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:aaa111"
	arm64OVNImage     = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:bbb222"
)

var fixedTime = metav1.NewTime(time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func scheme() *runtime.Scheme {
	s := runtime.NewScheme()
	must(provisioningv1alpha1.AddToScheme(s))
	must(dpuservicev1alpha1.AddToScheme(s))
	must(operatorv1alpha1.AddToScheme(s))
	must(configv1.Install(s))
	must(appsv1.AddToScheme(s))
	must(corev1.AddToScheme(s))
	return s
}

func dpfOperatorConfig(version string) *operatorv1alpha1.DPFOperatorConfig {
	return &operatorv1alpha1.DPFOperatorConfig{
		ObjectMeta: metav1.ObjectMeta{Name: "dpfoperatorconfig", Namespace: "dpf-operator-system"},
		Status:     operatorv1alpha1.DPFOperatorConfigStatus{Version: &version},
	}
}

func clusterVersion(image, version string) *configv1.ClusterVersion {
	return &configv1.ClusterVersion{
		ObjectMeta: metav1.ObjectMeta{Name: "version"},
		Spec:       configv1.ClusterVersionSpec{ClusterID: "test-cluster"},
		Status: configv1.ClusterVersionStatus{
			Desired: configv1.Release{Image: image, Version: version},
		},
	}
}

func pullSecret() *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: "pull-secret", Namespace: "openshift-config"},
		Data:       map[string][]byte{".dockerconfigjson": []byte(`{"auths":{}}`)},
		Type:       corev1.SecretTypeDockerConfigJson,
	}
}

func ovnDaemonSet(image string, generation int64, desired, updated, available int32) *appsv1.DaemonSet {
	ds := &appsv1.DaemonSet{
		ObjectMeta: metav1.ObjectMeta{Name: "ovnkube-node-dpu-host", Namespace: "openshift-ovn-kubernetes", Generation: generation},
		Spec: appsv1.DaemonSetSpec{
			Selector: &metav1.LabelSelector{MatchLabels: map[string]string{"app": "ovnkube"}},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{Labels: map[string]string{"app": "ovnkube"}},
				Spec: corev1.PodSpec{Containers: []corev1.Container{
					{Name: "sidecar", Image: "quay.io/other:v1"},
					{Name: "ovnkube-controller", Image: image},
				}},
			},
		},
	}
	if generation != 0 {
		ds.Status = appsv1.DaemonSetStatus{
			ObservedGeneration:     generation,
			DesiredNumberScheduled: desired,
			UpdatedNumberScheduled: updated,
			NumberAvailable:        available,
		}
	}
	return ds
}

func overridesConfigMap(data string) *corev1.ConfigMap {
	return &corev1.ConfigMap{
		ObjectMeta: metav1.ObjectMeta{Name: "dpuservicetemplate-overrides", Namespace: operatorNamespace},
		Data:       map[string]string{"overrides.json": data},
	}
}

func template(name string, labels, annotations map[string]string, repoURL, version string, values string, deleting bool) *dpuservicev1alpha1.DPUServiceTemplate {
	t := &dpuservicev1alpha1.DPUServiceTemplate{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: targetNamespace, Labels: labels, Annotations: annotations},
		Spec: dpuservicev1alpha1.DPUServiceTemplateSpec{
			DeploymentServiceName: name,
			HelmChart: dpuservicev1alpha1.HelmChart{
				Source: dpuservicev1alpha1.ApplicationSource{RepoURL: repoURL, Version: version, Chart: "old-chart"},
			},
		},
	}
	if values != "" {
		t.Spec.HelmChart.Values = &runtime.RawExtension{Raw: []byte(values)}
	}
	if deleting {
		t.DeletionTimestamp = &fixedTime
		t.Finalizers = []string{dpuservicev1alpha1.DPUServiceTemplateFinalizer}
	}
	return t
}

func managed() map[string]string {
	return map[string]string{common.LabelManagedBy: "true"}
}

func operatorConfigCR(manage bool) *provisioningv1alpha1.DPFHCPProvisionerConfig {
	return &provisioningv1alpha1.DPFHCPProvisionerConfig{
		ObjectMeta: metav1.ObjectMeta{Name: provisioningv1alpha1.DefaultConfigName},
		Spec: provisioningv1alpha1.DPFHCPProvisionerConfigSpec{
			ManageDPUServiceTemplates:  manage,
			DPUServicesImagePullSecret: "cfg-pull-secret",
		},
	}
}

func provisioner(name, dpuClusterNS, ddName string, deleting bool) *provisioningv1alpha1.DPFHCPProvisioner {
	p := &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: "default"},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			DPUClusterRef: provisioningv1alpha1.DPUClusterReference{Name: "cluster", Namespace: dpuClusterNS},
		},
	}
	if ddName != "" {
		p.Spec.DPUDeploymentRef = &provisioningv1alpha1.DPUDeploymentReference{Name: ddName, Namespace: targetNamespace}
	}
	if deleting {
		p.DeletionTimestamp = &fixedTime
		p.Finalizers = []string{"provisioning.dpu.hcp.io/finalizer"}
	}
	return p
}

func dpuDeployment(name string, deleting bool) *dpuservicev1alpha1.DPUDeployment {
	d := &dpuservicev1alpha1.DPUDeployment{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: targetNamespace},
		Spec: dpuservicev1alpha1.DPUDeploymentSpec{
			DPUs: dpuservicev1alpha1.DPUs{BFB: ptr.To("bfb"), Flavor: ptr.To("flavor")},
		},
	}
	if deleting {
		d.DeletionTimestamp = &fixedTime
		d.Finalizers = []string{dpuservicev1alpha1.DPUDeploymentFinalizer}
	}
	return d
}

// ---------------------------------------------------------------------------------------------
// Fake release reader
// ---------------------------------------------------------------------------------------------

type fakeReader struct {
	image string
	err   error
	calls []string
}

func (f *fakeReader) GetComponentImage(_ context.Context, ref, component string, _ authn.Keychain) (string, error) {
	f.calls = append(f.calls, ref+" "+component)
	return f.image, f.err
}

// ---------------------------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------------------------

// Step is the outcome of one call.
type Step struct {
	Error       string   `json:"error"`
	ReaderCalls []string `json:"readerCalls"`
}

// Fixture is one scenario.
type Fixture struct {
	Rules []Rule `json:"rules"`
	Steps []Step `json:"steps"`
	// Objects maps a template name to the encoding/json bytes of the stored object ("" when absent).
	Objects map[string]string `json:"objects"`
}

type env struct {
	host   string
	c      client.Client
	reader *fakeReader
	mgr    *dpuservicetemplate.DPUServiceTemplateManager
	fx     *Fixture
}

func (e *env) norm(s string) string {
	return strings.ReplaceAll(s, e.host, "REGISTRY")
}

func (e *env) record(err error) {
	s := Step{ReaderCalls: []string{}}
	if err != nil {
		s.Error = e.norm(err.Error())
	}
	for _, c := range e.reader.calls {
		s.ReaderCalls = append(s.ReaderCalls, e.norm(c))
	}
	e.reader.calls = nil
	e.fx.Steps = append(e.fx.Steps, s)
}

func (e *env) ensure(cfg *common.OperatorConfig) {
	e.record(e.mgr.EnsureTemplates(context.Background(), targetNamespace, cfg))
}

func (e *env) replace(old, new client.Object) {
	must(e.c.Delete(context.Background(), old))
	must(e.c.Create(context.Background(), new))
}

type scenario struct {
	name  string
	rules []Rule
	// objects builds the initial objects given the registry host.
	objects func(host string) []client.Object
	reader  fakeReader
	run     func(e *env)
}

func release(host, tag string) string {
	return host + "/ocp/release:" + tag
}

func prereqs(host, tag, version string) []client.Object {
	return []client.Object{
		dpfOperatorConfig("26.4.0-f314aa17"),
		clusterVersion(release(host, tag), version),
		pullSecret(),
		ovnDaemonSet(x86OVNImage, 0, 0, 0, 0),
	}
}

func with(objs []client.Object, more ...client.Object) []client.Object {
	return append(objs, more...)
}

var noCfg = &common.OperatorConfig{}

func scenarios() []scenario {
	return []scenario{
		{
			name:    "create_single_arch",
			objects: func(h string) []client.Object { return prereqs(h, "4.19.0-ec.5", "4.19.0-ec.5") },
			reader:  fakeReader{image: arm64OVNImage},
			run: func(e *env) {
				e.ensure(noCfg)
				e.ensure(noCfg) // unchanged: no update, no release pull
			},
		},
		{
			name:    "create_multi_arch_tag_with_pull_secret_feature_flags",
			objects: func(h string) []client.Object { return prereqs(h, "4.22.6-multi", "4.22.6") },
			reader:  fakeReader{image: arm64OVNImage},
			run: func(e *env) {
				e.ensure(&common.OperatorConfig{DPUServicesImagePullSecret: "my-pull-secret"})
				e.ensure(&common.OperatorConfig{DPUServicesImagePullSecret: "my-pull-secret"})
				// Pull secret removed: DTS/HBN updated.
				e.ensure(noCfg)
			},
		},
		{
			name:    "create_multi_arch_index",
			rules:   indexRules("ocp/release", "4.22.1"),
			objects: func(h string) []client.Object { return prereqs(h, "4.22.1", "4.22.1") },
			reader:  fakeReader{image: arm64OVNImage},
			run:     func(e *env) { e.ensure(noCfg) },
		},
		{
			name:    "single_arch_feature_flags_prerelease",
			objects: func(h string) []client.Object { return prereqs(h, "4.22.2-rc.1", "4.22.2-rc.1") },
			reader:  fakeReader{image: "quay.io/ovn/ovn-k:v4.22-arm64"},
			run:     func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_applied",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{
  "26.3": {"ovn": {"chartVersion": "ignored"}},
  "26.4": {
    "ovn": {"chartVersion": "v26.4.9-custom", "CHARTREPOURL": "oci://example.com/charts"},
    "dts": {"prometheusPort": 9999, "imageDTS": null},
    "Hbn": {"imageTag": "custom-tag", "bfSFCount": 5},
    "hbn": {"chartName": "doca-hbn-custom"}
  }
}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_other_version_only",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.3": {"ovn": {"chartVersion": "x"}}}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_unknown_field",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": {"ovn": {"chartVersion": 5}, "bogus": {}}}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_unknown_nested_field",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": {"hbn": {"ImagePullSecret": "x"}}}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_type_errors",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": {"dts": {"prometheusPort": "x", "chartName": 1}}}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_int_overflow",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": {"hbn": {"bfSFCount": 3000000000}}}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_struct_type_error",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": {"ovn": [1]}}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_version_not_object",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": "nope"}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_version_null",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": null}`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_invalid_json",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`{"26.4": {`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_not_object",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`[1, 2]`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "overrides_null_document",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), overridesConfigMap(`null`))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "drift_corrected_and_unmanaged_skipped",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"),
					template("ovn", map[string]string{common.LabelManagedBy: "true", "extra": "kept"},
						map[string]string{"note": "user", dpuservicetemplate.AnnotationSourceOVNImage: "old-image"},
						"oci://example.com", "1.0.0", `{"stale":true}`, false),
					template("hbn", map[string]string{"owner": "user"}, nil, "https://user", "9", "", false),
					template("doca-telemetry-service", map[string]string{common.LabelManagedBy: "false"}, nil, "https://user", "9", "", false),
				)
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "deleting_template_skipped",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"),
					template("ovn", managed(), nil, "https://stale", "stale", "", true))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "image_unchanged_keeps_values_cv_changed",
			objects: func(h string) []client.Object {
				return prereqs(h, "4.19.0-ec.5", "4.19.0-ec.5")
			},
			reader: fakeReader{image: arm64OVNImage},
			run: func(e *env) {
				e.ensure(noCfg)
				e.replace(clusterVersion(release(e.host, "4.19.0-ec.5"), "4.19.0-ec.5"), clusterVersion(release(e.host, "4.22.6"), "4.22.6"))
				e.ensure(noCfg)
			},
		},
		{
			name: "existing_template_values_used_when_image_matches",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"),
					template("ovn", managed(), map[string]string{dpuservicetemplate.AnnotationSourceOVNImage: x86OVNImage},
						"oci://example.com", "1.0.0", `{"dpuManifests":{"image":{"repository":"quay.io/kept/ovn","tag":"kept-tag"}},"ovnDaemonsetVersion":7}`, false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "existing_template_no_values",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"),
					template("ovn", managed(), map[string]string{dpuservicetemplate.AnnotationSourceOVNImage: x86OVNImage},
						"oci://example.com", "1.0.0", "", false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "existing_template_values_not_object",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"),
					template("ovn", managed(), map[string]string{dpuservicetemplate.AnnotationSourceOVNImage: x86OVNImage},
						"oci://example.com", "1.0.0", `["x"]`, false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "existing_template_values_missing_image",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"),
					template("ovn", managed(), map[string]string{dpuservicetemplate.AnnotationSourceOVNImage: x86OVNImage},
						"oci://example.com", "1.0.0", `{"dpuManifests":{"image":{"repository":"","tag":"t"}}}`, false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "rollout_incomplete_skips_update",
			objects: func(h string) []client.Object {
				return prereqs(h, "4.19.0-ec.5", "4.19.0-ec.5")
			},
			reader: fakeReader{image: arm64OVNImage},
			run: func(e *env) {
				e.ensure(noCfg)
				e.replace(ovnDaemonSet(x86OVNImage, 0, 0, 0, 0), ovnDaemonSet("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ccc333", 2, 3, 1, 2))
				e.ensure(noCfg)
			},
		},
		{
			name: "rollout_complete_resolves_new_image",
			objects: func(h string) []client.Object {
				return prereqs(h, "4.19.0-ec.5", "4.19.0-ec.5")
			},
			reader: fakeReader{image: arm64OVNImage},
			run: func(e *env) {
				e.ensure(noCfg)
				e.replace(ovnDaemonSet(x86OVNImage, 0, 0, 0, 0), ovnDaemonSet("quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:ccc333", 2, 3, 3, 3))
				e.reader.image = "quay.io/openshift-release-dev/ocp-v4.0-art-dev:ddd444"
				e.ensure(noCfg)
			},
		},
		{
			name:    "reader_error",
			objects: func(h string) []client.Object { return prereqs(h, "4.19.0-ec.5", "4.19.0-ec.5") },
			reader:  fakeReader{err: fmt.Errorf("unable to pull release image")},
			run:     func(e *env) { e.ensure(noCfg) },
		},
		{
			name:    "reader_returns_untagged_image",
			objects: func(h string) []client.Object { return prereqs(h, "4.19.0-ec.5", "4.19.0-ec.5") },
			reader:  fakeReader{image: "quay.io/no/tag"},
			run:     func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "multi_arch_untagged_daemonset_image",
			objects: func(h string) []client.Object {
				objs := prereqs(h, "4.19.0-multi", "4.19.0")
				objs[3] = ovnDaemonSet("quay.io/x/ovn", 0, 0, 0, 0)
				return objs
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "missing_daemonset",
			objects: func(h string) []client.Object {
				return prereqs(h, "4.19.0", "4.19.0")[:3]
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "daemonset_without_ovn_container",
			objects: func(h string) []client.Object {
				objs := prereqs(h, "4.19.0", "4.19.0")
				ds := ovnDaemonSet(x86OVNImage, 0, 0, 0, 0)
				ds.Spec.Template.Spec.Containers = ds.Spec.Template.Spec.Containers[:1]
				objs[3] = ds
				return objs
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "missing_dpf_operator_config",
			objects: func(h string) []client.Object {
				return prereqs(h, "4.19.0", "4.19.0")[1:]
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "unknown_dpf_version_uses_default",
			objects: func(h string) []client.Object {
				objs := prereqs(h, "4.19.0", "4.19.0")
				objs[0] = dpfOperatorConfig("99.0.0-unknown")
				return objs
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "bad_dpf_version",
			objects: func(h string) []client.Object {
				objs := prereqs(h, "4.19.0", "4.19.0")
				objs[0] = dpfOperatorConfig("garbage")
				return objs
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "missing_cluster_version",
			objects: func(h string) []client.Object {
				objs := prereqs(h, "4.19.0", "4.19.0")
				return []client.Object{objs[0], objs[2], objs[3]}
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "cluster_version_empty_image",
			objects: func(h string) []client.Object {
				objs := prereqs(h, "4.19.0", "4.19.0")
				objs[1] = clusterVersion("", "4.19.0")
				return objs
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "missing_pull_secret",
			objects: func(h string) []client.Object {
				objs := prereqs(h, "4.19.0", "4.19.0")
				return []client.Object{objs[0], objs[1], objs[3]}
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.ensure(noCfg) },
		},
		{
			name: "delete_templates",
			objects: func(h string) []client.Object {
				return []client.Object{
					template("ovn", managed(), nil, "oci://x", "1", "", false),
					template("hbn", map[string]string{"owner": "user"}, nil, "oci://x", "1", "", false),
					template("doca-telemetry-service", managed(), nil, "oci://x", "1", "", true),
				}
			},
			run: func(e *env) {
				e.record(e.mgr.DeleteTemplates(context.Background(), targetNamespace))
				e.record(e.mgr.DeleteTemplates(context.Background(), targetNamespace))
			},
		},
		// Reconciler
		{
			name: "reconcile_management_disabled",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), operatorConfigCR(false),
					provisioner("p", targetNamespace, "dd", false), dpuDeployment("dd", false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.reconcile() },
		},
		{
			name: "reconcile_no_config",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"),
					provisioner("p", targetNamespace, "dd", false), dpuDeployment("dd", false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.reconcile() },
		},
		{
			name: "reconcile_active_deployment",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), operatorConfigCR(true),
					provisioner("deleting", targetNamespace, "dd", true),
					provisioner("other-ns", "other-namespace", "dd", false),
					provisioner("no-dd", targetNamespace, "", false),
					provisioner("p", targetNamespace, "dd", false), dpuDeployment("dd", false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.reconcile() },
		},
		{
			name: "reconcile_deleting_deployment",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0"), operatorConfigCR(true),
					provisioner("p", targetNamespace, "dd", false), dpuDeployment("dd", true),
					template("ovn", managed(), nil, "oci://x", "1", "", false),
					template("hbn", nil, nil, "oci://x", "1", "", false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.reconcile() },
		},
		{
			name: "reconcile_ensure_error",
			objects: func(h string) []client.Object {
				return with(prereqs(h, "4.19.0", "4.19.0")[1:], operatorConfigCR(true),
					provisioner("p", targetNamespace, "dd", false), dpuDeployment("dd", false))
			},
			reader: fakeReader{image: arm64OVNImage},
			run:    func(e *env) { e.reconcile() },
		},
	}
}

func (e *env) reconcile() {
	r := &dpuservicetemplate.DPUServiceTemplateReconciler{Client: e.c, Scheme: e.c.Scheme(), Manager: e.mgr}
	_, err := r.Reconcile(context.Background(), ctrl.Request{NamespacedName: types.NamespacedName{Name: targetNamespace}})
	e.record(err)
}

func encode(o any) string {
	b, err := json.Marshal(o)
	must(err)
	return string(b)
}

func runScenario(sc scenario) *Fixture {
	srv := startRegistry(sc.rules)
	defer srv.Close()
	host := strings.TrimPrefix(srv.URL, "http://")

	objs := sc.objects(host)
	c := fake.NewClientBuilder().
		WithScheme(scheme()).
		WithObjects(objs...).
		WithStatusSubresource(&operatorv1alpha1.DPFOperatorConfig{}).
		Build()
	reader := sc.reader
	fx := &Fixture{Rules: sc.rules, Steps: []Step{}, Objects: map[string]string{}}
	if fx.Rules == nil {
		fx.Rules = []Rule{}
	}
	e := &env{
		host:   host,
		c:      c,
		reader: &reader,
		mgr:    dpuservicetemplate.NewDPUServiceTemplateManager(c, c, &reader, operatorNamespace),
		fx:     fx,
	}
	sc.run(e)

	for _, name := range []string{"ovn", "doca-telemetry-service", "hbn"} {
		t := &dpuservicev1alpha1.DPUServiceTemplate{}
		if err := c.Get(context.Background(), types.NamespacedName{Namespace: targetNamespace, Name: name}, t); err == nil {
			if t.DeletionTimestamp != nil {
				// The fake client stamps metav1.Now() on Delete.
				t.DeletionTimestamp = &fixedTime
			}
			fx.Objects[name] = e.norm(encode(t))
		} else {
			fx.Objects[name] = ""
		}
	}
	return fx
}

// ---------------------------------------------------------------------------------------------
// RemoteReleaseImageReader
// ---------------------------------------------------------------------------------------------

// ReaderCase is one GetComponentImage call against the rules registry.
type ReaderCase struct {
	Name      string `json:"name"`
	Rules     []Rule `json:"rules"`
	Ref       string `json:"ref"` // "REGISTRY" is replaced by the registry address
	Component string `json:"component"`
	Result    string `json:"result"`
	Error     string `json:"error"`
}

const imageReferences = `{
  "kind": "ImageStream",
  "apiVersion": "image.openshift.io/v1",
  "spec": {
    "tags": [
      {"name": "kube-proxy", "from": {"kind": "DockerImage", "name": "quay.io/other@sha256:111"}},
      {"name": "ovn-kubernetes", "from": {"kind": "DockerImage", "name": "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:arm64ovn"}},
      {"name": "empty", "from": {"kind": "DockerImage"}}
    ]
  }
}`

func readerCases() []ReaderCase {
	refsPath := "release-manifests/image-references"
	return []ReaderCase{
		{Name: "found", Rules: imageRules("ocp/release", "4.19.0-aarch64", layer([]file{{"release-manifests/0000_00_x.yaml", "x"}, {refsPath, imageReferences}})), Ref: "REGISTRY/ocp/release:4.19.0-aarch64", Component: "ovn-kubernetes"},
		{Name: "found_dot_slash_name", Rules: imageRules("ocp/release", "4.19.0-aarch64", layer([]file{{"./" + refsPath, imageReferences}})), Ref: "REGISTRY/ocp/release:4.19.0-aarch64", Component: "kube-proxy"},
		{Name: "upper_layer_wins", Rules: imageRules("ocp/release", "4.19.0-aarch64",
			layer([]file{{refsPath, `{"spec":{"tags":[{"name":"ovn-kubernetes","from":{"name":"old"}}]}}`}}),
			layer([]file{{refsPath, imageReferences}})), Ref: "REGISTRY/ocp/release:4.19.0-aarch64", Component: "ovn-kubernetes"},
		{Name: "component_missing", Rules: imageRules("ocp/release", "v1", layer([]file{{refsPath, imageReferences}})), Ref: "REGISTRY/ocp/release:v1", Component: "pause"},
		{Name: "component_empty", Rules: imageRules("ocp/release", "v1", layer([]file{{refsPath, imageReferences}})), Ref: "REGISTRY/ocp/release:v1", Component: "empty"},
		{Name: "file_missing", Rules: imageRules("ocp/release", "v1", layer([]file{{"other", "x"}})), Ref: "REGISTRY/ocp/release:v1", Component: "ovn-kubernetes"},
		{Name: "malformed_json", Rules: imageRules("ocp/release", "v1", layer([]file{{refsPath, `{"spec": {"tags": [`}})), Ref: "REGISTRY/ocp/release:v1", Component: "ovn-kubernetes"},
		{Name: "wrong_json_type", Rules: imageRules("ocp/release", "v1", layer([]file{{refsPath, `{"spec": {"tags": [{"name": "ovn-kubernetes", "from": "x"}]}}`}})), Ref: "REGISTRY/ocp/release:v1", Component: "ovn-kubernetes"},
		{Name: "manifest_not_found", Rules: []Rule{}, Ref: "REGISTRY/ocp/release:v1", Component: "ovn-kubernetes"},
		{Name: "bad_reference", Rules: []Rule{}, Ref: "REGISTRY/ocp/Release:v1", Component: "ovn-kubernetes"},
	}
}

func runReaderCase(rc ReaderCase) ReaderCase {
	srv := startRegistry(rc.Rules)
	defer srv.Close()
	host := strings.TrimPrefix(srv.URL, "http://")
	ref := strings.ReplaceAll(rc.Ref, "REGISTRY", host)
	kc := authn.NewMultiKeychain()
	res, err := (&dpuservicetemplate.RemoteReleaseImageReader{}).GetComponentImage(context.Background(), ref, rc.Component, kc)
	rc.Result = res
	if err != nil {
		rc.Error = strings.ReplaceAll(err.Error(), host, "REGISTRY")
	}
	return rc
}

func writeJSON(path string, v any) {
	b, err := json.MarshalIndent(v, "", "  ")
	must(err)
	must(os.WriteFile(path, append(b, '\n'), 0o644))
}

func main() {
	out := "crates/ctrl-dpuservicetemplate/tests/fixtures"
	if len(os.Args) > 1 {
		out = os.Args[1]
	}
	must(os.MkdirAll(out, 0o755))
	for _, sc := range scenarios() {
		writeJSON(filepath.Join(out, sc.name+".json"), runScenario(sc))
	}
	var cases []ReaderCase
	for _, rc := range readerCases() {
		cases = append(cases, runReaderCase(rc))
	}
	writeJSON(filepath.Join(out, "release_reader.json"), cases)
}
