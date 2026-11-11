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

// Command bfocplookup generates golden fixtures for the Rust port of
// internal/controller/bfocplookup (crates/ctrl-bfocplookup).
//
// Each scenario runs ImageLookup.LookupBlueFieldOCPLayerImage (one or more steps) against a
// controller-runtime fake client with a fake ImageChecker, and records for every step the
// result, the error, the emitted events and the image references passed to the checker, plus
// the encoding/json bytes of the caller's in-memory DPFHCPProvisioner and of the stored copy.
// Digest release images are resolved through an httptest "rules" registry (fixed responses per
// method+path, recorded in the fixture so the Rust test can serve the same bytes).
//
// Normalization: the registry address is replaced by "REGISTRY" everywhere, and condition
// lastTransitionTime values (metav1.Now()) are set to 2025-01-01T00:00:00Z before encoding.
//
// Usage: go run -mod=vendor ./hack/golden/bfocplookup crates/ctrl-bfocplookup/tests/fixtures
package main

import (
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
	"github.com/google/go-containerregistry/pkg/name"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
)

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

func b64(b []byte) string { return base64.StdEncoding.EncodeToString(b) }

const ociManifest = "application/vnd.oci.image.manifest.v1+json"

// releaseImage builds rules serving a single-platform image in repo "ocp/release" whose config
// has the given labels; returns the rules and the manifest digest. configStatus overrides the
// config blob response status (0 = 200).
func releaseImage(labels map[string]string, configStatus int) ([]Rule, string) {
	lb, _ := json.Marshal(labels)
	cfg := []byte(fmt.Sprintf(`{"architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":[]},"config":{"Labels":%s}}`, lb))
	manifest := []byte(fmt.Sprintf(`{"schemaVersion":2,"mediaType":%q,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":%d,"digest":%q},"layers":[]}`,
		ociManifest, len(cfg), digestOf(cfg)))
	if configStatus == 0 {
		configStatus = 200
	}
	d := digestOf(manifest)
	return []Rule{
		{Path: "/v2/ocp/release/blobs/" + digestOf(cfg), Status: configStatus, Headers: map[string]string{"Content-Type": "application/octet-stream"}, Body: b64(cfg)},
		{Path: "/v2/ocp/release/manifests/" + d, Status: 200, Headers: map[string]string{"Content-Type": ociManifest, "Docker-Content-Digest": d}, Body: b64(manifest)},
	}, d
}

var fixedTime = metav1.NewTime(time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func scheme() *runtime.Scheme {
	s := runtime.NewScheme()
	must(provisioningv1alpha1.AddToScheme(s))
	must(corev1.AddToScheme(s))
	return s
}

const (
	testRepository = "quay.io/test-org/bluefield-os"
	taggedRelease  = "quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-x86_64"
)

func provisioner(releaseImage string) *provisioningv1alpha1.DPFHCPProvisioner {
	return &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{Name: "test-provisioner", Namespace: "test-ns", Generation: 3, UID: "uid-1"},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			OCPReleaseImage: releaseImage,
			PullSecretRef:   corev1.LocalObjectReference{Name: "pull-secret"},
		},
	}
}

func pullSecret(data map[string][]byte) *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: "pull-secret", Namespace: "test-ns"},
		Data:       data,
		Type:       corev1.SecretTypeDockerConfigJson,
	}
}

func goodSecret() *corev1.Secret {
	return pullSecret(map[string][]byte{".dockerconfigjson": []byte(`{"auths":{"quay.io":{"auth":"dTpw"}}}`)})
}

type fakeChecker struct {
	errs  []string // per call; "" = success; last entry repeats
	n     int      // calls so far
	calls []string // references checked during the current step
}

func (f *fakeChecker) CheckTag(_ context.Context, ref name.Reference, _ authn.Keychain) error {
	f.calls = append(f.calls, ref.String())
	i := f.n
	f.n++
	if i >= len(f.errs) {
		i = len(f.errs) - 1
	}
	if i < 0 || f.errs[i] == "" {
		return nil
	}
	return fmt.Errorf("%s", f.errs[i])
}

