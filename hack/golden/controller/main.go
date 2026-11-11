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

// Command controller generates the golden differential-test fixtures of crates/controller:
// reconcile sequences of the Go DPFHCPProvisionerReconciler (internal/controller), wired with
// every real sub-feature exactly like cmd/main.go but against a controller-runtime fake
// client (status subresources registered for the CRDs that have one) and one shared
// FakeRecorder. The image checker is a fake that always finds the tag (as in the Go tests).
//
// Every scenario is a list of steps; a step is either a driver mutation (not recorded) followed
// by one Reconcile call. Per step the fixture holds the reconcile result, the error string,
// every write call (create/update/delete, status updates as verb "update-status") with the
// normalized body, the emitted events, and the resulting DPFHCPProvisioner status (conditions
// without lastTransitionTime) and finalizers.
//
// Normalizations (applied identically by the Rust test), on every recorded body and status:
//   - apiVersion/kind, metadata.resourceVersion/creationTimestamp/deletionTimestamp/
//     managedFields and conditions[].lastTransitionTime are removed;
//   - null values, empty objects and empty arrays are removed (recursively);
//   - HostedCluster spec.infraID must match "<name>-[bcdfghjklmnpqrstvwxz2456789]{5}" and is
//     replaced by "INFRAID"; the ETCD encryption key secret's data.key must be 32 bytes and is
//     replaced by "KEY".
//
// Run from the repository root: go run -mod=vendor ./hack/golden/controller
package main

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/google/go-containerregistry/pkg/authn"
	"github.com/google/go-containerregistry/pkg/name"
	operatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	configv1 "github.com/openshift/api/config/v1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/meta"
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

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/bfocplookup"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpucluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/dpuservicetemplate"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/finalizer"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/hostedcluster"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ignitiongenerator"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/kubeconfiginjection"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/metallb"
	ovshugepages "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/ovs-hugepages"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/secrets"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/hostedclient"
)

const fixtureDir = "crates/controller/tests/fixtures"

var scheme = func() *runtime.Scheme {
	s := runtime.NewScheme()
	must(clientgoscheme.AddToScheme(s))
	must(provisioningv1alpha1.AddToScheme(s))
	must(dpuprovisioningv1alpha1.AddToScheme(s))
	must(dpuservicev1alpha1.AddToScheme(s))
	must(operatorv1alpha1.AddToScheme(s))
	must(hyperv1.AddToScheme(s))
	must(metallbv1beta1.AddToScheme(s))
	return s
}()

func must(err error) {
	if err != nil {
		panic(err)
	}
}

// ---------------------------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------------------------

type action struct {
	Verb      string          `json:"verb"`
	Kind      string          `json:"kind"`
	Namespace string          `json:"namespace"`
	Name      string          `json:"name"`
	Object    json.RawMessage `json:"object,omitempty"`
}

type step struct {
	Label        string          `json:"label"`
	Requeue      bool            `json:"requeue"`
	RequeueAfter string          `json:"requeueAfter"`
	Error        string          `json:"error"`
	Actions      []action        `json:"actions"`
	Events       []string        `json:"events"`
	Exists       bool            `json:"exists"`
	Status       json.RawMessage `json:"status,omitempty"`
	Finalizers   []string        `json:"finalizers"`
}

type scenario struct {
	Name  string `json:"name"`
	Steps []step `json:"steps"`
}

var infraIDRe = regexp.MustCompile(`^[a-z0-9-]+-[bcdfghjklmnpqrstvwxz2456789]{5}$`)

// normalizeValue applies the documented normalizations to a decoded JSON document.
func normalizeValue(v interface{}) interface{} {
	switch t := v.(type) {
	case map[string]interface{}:
		out := map[string]interface{}{}
		for k, val := range t {
			if k == "lastTransitionTime" {
				continue
			}
			n := normalizeValue(val)
			if isEmpty(n) {
				continue
			}
			out[k] = n
		}
		return out
	case []interface{}:
		out := []interface{}{}
		for _, val := range t {
			out = append(out, normalizeValue(val))
		}
		return out
	default:
		return v
	}
}

func isEmpty(v interface{}) bool {
	switch t := v.(type) {
	case nil:
		return true
	case map[string]interface{}:
		return len(t) == 0
	case []interface{}:
		return len(t) == 0
	}
	return false
}

