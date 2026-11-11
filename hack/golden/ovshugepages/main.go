// Command ovshugepages generates golden fixtures for the Rust port of
// internal/controller/ovs-hugepages (crates/ctrl-ovshugepages).
//
// scenarios.json: each scenario drives the Go ovshugepages.Manager (NewManager +
// ReconcileHugepagesDaemonSet) through a sequence of steps against
//   - a controller-runtime fake client (management cluster: pull secret), and
//   - a client-go kubernetes/fake clientset (hosted cluster). The Go ClientManager caches a
//     concrete *kubernetes.Clientset, so the generator builds a real *kubernetes.Clientset
//     whose HTTP transport serves every request from the fake clientset (fakeTransport) and
//     injects it into the ClientManager's unexported cache (reflect + unsafe), which is what
//     the Rust test does with ClientManager::set_hosted_cluster_client.
//
// For every step it records the returned error (or the recovered panic), whether the hosted
// client is still cached (stale-client invalidation), the fake ReleaseImageReader calls, the
// log records (level, message, key/values) and the encoding/json bytes of the hosted-cluster
// Namespace and DaemonSet read back from the fake clientset ("" when absent).
//
// quantities.json: hugepagesQuantity(size, amount).String() (called through go:linkname),
// or the resource.MustParse panic message, for a table of sizes and amounts.
//
// Nothing is nondeterministic: the client-go fake sets no resourceVersion/uid/timestamps.
//
// Usage: go run -mod=vendor ./hack/golden/ovshugepages crates/ctrl-ovshugepages/tests/fixtures
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"unsafe"

	"github.com/go-logr/logr"
	"github.com/google/go-containerregistry/pkg/authn"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/client-go/kubernetes"
	k8sfake "k8s.io/client-go/kubernetes/fake"
	"k8s.io/client-go/rest"
	k8stesting "k8s.io/client-go/testing"
	"k8s.io/client-go/util/flowcontrol"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	logf "sigs.k8s.io/controller-runtime/pkg/log"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	ovshugepages "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ovs-hugepages"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/hostedclient"
)

//go:linkname hugepagesQuantity github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ovs-hugepages.hugepagesQuantity
func hugepagesQuantity(size string, amount int32) resource.Quantity

const (
	crNamespace = "test-ns"
	crName      = "test"
	multiImage  = "quay.io/openshift-release-dev/ocp-release:4.17.6-multi"
	multiImageB = "quay.io/openshift-release-dev/ocp-release:4.18.1-multi"
	// The .invalid TLD never resolves: IsMultiArchReleaseImage's HEAD fails (=> false)
	// deterministically, and the version comes from the tag (no registry call).
	singleImage  = "registry.invalid/ocp-release:4.17.6-x86_64"
	aarch64Image = "registry.invalid/ocp-release:4.17.6-aarch64"
	pauseA       = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:aaaa"
	pauseB       = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:bbbb"
	pauseArm     = "quay.io/openshift-release-dev/ocp-v4.0-art-dev@sha256:a64a"
)

// ReaderResult is the fake ReleaseImageReader's answer for one release ref.
type ReaderResult struct {
	Image string `json:"image"`
	Err   string `json:"err,omitempty"`
}

// Fail makes the hosted fake clientset reject matching calls with a BadRequest error.
type Fail struct {
	Verb     string `json:"verb"`
	Resource string `json:"resource"`
	Message  string `json:"message"`
}

// Step is one ReconcileHugepagesDaemonSet call (inputs, then outputs).
type Step struct {
	Size    string `json:"size"`
	Amount  int32  `json:"amount"`
	Release string `json:"release"`
	Fail    *Fail  `json:"fail,omitempty"`
	// Reinject puts the hosted client back into the ClientManager cache before the step.
	Reinject bool `json:"reinject,omitempty"`
	// ReplaceNamespace/ReplaceDaemonSet overwrite the hosted objects before the step
	// (external drift), via Update on the fake clientset.
	ReplaceNamespace json.RawMessage `json:"replaceNamespace,omitempty"`
	ReplaceDaemonSet json.RawMessage `json:"replaceDaemonSet,omitempty"`

	Error       string   `json:"error"`
	Panic       bool     `json:"panic"`
	Cached      bool     `json:"cached"`
	ReaderCalls []string `json:"readerCalls"`
	Logs        []string `json:"logs"`
	Namespace   string   `json:"namespace"`
	DaemonSet   string   `json:"daemonSet"`
}

