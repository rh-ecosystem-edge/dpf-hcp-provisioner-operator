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

// Command ignitiongenerator generates the golden differential-test fixtures of
// crates/ctrl-ignitiongenerator: fixed scenarios are run through
// (*ignitiongenerator.IgnitionGenerator).GenerateIgnition against a controller-runtime fake
// client and an httptest TLS ignition server.
//
// Every fixture holds the scenario *input* (the CR, the seeded objects as JSON, the ignition
// server behaviour and the runs) and the Go *outputs* of each run, so the Rust test replays
// exactly the same input:
//
//   - result (RequeueAfter as Go duration string) and error string — a Go runtime panic is
//     recovered and rendered like controller-runtime does ("panic: <v> [recovered]");
//   - recorded events ("<type> <reason> <message>");
//   - the CR's status conditions after the run (lastTransitionTime is not recorded);
//   - create / update / status-update calls (verb, subresource, kind, namespace, name);
//   - the ignition ConfigMap after the run (labels, annotations, owner references, data — the
//     BF_CFG_TEMPLATE payload is compared byte-for-byte) and the embedded target ignition
//     (/var/target.ign, base64 + gunzip of the data URL) as text;
//   - the Authorization headers received by the ignition server.
//
// Placeholders (replaced identically by the Rust test):
//   - "ENDPOINT_PLACEHOLDER" → "127.0.0.1:<port>" of the test server and
//     "LOCALHOST_ENDPOINT_PLACEHOLDER" → "localhost:<port>" (in the seeded object JSON);
//     outputs map them back (normalization of the random port);
//   - Secret data base64("CA_PLACEHOLDER") → PEM of the test server's CA,
//     base64("OTHER_CA_PLACEHOLDER") → PEM of an unrelated, freshly generated CA.
//
// Run from the repository root: go run -mod=vendor ./hack/golden/ignitiongenerator
package main

import (
	"bytes"
	"compress/gzip"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"hash/fnv"
	"io"
	"math/big"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	igntypes "github.com/coreos/ignition/v2/config/v3_4/types"
	"github.com/go-logr/logr"
	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	k8stypes "k8s.io/apimachinery/pkg/types"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	"k8s.io/client-go/tools/record"
	"k8s.io/utils/ptr"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/apiutil"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/hostedcluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ignitiongenerator"
)

const fixtureDir = "crates/ctrl-ignitiongenerator/tests/fixtures"

const (
	endpointPlaceholder          = "ENDPOINT_PLACEHOLDER"
	localhostEndpointPlaceholder = "LOCALHOST_ENDPOINT_PLACEHOLDER"
	caPlaceholder                = "CA_PLACEHOLDER"
	otherCAPlaceholder           = "OTHER_CA_PLACEHOLDER"
)

var scheme = func() *runtime.Scheme {
	s := runtime.NewScheme()
	must(clientgoscheme.AddToScheme(s))
	must(provisioningv1alpha1.AddToScheme(s))
	must(hyperv1.AddToScheme(s))
	must(dpuservicev1alpha1.AddToScheme(s))
	must(dpuprovisioningv1alpha1.AddToScheme(s))
	must(operatorv1alpha1.AddToScheme(s))
	return s
}()

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

// ---------------------------------------------------------------------------------------------
// Fixture format
// ---------------------------------------------------------------------------------------------

type objEntry struct {
	Kind   string          `json:"kind"`
	Object json.RawMessage `json:"object"`
}

type serverSpec struct {
	// Payload served on /ignition.
	Payload string `json:"payload"`
	// AcceptTokens: when non-empty only "Bearer <t>" for these t is accepted (403 otherwise);
	// when empty any "Bearer " header is accepted (401 without one).
	AcceptTokens []string `json:"acceptTokens"`
	// Status, when non-zero, is returned for every request (with an empty body).
	Status int `json:"status"`
}

type runSpec struct {
	// OCPReleaseImage, when set, is written to cr.spec.ocpReleaseImage (and the CR updated in
	// the client) before the run.
	OCPReleaseImage *string `json:"ocpReleaseImage,omitempty"`
	// Payload, when set, replaces the served payload before the run.
	Payload *string `json:"payload,omitempty"`
}

type input struct {
	CR         json.RawMessage `json:"cr"`
	CRInClient bool            `json:"crInClient"`
	Objects    []objEntry      `json:"objects"`
	Server     serverSpec      `json:"server"`
	Runs       []runSpec       `json:"runs"`
}

type condition struct {
	Type               string `json:"type"`
	Status             string `json:"status"`
	Reason             string `json:"reason"`
	Message            string `json:"message"`
	ObservedGeneration int64  `json:"observedGeneration"`
}

type action struct {
	Verb        string `json:"verb"`
	Subresource string `json:"subresource"`
	Kind        string `json:"kind"`
	Namespace   string `json:"namespace"`
	Name        string `json:"name"`
}

type cmDump struct {
	Name            string                  `json:"name"`
	Namespace       string                  `json:"namespace"`
	ResourceVersion string                  `json:"resourceVersion"`
	Labels          map[string]string       `json:"labels"`
	Annotations     map[string]string       `json:"annotations"`
	OwnerReferences []metav1.OwnerReference `json:"ownerReferences"`
	Data            map[string]string       `json:"data"`
}

type output struct {
	RequeueAfter   string      `json:"requeueAfter"`
	Error          string      `json:"error"`
	Events         []string    `json:"events"`
	Conditions     []condition `json:"conditions"`
	Actions        []action    `json:"actions"`
	ConfigMap      *cmDump     `json:"configMap"`
	TargetIgnition string      `json:"targetIgnition"`
	Requests       []string    `json:"requests"`
}

type scenario struct {
	Name    string   `json:"name"`
	Input   input    `json:"input"`
	Outputs []output `json:"outputs"`
}

// ---------------------------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------------------------

type ignitionServer struct {
	srv      *httptest.Server
	mu       sync.Mutex
	spec     serverSpec
	requests []string
}