func normalizeObject(obj client.Object) json.RawMessage {
	c := obj.DeepCopyObject().(client.Object)
	switch o := c.(type) {
	case *hyperv1.HostedCluster:
		if o.Spec.InfraID != "" {
			if !infraIDRe.MatchString(o.Spec.InfraID) || !strings.HasPrefix(o.Spec.InfraID, o.Name+"-") {
				panic("unexpected infraID " + o.Spec.InfraID)
			}
			o.Spec.InfraID = "INFRAID"
		}
	case *corev1.Secret:
		if strings.HasSuffix(o.Name, "-etcd-encryption-key") {
			if len(o.Data[hyperv1.AESCBCKeySecretKey]) != 32 {
				panic("unexpected etcd key length")
			}
			o.Data[hyperv1.AESCBCKeySecretKey] = []byte("KEY")
		}
	}
	b, err := json.Marshal(c)
	must(err)
	var v map[string]interface{}
	must(json.Unmarshal(b, &v))
	delete(v, "apiVersion")
	delete(v, "kind")
	if md, ok := v["metadata"].(map[string]interface{}); ok {
		for _, k := range []string{"resourceVersion", "creationTimestamp", "deletionTimestamp", "managedFields"} {
			delete(md, k)
		}
	}
	out, err := json.Marshal(normalizeValue(v))
	must(err)
	return out
}

type env struct {
	client    client.Client
	recorder  *record.FakeRecorder
	actions   *[]action
	recording *bool
	r         *controller.DPFHCPProvisionerReconciler
}

// fakeImageChecker implements bfocplookup.ImageChecker (the tag always exists).
type fakeImageChecker struct{}

func (f *fakeImageChecker) CheckTag(_ context.Context, _ name.Reference, _ authn.Keychain) error {
	return nil
}

func newEnv(objs ...client.Object) *env {
	actions := &[]action{}
	recording := new(bool)
	rec := func(verb string, obj client.Object, withBody bool) {
		if !*recording {
			return
		}
		gvk, err := apiutil.GVKForObject(obj, scheme)
		must(err)
		a := action{Verb: verb, Kind: gvk.Kind, Namespace: obj.GetNamespace(), Name: obj.GetName()}
		if withBody {
			a.Object = normalizeObject(obj)
		}
		*actions = append(*actions, a)
	}
	c := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(objs...).
		WithStatusSubresource(
			&provisioningv1alpha1.DPFHCPProvisioner{},
			&dpuprovisioningv1alpha1.DPUCluster{},
			&hyperv1.HostedCluster{},
			&hyperv1.NodePool{},
		).
		WithInterceptorFuncs(interceptor.Funcs{
			Create: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.CreateOption) error {
				rec("create", obj, true)
				return c.Create(ctx, obj, opts...)
			},
			Update: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.UpdateOption) error {
				rec("update", obj, true)
				return c.Update(ctx, obj, opts...)
			},
			Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
				rec("delete", obj, false)
				return c.Delete(ctx, obj, opts...)
			},
			SubResourceUpdate: func(ctx context.Context, c client.Client, subResourceName string, obj client.Object, opts ...client.SubResourceUpdateOption) error {
				rec("update-"+subResourceName, obj, true)
				return c.SubResource(subResourceName).Update(ctx, obj, opts...)
			},
		}).
		Build()
	recorder := record.NewFakeRecorder(1000)

	// Wiring of cmd/main.go (one shared recorder).
	imageLookup := bfocplookup.NewImageLookup(c, recorder)
	imageLookup.ImageChecker = &fakeImageChecker{}
	hostedClientManager := hostedclient.NewClientManager(c)
	finalizerManager := finalizer.NewManager(c, recorder)
	finalizerManager.RegisterHandler(kubeconfiginjection.NewCleanupHandler(c, recorder))
	finalizerManager.RegisterHandler(hostedcluster.NewCleanupHandler(c, recorder))
	finalizerManager.RegisterHandler(metallb.NewCleanupHandler(c, recorder))
	r := &controller.DPFHCPProvisionerReconciler{
		Client:               c,
		Scheme:               scheme,
		Recorder:             recorder,
		ImageLookup:          imageLookup,
		DPUClusterValidator:  dpucluster.NewValidator(c, recorder),
		SecretsValidator:     secrets.NewValidator(c, recorder),
		SecretManager:        hostedcluster.NewSecretManager(c, scheme),
		MetalLBManager:       metallb.NewMetalLBManager(c, recorder),
		HostedClusterManager: hostedcluster.NewHostedClusterManager(c, scheme),
		NodePoolManager:      hostedcluster.NewNodePoolManager(c, scheme),
		FinalizerManager:     finalizerManager,
		StatusSyncer:         hostedcluster.NewStatusSyncer(c),
		KubeconfigInjector:   kubeconfiginjection.NewKubeconfigInjector(c, recorder),
		IgnitionGenerator:    ignitiongenerator.NewIgnitionGenerator(c, scheme, recorder),
		HugepagesManager: ovshugepages.NewManager(c, hostedClientManager,
			&dpuservicetemplate.RemoteReleaseImageReader{}),
	}
	return &env{client: c, recorder: recorder, actions: actions, recording: recording, r: r}
}