// Scenario is one Manager with its fakes.
type Scenario struct {
	// PullSecret: "valid", "missing" (no secret) or "nokey" (no .dockerconfigjson).
	PullSecret     string                  `json:"pullSecret"`
	InjectClient   bool                    `json:"injectClient"`
	Reader         map[string]ReaderResult `json:"reader"`
	SeedNamespaces []json.RawMessage       `json:"seedNamespaces,omitempty"`
	Steps          []Step                  `json:"steps"`
}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func mustJSON(v any) json.RawMessage {
	b, err := json.Marshal(v)
	must(err)
	return b
}

// --- fake release reader ---------------------------------------------------------------------

type fakeReader struct {
	results map[string]ReaderResult
	calls   []string
}

func (f *fakeReader) GetComponentImage(_ context.Context, ref, component string, _ authn.Keychain) (string, error) {
	f.calls = append(f.calls, ref+" "+component)
	r := f.results[ref]
	if r.Err != "" {
		return "", errors.New(r.Err)
	}
	return r.Image, nil
}

// maxLogLevel: records above it come from client-go's REST client (the harness), not from
// the package under test (which logs at V(0)/V(1)).
const maxLogLevel = 4

// --- log capture -----------------------------------------------------------------------------

type captureSink struct {
	lines  *[]string
	name   string
	values []any
}

func (s *captureSink) Init(logr.RuntimeInfo) {}
func (s *captureSink) Enabled(int) bool      { return true }
func (s *captureSink) record(prefix, msg string, kvs []any) {
	all := append(append([]any{}, s.values...), kvs...)
	b, err := json.Marshal(all)
	must(err)
	*s.lines = append(*s.lines, fmt.Sprintf("%s %s: %s %s", prefix, s.name, msg, b))
}
func (s *captureSink) Info(level int, msg string, kvs ...any) {
	if level > maxLogLevel {
		return // client-go's own request/response tracing (V(6)+) of the HTTP harness
	}
	s.record(fmt.Sprintf("V%d", level), msg, kvs)
}
func (s *captureSink) Error(err error, msg string, kvs ...any) {
	s.record("E", msg, append(kvs, "error", err.Error()))
}
func (s *captureSink) WithValues(kvs ...any) logr.LogSink {
	c := *s
	c.values = append(append([]any{}, s.values...), kvs...)
	return &c
}
func (s *captureSink) WithName(name string) logr.LogSink {
	c := *s
	if c.name == "" {
		c.name = name
	} else {
		c.name += "." + name
	}
	return &c
}

// --- *kubernetes.Clientset served by a kubernetes/fake clientset -------------------------

type fakeTransport struct {
	backend *k8sfake.Clientset
}