func newIgnitionServer(spec serverSpec) *ignitionServer {
	s := &ignitionServer{spec: spec}
	s.srv = httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		s.mu.Lock()
		defer s.mu.Unlock()
		auth := r.Header.Get("Authorization")
		s.requests = append(s.requests, auth)
		if s.spec.Status != 0 {
			w.WriteHeader(s.spec.Status)
			return
		}
		if r.URL.Path != "/ignition" {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		if !strings.HasPrefix(auth, "Bearer ") {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		if len(s.spec.AcceptTokens) > 0 {
			ok := false
			for _, t := range s.spec.AcceptTokens {
				if auth == "Bearer "+t {
					ok = true
				}
			}
			if !ok {
				w.WriteHeader(http.StatusForbidden)
				return
			}
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(s.spec.Payload))
	}))
	return s
}

func (s *ignitionServer) caPEM() []byte {
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: s.srv.TLS.Certificates[0].Certificate[0]})
}

func (s *ignitionServer) host() string {
	u, err := url.Parse(s.srv.URL)
	must(err)
	return u.Host
}

func otherCAPEM() []byte {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	must(err)
	tmpl := &x509.Certificate{
		SerialNumber:          big.NewInt(42),
		Subject:               pkix.Name{CommonName: "other-ca"},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(24 * time.Hour),
		IsCA:                  true,
		BasicConstraintsValid: true,
		KeyUsage:              x509.KeyUsageCertSign | x509.KeyUsageDigitalSignature,
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	must(err)
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
}

func b64(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }

func newObject(kind string) client.Object {
	switch kind {
	case "Secret":
		return &corev1.Secret{}
	case "ConfigMap":
		return &corev1.ConfigMap{}
	case "Service":
		return &corev1.Service{}
	case "Node":
		return &corev1.Node{}
	case "HostedCluster":
		return &hyperv1.HostedCluster{}
	case "NodePool":
		return &hyperv1.NodePool{}
	case "DPUDeployment":
		return &dpuservicev1alpha1.DPUDeployment{}
	case "DPUFlavor":
		return &dpuprovisioningv1alpha1.DPUFlavor{}
	case "DPFOperatorConfig":
		return &operatorv1alpha1.DPFOperatorConfig{}
	}
	panic("unknown kind " + kind)
}

func run(name string, in input) scenario {
	srv := newIgnitionServer(in.Server)
	defer srv.srv.Close()
	host := srv.host()
	port := host[strings.LastIndex(host, ":")+1:]
	localhost := "localhost:" + port
	replacer := strings.NewReplacer(
		localhostEndpointPlaceholder, localhost,
		endpointPlaceholder, host,
		b64(caPlaceholder), base64.StdEncoding.EncodeToString(srv.caPEM()),
		b64(otherCAPlaceholder), base64.StdEncoding.EncodeToString(otherCAPEM()),
	)
	normalize := func(s string) string {
		return strings.NewReplacer(localhost, localhostEndpointPlaceholder, host, endpointPlaceholder).Replace(s)
	}

	cr := &provisioningv1alpha1.DPFHCPProvisioner{}
	must(json.Unmarshal(in.CR, cr))

	var objs []client.Object
	if in.CRInClient {
		objs = append(objs, cr)
	}
	for _, o := range in.Objects {
		obj := newObject(o.Kind)
		must(json.Unmarshal([]byte(replacer.Replace(string(o.Object))), obj))
		objs = append(objs, obj)
	}

	var mu sync.Mutex
	var actions []action
	rec := func(verb, sub string, obj client.Object) {
		gvk, err := apiutil.GVKForObject(obj, scheme)
		must(err)
		mu.Lock()
		defer mu.Unlock()
		actions = append(actions, action{Verb: verb, Subresource: sub, Kind: gvk.Kind, Namespace: obj.GetNamespace(), Name: obj.GetName()})
	}
	c := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(objs...).
		WithStatusSubresource(&provisioningv1alpha1.DPFHCPProvisioner{}).
		WithInterceptorFuncs(interceptor.Funcs{
			Create: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.CreateOption) error {
				rec("create", "", obj)
				return c.Create(ctx, obj, opts...)
			},
			Update: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.UpdateOption) error {
				rec("update", "", obj)
				return c.Update(ctx, obj, opts...)
			},
			SubResourceUpdate: func(ctx context.Context, c client.Client, sub string, obj client.Object, opts ...client.SubResourceUpdateOption) error {
				rec("update", sub, obj)
				return c.SubResource(sub).Update(ctx, obj, opts...)
			},
		}).
		Build()

	recorder := record.NewFakeRecorder(100)
	ig := ignitiongenerator.NewIgnitionGenerator(c, scheme, recorder)
	ctx := logr.NewContext(context.Background(), logr.Discard())

	s := scenario{Name: name, Input: in}
	for _, r := range in.Runs {
		if r.OCPReleaseImage != nil {
			cr.Spec.OCPReleaseImage = *r.OCPReleaseImage
			if in.CRInClient {
				must(c.Update(ctx, cr))
			}
		}
		if r.Payload != nil {
			srv.mu.Lock()
			srv.spec.Payload = *r.Payload
			srv.mu.Unlock()
		}
		mu.Lock()
		actions = nil
		mu.Unlock()
		srv.mu.Lock()
		srv.requests = nil
		srv.mu.Unlock()

		res, err := func() (res ctrl.Result, err error) {
			defer func() {
				if p := recover(); p != nil {
					err = fmt.Errorf("panic: %v [recovered]", p)
				}
			}()
			return ig.GenerateIgnition(ctx, cr)
		}()

		out := output{RequeueAfter: res.RequeueAfter.String(), Events: []string{}, Conditions: []condition{}, Actions: []action{}, Requests: []string{}}
		if err != nil {
			out.Error = normalize(err.Error())
		}
		for done := false; !done; {
			select {
			case ev := <-recorder.Events:
				out.Events = append(out.Events, normalize(ev))
			default:
				done = true
			}
		}
		for _, cond := range cr.Status.Conditions {
			out.Conditions = append(out.Conditions, condition{
				Type: cond.Type, Status: string(cond.Status), Reason: cond.Reason,
				Message: normalize(cond.Message), ObservedGeneration: cond.ObservedGeneration,
			})
		}
		mu.Lock()
		out.Actions = append(out.Actions, actions...)
		mu.Unlock()
		srv.mu.Lock()
		out.Requests = append(out.Requests, srv.requests...)
		srv.mu.Unlock()

		cm := &corev1.ConfigMap{}
		key := k8stypes.NamespacedName{Namespace: cr.Spec.DPUClusterRef.Namespace, Name: ignitiongenerator.ConfigMapName(cr.Spec.DPUClusterRef.Name)}
		if err := c.Get(ctx, key, cm); err == nil {
			out.ConfigMap = &cmDump{
				Name: cm.Name, Namespace: cm.Namespace, ResourceVersion: cm.ResourceVersion,
				Labels: cm.Labels, Annotations: cm.Annotations, OwnerReferences: cm.OwnerReferences, Data: cm.Data,
			}
			out.TargetIgnition = targetIgnition(cm.Data["BF_CFG_TEMPLATE"])
		}
		s.Outputs = append(s.Outputs, out)
	}
	return s
}