var ctx = ctrl.LoggerInto(context.Background(), ctrl.Log)

// reconcile runs one Reconcile of the CR and records the step.
func (e *env) reconcile(label string, key k8stypes.NamespacedName) step {
	*e.actions = []action{}
	*e.recording = true
	res, err := e.r.Reconcile(ctx, ctrl.Request{NamespacedName: key})
	*e.recording = false
	s := step{
		Label:        label,
		Requeue:      res.Requeue, //nolint:staticcheck
		RequeueAfter: res.RequeueAfter.String(),
		Actions:      *e.actions,
		Events:       []string{},
		Finalizers:   []string{},
	}
	if err != nil {
		s.Error = err.Error()
	}
	for {
		select {
		case ev := <-e.recorder.Events:
			s.Events = append(s.Events, ev)
			continue
		default:
		}
		break
	}
	cr := &provisioningv1alpha1.DPFHCPProvisioner{}
	if err := e.client.Get(ctx, key, cr); err == nil {
		s.Exists = true
		b, err := json.Marshal(cr.Status)
		must(err)
		var v interface{}
		must(json.Unmarshal(b, &v))
		out, err := json.Marshal(normalizeValue(v))
		must(err)
		s.Status = out
		s.Finalizers = append(s.Finalizers, cr.Finalizers...)
	}
	return s
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
	crName    = "dpf-hcp"
	crNS      = "clusters"
	crUID     = "cr-uid-1234"
	dpuName   = "dpu"
	dpuNS     = "dpu-system"
	ddName    = "dd"
	dpfNS     = "dpf-operator-system"
	image419  = "quay.io/openshift-release-dev/ocp-release:4.19.0-multi"
	image420  = "quay.io/openshift-release-dev/ocp-release:4.20.0-multi"
	bfRepo    = "quay.io/example/bf-ocp-layer"
	pullName  = "my-pull-secret"
	sshName   = "my-ssh-key"
	kubeconfg = "apiVersion: v1\nkind: Config\nclusters: []\n"
)

var key = k8stypes.NamespacedName{Namespace: crNS, Name: crName}

func configCR() *provisioningv1alpha1.DPFHCPProvisionerConfig {
	return &provisioningv1alpha1.DPFHCPProvisionerConfig{
		ObjectMeta: metav1.ObjectMeta{Name: provisioningv1alpha1.DefaultConfigName},
		Spec:       provisioningv1alpha1.DPFHCPProvisionerConfigSpec{BlueFieldOCPLayerRepo: bfRepo},
	}
}

func baseCR() *provisioningv1alpha1.DPFHCPProvisioner {
	return &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNS, UID: crUID, Generation: 1},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			DPUClusterRef:                  provisioningv1alpha1.DPUClusterReference{Name: dpuName, Namespace: dpuNS},
			DPUDeploymentRef:               &provisioningv1alpha1.DPUDeploymentReference{Name: ddName, Namespace: dpfNS},
			BaseDomain:                     "example.com",
			OCPReleaseImage:                image419,
			SSHKeySecretRef:                corev1.LocalObjectReference{Name: sshName},
			PullSecretRef:                  corev1.LocalObjectReference{Name: pullName},
			EtcdStorageClass:               "standard",
			ControlPlaneAvailabilityPolicy: hyperv1.SingleReplica,
		},
	}
}

