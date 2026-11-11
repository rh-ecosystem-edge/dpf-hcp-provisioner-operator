// Command metallb generates golden fixtures for the Rust port of internal/controller/metallb
// (crates/ctrl-metallb). Each scenario runs the Go MetalLBManager / CleanupHandler against a
// controller-runtime fake client with fixed inputs and records the result, error, events and
// the JSON encoding (encoding/json) of every object the operator wrote, read back from the
// fake client, plus the caller's in-memory DPFHCPProvisioner.
//
// Normalization: condition lastTransitionTime values (metav1.Now()) are replaced by
// 2025-01-01T00:00:00Z before encoding; the Rust test does the same.
//
// Usage: go run -mod=vendor ./hack/golden/metallb crates/ctrl-metallb/tests/fixtures
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"

	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"
	"k8s.io/apimachinery/pkg/api/meta"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/common"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/metallb"
)

var fixedTime = metav1.NewTime(time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))

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
}

func scheme() *runtime.Scheme {
	s := runtime.NewScheme()
	must(provisioningv1alpha1.AddToScheme(s))
	must(metallbv1beta1.AddToScheme(s))
	must(hyperv1.AddToScheme(s))
	return s
}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func provisioner(policy hyperv1.AvailabilityPolicy, vip string) *provisioningv1alpha1.DPFHCPProvisioner {
	return &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{
			Name:       "test-provisioner",
			Namespace:  "test-ns",
			Generation: 3,
			UID:        "uid-1",
		},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			ControlPlaneAvailabilityPolicy: policy,
			VirtualIP:                      vip,
		},
	}
}

func ownerLabels() map[string]string {
	return map[string]string{
		common.LabelDPFHCPProvisionerName:      "test-provisioner",
		common.LabelDPFHCPProvisionerNamespace: "test-ns",
	}
}

func driftedPool() *metallbv1beta1.IPAddressPool {
	labels := ownerLabels()
	labels["extra"] = "kept"
	return &metallbv1beta1.IPAddressPool{
		ObjectMeta: metav1.ObjectMeta{
			Name:        "test-provisioner",
			Namespace:   common.OpenshiftOperatorsNamespace,
			Labels:      labels,
			Annotations: map[string]string{"note": "user"},
		},
		Spec: metallbv1beta1.IPAddressPoolSpec{
			Addresses:     []string{"192.168.1.200/32", "10.0.0.0/24"},
			AvoidBuggyIPs: true,
			AllocateTo: &metallbv1beta1.ServiceAllocation{
				Priority:   5,
				Namespaces: []string{"clusters-test-provisioner"},
			},
			AutoAssign: ptr.To(false),
		},
	}
}

func driftedAdvert() *metallbv1beta1.L2Advertisement {
	return &metallbv1beta1.L2Advertisement{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "advertise-test-provisioner",
			Namespace: common.OpenshiftOperatorsNamespace,
			Labels:    ownerLabels(),
		},
		Spec: metallbv1beta1.L2AdvertisementSpec{
			IPAddressPools: []string{"wrong-pool"},
			Interfaces:     []string{"eth0"},
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

func dump(ctx context.Context, c client.Client, p *provisioningv1alpha1.DPFHCPProvisioner) map[string]string {
	out := map[string]string{}
	normalize(p)
	out["provisioner.memory"] = encode(p)

	stored := &provisioningv1alpha1.DPFHCPProvisioner{}
	if err := c.Get(ctx, types.NamespacedName{Namespace: "test-ns", Name: "test-provisioner"}, stored); err == nil {
		normalize(stored)
		out["provisioner.stored"] = encode(stored)
	} else {
		out["provisioner.stored"] = ""
	}
	pool := &metallbv1beta1.IPAddressPool{}
	if err := c.Get(ctx, types.NamespacedName{Namespace: common.OpenshiftOperatorsNamespace, Name: "test-provisioner"}, pool); err == nil {
		out["ipaddresspool"] = encode(pool)
	} else {
		out["ipaddresspool"] = ""
	}
	advert := &metallbv1beta1.L2Advertisement{}
	if err := c.Get(ctx, types.NamespacedName{Namespace: common.OpenshiftOperatorsNamespace, Name: "advertise-test-provisioner"}, advert); err == nil {
		out["l2advertisement"] = encode(advert)
	} else {
		out["l2advertisement"] = ""
	}
	return out
}

// noMatch makes every call for the given kind fail like a RESTMapper without the CRD.
func noMatch(kind string) interceptor.Funcs {
	check := func(obj runtime.Object) error {
		switch obj.(type) {
		case *metallbv1beta1.IPAddressPool, *metallbv1beta1.IPAddressPoolList:
			if kind == "IPAddressPool" {
				return &meta.NoKindMatchError{GroupKind: schema.GroupKind{Group: "metallb.io", Kind: "IPAddressPool"}, SearchedVersions: []string{"v1beta1"}}
			}
		case *metallbv1beta1.L2Advertisement, *metallbv1beta1.L2AdvertisementList:
			if kind == "L2Advertisement" {
				return &meta.NoKindMatchError{GroupKind: schema.GroupKind{Group: "metallb.io", Kind: "L2Advertisement"}, SearchedVersions: []string{"v1beta1"}}
			}
		}
		return nil
	}
	return interceptor.Funcs{
		Get: func(ctx context.Context, c client.WithWatch, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
			if err := check(obj); err != nil {
				return err
			}
			return c.Get(ctx, key, obj, opts...)
		},
		Create: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.CreateOption) error {
			if err := check(obj); err != nil {
				return err
			}
			return c.Create(ctx, obj, opts...)
		},
		Update: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.UpdateOption) error {
			if err := check(obj); err != nil {
				return err
			}
			return c.Update(ctx, obj, opts...)
		},
		Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
			if err := check(obj); err != nil {
				return err
			}
			return c.Delete(ctx, obj, opts...)
		},
	}
}