// targetIgnition extracts /var/target.ign of a live ignition payload (base64 + gunzip).
func targetIgnition(data string) string {
	live := &igntypes.Config{}
	if err := json.Unmarshal([]byte(strings.TrimSpace(data)), live); err != nil {
		return ""
	}
	for _, f := range live.Storage.Files {
		if f.Path != "/var/target.ign" || f.Contents.Source == nil {
			continue
		}
		raw, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(*f.Contents.Source, "data:;base64,"))
		must(err)
		zr, err := gzip.NewReader(bytes.NewReader(raw))
		must(err)
		out, err := io.ReadAll(zr)
		must(err)
		return string(out)
	}
	return ""
}

func write(s scenario) {
	b, err := json.MarshalIndent(s, "", "  ")
	must(err)
	must(os.WriteFile(filepath.Join(fixtureDir, s.Name+".json"), append(b, '\n'), 0o644))
}

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

const (
	crName       = "doca"
	crNamespace  = "clusters"
	dpuClusterNS = "dpf-operator-system"
	cpNamespace  = "clusters-doca"
	sshKey       = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQ golden@example.com"
	legacyToken  = "test-bearer-token"
	currentToken = "current-token-data"
	pullSecret   = `{"auths":{"quay.io":{"auth":"Zm9vOmJhcg=="}}}`
)

// hashSimple is HyperShift's support/util.HashSimple for a []byte.
func hashSimple(data []byte) string {
	h := fnv.New32a()
	_, _ = fmt.Fprintf(h, "%v", data)
	return fmt.Sprintf("%08x", h.Sum32())
}

func mcSource(osImageURL string) string {
	mc, err := json.Marshal(map[string]any{"spec": map[string]any{"osImageURL": osImageURL, "config": map[string]any{"ignition": map[string]any{"version": "3.2.0"}}}})
	must(err)
	return "data:," + url.QueryEscape(string(mc))
}

type payloadOpts struct {
	version     string
	osImageURL  string
	sshKeys     []string
	noMC        bool
	duplicate   bool
	extraUnit   bool
	withGroups  bool
	withNoUsers bool
}

func payload(o payloadOpts) string {
	if o.version == "" {
		o.version = "3.4.0"
	}
	if o.osImageURL == "" {
		o.osImageURL = "https://registry.example.com/rhcos:4.17"
	}
	if o.sshKeys == nil {
		o.sshKeys = []string{sshKey}
	}
	ign := &igntypes.Config{Ignition: igntypes.Ignition{Version: o.version}}
	if !o.noMC {
		src := mcSource(o.osImageURL)
		ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
			Node:          igntypes.Node{Path: "/etc/ignition-machine-config-encapsulated.json", Overwrite: ptr.To(true)},
			FileEmbedded1: igntypes.FileEmbedded1{Mode: ptr.To(0o644), Contents: igntypes.Resource{Source: &src}},
		})
	}
	kubeletConf := "data:,kind%3A%20KubeletConfiguration%0A"
	ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
		Node:          igntypes.Node{Path: "/etc/kubernetes/kubelet.conf", Overwrite: ptr.To(true)},
		FileEmbedded1: igntypes.FileEmbedded1{Mode: ptr.To(0o644), Contents: igntypes.Resource{Source: &kubeletConf}},
	})
	gz := "data:;base64,H4sIAAAAAAAC/0rLz+cCAAAA//8BAAD//7l5dXUEAAAA"
	ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
		Node:          igntypes.Node{Path: "/etc/precompressed", Overwrite: ptr.To(true)},
		FileEmbedded1: igntypes.FileEmbedded1{Mode: ptr.To(0o600), Contents: igntypes.Resource{Source: &gz, Compression: ptr.To("gzip")}},
	})
	if o.duplicate {
		dup := "data:,content"
		for i := 0; i < 2; i++ {
			ign.Storage.Files = append(ign.Storage.Files, igntypes.File{
				Node:          igntypes.Node{Path: "/etc/duplicate-file", Overwrite: ptr.To(true)},
				FileEmbedded1: igntypes.FileEmbedded1{Mode: ptr.To(0o644), Contents: igntypes.Resource{Source: &dup}},
			})
		}
	}
	ign.Storage.Directories = append(ign.Storage.Directories, igntypes.Directory{
		Node: igntypes.Node{Path: "/var/lib/kubelet"}, DirectoryEmbedded1: igntypes.DirectoryEmbedded1{Mode: ptr.To(0o755)},
	})
	if !o.withNoUsers {
		var keys []igntypes.SSHAuthorizedKey
		for _, k := range o.sshKeys {
			keys = append(keys, igntypes.SSHAuthorizedKey(k))
		}
		ign.Passwd.Users = []igntypes.PasswdUser{{Name: "core", SSHAuthorizedKeys: keys}}
	}
	if o.withGroups {
		ign.Passwd.Groups = []igntypes.PasswdGroup{{Name: "wheel-ops", Gid: ptr.To(1500)}}
	}
	ign.Systemd.Units = []igntypes.Unit{{
		Name:     "kubelet.service",
		Enabled:  ptr.To(true),
		Contents: ptr.To("[Unit]\nDescription=Kubelet\n[Service]\nExecStart=/usr/bin/kubelet\n[Install]\nWantedBy=multi-user.target"),
	}}
	if o.extraUnit {
		ign.Systemd.Units = append(ign.Systemd.Units, igntypes.Unit{
			Name:     "machine-config-daemon-firstboot.service",
			Enabled:  ptr.To(true),
			Contents: ptr.To("[Unit]\nDescription=MCD firstboot\n[Service]\nType=oneshot\nExecStart=/usr/bin/true\n[Install]\nWantedBy=multi-user.target\n"),
		})
	}
	ign.KernelArguments.ShouldExist = []igntypes.KernelArgument{"systemd.unified_cgroup_hierarchy=1"}
	return string(mustJSON(ign))
}