func (t *fakeTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	ctx := req.Context()
	parts := strings.Split(strings.Trim(req.URL.Path, "/"), "/")
	var body []byte
	if req.Body != nil {
		b, err := io.ReadAll(req.Body)
		must(err)
		body = b
	}
	var (
		obj  runtime.Object
		err  error
		code = http.StatusOK
	)
	switch {
	case len(parts) >= 3 && parts[0] == "api" && parts[1] == "v1" && parts[2] == "namespaces" && len(parts) <= 4:
		c := t.backend.CoreV1().Namespaces()
		switch {
		case req.Method == http.MethodGet && len(parts) == 4:
			obj, err = c.Get(ctx, parts[3], metav1.GetOptions{})
		case req.Method == http.MethodPost && len(parts) == 3:
			ns := &corev1.Namespace{}
			must(json.Unmarshal(body, ns))
			ns.TypeMeta = metav1.TypeMeta{}
			obj, err = c.Create(ctx, ns, metav1.CreateOptions{})
			code = http.StatusCreated
		case req.Method == http.MethodPut && len(parts) == 4:
			ns := &corev1.Namespace{}
			must(json.Unmarshal(body, ns))
			ns.TypeMeta = metav1.TypeMeta{}
			obj, err = c.Update(ctx, ns, metav1.UpdateOptions{})
		default:
			panic("unexpected request " + req.Method + " " + req.URL.Path)
		}
	case len(parts) >= 6 && parts[0] == "apis" && parts[1] == "apps" && parts[2] == "v1" && parts[3] == "namespaces" && parts[5] == "daemonsets" && len(parts) <= 7:
		c := t.backend.AppsV1().DaemonSets(parts[4])
		switch {
		case req.Method == http.MethodGet && len(parts) == 7:
			obj, err = c.Get(ctx, parts[6], metav1.GetOptions{})
		case req.Method == http.MethodPost && len(parts) == 6:
			ds := &appsv1.DaemonSet{}
			must(json.Unmarshal(body, ds))
			ds.TypeMeta = metav1.TypeMeta{}
			obj, err = c.Create(ctx, ds, metav1.CreateOptions{})
			code = http.StatusCreated
		case req.Method == http.MethodPut && len(parts) == 7:
			ds := &appsv1.DaemonSet{}
			must(json.Unmarshal(body, ds))
			ds.TypeMeta = metav1.TypeMeta{}
			obj, err = c.Update(ctx, ds, metav1.UpdateOptions{})
		case req.Method == http.MethodDelete && len(parts) == 7:
			err = c.Delete(ctx, parts[6], metav1.DeleteOptions{})
			obj = &metav1.Status{TypeMeta: metav1.TypeMeta{Kind: "Status", APIVersion: "v1"}, Status: metav1.StatusSuccess}
		default:
			panic("unexpected request " + req.Method + " " + req.URL.Path)
		}
	default:
		panic("unexpected request " + req.Method + " " + req.URL.Path)
	}
	if err == nil {
		// Like an API server, answer with kind/apiVersion set (the stored objects carry an
		// empty TypeMeta, as with direct typed fake calls: request bodies are stored with TypeMeta cleared).
		obj = obj.DeepCopyObject()
		switch o := obj.(type) {
		case *corev1.Namespace:
			o.SetGroupVersionKind(corev1.SchemeGroupVersion.WithKind("Namespace"))
		case *appsv1.DaemonSet:
			o.SetGroupVersionKind(appsv1.SchemeGroupVersion.WithKind("DaemonSet"))
		}
	}
	if err != nil {
		var apiStatus apierrors.APIStatus
		if !errors.As(err, &apiStatus) {
			panic(err)
		}
		st := apiStatus.Status()
		st.Kind = "Status"
		st.APIVersion = "v1"
		obj = &st
		code = int(st.Code)
	}
	out, merr := json.Marshal(obj)
	must(merr)
	return &http.Response{
		StatusCode: code,
		Header:     http.Header{"Content-Type": []string{"application/json"}},
		Body:       io.NopCloser(bytes.NewReader(out)),
		Request:    req,
	}, nil
}

func newClientset(backend *k8sfake.Clientset) *kubernetes.Clientset {
	cfg := &rest.Config{
		Host:        "http://hosted.invalid",
		RateLimiter: flowcontrol.NewFakeAlwaysRateLimiter(),
		ContentConfig: rest.ContentConfig{
			ContentType: "application/json",
		},
	}
	cs, err := kubernetes.NewForConfigAndClient(cfg, &http.Client{Transport: &fakeTransport{backend: backend}})
	must(err)
	return cs
}

// hcClients returns the ClientManager's unexported cache.
func hcClients(cm *hostedclient.ClientManager) reflect.Value {
	f := reflect.ValueOf(cm).Elem().FieldByName("hcClients")
	return reflect.NewAt(f.Type(), unsafe.Pointer(f.UnsafeAddr())).Elem()
}

func cacheKey() reflect.Value { return reflect.ValueOf(crNamespace + "/" + crName) }

// --- objects -------------------------------------------------------------------------------

func scheme() *runtime.Scheme {
	s := runtime.NewScheme()
	must(corev1.AddToScheme(s))
	must(provisioningv1alpha1.AddToScheme(s))
	return s
}

func provisioner(release string) *provisioningv1alpha1.DPFHCPProvisioner {
	return &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNamespace},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			OCPReleaseImage: release,
			PullSecretRef:   corev1.LocalObjectReference{Name: "pull-secret"},
		},
	}
}

func pullSecret(kind string) []client.Object {
	switch kind {
	case "valid":
		return []client.Object{&corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{Name: "pull-secret", Namespace: crNamespace},
			Data:       map[string][]byte{".dockerconfigjson": []byte(`{"auths":{}}`)},
			Type:       corev1.SecretTypeDockerConfigJson,
		}}
	case "nokey":
		return []client.Object{&corev1.Secret{
			ObjectMeta: metav1.ObjectMeta{Name: "pull-secret", Namespace: crNamespace},
			Data:       map[string][]byte{"other": []byte(`x`)},
		}}
	default:
		return nil
	}
}