type configureCase struct {
	name     string
	p        *provisioningv1alpha1.DPFHCPProvisioner
	objs     []client.Object
	disabled bool
	noMatch  string
	runs     int
}

func runConfigure(tc configureCase) Fixture {
	ctx := context.Background()
	objs := append([]client.Object{tc.p}, tc.objs...)
	b := fake.NewClientBuilder().WithScheme(scheme()).WithObjects(objs...).WithStatusSubresource(tc.p)
	if tc.noMatch != "" {
		b = b.WithInterceptorFuncs(noMatch(tc.noMatch))
	}
	c := b.Build()
	r := record.NewFakeRecorder(100)
	m := metallb.NewMetalLBManager(c, r)
	cfg := &common.OperatorConfig{DisableMetalLB: tc.disabled}
	f := Fixture{}
	runs := tc.runs
	if runs == 0 {
		runs = 1
	}
	for i := 0; i < runs; i++ {
		res, err := m.ConfigureMetalLB(ctx, tc.p, cfg)
		f.Steps = append(f.Steps, step(res, err, r))
	}
	f.Objects = dump(ctx, c, tc.p)
	return f
}

type cleanupCase struct {
	name    string
	objs    []client.Object
	noMatch string
	runs    int
}

func runCleanup(tc cleanupCase) Fixture {
	ctx := context.Background()
	p := provisioner(hyperv1.HighlyAvailable, "192.168.1.100")
	objs := append([]client.Object{p}, tc.objs...)
	b := fake.NewClientBuilder().WithScheme(scheme()).WithObjects(objs...)
	if tc.noMatch != "" {
		b = b.WithInterceptorFuncs(noMatch(tc.noMatch))
	}
	c := b.Build()
	r := record.NewFakeRecorder(100)
	h := metallb.NewCleanupHandler(c, r)
	f := Fixture{}
	for i := 0; i < tc.runs; i++ {
		res, err := h.Cleanup(ctx, p)
		f.Steps = append(f.Steps, step(res, err, r))
	}
	f.Objects = dump(ctx, c, p)
	return f
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: metallb <fixtures-dir>")
		os.Exit(2)
	}
	dir := os.Args[1]
	must(os.MkdirAll(dir, 0o755))

	unownedPool := func() client.Object {
		p := driftedPool()
		p.Labels = map[string]string{"other": "owner"}
		return p
	}
	unownedAdvert := func() client.Object {
		a := driftedAdvert()
		a.Labels = nil
		return a
	}

	fixtures := map[string]Fixture{}
	for _, tc := range []configureCase{
		{name: "configure_create", p: provisioner(hyperv1.HighlyAvailable, "192.168.1.100"), runs: 2},
		{name: "configure_single_replica_vip", p: provisioner(hyperv1.SingleReplica, "10.1.2.3")},
		{name: "configure_skip", p: provisioner(hyperv1.SingleReplica, "")},
		{name: "configure_disabled", p: provisioner(hyperv1.HighlyAvailable, "192.168.1.100"), disabled: true, runs: 2},
		{name: "configure_drift", p: provisioner(hyperv1.HighlyAvailable, "192.168.1.100"), objs: []client.Object{driftedPool(), driftedAdvert()}},
		{name: "configure_pool_not_owned", p: provisioner(hyperv1.HighlyAvailable, "192.168.1.100"), objs: []client.Object{unownedPool()}},
		{name: "configure_advert_not_owned", p: provisioner(hyperv1.HighlyAvailable, "192.168.1.100"), objs: []client.Object{unownedAdvert()}},
		{name: "configure_pool_crd_missing", p: provisioner(hyperv1.HighlyAvailable, "192.168.1.100"), noMatch: "IPAddressPool"},
		{name: "configure_advert_crd_missing", p: provisioner(hyperv1.HighlyAvailable, "192.168.1.100"), noMatch: "L2Advertisement"},
	} {
		fixtures[tc.name] = runConfigure(tc)
	}
	for _, tc := range []cleanupCase{
		{name: "cleanup_both", objs: []client.Object{driftedPool(), driftedAdvert()}, runs: 3},
		{name: "cleanup_none", runs: 1},
		{name: "cleanup_not_owned", objs: []client.Object{unownedPool(), unownedAdvert()}, runs: 1},
		{name: "cleanup_crd_missing", noMatch: "IPAddressPool", objs: []client.Object{driftedAdvert()}, runs: 2},
	} {
		fixtures[tc.name] = runCleanup(tc)
	}

	for name, f := range fixtures {
		b, err := json.MarshalIndent(f, "", "  ")
		must(err)
		must(os.WriteFile(filepath.Join(dir, name+".json"), append(b, '\n'), 0o644))
	}
}