func baseCR() *provisioningv1alpha1.DPFHCPProvisioner {
	return &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNamespace, Generation: 3, UID: "cr-uid-1234"},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			DPUClusterRef:    provisioningv1alpha1.DPUClusterReference{Name: "dpucluster-1", Namespace: dpuClusterNS},
			DPUDeploymentRef: &provisioningv1alpha1.DPUDeploymentReference{Name: "my-deployment", Namespace: dpuClusterNS},
			OCPReleaseImage:  "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
			MachineOSURL:     "https://registry.example.com/rhcos-bf:4.17",
		},
		Status: provisioningv1alpha1.DPFHCPProvisionerStatus{
			Phase:            provisioningv1alpha1.PhaseGeneratingIgnition,
			HostedClusterRef: &corev1.ObjectReference{Name: crName, Namespace: crNamespace},
			Conditions: []metav1.Condition{{
				Type: provisioningv1alpha1.HostedClusterAvailable, Status: metav1.ConditionTrue, Reason: "AsExpected",
				Message: "The hosted control plane is available", ObservedGeneration: 3,
				LastTransitionTime: metav1.NewTime(time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC)),
			}},
		},
	}
}

func strictCR() *provisioningv1alpha1.DPFHCPProvisioner {
	cr := baseCR()
	cr.Spec.PullSecretRef = corev1.LocalObjectReference{Name: "my-pull-secret"}
	cr.Spec.SSHKeySecretRef = corev1.LocalObjectReference{Name: "my-ssh-key"}
	return cr
}

func secret(name, ns string, data map[string]string) *corev1.Secret {
	s := &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: ns}, Data: map[string][]byte{}}
	for k, v := range data {
		s.Data[k] = []byte(v)
	}
	return s
}

func caSecret() *corev1.Secret {
	return secret("ignition-server-ca-cert", cpNamespace, map[string]string{"tls.crt": caPlaceholder})
}

func legacyTokenSecret() *corev1.Secret {
	return secret("token-doca-abc123", cpNamespace, map[string]string{"token": legacyToken})
}

func hostedClusterObj(endpoint string) *hyperv1.HostedCluster {
	return &hyperv1.HostedCluster{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNamespace},
		Status:     hyperv1.HostedClusterStatus{IgnitionEndpoint: endpoint},
	}
}

func strictHostedCluster(cr *provisioningv1alpha1.DPFHCPProvisioner) *hyperv1.HostedCluster {
	hc := hostedClusterObj(endpointPlaceholder)
	hc.Spec.PullSecret = corev1.LocalObjectReference{Name: hostedcluster.PullSecretCopyName(cr)}
	hc.Spec.SSHKey = corev1.LocalObjectReference{Name: hostedcluster.SSHKeyCopyName(cr)}
	return hc
}

func deployment(bfb, flavor *string) *dpuservicev1alpha1.DPUDeployment {
	return &dpuservicev1alpha1.DPUDeployment{
		ObjectMeta: metav1.ObjectMeta{Name: "my-deployment", Namespace: dpuClusterNS},
		Spec:       dpuservicev1alpha1.DPUDeploymentSpec{DPUs: dpuservicev1alpha1.DPUs{BFB: bfb, Flavor: flavor}},
	}
}

func richFlavor() *dpuprovisioningv1alpha1.DPUFlavor {
	return &dpuprovisioningv1alpha1.DPUFlavor{
		ObjectMeta: metav1.ObjectMeta{Name: "test-flavor", Namespace: dpuClusterNS, Labels: map[string]string{"app": "golden"}, ResourceVersion: "5"},
		Spec: dpuprovisioningv1alpha1.DPUFlavorSpec{
			Grub:   dpuprovisioningv1alpha1.DPUFlavorGrub{KernelParameters: []string{"console=hvc0", "console=ttyAMA0", "hugepagesz=2048kB", "hugepages=3072"}},
			Sysctl: dpuprovisioningv1alpha1.DPUFLavorSysctl{Parameters: []string{"net.ipv4.ip_forward=1", "net.ipv4.ip_nonlocal_bind=1"}},
			NVConfig: []dpuprovisioningv1alpha1.NVConfig{{
				Device:     ptr.To("*"),
				Parameters: []string{"PF_BAR2_ENABLE=0", "PER_PF_NUM_SF=1", "LINK_TYPE_P1=2"},
			}},
			OVS:             dpuprovisioningv1alpha1.DPUFlavorOVS{RawConfigScript: "#!/bin/bash\n_ovs-vsctl() {\n  ovs-vsctl --no-wait --timeout 15 \"$@\"\n}\n_ovs-vsctl set Open_vSwitch . other_config:doca-init=true\n_ovs-vsctl add-br br-sfc\n"},
			BFCfgParameters: []string{"UPDATE_ATF_UEFI=yes", "UPDATE_DPU_OS=yes"},
			ConfigFiles: []dpuprovisioningv1alpha1.ConfigFile{
				{Path: "/etc/sysctl.d/98-dpunet.conf", Operation: dpuprovisioningv1alpha1.FileAppend, Raw: ptr.To("net.core.rmem_max=16777216\n"), Permissions: "0644"},
				{Path: "/etc/mellanox/mlnx-bf.conf", Operation: dpuprovisioningv1alpha1.FileOverride, Raw: ptr.To("ALLOW_SHARED_RQ=\"no\"\nIPSEC_FULL_OFFLOAD=\"no\"\nENABLE_ESWITCH_MULTIPORT=\"yes\"\n"), Permissions: "0600"},
				{Path: "/etc/dpf/custom.conf", Raw: ptr.To("key=value\n")},
				{Path: "", Raw: ptr.To("ignored")},
			},
			ContainerdConfig: dpuprovisioningv1alpha1.ContainerdConfig{RegistryEndpoint: "https://registry.example.com"},
			DPUResources:     corev1.ResourceList{corev1.ResourceCPU: resource.MustParse("16"), corev1.ResourceMemory: resource.MustParse("16Gi")},
			SystemReservedResources: corev1.ResourceList{
				corev1.ResourceCPU: resource.MustParse("500m"), corev1.ResourceMemory: resource.MustParse("1Gi"),
			},
		},
	}
}