func dpuCluster() *dpuprovisioningv1alpha1.DPUCluster {
	return &dpuprovisioningv1alpha1.DPUCluster{
		ObjectMeta: metav1.ObjectMeta{Name: dpuName, Namespace: dpuNS},
		Spec:       dpuprovisioningv1alpha1.DPUClusterSpec{Type: string(dpuprovisioningv1alpha1.StaticCluster)},
		Status:     dpuprovisioningv1alpha1.DPUClusterStatus{Phase: dpuprovisioningv1alpha1.PhaseReady},
	}
}

func pullSecret() *corev1.Secret {
	auth := base64.StdEncoding.EncodeToString([]byte("test:test"))
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: pullName, Namespace: crNS},
		Type:       corev1.SecretTypeDockerConfigJson,
		Data:       map[string][]byte{".dockerconfigjson": []byte(fmt.Sprintf(`{"auths":{"quay.io":{"auth":"%s"}}}`, auth))},
	}
}

func sshKey() *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: sshName, Namespace: crNS},
		Type:       corev1.SecretTypeOpaque,
		Data:       map[string][]byte{"id_rsa.pub": []byte("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ...")},
	}
}

func node() *corev1.Node {
	return &corev1.Node{
		ObjectMeta: metav1.ObjectMeta{Name: "node-a"},
		Status: corev1.NodeStatus{Addresses: []corev1.NodeAddress{
			{Type: corev1.NodeInternalIP, Address: "10.1.1.1"},
		}},
	}
}

func dpuDeployment(bfb, flavor string) *dpuservicev1alpha1.DPUDeployment {
	return &dpuservicev1alpha1.DPUDeployment{
		ObjectMeta: metav1.ObjectMeta{Name: ddName, Namespace: dpfNS},
		Spec: dpuservicev1alpha1.DPUDeploymentSpec{
			DPUs: dpuservicev1alpha1.DPUs{BFB: ptr.To(bfb), Flavor: ptr.To(flavor)},
		},
	}
}

func dpuFlavor(name string) *dpuprovisioningv1alpha1.DPUFlavor {
	return &dpuprovisioningv1alpha1.DPUFlavor{ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: dpfNS}}
}

func ignitionCM(flavor, bfb string) *corev1.ConfigMap {
	return &corev1.ConfigMap{
		ObjectMeta: metav1.ObjectMeta{
			Name:      ignitiongenerator.ConfigMapName(dpuName),
			Namespace: dpuNS,
			Labels:    map[string]string{ignitiongenerator.BfcfgTemplateLabel: "true"},
			Annotations: map[string]string{
				ignitiongenerator.BfcfgTemplateClusterNameAnnotation:      dpuName,
				ignitiongenerator.BfcfgTemplateClusterNamespaceAnnotation: dpuNS,
				ignitiongenerator.BfcfgTemplateDPUFlavorNameAnnotation:    flavor,
				ignitiongenerator.BfcfgTemplateBFBNameAnnotation:          bfb,
			},
		},
		Data: map[string]string{"BF_CFG_TEMPLATE": `{"ignition":{"version":"3.4.0"}}`},
	}
}

func kubeconfigSecret() *corev1.Secret {
	return &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: crName + "-admin-kubeconfig", Namespace: crNS},
		Data:       map[string][]byte{"kubeconfig": []byte(kubeconfg)},
	}
}

// world returns the objects of a fully valid environment (without the CR).
func world() []client.Object {
	return []client.Object{
		configCR(), dpuCluster(), pullSecret(), sshKey(), node(),
		dpuDeployment("bfb-1", "flavor-1"), dpuFlavor("flavor-1"),
	}
}

func hcConditions(available metav1.ConditionStatus) []metav1.Condition {
	return []metav1.Condition{
		{Type: string(hyperv1.HostedClusterAvailable), Status: available, Reason: "AsExpected", Message: "The hosted control plane is available", LastTransitionTime: metav1.Now()},
		{Type: string(hyperv1.HostedClusterProgressing), Status: metav1.ConditionFalse, Reason: "AsExpected", Message: "HostedCluster is at expected version", LastTransitionTime: metav1.Now()},
		{Type: string(hyperv1.HostedClusterDegraded), Status: metav1.ConditionFalse, Reason: "AsExpected", Message: "", LastTransitionTime: metav1.Now()},
	}
}

