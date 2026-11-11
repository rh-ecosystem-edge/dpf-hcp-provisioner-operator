// Command kubeconfiginjection generates golden fixtures for the Rust port of
// internal/controller/kubeconfiginjection (crates/ctrl-kubeconfiginjection). Each scenario runs
// the Go KubeconfigInjector / CleanupHandler / FindProvisionerForKubeconfigSecret against a
// controller-runtime fake client with fixed inputs and records the result, error, events and
// the JSON encoding (encoding/json) of every object involved, read back from the fake client,
// plus the caller's in-memory DPFHCPProvisioner.
//
// Normalization: condition lastTransitionTime values (metav1.Now()) are replaced by
// 2025-01-01T00:00:00Z before encoding; the Rust test does the same.
//
// Usage: go run -mod=vendor ./hack/golden/kubeconfiginjection crates/ctrl-kubeconfiginjection/tests/fixtures
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/kubeconfiginjection"
)

var fixedTime = metav1.NewTime(time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))

const secretName = "test-provisioner-admin-kubeconfig"

// Step is the outcome of one call.
type Step struct {
	Requeue      bool     `json:"requeue"`
	RequeueAfter int64    `json:"requeueAfter"`
	Error        string   `json:"error"`
	Events       []string `json:"events"`
}

// Fixture is one scenario.
type Fixture struct {
	Steps []Step `json:"steps"`
	// Objects maps a label to the encoding/json bytes of the object ("" when absent).
	Objects map[string]string `json:"objects"`
	// Requests are the reconcile requests returned by FindProvisionerForKubeconfigSecret.
	Requests []string `json:"requests,omitempty"`
}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func scheme() *runtime.Scheme {
	s := runtime.NewScheme()
	must(provisioningv1alpha1.AddToScheme(s))
	must(corev1.AddToScheme(s))
	must(dpuprovisioningv1alpha1.AddToScheme(s))
	return s
}

func provisioner(withHC bool) *provisioningv1alpha1.DPFHCPProvisioner {
	p := &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{
			Name:       "test-provisioner",
			Namespace:  "test-ns",
			Generation: 2,
			UID:        "uid-1",
		},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			DPUClusterRef: provisioningv1alpha1.DPUClusterReference{Name: "test-dpu", Namespace: "dpu-ns"},
		},
	}
	if withHC {
		p.Status.HostedClusterRef = &corev1.ObjectReference{Name: "test-provisioner", Namespace: "test-ns"}
	}
	return p
}

func previouslyInjected(p *provisioningv1alpha1.DPFHCPProvisioner) *provisioningv1alpha1.DPFHCPProvisioner {
	p.Status.KubeConfigSecretRef = &corev1.LocalObjectReference{Name: secretName}
	meta.SetStatusCondition(&p.Status.Conditions, metav1.Condition{
		Type:               provisioningv1alpha1.KubeConfigInjected,
		Status:             metav1.ConditionTrue,
		Reason:             provisioningv1alpha1.ReasonKubeConfigInjected,
		Message:            "Previously injected",
		LastTransitionTime: fixedTime,
	})
	return p
}

func sourceSecret(data map[string][]byte) *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:        secretName,
			Namespace:   "test-ns",
			Labels:      map[string]string{"hypershift.openshift.io/hosted-control-plane": "true"},
			Annotations: map[string]string{"source": "hypershift"},
		},
		Data: data,
		Type: corev1.SecretTypeOpaque,
	}
}

func destSecret(data map[string][]byte) *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:        secretName,
			Namespace:   "dpu-ns",
			Labels:      map[string]string{"user": "label"},
			Annotations: map[string]string{"keep": "me"},
		},
		Data: data,
	}
}