func operatorConfig(mode operatorv1alpha1.DeploymentMode, mtu *int) *operatorv1alpha1.DPFOperatorConfig {
	cfg := &operatorv1alpha1.DPFOperatorConfig{
		ObjectMeta: metav1.ObjectMeta{Name: "dpfoperatorconfig", Namespace: dpuClusterNS},
		Spec: operatorv1alpha1.DPFOperatorConfigSpec{
			DeploymentMode: mode,
			ProvisioningController: &operatorv1alpha1.ProvisioningControllerConfiguration{
				InstallInterface: &operatorv1alpha1.ProvisioningInstallInterface{InstallViaHostAgent: &operatorv1alpha1.InstallViaHostAgent{}},
			},
		},
	}
	if mtu != nil {
		cfg.Spec.Networking = &operatorv1alpha1.Networking{ControlPlaneMTU: mtu}
	}
	return cfg
}

func entries(objs ...client.Object) []objEntry {
	var out []objEntry
	for _, o := range objs {
		gvk, err := apiutil.GVKForObject(o, scheme)
		must(err)
		out = append(out, objEntry{Kind: gvk.Kind, Object: mustJSON(o)})
	}
	return out
}

// legacyObjects: everything for a successful legacy (no credential refs) host-trusted run.
func legacyObjects() map[string]client.Object {
	return map[string]client.Object{
		"ca":         caSecret(),
		"token":      legacyTokenSecret(),
		"hc":         hostedClusterObj(endpointPlaceholder),
		"deployment": deployment(ptr.To("test-bfb"), ptr.To("test-flavor")),
		"flavor":     richFlavor(),
		"operator":   operatorConfig(operatorv1alpha1.DeploymentModeHostTrusted, ptr.To(9000)),
	}
}

var objectOrder = []string{"ca", "token", "token2", "token3", "token4", "hc", "pullSecret", "nodePool", "currentToken", "sshKey", "deployment", "flavor", "operator", "operator2", "service", "node1", "node2", "cm"}

func ordered(m map[string]client.Object) []client.Object {
	var out []client.Object
	for _, k := range objectOrder {
		if o, ok := m[k]; ok && o != nil {
			out = append(out, o)
		}
	}
	if len(out) != len(m) {
		panic("unknown object key")
	}
	return out
}

// strictObjects: everything for a successful strict credential sync run.
func strictObjects(cr *provisioningv1alpha1.DPFHCPProvisioner) map[string]client.Object {
	m := legacyObjects()
	delete(m, "token")
	m["hc"] = strictHostedCluster(cr)
	m["pullSecret"] = secret(hostedcluster.PullSecretCopyName(cr), crNamespace, map[string]string{".dockerconfigjson": pullSecret})
	m["nodePool"] = &hyperv1.NodePool{ObjectMeta: metav1.ObjectMeta{
		Name: crName, Namespace: crNamespace,
		Annotations: map[string]string{"hypershift.openshift.io/nodePoolCurrentConfigVersion": "5f3a9c1e"},
	}}
	m["currentToken"] = secret("token-doca-5f3a9c1e", cpNamespace, map[string]string{
		"token": currentToken, "pull-secret-hash": hashSimple([]byte(pullSecret)),
	})
	// a stale token secret that must be ignored
	m["token2"] = secret("token-doca-0000aaaa", cpNamespace, map[string]string{"token": "stale-token", "pull-secret-hash": "deadbeef"})
	m["sshKey"] = secret(hostedcluster.SSHKeyCopyName(cr), crNamespace, map[string]string{"id_rsa.pub": "  " + sshKey + "\n"})
	return m
}

func defaultServer() serverSpec {
	return serverSpec{Payload: payload(payloadOpts{})}
}

func strictServer() serverSpec {
	return serverSpec{Payload: payload(payloadOpts{}), AcceptTokens: []string{b64(currentToken)}}
}

func oneRun() []runSpec { return []runSpec{{}} }

type builder struct {
	name   string
	cr     *provisioningv1alpha1.DPFHCPProvisioner
	notIn  bool
	objs   map[string]client.Object
	server serverSpec
	runs   []runSpec
}

func (b builder) gen() {
	if b.runs == nil {
		b.runs = oneRun()
	}
	in := input{CR: mustJSON(b.cr), CRInClient: !b.notIn, Objects: entries(ordered(b.objs)...), Server: b.server, Runs: b.runs}
	write(run(b.name, in))
}