func labels() map[string]string {
	return map[string]string{
		"app.kubernetes.io/name":       ovshugepages.DaemonSetName,
		"app.kubernetes.io/managed-by": "dpf-hcp-provisioner-operator",
	}
}

// driftedDaemonSet is the DaemonSet created for (pauseA, 2Mi, 250) after external edits.
func driftedDaemonSet() *appsv1.DaemonSet {
	qty := resource.MustParse("500Mi")
	return &appsv1.DaemonSet{
		ObjectMeta: metav1.ObjectMeta{
			Name:        ovshugepages.DaemonSetName,
			Namespace:   ovshugepages.OVSHugepagesNamespace,
			Labels:      map[string]string{"app.kubernetes.io/name": ovshugepages.DaemonSetName, "extra": "label"},
			Annotations: map[string]string{"keep": "me"},
		},
		Spec: appsv1.DaemonSetSpec{
			Selector: &metav1.LabelSelector{MatchLabels: labels()},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{Labels: labels(), Annotations: map[string]string{"drift": "true"}},
				Spec: corev1.PodSpec{
					NodeSelector:      map[string]string{"kubernetes.io/arch": "arm64"},
					PriorityClassName: "system-cluster-critical",
					Containers: []corev1.Container{{
						Name:    "allocate",
						Image:   "example.com/other:latest",
						Command: []string{"sleep", "infinity"},
						Resources: corev1.ResourceRequirements{
							Requests: corev1.ResourceList{"hugepages-2Mi": qty},
							Limits:   corev1.ResourceList{"hugepages-2Mi": qty},
						},
					}},
				},
			},
			UpdateStrategy:  appsv1.DaemonSetUpdateStrategy{Type: appsv1.OnDeleteDaemonSetStrategyType},
			MinReadySeconds: 5,
		},
		Status: appsv1.DaemonSetStatus{DesiredNumberScheduled: 3, NumberReady: 2, ObservedGeneration: 1},
	}
}

// --- runner --------------------------------------------------------------------------------

func run(sc *Scenario) {
	ctx := context.Background()
	mgmt := fake.NewClientBuilder().WithScheme(scheme()).WithObjects(pullSecret(sc.PullSecret)...).Build()

	var seeds []runtime.Object
	for _, raw := range sc.SeedNamespaces {
		ns := &corev1.Namespace{}
		must(json.Unmarshal(raw, ns))
		seeds = append(seeds, ns)
	}
	backend := k8sfake.NewSimpleClientset(seeds...)
	var fail *Fail
	backend.PrependReactor("*", "*", func(a k8stesting.Action) (bool, runtime.Object, error) {
		if fail != nil && a.Matches(fail.Verb, fail.Resource) {
			return true, nil, apierrors.NewBadRequest(fail.Message)
		}
		return false, nil, nil
	})
	cs := newClientset(backend)

	cm := hostedclient.NewClientManager(mgmt)
	inject := func() { hcClients(cm).SetMapIndex(cacheKey(), reflect.ValueOf(cs)) }
	if sc.InjectClient {
		inject()
	}
	reader := &fakeReader{results: sc.Reader}
	m := ovshugepages.NewManager(mgmt, cm, reader)

	for i := range sc.Steps {
		st := &sc.Steps[i]
		if st.Reinject {
			inject()
		}
		if st.ReplaceNamespace != nil {
			ns := &corev1.Namespace{}
			must(json.Unmarshal(st.ReplaceNamespace, ns))
			_, err := backend.CoreV1().Namespaces().Update(ctx, ns, metav1.UpdateOptions{})
			must(err)
		}
		if st.ReplaceDaemonSet != nil {
			ds := &appsv1.DaemonSet{}
			must(json.Unmarshal(st.ReplaceDaemonSet, ds))
			_, err := backend.AppsV1().DaemonSets(ds.Namespace).Update(ctx, ds, metav1.UpdateOptions{})
			must(err)
		}
		fail = st.Fail
		reader.calls = nil
		var lines []string
		lctx := logf.IntoContext(ctx, logr.New(&captureSink{lines: &lines}))

		func() {
			defer func() {
				if r := recover(); r != nil {
					st.Panic = true
					st.Error = fmt.Sprint(r)
				}
			}()
			if err := m.ReconcileHugepagesDaemonSet(lctx, provisioner(st.Release), st.Size, st.Amount); err != nil {
				st.Error = err.Error()
			}
		}()
		fail = nil

		st.Cached = hcClients(cm).MapIndex(cacheKey()).IsValid()
		st.ReaderCalls = reader.calls
		st.Logs = lines
		if ns, err := backend.CoreV1().Namespaces().Get(ctx, ovshugepages.OVSHugepagesNamespace, metav1.GetOptions{}); err == nil {
			st.Namespace = string(mustJSON(ns))
		} else if !apierrors.IsNotFound(err) {
			panic(err)
		}
		if ds, err := backend.AppsV1().DaemonSets(ovshugepages.OVSHugepagesNamespace).Get(ctx, ovshugepages.DaemonSetName, metav1.GetOptions{}); err == nil {
			st.DaemonSet = string(mustJSON(ds))
		} else if !apierrors.IsNotFound(err) {
			panic(err)
		}
	}
}