func dpuCluster(kubeconfig string) *dpuprovisioningv1alpha1.DPUCluster {
	return &dpuprovisioningv1alpha1.DPUCluster{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "test-dpu",
			Namespace: "dpu-ns",
			Labels:    map[string]string{"a": "b"},
		},
		Spec: dpuprovisioningv1alpha1.DPUClusterSpec{
			Type:       string(dpuprovisioningv1alpha1.StaticCluster),
			MaxNodes:   10,
			Kubeconfig: kubeconfig,
		},
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

func step(res ctrl.Result, err error, r *record.FakeRecorder) Step {
	s := Step{Requeue: res.Requeue, RequeueAfter: res.RequeueAfter.Nanoseconds(), Events: drain(r)} //nolint:staticcheck
	if err != nil {
		s.Error = err.Error()
	}
	return s
}

func normalize(p *provisioningv1alpha1.DPFHCPProvisioner) {
	for i := range p.Status.Conditions {
		p.Status.Conditions[i].LastTransitionTime = fixedTime
	}
}

func encode(o any) string {
	b, err := json.Marshal(o)
	must(err)
	return string(b)
}

func get(ctx context.Context, c client.Client, ns, name string, obj client.Object) string {
	if err := c.Get(ctx, types.NamespacedName{Namespace: ns, Name: name}, obj); err != nil {
		return ""
	}
	if p, ok := obj.(*provisioningv1alpha1.DPFHCPProvisioner); ok {
		normalize(p)
	}
	return encode(obj)
}

func dump(ctx context.Context, c client.Client, p *provisioningv1alpha1.DPFHCPProvisioner) map[string]string {
	normalize(p)
	out := map[string]string{
		"provisioner.memory": encode(p),
		"provisioner.stored": get(ctx, c, "test-ns", "test-provisioner", &provisioningv1alpha1.DPFHCPProvisioner{}),
		"dpucluster":         get(ctx, c, "dpu-ns", "test-dpu", &dpuprovisioningv1alpha1.DPUCluster{}),
		"secret.source":      get(ctx, c, "test-ns", secretName, &corev1.Secret{}),
		"secret.dest":        get(ctx, c, "dpu-ns", secretName, &corev1.Secret{}),
	}
	return out
}

type injectCase struct {
	name string
	p    *provisioningv1alpha1.DPFHCPProvisioner
	objs []client.Object
	runs int
}

func runInject(tc injectCase) Fixture {
	ctx := context.Background()
	objs := append([]client.Object{tc.p}, tc.objs...)
	c := fake.NewClientBuilder().WithScheme(scheme()).WithObjects(objs...).
		WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}, &dpuprovisioningv1alpha1.DPUCluster{}).Build()
	r := record.NewFakeRecorder(100)
	ki := kubeconfiginjection.NewKubeconfigInjector(c, r)
	f := Fixture{}
	runs := tc.runs
	if runs == 0 {
		runs = 1
	}
	for i := 0; i < runs; i++ {
		res, err := ki.InjectKubeconfig(ctx, tc.p)
		f.Steps = append(f.Steps, step(res, err, r))
	}
	f.Objects = dump(ctx, c, tc.p)
	return f
}

func labelled(name, ns string, owner bool) *corev1.Secret {
	s := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: ns},
		Data:       map[string][]byte{"super-admin.conf": []byte("x")},
	}
	if owner {
		s.Labels = map[string]string{
			common.LabelDPFHCPProvisionerName:      "test-provisioner",
			common.LabelDPFHCPProvisionerNamespace: "test-ns",
		}
	}
	return s
}

func runCleanup(objs []client.Object, runs int) Fixture {
	ctx := context.Background()
	p := provisioner(true)
	c := fake.NewClientBuilder().WithScheme(scheme()).WithObjects(append([]client.Object{p}, objs...)...).Build()
	r := record.NewFakeRecorder(100)
	h := kubeconfiginjection.NewCleanupHandler(c, r)
	f := Fixture{}
	for i := 0; i < runs; i++ {
		res, err := h.Cleanup(ctx, p)
		f.Steps = append(f.Steps, step(res, err, r))
	}
	list := &corev1.SecretList{}
	must(c.List(ctx, list))
	f.Objects = map[string]string{}
	for i := range list.Items {
		s := &list.Items[i]
		f.Objects["secret "+s.Namespace+"/"+s.Name] = encode(s)
	}
	return f
}