// Step is the outcome of one call.
type Step struct {
	Requeue      bool     `json:"requeue"`
	RequeueAfter int64    `json:"requeueAfter"`
	Error        string   `json:"error"`
	Events       []string `json:"events"`
	Checked      []string `json:"checked"`
}

// Fixture is one scenario.
type Fixture struct {
	Rules   []Rule            `json:"rules"`
	Steps   []Step            `json:"steps"`
	Objects map[string]string `json:"objects"`
}

type scenario struct {
	name       string
	rules      func() ([]Rule, string) // rules + manifest digest (for digest images)
	release    func(host, digest string) string
	repository string
	secret     *corev1.Secret
	noStored   bool
	checker    []string
	steps      int
}

func scenarios() []scenario {
	tagged := func(string, string) string { return taggedRelease }
	digest := func(h, d string) string { return h + "/ocp/release@" + d }
	labelled := func() ([]Rule, string) {
		return releaseImage(map[string]string{"io.openshift.release": "4.20.1", "other": "x"}, 0)
	}
	return []scenario{
		{name: "empty_release_image", release: func(string, string) string { return "" }, repository: testRepository, secret: goodSecret(), steps: 2},
		{name: "missing_pull_secret", release: tagged, repository: testRepository},
		{name: "pull_secret_missing_key", release: tagged, repository: testRepository, secret: pullSecret(map[string][]byte{"other": []byte("x")})},
		{name: "pull_secret_invalid_json", release: tagged, repository: testRepository, secret: pullSecret(map[string][]byte{".dockerconfigjson": []byte(`{"auths":`)})},
		{name: "found", release: tagged, repository: testRepository, secret: goodSecret(), checker: []string{""}, steps: 2},
		{name: "found_then_not_found", release: tagged, repository: testRepository, secret: goodSecret(), checker: []string{"", "MANIFEST_UNKNOWN: manifest unknown"}, steps: 3},
		{name: "version_not_found", release: tagged, repository: testRepository, secret: goodSecret(), checker: []string{"GET https://quay.io/v2/x/manifests/4.19.0-ec.5: MANIFEST_UNKNOWN: manifest unknown; unknown tag=4.19.0-ec.5"}},
		{name: "auth_error", release: tagged, repository: testRepository, secret: goodSecret(), checker: []string{"UNAUTHORIZED: authentication required"}},
		{name: "transient_error", release: tagged, repository: testRepository, secret: goodSecret(), checker: []string{"Get \"https://quay.io/v2/\": dial tcp: connection refused"}, steps: 2},
		{name: "invalid_repository", release: tagged, repository: "INVALID//repo", secret: goodSecret(), checker: []string{""}},
		{name: "empty_repository", release: tagged, repository: "", secret: goodSecret(), checker: []string{""}},
		{name: "status_update_fails_on_success", release: tagged, repository: testRepository, secret: goodSecret(), checker: []string{""}, noStored: true},
		{name: "status_update_fails_on_permanent_error", release: tagged, repository: testRepository, secret: goodSecret(), checker: []string{"MANIFEST_UNKNOWN"}, noStored: true},
		{name: "digest_with_label", rules: labelled, release: digest, repository: testRepository, secret: goodSecret(), checker: []string{""}},
		{name: "digest_without_label", rules: func() ([]Rule, string) { return releaseImage(map[string]string{"other": "x"}, 0) }, release: digest, repository: testRepository, secret: goodSecret(), checker: []string{""}},
		{name: "digest_empty_label", rules: func() ([]Rule, string) { return releaseImage(map[string]string{"io.openshift.release": ""}, 0) }, release: digest, repository: testRepository, secret: goodSecret(), checker: []string{""}},
		{name: "digest_manifest_not_found", rules: func() ([]Rule, string) {
			_, d := labelled()
			return []Rule{}, d
		}, release: digest, repository: testRepository, secret: goodSecret(), checker: []string{""}},
		{name: "digest_unauthorized", rules: func() ([]Rule, string) {
			_, d := labelled()
			return []Rule{{Path: "/v2/ocp/release/manifests/" + d, Status: 401, Headers: map[string]string{"Content-Type": "application/json"},
				Body: b64([]byte(`{"errors":[{"code":"UNAUTHORIZED","message":"authentication required"}]}`))}}, d
		}, release: digest, repository: testRepository, secret: goodSecret(), checker: []string{""}},
		{name: "digest_config_not_found", rules: func() ([]Rule, string) {
			return releaseImage(map[string]string{"io.openshift.release": "4.20.1"}, 404)
		}, release: digest, repository: testRepository, secret: goodSecret(), checker: []string{""}},
		{name: "digest_bad_reference", release: func(h, _ string) string { return h + "/ocp/release@sha256:abc" }, repository: testRepository, secret: goodSecret(), checker: []string{""}},
	}
}