func step(size string, amount int32, release string) Step {
	return Step{Size: size, Amount: amount, Release: release}
}

func scenarios() map[string]*Scenario {
	readers := map[string]ReaderResult{
		multiImage:   {Image: pauseA},
		multiImageB:  {Image: pauseB},
		aarch64Image: {Image: pauseArm},
	}
	valid := func(steps ...Step) *Scenario {
		return &Scenario{PullSecret: "valid", InjectClient: true, Reader: readers, Steps: steps}
	}
	withFail := func(s Step, verb, res, msg string) Step {
		s.Fail = &Fail{Verb: verb, Resource: res, Message: msg}
		return s
	}
	reinject := func(s Step) Step {
		s.Reinject = true
		return s
	}

	driftNs := mustJSON(&corev1.Namespace{ObjectMeta: metav1.ObjectMeta{
		Name:   ovshugepages.OVSHugepagesNamespace,
		Labels: map[string]string{"app.kubernetes.io/name": "wrong", "other": "x"},
	}})
	driftStep := step("2Mi", 250, multiImage)
	driftStep.ReplaceNamespace = driftNs
	driftStep.ReplaceDaemonSet = mustJSON(driftedDaemonSet())

	return map[string]*Scenario{
		"create_default": valid(
			step("2Mi", 250, multiImage),
			step("2Mi", 250, multiImage),
		),
		"defaults_for_empty_size_and_negative_amount": valid(
			step("", -1, multiImage),
			step("2Mi", 250, multiImage),
		),
		"custom_sizes": valid(
			step("1Gi", 4, multiImage),
			step("32Mi", 8, multiImage),
			step("64Ki", 1000, multiImage),
			step("1.5Mi", 3, multiImage),
			step("1k", 1, multiImage),
			step("500", 3, multiImage),
			step("-2Mi", 250, multiImage),
			step("2Mi", 2147483647, multiImage),
		),
		"amount_and_image_change": valid(
			step("2Mi", 250, multiImage),
			step("2Mi", 500, multiImage),
			step("2Mi", 500, multiImageB),
			step("2Mi", 500, multiImage),
		),
		"drift_corrected": valid(
			step("2Mi", 250, multiImage),
			driftStep,
			step("2Mi", 250, multiImage),
		),
		"existing_namespace_labels_merged": {
			PullSecret: "valid", InjectClient: true, Reader: readers,
			SeedNamespaces: []json.RawMessage{mustJSON(&corev1.Namespace{ObjectMeta: metav1.ObjectMeta{
				Name:        ovshugepages.OVSHugepagesNamespace,
				Labels:      map[string]string{"other": "x"},
				Annotations: map[string]string{"a": "b"},
			}})},
			Steps: []Step{step("2Mi", 250, multiImage)},
		},
		"disable_after_create": valid(
			step("2Mi", 250, multiImage),
			step("2Mi", 0, multiImage),
			step("2Mi", 0, multiImage),
			step("2Mi", 250, multiImage),
		),
		"disabled_nothing_exists": valid(
			step("2Mi", 0, multiImage),
			step("bogus", 0, ""),
		),
		"aarch64_release": valid(
			step("2Mi", 250, singleImage),
			step("2Mi", 250, singleImage),
		),
		"aarch64_reader_error": {
			PullSecret: "valid", InjectClient: true,
			Reader: map[string]ReaderResult{aarch64Image: {Err: "registry unreachable"}},
			Steps:  []Step{step("2Mi", 250, singleImage), step("2Mi", 250, singleImage)},
		},
		"aarch64_empty_image": {
			PullSecret: "valid", InjectClient: true,
			Reader: map[string]ReaderResult{aarch64Image: {Image: ""}},
			Steps:  []Step{step("2Mi", 250, singleImage)},
		},
		"multi_reader_error": {
			PullSecret: "valid", InjectClient: true,
			Reader: map[string]ReaderResult{multiImage: {Err: "manifest unknown"}},
			Steps:  []Step{step("2Mi", 250, multiImage)},
		},
		"multi_empty_image": {
			PullSecret: "valid", InjectClient: true,
			Reader: map[string]ReaderResult{},
			Steps:  []Step{step("2Mi", 250, multiImage)},
		},
		"pull_secret_missing": {
			PullSecret: "missing", InjectClient: true, Reader: readers,
			Steps: []Step{step("2Mi", 250, multiImage)},
		},
		"pull_secret_missing_key": {
			PullSecret: "nokey", InjectClient: true, Reader: readers,
			Steps: []Step{step("2Mi", 250, multiImage)},
		},
		"hosted_client_unavailable": {
			PullSecret: "valid", InjectClient: false, Reader: readers,
			Steps: []Step{step("2Mi", 250, multiImage), step("2Mi", 0, multiImage)},
		},
		"namespace_get_fails": valid(
			withFail(step("2Mi", 250, multiImage), "get", "namespaces", "namespaces get boom"),
			reinject(step("2Mi", 250, multiImage)),
		),
		"namespace_create_fails": valid(
			withFail(step("2Mi", 250, multiImage), "create", "namespaces", "namespaces create boom"),
		),
		"namespace_update_fails": valid(
			step("2Mi", 250, multiImage),
			func() Step {
				s := withFail(step("2Mi", 250, multiImage), "update", "namespaces", "namespaces update boom")
				s.ReplaceNamespace = driftNs
				return s
			}(),
		),
		"daemonset_create_fails": valid(
			withFail(step("2Mi", 250, multiImage), "create", "daemonsets", "daemonsets create boom"),
			reinject(step("2Mi", 250, multiImage)),
		),
		"daemonset_get_fails": valid(
			withFail(step("2Mi", 250, multiImage), "get", "daemonsets", "daemonsets get boom"),
		),
		"daemonset_update_fails": valid(
			step("2Mi", 250, multiImage),
			withFail(step("2Mi", 300, multiImage), "update", "daemonsets", "daemonsets update boom"),
		),
		"daemonset_delete_fails": valid(
			step("2Mi", 250, multiImage),
			withFail(step("2Mi", 0, multiImage), "delete", "daemonsets", "daemonsets delete boom"),
			reinject(step("2Mi", 0, multiImage)),
		),
		"invalid_size": valid(
			step("bogus", 250, multiImage),
			step("", 250, multiImage),
		),
	}
}