func runMap() Fixture {
	ctx := context.Background()
	c := fake.NewClientBuilder().WithScheme(scheme()).WithObjects(
		provisioner(true),
		&provisioningv1alpha1.DPFHCPProvisioner{ObjectMeta: metav1.ObjectMeta{Name: "a-admin-kubeconfig", Namespace: "x"}},
	).Build()
	f := Fixture{Objects: map[string]string{}, Requests: []string{}}
	for _, o := range []client.Object{
		&corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: secretName, Namespace: "test-ns"}},
		&corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: secretName, Namespace: "other"}},
		&corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: "a-admin-kubeconfig-admin-kubeconfig", Namespace: "x"}},
		&corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: "test-provisioner", Namespace: "test-ns"}},
		&corev1.ConfigMap{ObjectMeta: metav1.ObjectMeta{Name: secretName, Namespace: "test-ns"}},
	} {
		reqs := kubeconfiginjection.FindProvisionerForKubeconfigSecret(ctx, c, o)
		for _, r := range reqs {
			f.Requests = append(f.Requests, fmt.Sprintf("%s -> %s", o.GetNamespace()+"/"+o.GetName(), r.String()))
		}
		f.Requests = append(f.Requests, fmt.Sprintf("%s: %d", o.GetNamespace()+"/"+o.GetName(), len(reqs)))
	}
	return f
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: kubeconfiginjection <fixtures-dir>")
		os.Exit(2)
	}
	dir := os.Args[1]
	must(os.MkdirAll(dir, 0o755))

	kc := func(v string) map[string][]byte { return map[string][]byte{"kubeconfig": []byte(v)} }
	sa := func(v string) map[string][]byte {
		return map[string][]byte{"super-admin.conf": []byte(v), "extra": []byte("e")}
	}

	fixtures := map[string]Fixture{}
	for _, tc := range []injectCase{
		{name: "inject_happy", p: provisioner(true), objs: []client.Object{sourceSecret(kc("fake-kubeconfig-data")), dpuCluster("")}, runs: 2},
		{name: "inject_drift", p: previouslyInjected(provisioner(true)), objs: []client.Object{sourceSecret(kc("new-rotated")), destSecret(sa("old")), dpuCluster(secretName)}},
		{name: "inject_no_drift", p: previouslyInjected(provisioner(true)), objs: []client.Object{sourceSecret(kc("same")), destSecret(sa("same")), dpuCluster(secretName)}},
		{name: "inject_scenario_b", p: provisioner(true), objs: []client.Object{sourceSecret(kc("data")), destSecret(sa("data")), dpuCluster("")}},
		{name: "inject_scenario_c", p: previouslyInjected(provisioner(true)), objs: []client.Object{sourceSecret(kc("data")), dpuCluster(secretName)}},
		{name: "inject_dpucluster_missing", p: provisioner(true), objs: []client.Object{sourceSecret(kc("data"))}, runs: 2},
		{name: "inject_secret_not_ready", p: provisioner(true), objs: []client.Object{dpuCluster("")}},
		{name: "inject_hc_not_created", p: provisioner(false), objs: []client.Object{sourceSecret(kc("data")), dpuCluster("")}},
		{name: "inject_source_missing_key", p: provisioner(true), objs: []client.Object{sourceSecret(map[string][]byte{"other": []byte("x")}), dpuCluster("")}},
		{name: "inject_dest_missing_key", p: previouslyInjected(provisioner(true)), objs: []client.Object{sourceSecret(kc("data")), destSecret(map[string][]byte{"other": []byte("x")}), dpuCluster(secretName)}},
		{name: "inject_other_kubeconfig", p: provisioner(true), objs: []client.Object{sourceSecret(kc("data")), dpuCluster("someone-else")}},
	} {
		fixtures[tc.name] = runInject(tc)
	}
	fixtures["cleanup_secrets"] = runCleanup([]client.Object{
		labelled(secretName, "dpu-ns", true),
		labelled("another", "other-ns", true),
		labelled("unrelated", "dpu-ns", false),
	}, 2)
	fixtures["cleanup_none"] = runCleanup(nil, 1)
	fixtures["map_requests"] = runMap()

	for name, f := range fixtures {
		b, err := json.MarshalIndent(f, "", "  ")
		must(err)
		must(os.WriteFile(filepath.Join(dir, name+".json"), append(b, '\n'), 0o644))
	}
}