func legacy(name string, mut func(cr *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, s *serverSpec)) {
	cr := baseCR()
	m := legacyObjects()
	s := defaultServer()
	if mut != nil {
		mut(cr, m, &s)
	}
	builder{name: name, cr: cr, objs: m, server: s}.gen()
}

func strict(name string, mut func(cr *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, s *serverSpec)) {
	cr := strictCR()
	m := strictObjects(cr)
	s := strictServer()
	if mut != nil {
		mut(cr, m, &s)
	}
	builder{name: name, cr: cr, objs: m, server: s}.gen()
}

type mutFn = func(cr *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, s *serverSpec)

func flavorOf(m map[string]client.Object) *dpuprovisioningv1alpha1.DPUFlavor {
	return m["flavor"].(*dpuprovisioningv1alpha1.DPUFlavor)
}

func operatorOf(m map[string]client.Object) *operatorv1alpha1.DPFOperatorConfig {
	return m["operator"].(*operatorv1alpha1.DPFOperatorConfig)
}

func registryNodePortObjects(m map[string]client.Object) {
	m["service"] = &corev1.Service{
		ObjectMeta: metav1.ObjectMeta{Name: "bfb-registry", Namespace: dpuClusterNS},
		Spec: corev1.ServiceSpec{Type: corev1.ServiceTypeNodePort, Ports: []corev1.ServicePort{
			{Name: "metrics", Port: 9090},
			{Name: "http", Port: 80, NodePort: 30080},
			{Name: "https", Port: 443, NodePort: 30443},
		}},
	}
	m["node1"] = &corev1.Node{
		ObjectMeta: metav1.ObjectMeta{Name: "node-a"},
		Status: corev1.NodeStatus{Addresses: []corev1.NodeAddress{
			{Type: corev1.NodeHostName, Address: "node-a"}, {Type: corev1.NodeExternalIP, Address: "203.0.113.10"},
		}},
	}
	m["node2"] = &corev1.Node{
		ObjectMeta: metav1.ObjectMeta{Name: "node-b"},
		Status: corev1.NodeStatus{Addresses: []corev1.NodeAddress{
			{Type: corev1.NodeInternalIP, Address: "10.1.2.3"}, {Type: corev1.NodeInternalIP, Address: "10.1.2.4"},
		}},
	}
}

func redfishZeroTrust(m map[string]client.Object) {
	op := operatorOf(m)
	op.Spec.DeploymentMode = ""
	op.Spec.ProvisioningController.InstallInterface = &operatorv1alpha1.ProvisioningInstallInterface{InstallViaRedfish: &operatorv1alpha1.InstallViaRedfish{}}
}