// Quantity is one hugepagesQuantity case.
type Quantity struct {
	Size     string `json:"size"`
	Amount   int32  `json:"amount"`
	Quantity string `json:"quantity"`
	Error    string `json:"error"`
}

func quantities() []Quantity {
	sizes := []string{
		"1Gi", "2Mi", "32Mi", "64Ki", "1.5Mi", "500m", "1k", "0", "1e3", "-2Mi", "1Ei", "8Ei",
		"1.5", "-1.5", "0.5n", "1.5e-10", "12345678901234567890", "-12345678901234567890",
		"1Pi", "3Ti", "123Ti", "100000Gi", "1.Gi", "+4Ki", "007", "10E", "1e30", "999",
		"bogus", "", "1Zi", "1.5Ki", "0.001", "-0.5n", "1023", "1024", "1M", "2G",
	}
	amounts := []int32{0, 1, 3, 250, 1000, 2147483647, -1}
	var out []Quantity
	for _, s := range sizes {
		for _, a := range amounts {
			q := Quantity{Size: s, Amount: a}
			func() {
				defer func() {
					if r := recover(); r != nil {
						q.Error = fmt.Sprint(r)
					}
				}()
				v := hugepagesQuantity(s, a)
				q.Quantity = v.String()
			}()
			out = append(out, q)
		}
	}
	return out
}

func write(dir, name string, v any) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetIndent("", "  ")
	must(enc.Encode(v))
	must(os.WriteFile(filepath.Join(dir, name), buf.Bytes(), 0o644))
}

func main() {
	dir := "crates/ctrl-ovshugepages/tests/fixtures"
	if len(os.Args) > 1 {
		dir = os.Args[1]
	}
	must(os.MkdirAll(dir, 0o755))

	all := scenarios()
	for name, sc := range all {
		run(sc)
		write(dir, name+".json", sc)
	}
	write(dir, "quantities.json", quantities())
}