func history(entries ...[2]string) []hyperv1.ControlPlaneUpdateHistory {
	out := []hyperv1.ControlPlaneUpdateHistory{}
	for _, e := range entries {
		out = append(out, hyperv1.ControlPlaneUpdateHistory{State: configv1.CompletedUpdate, Version: e[0], Image: e[1]})
	}
	return out
}

// --- driver mutations (not recorded) ---------------------------------------------------------

func (e *env) create(obj client.Object) {
	must(e.client.Create(ctx, obj))
}

// ensure creates obj unless it already exists.
func (e *env) ensure(obj client.Object) {
	if err := e.client.Create(ctx, obj); err != nil && !apierrors.IsAlreadyExists(err) {
		panic(err)
	}
}

func (e *env) mutateCR(f func(cr *provisioningv1alpha1.DPFHCPProvisioner)) {
	cr := &provisioningv1alpha1.DPFHCPProvisioner{}
	must(e.client.Get(ctx, key, cr))
	f(cr)
	must(e.client.Update(ctx, cr))
}

func (e *env) mutateCRStatus(f func(cr *provisioningv1alpha1.DPFHCPProvisioner)) {
	cr := &provisioningv1alpha1.DPFHCPProvisioner{}
	must(e.client.Get(ctx, key, cr))
	f(cr)
	must(e.client.Status().Update(ctx, cr))
}

func (e *env) mutateHCStatus(f func(hc *hyperv1.HostedCluster)) {
	hc := &hyperv1.HostedCluster{}
	must(e.client.Get(ctx, key, hc))
	f(hc)
	must(e.client.Status().Update(ctx, hc))
}

func (e *env) mutateDD(f func(dd *dpuservicev1alpha1.DPUDeployment)) {
	dd := &dpuservicev1alpha1.DPUDeployment{}
	must(e.client.Get(ctx, k8stypes.NamespacedName{Namespace: dpfNS, Name: ddName}, dd))
	f(dd)
	must(e.client.Update(ctx, dd))
}

func (e *env) deleteObj(obj client.Object) {
	must(e.client.Delete(ctx, obj))
}

func setCond(cr *provisioningv1alpha1.DPFHCPProvisioner, t string, s metav1.ConditionStatus, reason, msg string, gen int64) {
	meta.SetStatusCondition(&cr.Status.Conditions, metav1.Condition{Type: t, Status: s, Reason: reason, Message: msg, ObservedGeneration: gen})
}

func readyCR() *provisioningv1alpha1.DPFHCPProvisioner {
	cr := baseCR()
	cr.Finalizers = []string{controller.FinalizerName}
	cr.Status.Phase = provisioningv1alpha1.PhaseReady
	cr.Status.HostedClusterRef = &corev1.ObjectReference{Name: crName, Namespace: crNS, Kind: "HostedCluster", APIVersion: "hypershift.openshift.io/v1beta1"}
	setCond(cr, provisioningv1alpha1.HostedClusterAvailable, metav1.ConditionTrue, "AsExpected", "The hosted control plane is available", 0)
	setCond(cr, provisioningv1alpha1.KubeConfigInjected, metav1.ConditionTrue, provisioningv1alpha1.ReasonKubeConfigInjected, "Kubeconfig injected", 0)
	setCond(cr, provisioningv1alpha1.IgnitionConfigured, metav1.ConditionTrue, provisioningv1alpha1.ReasonIgnitionGenerated, "Ignition ConfigMap bfcfg-dpu.cfg created successfully", 1)
	return cr
}

func ownedHC(image string) *hyperv1.HostedCluster {
	return &hyperv1.HostedCluster{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNS, OwnerReferences: []metav1.OwnerReference{{
			APIVersion: "provisioning.dpu.hcp.io/v1alpha1", Kind: "DPFHCPProvisioner", Name: crName,
			UID: crUID, Controller: ptr.To(true), BlockOwnerDeletion: ptr.To(true),
		}}},
		Spec: hyperv1.HostedClusterSpec{
			Release:    hyperv1.Release{Image: image},
			InfraID:    "dpf-hcp-bcdfg",
			PullSecret: corev1.LocalObjectReference{Name: hostedcluster.PullSecretCopyName(baseCR())},
			SSHKey:     corev1.LocalObjectReference{Name: hostedcluster.SSHKeyCopyName(baseCR())},
		},
		Status: hyperv1.HostedClusterStatus{
			Conditions:          hcConditions(metav1.ConditionTrue),
			ControlPlaneVersion: hyperv1.ControlPlaneVersionStatus{History: history([2]string{"4.19.0", image419})},
		},
	}
}