func main() {
	must(os.MkdirAll(fixtureDir, 0o755))

	// ------------------------------------------------------------------ success paths (legacy)
	legacy("legacy_success_mtu_9000", nil)
	legacy("legacy_success_mtu_1500", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		operatorOf(m).Spec.Networking.ControlPlaneMTU = ptr.To(1500)
	})
	legacy("legacy_success_minimal_flavor_no_bfb", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, s *serverSpec) {
		m["deployment"] = deployment(nil, ptr.To("test-flavor"))
		m["flavor"] = &dpuprovisioningv1alpha1.DPUFlavor{ObjectMeta: metav1.ObjectMeta{Name: "test-flavor", Namespace: dpuClusterNS}}
		s.Payload = payload(payloadOpts{withGroups: true, withNoUsers: true})
	})
	legacy("legacy_success_hcp_version_3_2_0", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = payload(payloadOpts{version: "3.2.0"})
	})
	legacy("legacy_success_mtu_truncated_uint16", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		operatorOf(m).Spec.Networking.ControlPlaneMTU = ptr.To(65536 + 1500)
	})
	legacy("machine_os_url_from_status_fallback", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Spec.MachineOSURL = ""
		cr.Status.BlueFieldOCPLayerImage = "quay.io/example/bluefield-ocp-layer:4.19.0"
	})
	legacy("machine_os_url_missing", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Spec.MachineOSURL = ""
	})
	legacy("same_namespace_owner_reference", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Namespace = dpuClusterNS
	})
	legacy("existing_configmap_updated", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["cm"] = &corev1.ConfigMap{
			ObjectMeta: metav1.ObjectMeta{
				Name: "bfcfg-dpucluster-1.cfg", Namespace: dpuClusterNS,
				Labels:      map[string]string{"stale": "label", ignitiongenerator.BfcfgTemplateLabel: "false"},
				Annotations: map[string]string{"stale": "annotation"},
				OwnerReferences: []metav1.OwnerReference{{
					APIVersion: "v1", Kind: "ConfigMap", Name: "other-owner", UID: "other-uid",
				}},
			},
			Data:       map[string]string{"BF_CFG_TEMPLATE": "old-data", "extra": "kept?"},
			BinaryData: map[string][]byte{"bin": []byte{1, 2, 3}},
		}
	})

	// ------------------------------------------------------------------ zero-trust
	legacy("zero_trust_flavor_mode_load_balancer_address", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		flavorOf(m).Spec.DpuMode = dpuprovisioningv1alpha1.ZeroTrustMode
		operatorOf(m).Spec.ProvisioningController.Registry = &operatorv1alpha1.RegistryConfiguration{
			LoadBalancerAddress: ptr.To("http://10.0.0.5:8080/"),
			Address:             ptr.To("http://ignored:5000"),
		}
	})
	legacy("zero_trust_deployment_mode_address", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		op := operatorOf(m)
		op.Spec.DeploymentMode = operatorv1alpha1.DeploymentModeZeroTrust
		op.Spec.Networking.ControlPlaneMTU = ptr.To(1500)
		op.Spec.ProvisioningController.Registry = &operatorv1alpha1.RegistryConfiguration{
			LoadBalancerAddress: ptr.To(""),
			Address:             ptr.To("http://registry.dpf.svc:5000//"),
		}
	})
	legacy("zero_trust_redfish_node_port_discovery", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		redfishZeroTrust(m)
		registryNodePortObjects(m)
	})
	legacy("zero_trust_service_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		redfishZeroTrust(m)
	})
	legacy("zero_trust_service_without_node_port", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		redfishZeroTrust(m)
		registryNodePortObjects(m)
		svc := m["service"].(*corev1.Service)
		svc.Spec.Type = corev1.ServiceTypeClusterIP
		for i := range svc.Spec.Ports {
			svc.Spec.Ports[i].NodePort = 0
		}
	})
	legacy("zero_trust_no_internal_ip", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		redfishZeroTrust(m)
		registryNodePortObjects(m)
		delete(m, "node2")
	})
	legacy("zero_trust_nil_provisioning_controller_panics", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		op := operatorOf(m)
		op.Spec.DeploymentMode = operatorv1alpha1.DeploymentModeZeroTrust
		op.Spec.ProvisioningController = nil
	})

	// ------------------------------------------------------------------ DPU mode errors
	legacy("mode_nic_flavor", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		flavorOf(m).Spec.DpuMode = dpuprovisioningv1alpha1.NicMode
	})
	legacy("mode_invalid_flavor", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		flavorOf(m).Spec.DpuMode = "bogus"
	})
	legacy("mode_unsupported_deployment_mode", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		operatorOf(m).Spec.DeploymentMode = "half-trusted"
	})
	legacy("mode_empty", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		operatorOf(m).Spec.DeploymentMode = ""
	})
	legacy("mode_flavor_dpu_overrides_zero_trust_deployment", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		flavorOf(m).Spec.DpuMode = dpuprovisioningv1alpha1.DpuMode
		operatorOf(m).Spec.DeploymentMode = operatorv1alpha1.DeploymentModeZeroTrust
	})

	// ------------------------------------------------------------------ dependency errors
	legacy("operator_config_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "operator")
	})
	legacy("operator_config_two", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		op2 := operatorConfig(operatorv1alpha1.DeploymentModeHostTrusted, ptr.To(1500))
		op2.Name = "second"
		m["operator2"] = op2
	})
	legacy("operator_config_networking_nil_panics", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		operatorOf(m).Spec.Networking = nil
	})
	legacy("operator_config_mtu_nil_panics", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		operatorOf(m).Spec.Networking.ControlPlaneMTU = nil
	})
	legacy("dpu_deployment_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "deployment")
	})
	legacy("dpu_deployment_ref_nil_panics", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Spec.DPUDeploymentRef = nil
	})
	legacy("dpu_deployment_flavor_empty", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["deployment"] = deployment(ptr.To("test-bfb"), ptr.To(""))
	})
	legacy("dpu_deployment_flavor_nil", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["deployment"] = deployment(ptr.To("test-bfb"), nil)
	})
	legacy("dpu_flavor_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "flavor")
	})
	legacy("flavor_invalid_permissions", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		flavorOf(m).Spec.ConfigFiles[0].Permissions = "0x644"
	})
	legacy("flavor_config_file_without_raw", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		flavorOf(m).Spec.ConfigFiles[1].Raw = nil
	})
	legacy("hosted_cluster_ref_nil_panics", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Status.HostedClusterRef = nil
	})

	// ------------------------------------------------------------------ HCP payload errors
	legacy("hcp_duplicate_files_validation_failed", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = payload(payloadOpts{duplicate: true})
	})
	legacy("hcp_invalid_json", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = `{"ignition":{"version":"3.4.0"},"storage":{"files":[{"path":1}]}}`
	})
	legacy("hcp_not_json", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = "<html>not ignition</html>"
	})
	legacy("hcp_without_machine_config", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = payload(payloadOpts{noMC: true})
	})
	legacy("hcp_unsupported_version", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = payload(payloadOpts{version: "2.2.0"})
	})

	// ------------------------------------------------------------------ download errors (legacy)
	legacy("ca_secret_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "ca")
	})
	legacy("ca_secret_without_tls_crt", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["ca"] = secret("ignition-server-ca-cert", cpNamespace, map[string]string{"ca.crt": "x"})
	})
	legacy("ca_invalid_pem", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["ca"] = secret("ignition-server-ca-cert", cpNamespace, map[string]string{"tls.crt": "not a certificate"})
	})
	legacy("ca_unknown_authority", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["ca"] = secret("ignition-server-ca-cert", cpNamespace, map[string]string{"tls.crt": otherCAPlaceholder})
	})
	legacy("tls_hostname_mismatch", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["hc"] = hostedClusterObj(localhostEndpointPlaceholder)
	})
	legacy("connection_refused", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["hc"] = hostedClusterObj("127.0.0.1:1")
	})
	legacy("hosted_cluster_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "hc")
	})
	legacy("hosted_cluster_without_endpoint", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["hc"] = hostedClusterObj("")
	})
	legacy("legacy_no_token_secrets", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["token"] = secret("token-docaabc", cpNamespace, map[string]string{"token": "x"})
	})
	legacy("legacy_token_secrets_without_data", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["token"] = secret("token-doca-abc123", cpNamespace, map[string]string{"other": "x"})
	})
	legacy("legacy_newest_token_first", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, s *serverSpec) {
		old := secret("token-doca-aaa", cpNamespace, map[string]string{"token": "old-token"})
		old.CreationTimestamp = metav1.NewTime(time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC))
		newer := secret("token-doca-zzz", cpNamespace, map[string]string{"token": "new-token"})
		newer.CreationTimestamp = metav1.NewTime(time.Date(2025, 6, 1, 0, 0, 0, 0, time.UTC))
		middle := secret("token-doca-mmm", cpNamespace, map[string]string{"token": "middle-token"})
		middle.CreationTimestamp = metav1.NewTime(time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))
		noData := secret("token-doca-nodata", cpNamespace, map[string]string{"x": "y"})
		noData.CreationTimestamp = metav1.NewTime(time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC))
		m["token"], m["token2"], m["token3"], m["token4"] = old, newer, middle, noData
		s.AcceptTokens = []string{b64("old-token")}
	})
	legacy("legacy_equal_timestamps_keep_list_order", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, s *serverSpec) {
		m["token2"] = secret("token-doca-b", cpNamespace, map[string]string{"token": "b-token"})
		m["token3"] = secret("token-doca-a", cpNamespace, map[string]string{"token": "a-token"})
		s.AcceptTokens = []string{b64("nothing-matches")}
	})
	legacy("legacy_server_error_status", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Status = http.StatusInternalServerError
	})

	// ------------------------------------------------------------------ strict credential sync
	strict("strict_success", nil)
	strict("strict_only_pull_secret_ref", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Spec.SSHKeySecretRef.Name = ""
	})
	strict("strict_only_ssh_key_ref", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Spec.PullSecretRef.Name = ""
	})
	strict("strict_hosted_cluster_ref_nil_panics", func(cr *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, _ *serverSpec) {
		cr.Status.HostedClusterRef = nil
	})
	strict("strict_hosted_cluster_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "hc")
	})
	strict("strict_pull_secret_name_mismatch", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["hc"].(*hyperv1.HostedCluster).Spec.PullSecret.Name = "doca-pull-secret-old"
	})
	strict("strict_ssh_key_name_mismatch", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["hc"].(*hyperv1.HostedCluster).Spec.SSHKey.Name = ""
	})
	strict("strict_pull_secret_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "pullSecret")
	})
	strict("strict_pull_secret_without_dockerconfigjson", func(cr *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["pullSecret"] = secret(hostedcluster.PullSecretCopyName(cr), crNamespace, map[string]string{"config.json": pullSecret})
	})
	strict("strict_node_pool_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "nodePool")
	})
	strict("strict_node_pool_without_config_version", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["nodePool"].(*hyperv1.NodePool).Annotations = map[string]string{"hypershift.openshift.io/nodePoolCurrentConfigVersion": ""}
	})
	strict("strict_current_token_secret_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "currentToken")
	})
	strict("strict_current_token_secret_without_token", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["currentToken"] = secret("token-doca-5f3a9c1e", cpNamespace, map[string]string{"pull-secret-hash": hashSimple([]byte(pullSecret))})
	})
	strict("strict_pull_secret_hash_mismatch", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["currentToken"] = secret("token-doca-5f3a9c1e", cpNamespace, map[string]string{"token": currentToken, "pull-secret-hash": hashSimple([]byte("old"))})
	})
	strict("strict_pull_secret_hash_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["currentToken"] = secret("token-doca-5f3a9c1e", cpNamespace, map[string]string{"token": currentToken})
	})
	strict("strict_ssh_key_secret_missing", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		delete(m, "sshKey")
	})
	strict("strict_ssh_key_blank", func(cr *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["sshKey"] = secret(hostedcluster.SSHKeyCopyName(cr), crNamespace, map[string]string{"id_rsa.pub": " \n\t"})
	})
	strict("strict_ssh_key_secret_without_key", func(cr *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["sshKey"] = secret(hostedcluster.SSHKeyCopyName(cr), crNamespace, map[string]string{"id_ed25519.pub": sshKey})
	})
	strict("strict_token_rejected", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.AcceptTokens = []string{b64("some-other-token")}
	})
	strict("strict_ssh_key_not_propagated", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = payload(payloadOpts{sshKeys: []string{"ssh-rsa OLDKEY old@example.com"}})
	})
	strict("strict_ssh_key_among_several", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = payload(payloadOpts{sshKeys: []string{"ssh-rsa OLDKEY old@example.com", "\t" + sshKey + "  "}})
	})
	strict("strict_payload_not_json", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Payload = "not-json"
	})
	strict("strict_connection_refused", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["hc"].(*hyperv1.HostedCluster).Status.IgnitionEndpoint = "127.0.0.1:1"
	})
	strict("strict_unknown_authority", func(_ *provisioningv1alpha1.DPFHCPProvisioner, m map[string]client.Object, _ *serverSpec) {
		m["ca"] = secret("ignition-server-ca-cert", cpNamespace, map[string]string{"tls.crt": otherCAPlaceholder})
	})
	strict("strict_server_error_status", func(_ *provisioningv1alpha1.DPFHCPProvisioner, _ map[string]client.Object, s *serverSpec) {
		s.Status = http.StatusServiceUnavailable
	})

	// ------------------------------------------------------------------ status update failures
	{
		cr := baseCR()
		builder{name: "status_update_fails_after_success", cr: cr, notIn: true, objs: legacyObjects(), server: defaultServer()}.gen()
	}
	{
		cr := baseCR()
		m := legacyObjects()
		delete(m, "ca")
		builder{name: "status_update_fails_after_error", cr: cr, notIn: true, objs: m, server: defaultServer()}.gen()
	}
	{
		cr := strictCR()
		m := strictObjects(cr)
		delete(m, "nodePool")
		builder{name: "status_update_fails_while_sync_pending", cr: cr, notIn: true, objs: m, server: strictServer()}.gen()
	}

	// ------------------------------------------------------------------ regeneration
	{
		cr := baseCR()
		newImage := "quay.io/openshift-release-dev/ocp-release:4.19.5-multi"
		newPayload := payload(payloadOpts{osImageURL: "https://registry.example.com/rhcos:4.19.5", extraUnit: true})
		builder{
			name: "regeneration_after_release_image_change", cr: cr, objs: legacyObjects(), server: defaultServer(),
			runs: []runSpec{{}, {}, {OCPReleaseImage: &newImage, Payload: &newPayload}},
		}.gen()
	}
	{
		cr := strictCR()
		newPayload := payload(payloadOpts{sshKeys: []string{"ssh-rsa OLDKEY old@example.com"}})
		goodPayload := payload(payloadOpts{})
		builder{
			name: "strict_pending_then_synced", cr: cr, objs: strictObjects(cr), server: strictServer(),
			runs: []runSpec{{Payload: &newPayload}, {Payload: &goodPayload}},
		}.gen()
	}
}