func drain(r *record.FakeRecorder) []string {
	out := []string{}
	for {
		select {
		case e := <-r.Events:
			out = append(out, e)
		default:
			return out
		}
	}
}

func normalize(p *provisioningv1alpha1.DPFHCPProvisioner) {
	for i := range p.Status.Conditions {
		p.Status.Conditions[i].LastTransitionTime = fixedTime
	}
}

func run(sc scenario) *Fixture {
	var rules []Rule
	var digest string
	if sc.rules != nil {
		rules, digest = sc.rules()
	}
	if rules == nil {
		rules = []Rule{}
	}
	srv := startRegistry(rules)
	defer srv.Close()
	host := strings.TrimPrefix(srv.URL, "http://")
	norm := func(s string) string { return strings.ReplaceAll(s, host, "REGISTRY") }

	cr := provisioner(sc.release(host, digest))
	var objs []client.Object
	if !sc.noStored {
		objs = append(objs, cr.DeepCopy())
	}
	if sc.secret != nil {
		objs = append(objs, sc.secret)
	}
	c := fake.NewClientBuilder().WithScheme(scheme()).WithObjects(objs...).
		WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).Build()
	rec := record.NewFakeRecorder(100)
	checker := &fakeChecker{errs: sc.checker}
	lookup := bfocplookup.NewImageLookup(c, rec)
	lookup.ImageChecker = checker
	lookup.Repository = sc.repository

	fx := &Fixture{Rules: rules, Steps: []Step{}, Objects: map[string]string{}}
	steps := sc.steps
	if steps == 0 {
		steps = 1
	}
	for i := 0; i < steps; i++ {
		if !sc.noStored {
			// Like a reconcile: start from the stored object.
			cr = &provisioningv1alpha1.DPFHCPProvisioner{}
			must(c.Get(context.Background(), types.NamespacedName{Namespace: "test-ns", Name: "test-provisioner"}, cr))
		}
		res, err := lookup.LookupBlueFieldOCPLayerImage(context.Background(), cr)
		s := Step{Requeue: res.Requeue, RequeueAfter: int64(res.RequeueAfter), Events: drain(rec), Checked: []string{}}
		if err != nil {
			s.Error = norm(err.Error())
		}
		for i := range s.Events {
			s.Events[i] = norm(s.Events[i])
		}
		for _, ch := range checker.calls {
			s.Checked = append(s.Checked, norm(ch))
		}
		checker.calls = nil
		fx.Steps = append(fx.Steps, s)
	}

	normalize(cr)
	b, err := json.Marshal(cr)
	must(err)
	fx.Objects["provisioner.memory"] = norm(string(b))
	stored := &provisioningv1alpha1.DPFHCPProvisioner{}
	if err := c.Get(context.Background(), types.NamespacedName{Namespace: "test-ns", Name: "test-provisioner"}, stored); err == nil {
		normalize(stored)
		b, err := json.Marshal(stored)
		must(err)
		fx.Objects["provisioner.stored"] = norm(string(b))
	} else {
		fx.Objects["provisioner.stored"] = ""
	}
	return fx
}

func main() {
	out := "crates/ctrl-bfocplookup/tests/fixtures"
	if len(os.Args) > 1 {
		out = os.Args[1]
	}
	must(os.MkdirAll(out, 0o755))
	for _, sc := range scenarios() {
		b, err := json.MarshalIndent(run(sc), "", "  ")
		must(err)
		must(os.WriteFile(filepath.Join(out, sc.name+".json"), append(b, '\n'), 0o644))
	}
}