// readyWorld: a provisioned CR (Ready) with its HC, kubeconfig secret and ignition ConfigMap.
func readyWorld(extra ...client.Object) *env {
	objs := append(world(), readyCR(), ownedHC(image419), kubeconfigSecret(), ignitionCM("flavor-1", "bfb-1"))
	return newEnv(append(objs, extra...)...)
}

// ---------------------------------------------------------------------------------------------

func main() {
	must(os.MkdirAll(fixtureDir, 0o755))

	// --- full lifecycle: create → provision → ignition → ready → dependency changes →
	//     upgrade → deletion ----------------------------------------------------------------
	{
		e := newEnv(append(world(), baseCR())...)
		s := scenario{Name: "lifecycle"}
		s.Steps = append(s.Steps, e.reconcile("add finalizer", key))
		s.Steps = append(s.Steps, e.reconcile("initial provisioning", key))
		s.Steps = append(s.Steps, e.reconcile("waiting for control plane", key))

		e.mutateHCStatus(func(hc *hyperv1.HostedCluster) {
			hc.Status.Conditions = hcConditions(metav1.ConditionTrue)
			hc.Status.ControlPlaneVersion.History = history([2]string{"4.19.0", image419})
		})
		e.create(kubeconfigSecret())
		s.Steps = append(s.Steps, e.reconcile("hc available, kubeconfig injected", key))
		s.Steps = append(s.Steps, e.reconcile("generating ignition fails", key))
		s.Steps = append(s.Steps, e.reconcile("ignition retry after failure", key))

		e.create(ignitionCM("flavor-1", "bfb-1"))
		e.mutateCRStatus(func(cr *provisioningv1alpha1.DPFHCPProvisioner) {
			setCond(cr, provisioningv1alpha1.IgnitionConfigured, metav1.ConditionTrue, provisioningv1alpha1.ReasonIgnitionGenerated, "Ignition ConfigMap bfcfg-dpu.cfg created successfully", cr.Generation)
		})
		s.Steps = append(s.Steps, e.reconcile("ignition configured", key))
		s.Steps = append(s.Steps, e.reconcile("ready steady state", key))

		e.mutateDD(func(dd *dpuservicev1alpha1.DPUDeployment) { dd.Spec.DPUs.BFB = ptr.To("bfb-2") })
		s.Steps = append(s.Steps, e.reconcile("bfb changed", key))
		s.Steps = append(s.Steps, e.reconcile("after bfb change", key))

		e.create(dpuFlavor("flavor-2"))
		e.mutateDD(func(dd *dpuservicev1alpha1.DPUDeployment) { dd.Spec.DPUs.Flavor = ptr.To("flavor-2") })
		s.Steps = append(s.Steps, e.reconcile("flavor changed", key))
		s.Steps = append(s.Steps, e.reconcile("regenerate after flavor change", key))

		// simulate a successful regeneration, then upgrade the release image
		e.create(ignitionCM("flavor-2", "bfb-2"))
		e.mutateCRStatus(func(cr *provisioningv1alpha1.DPFHCPProvisioner) {
			setCond(cr, provisioningv1alpha1.IgnitionConfigured, metav1.ConditionTrue, provisioningv1alpha1.ReasonIgnitionGenerated, "Ignition ConfigMap bfcfg-dpu.cfg created successfully", cr.Generation)
		})
		s.Steps = append(s.Steps, e.reconcile("ready again", key))
		e.mutateCR(func(cr *provisioningv1alpha1.DPFHCPProvisioner) {
			cr.Spec.OCPReleaseImage = image420
			cr.Generation = 2
		})
		s.Steps = append(s.Steps, e.reconcile("upgrade detected", key))
		s.Steps = append(s.Steps, e.reconcile("upgrade in progress", key))
		e.mutateHCStatus(func(hc *hyperv1.HostedCluster) {
			hc.Status.ControlPlaneVersion.History = history([2]string{"4.20.0", image420}, [2]string{"4.19.0", image419})
		})
		s.Steps = append(s.Steps, e.reconcile("upgrade complete", key))
		s.Steps = append(s.Steps, e.reconcile("after upgrade", key))

		e.deleteObj(baseCR())
		for i := 1; i <= 4; i++ {
			s.Steps = append(s.Steps, e.reconcile(fmt.Sprintf("deletion %d", i), key))
		}
		write(s)
	}

	// --- operator idle without the Config CR; CR not found -------------------------------
	{
		cr := baseCR()
		e := newEnv(cr, dpuCluster())
		s := scenario{Name: "no_config"}
		s.Steps = append(s.Steps, e.reconcile("no config", key))
		s.Steps = append(s.Steps, e.reconcile("not found", k8stypes.NamespacedName{Namespace: crNS, Name: "missing"}))
		write(s)
	}

	// --- validation failures -------------------------------------------------------------
	{
		cr := baseCR()
		cr.Finalizers = []string{controller.FinalizerName}
		e := newEnv(configCR(), cr)
		s := scenario{Name: "dpucluster_missing"}
		s.Steps = append(s.Steps, e.reconcile("dpucluster missing", key))
		s.Steps = append(s.Steps, e.reconcile("dpucluster missing again", key))
		e.create(dpuCluster())
		s.Steps = append(s.Steps, e.reconcile("dpucluster created, secrets missing", key))
		e.create(pullSecret())
		e.create(sshKey())
		s.Steps = append(s.Steps, e.reconcile("secrets created", key))
		write(s)
	}
	{
		cr := baseCR()
		cr.Finalizers = []string{controller.FinalizerName}
		cr.Spec.OCPReleaseImage = "not-an-image"
		e := newEnv(append(world(), cr)...)
		s := scenario{Name: "invalid_release_image"}
		s.Steps = append(s.Steps, e.reconcile("invalid release image", key))
		s.Steps = append(s.Steps, e.reconcile("invalid release image again", key))
		write(s)
	}

	// --- machineOSURL skips the lookup; no nodes → HC creation fails -----------------------
	{
		cr := baseCR()
		cr.Finalizers = []string{controller.FinalizerName}
		cr.Spec.MachineOSURL = "https://example.com/rhcos.qcow2"
		e := newEnv(configCR(), dpuCluster(), pullSecret(), sshKey())
		e.create(cr)
		s := scenario{Name: "machine_os_url"}
		s.Steps = append(s.Steps, e.reconcile("lookup skipped, no nodes", key))
		s.Steps = append(s.Steps, e.reconcile("again", key))
		write(s)
	}

	// --- HighlyAvailable: MetalLB configuration + LoadBalancer HC ------------------------
	{
		cr := baseCR()
		cr.Finalizers = []string{controller.FinalizerName}
		cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.HighlyAvailable
		cr.Spec.VirtualIP = "10.0.0.100"
		e := newEnv(append(world(), cr)...)
		s := scenario{Name: "metallb_highly_available"}
		s.Steps = append(s.Steps, e.reconcile("configure metallb and create hc", key))
		s.Steps = append(s.Steps, e.reconcile("again", key))
		e.deleteObj(baseCR())
		for i := 1; i <= 3; i++ {
			s.Steps = append(s.Steps, e.reconcile(fmt.Sprintf("deletion %d", i), key))
		}
		write(s)
	}
	{
		cfg := configCR()
		cfg.Spec.DisableMetalLB = true
		cr := baseCR()
		cr.Finalizers = []string{controller.FinalizerName}
		cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.HighlyAvailable
		cr.Spec.VirtualIP = "10.0.0.100"
		objs := append([]client.Object{cfg}, world()[1:]...)
		e := newEnv(append(objs, cr)...)
		s := scenario{Name: "metallb_disabled"}
		s.Steps = append(s.Steps, e.reconcile("metallb disabled", key))
		write(s)
	}

	// --- dependency handling from the Ready state -----------------------------------------
	{
		e := readyWorld()
		s := scenario{Name: "ready_restart"}
		s.Steps = append(s.Steps, e.reconcile("operator restarted", key))
		s.Steps = append(s.Steps, e.reconcile("regenerate after restart", key))
		write(s)
	}
	{
		e := readyWorld()
		s := scenario{Name: "dependency_deleted"}
		e.deleteObj(dpuDeployment("", ""))
		s.Steps = append(s.Steps, e.reconcile("dpudeployment deleted", key))
		s.Steps = append(s.Steps, e.reconcile("dpudeployment still missing", key))
		e.create(dpuDeployment("bfb-1", ""))
		s.Steps = append(s.Steps, e.reconcile("dpudeployment without flavor", key))
		e.mutateDD(func(dd *dpuservicev1alpha1.DPUDeployment) { dd.Spec.DPUs.Flavor = ptr.To("missing-flavor") })
		s.Steps = append(s.Steps, e.reconcile("flavor missing", key))
		write(s)
	}
	{
		e := readyWorld()
		s := scenario{Name: "configmap_deleted"}
		// first reconcile marks the ignition as reconciled (operator-restart regeneration)
		s.Steps = append(s.Steps, e.reconcile("operator restarted", key))
		e.ensure(ignitionCM("flavor-1", "bfb-1"))
		e.mutateCRStatus(func(cr *provisioningv1alpha1.DPFHCPProvisioner) {
			setCond(cr, provisioningv1alpha1.IgnitionConfigured, metav1.ConditionTrue, provisioningv1alpha1.ReasonIgnitionGenerated, "Ignition ConfigMap bfcfg-dpu.cfg created successfully", cr.Generation)
		})
		s.Steps = append(s.Steps, e.reconcile("ready", key))
		e.deleteObj(ignitionCM("", ""))
		s.Steps = append(s.Steps, e.reconcile("configmap deleted", key))
		s.Steps = append(s.Steps, e.reconcile("regenerate after deletion", key))
		write(s)
	}

	// --- HostedCluster blocked / degraded → Error ------------------------------------------
	{
		e := readyWorld()
		s := scenario{Name: "hc_blocked"}
		e.mutateHCStatus(func(hc *hyperv1.HostedCluster) {
			hc.Status.Conditions = []metav1.Condition{
				{Type: string(hyperv1.HostedClusterAvailable), Status: metav1.ConditionFalse, Reason: "WaitingForAvailable", Message: "failed to extract release metadata", LastTransitionTime: metav1.Now()},
				{Type: string(hyperv1.HostedClusterProgressing), Status: metav1.ConditionFalse, Reason: hyperv1.BlockedReason, Message: "failed to extract release metadata", LastTransitionTime: metav1.Now()},
			}
		})
		s.Steps = append(s.Steps, e.reconcile("hc blocked", key))
		e.mutateHCStatus(func(hc *hyperv1.HostedCluster) {
			hc.Status.Conditions = []metav1.Condition{
				{Type: string(hyperv1.HostedClusterAvailable), Status: metav1.ConditionTrue, Reason: "AsExpected", Message: "available", LastTransitionTime: metav1.Now()},
				{Type: string(hyperv1.HostedClusterDegraded), Status: metav1.ConditionTrue, Reason: "DegradedError", Message: "errors requiring intervention", LastTransitionTime: metav1.Now()},
			}
		})
		s.Steps = append(s.Steps, e.reconcile("hc degraded", key))
		write(s)
	}

	// --- missed upgrade recovery (HC already on the target image) --------------------------
	{
		cr := readyCR()
		cr.Spec.OCPReleaseImage = image420
		cr.Generation = 2
		hc := ownedHC(image420)
		np := &hyperv1.NodePool{
			ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNS, OwnerReferences: hc.OwnerReferences},
			Spec:       hyperv1.NodePoolSpec{ClusterName: crName, Release: hyperv1.Release{Image: image419}},
		}
		objs := append(world(), cr, hc, np, kubeconfigSecret(), ignitionCM("flavor-1", "bfb-1"))
		e := newEnv(objs...)
		s := scenario{Name: "upgrade_recovery"}
		s.Steps = append(s.Steps, e.reconcile("recover missed upgrade", key))
		s.Steps = append(s.Steps, e.reconcile("upgrade in progress", key))
		write(s)
	}
}
