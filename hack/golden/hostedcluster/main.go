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

// Command hostedcluster generates the golden differential-test fixtures of
// crates/ctrl-hostedcluster: fixed scenarios are run through the Go managers of
// internal/controller/hostedcluster (and the finalizer manager) against a controller-runtime
// fake client. Every write call (create/update/delete) is recorded through an interceptor,
// together with the object body as the real client would send it (TypeMeta filled from the
// scheme), the returned result/error, emitted events and resulting conditions.
//
// Normalizations (applied identically by the Rust test):
//   - HostedCluster spec.infraID: must match "<cr-name>-[bcdfghjklmnpqrstvwxz2456789]{5}",
//     replaced by "INFRAID".
//   - ETCD encryption key secret data["key"]: must be 32 bytes, replaced by "KEY".
//
// Run from the repository root: go run -mod=vendor ./hack/golden/hostedcluster
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	"github.com/openshift/hypershift/api/util/ipnet"
	corev1 "k8s.io/api/core/v1"
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
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/finalizer"
	"github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/internal/controller/hostedcluster"
)

const fixtureDir = "crates/ctrl-hostedcluster/tests/fixtures"

var scheme = func() *runtime.Scheme {
	s := runtime.NewScheme()
	must(clientgoscheme.AddToScheme(s))
	must(provisioningv1alpha1.AddToScheme(s))
	must(hyperv1.AddToScheme(s))
	return s
}()

func must(err error) {
	if err != nil {
		panic(err)
	}
}

type action struct {
	Verb      string          `json:"verb"`
	Kind      string          `json:"kind"`
	Namespace string          `json:"namespace"`
	Name      string          `json:"name"`
	Object    json.RawMessage `json:"object,omitempty"`
	// Raw is the exact json.Marshal output of Object (byte-compared for hypershift types).
	Raw string `json:"raw,omitempty"`
}

type condition struct {
	Type               string `json:"type"`
	Status             string `json:"status"`
	Reason             string `json:"reason"`
	Message            string `json:"message"`
	ObservedGeneration int64  `json:"observedGeneration"`
}

type scenario struct {
	Name         string      `json:"name"`
	RequeueAfter string      `json:"requeueAfter"`
	Error        string      `json:"error"`
	Actions      []action    `json:"actions"`
	Events       []string    `json:"events"`
	Conditions   []condition `json:"conditions,omitempty"`
}

var infraIDRe = regexp.MustCompile(`^[a-z0-9-]+-[bcdfghjklmnpqrstvwxz2456789]{5}$`)

// normalize replaces non-deterministic fields (see package doc).
func normalize(obj client.Object) {
	switch o := obj.(type) {
	case *hyperv1.HostedCluster:
		if !infraIDRe.MatchString(o.Spec.InfraID) || !strings.HasPrefix(o.Spec.InfraID, o.Name+"-") {
			panic("unexpected infraID " + o.Spec.InfraID)
		}
		o.Spec.InfraID = "INFRAID"
	case *corev1.Secret:
		if strings.HasSuffix(o.Name, "-etcd-encryption-key") {
			if len(o.Data[hyperv1.AESCBCKeySecretKey]) != 32 {
				panic("unexpected etcd key length")
			}
			o.Data[hyperv1.AESCBCKeySecretKey] = []byte("KEY")
		}
	}
}

type env struct {
	client   client.Client
	recorder *record.FakeRecorder
	actions  *[]action
}

func newEnv(objs ...client.Object) env {
	actions := &[]action{}
	rec := func(verb string, obj client.Object, withBody bool) {
		gvk, err := apiutil.GVKForObject(obj, scheme)
		must(err)
		a := action{Verb: verb, Kind: gvk.Kind, Namespace: obj.GetNamespace(), Name: obj.GetName()}
		if withBody {
			c := obj.DeepCopyObject().(client.Object)
			c.GetObjectKind().SetGroupVersionKind(gvk)
			normalize(c)
			b, err := json.Marshal(c)
			must(err)
			a.Object = b
			a.Raw = string(b)
		}
		*actions = append(*actions, a)
	}
	c := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(objs...).
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
		}).
		Build()
	return env{client: c, recorder: record.NewFakeRecorder(100), actions: actions}
}

func (e env) finish(name string, res ctrl.Result, err error) scenario {
	s := scenario{Name: name, RequeueAfter: res.RequeueAfter.String(), Actions: *e.actions, Events: []string{}}
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
	if s.Actions == nil {
		s.Actions = []action{}
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
	crName = "dpf-hcp"
	crNS   = "clusters"
	crUID  = "cr-uid-1234"
)

func baseCR() *provisioningv1alpha1.DPFHCPProvisioner {
	return &provisioningv1alpha1.DPFHCPProvisioner{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNS, UID: crUID, Generation: 2},
		Spec: provisioningv1alpha1.DPFHCPProvisionerSpec{
			DPUClusterRef:   provisioningv1alpha1.DPUClusterReference{Name: "dpu", Namespace: "dpu-system"},
			BaseDomain:      "example.com",
			OCPReleaseImage: "quay.io/openshift-release-dev/ocp-release:4.19.0-multi",
			SSHKeySecretRef: corev1.LocalObjectReference{Name: "my-ssh-key"},
			PullSecretRef:   corev1.LocalObjectReference{Name: "my-pull-secret"},
		},
	}
}

func ownerRef(uid string) []metav1.OwnerReference {
	return []metav1.OwnerReference{{
		APIVersion:         "provisioning.dpu.hcp.io/v1alpha1",
		Kind:               "DPFHCPProvisioner",
		Name:               crName,
		UID:                k8stypes.UID(uid),
		Controller:         ptr.To(true),
		BlockOwnerDeletion: ptr.To(true),
	}}
}

func secret(name string, owner string, labels map[string]string, data map[string][]byte) *corev1.Secret {
	s := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: crNS, Labels: labels},
		Type:       corev1.SecretTypeOpaque,
		Data:       data,
	}
	if owner != "" {
		s.OwnerReferences = ownerRef(owner)
	}
	return s
}

func node(name string, addrs ...corev1.NodeAddress) *corev1.Node {
	return &corev1.Node{ObjectMeta: metav1.ObjectMeta{Name: name}, Status: corev1.NodeStatus{Addresses: addrs}}
}

func ownedHC(cr *provisioningv1alpha1.DPFHCPProvisioner, owner, pull, ssh string) *hyperv1.HostedCluster {
	return &hyperv1.HostedCluster{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNS, OwnerReferences: ownerRef(owner)},
		Spec: hyperv1.HostedClusterSpec{
			Release:    hyperv1.Release{Image: cr.Spec.OCPReleaseImage},
			InfraID:    "dpf-hcp-bcdfg",
			PullSecret: corev1.LocalObjectReference{Name: pull},
			SSHKey:     corev1.LocalObjectReference{Name: ssh},
		},
	}
}

func ownedNP(owner string) *hyperv1.NodePool {
	return &hyperv1.NodePool{
		ObjectMeta: metav1.ObjectMeta{Name: crName, Namespace: crNS, OwnerReferences: ownerRef(owner)},
		Spec:       hyperv1.NodePoolSpec{ClusterName: crName, Release: hyperv1.Release{Image: "img"}},
	}
}

func main() {
	must(os.MkdirAll(fixtureDir, 0o755))
	ctx := ctrl.LoggerInto(context.Background(), ctrl.Log)

	// --- HostedClusterManager.CreateOrUpdateHostedCluster -----------------------------------
	{
		cr := baseCR()
		cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.HighlyAvailable
		cr.Spec.VirtualIP = "10.0.0.100"
		cr.Spec.EtcdStorageClass = "ceph-rbd"
		e := newEnv()
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_create_loadbalancer", res, err))
	}
	{
		cr := baseCR()
		cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.SingleReplica
		cr.Spec.FlannelEnabled = ptr.To(false)
		cr.Spec.NodeSelector = map[string]string{"zone": "a", "node-role.kubernetes.io/master": ""}
		cr.Spec.Networking = &provisioningv1alpha1.ClusterNetworkConfig{
			ServiceNetwork: []hyperv1.ServiceNetworkEntry{{CIDR: *ipnet.MustParseCIDR("10.96.0.0/12")}},
			ClusterNetwork: []hyperv1.ClusterNetworkEntry{{CIDR: *ipnet.MustParseCIDR("10.244.0.0/16"), HostPrefix: 23}},
			MachineNetwork: []hyperv1.MachineNetworkEntry{{CIDR: *ipnet.MustParseCIDR("192.168.10.0/24")}},
		}
		e := newEnv(
			node("node-b", corev1.NodeAddress{Type: corev1.NodeInternalIP, Address: "10.1.1.2"}),
			node("node-a",
				corev1.NodeAddress{Type: corev1.NodeHostName, Address: "node-a"},
				corev1.NodeAddress{Type: corev1.NodeInternalIP, Address: "10.1.1.1"},
				corev1.NodeAddress{Type: corev1.NodeExternalIP, Address: "1.2.3.4"}),
		)
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_create_nodeport", res, err))
	}
	{
		cr := baseCR()
		cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.SingleReplica
		e := newEnv(node("node-a",
			corev1.NodeAddress{Type: corev1.NodeInternalIP, Address: "10.1.1.1"},
			corev1.NodeAddress{Type: corev1.NodeExternalDNS, Address: "node-a.example.com"}))
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_create_nodeport_external_dns", res, err))
	}
	{
		cr := baseCR()
		cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.SingleReplica
		e := newEnv()
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_create_nodeport_no_nodes", res, err))
	}
	{
		cr := baseCR()
		cr.Spec.ControlPlaneAvailabilityPolicy = hyperv1.SingleReplica
		e := newEnv(node("node-x", corev1.NodeAddress{Type: corev1.NodeHostName, Address: "node-x"}))
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_create_nodeport_no_address", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(ownedHC(cr, crUID, "dpf-hcp-pull-secret", "dpf-hcp-ssh-key"))
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_update_secret_refs", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(ownedHC(cr, crUID, hostedcluster.PullSecretCopyName(cr), hostedcluster.SSHKeyCopyName(cr)))
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_noop", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(ownedHC(cr, "other-uid", "a", "b"))
		res, err := hostedcluster.NewHostedClusterManager(e.client, scheme).CreateOrUpdateHostedCluster(ctx, cr)
		write(e.finish("hc_conflict", res, err))
	}

	// --- NodePoolManager.CreateNodePool -------------------------------------------------------
	{
		cr := baseCR()
		e := newEnv()
		res, err := hostedcluster.NewNodePoolManager(e.client, scheme).CreateNodePool(ctx, cr)
		write(e.finish("np_create", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(ownedNP(crUID))
		res, err := hostedcluster.NewNodePoolManager(e.client, scheme).CreateNodePool(ctx, cr)
		write(e.finish("np_noop", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(ownedNP("other-uid"))
		res, err := hostedcluster.NewNodePoolManager(e.client, scheme).CreateNodePool(ctx, cr)
		write(e.finish("np_conflict", res, err))
	}

	// --- SecretManager.CopySecrets -----------------------------------------------------------
	pullData := map[string][]byte{".dockerconfigjson": []byte(`{"auths":{"quay.io":{"auth":"Zm9vOmJhcg=="}}}`)}
	sshData := map[string][]byte{"id_rsa.pub": []byte("ssh-rsa AAAAB3NzaC1yc2E user@host")}
	copyLabel := "provisioning.dpu.hcp.io/credential-secret-copy"
	{
		cr := baseCR()
		e := newEnv(
			secret("my-pull-secret", "", nil, pullData),
			secret("my-ssh-key", "", nil, sshData),
			// legacy fixed-name copy (owned, no label) -> stale
			secret("dpf-hcp-pull-secret", crUID, nil, pullData),
			// old reference-encoded ssh copy (owned, labeled) -> stale
			secret("dpf-hcp-ssh-key-deadbeef", crUID, map[string]string{copyLabel: "ssh-key"}, sshData),
			// labeled but owned by someone else -> kept
			secret("dpf-hcp-pull-secret-00000000", "other-uid", map[string]string{copyLabel: "pull-secret"}, pullData),
			// owned but not a credential copy -> kept
			secret("dpf-hcp-etcd-encryption-key", crUID, nil, map[string][]byte{"key": []byte("k")}),
		)
		res, err := hostedcluster.NewSecretManager(e.client, scheme).CopySecrets(ctx, cr)
		write(e.finish("secrets_copy_create", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(
			secret("my-pull-secret", "", nil, pullData),
			secret("my-ssh-key", "", nil, sshData),
			secret(hostedcluster.PullSecretCopyName(cr), crUID, map[string]string{"other": "x"}, pullData),
			secret(hostedcluster.SSHKeyCopyName(cr), crUID, map[string]string{copyLabel: "ssh-key"}, sshData),
		)
		res, err := hostedcluster.NewSecretManager(e.client, scheme).CopySecrets(ctx, cr)
		write(e.finish("secrets_copy_relabel", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(
			secret("my-pull-secret", "", nil, pullData),
			secret("my-ssh-key", "", nil, sshData),
			secret(hostedcluster.SSHKeyCopyName(cr), "other-uid", nil, sshData),
		)
		res, err := hostedcluster.NewSecretManager(e.client, scheme).CopySecrets(ctx, cr)
		write(e.finish("secrets_copy_conflict", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(secret("my-ssh-key", "", nil, sshData))
		res, err := hostedcluster.NewSecretManager(e.client, scheme).CopySecrets(ctx, cr)
		write(e.finish("secrets_copy_missing_source", res, err))
	}

	// --- SecretManager.GenerateETCDEncryptionKey ---------------------------------------------
	{
		cr := baseCR()
		e := newEnv()
		res, err := hostedcluster.NewSecretManager(e.client, scheme).GenerateETCDEncryptionKey(ctx, cr)
		write(e.finish("etcd_key_create", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(secret("dpf-hcp-etcd-encryption-key", crUID, nil, map[string][]byte{"key": []byte("k")}))
		res, err := hostedcluster.NewSecretManager(e.client, scheme).GenerateETCDEncryptionKey(ctx, cr)
		write(e.finish("etcd_key_noop", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(secret("dpf-hcp-etcd-encryption-key", "other-uid", nil, map[string][]byte{"key": []byte("k")}))
		res, err := hostedcluster.NewSecretManager(e.client, scheme).GenerateETCDEncryptionKey(ctx, cr)
		write(e.finish("etcd_key_conflict", res, err))
	}

	// --- CleanupHandler.Cleanup (+ finalizer.Manager) ----------------------------------------
	{
		cr := baseCR()
		e := newEnv(ownedHC(cr, crUID, "a", "b"), ownedNP(crUID))
		res, err := hostedcluster.NewCleanupHandler(e.client, e.recorder).Cleanup(ctx, cr)
		write(e.finish("cleanup_hc_delete", res, err))
	}
	{
		cr := baseCR()
		np := ownedNP(crUID)
		np.Finalizers = []string{"hypershift.openshift.io/finalizer"}
		np.DeletionTimestamp = &metav1.Time{Time: time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC)}
		e := newEnv(ownedHC(cr, "other-uid", "a", "b"), np)
		res, err := hostedcluster.NewCleanupHandler(e.client, e.recorder).Cleanup(ctx, cr)
		write(e.finish("cleanup_np_deleting", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(
			secret("dpf-hcp-pull-secret", crUID, nil, pullData),
			secret("dpf-hcp-ssh-key", "other-uid", nil, sshData),
			secret("dpf-hcp-etcd-encryption-key", crUID, nil, map[string][]byte{"key": []byte("k")}),
		)
		res, err := hostedcluster.NewCleanupHandler(e.client, e.recorder).Cleanup(ctx, cr)
		write(e.finish("cleanup_secrets", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(secret("dpf-hcp-etcd-encryption-key", crUID, nil, map[string][]byte{"key": []byte("k")}))
		m := finalizer.NewManager(e.client, e.recorder)
		m.RegisterHandler(hostedcluster.NewCleanupHandler(e.client, e.recorder))
		res, err := m.HandleFinalizerCleanup(ctx, cr)
		write(e.finish("finalizer_manager_complete", res, err))
	}
	{
		cr := baseCR()
		e := newEnv(ownedHC(cr, crUID, "a", "b"))
		m := finalizer.NewManager(e.client, e.recorder)
		m.RegisterHandler(hostedcluster.NewCleanupHandler(e.client, e.recorder))
		res, err := m.HandleFinalizerCleanup(ctx, cr)
		write(e.finish("finalizer_manager_in_progress", res, err))
	}

	// --- StatusSyncer.SyncStatusFromHostedCluster --------------------------------------------
	{
		cr := baseCR()
		cr.Status.HostedClusterRef = &corev1.ObjectReference{Name: crName, Namespace: crNS}
		cr.Status.Conditions = []metav1.Condition{{
			Type: provisioningv1alpha1.HostedClusterAvailable, Status: metav1.ConditionFalse,
			Reason: "Old", Message: "old", LastTransitionTime: metav1.Now(), ObservedGeneration: 1,
		}}
		hc := ownedHC(cr, crUID, "a", "b")
		for i, t := range []string{
			string(hyperv1.HostedClusterAvailable), string(hyperv1.HostedClusterProgressing),
			string(hyperv1.HostedClusterDegraded), string(hyperv1.ValidReleaseInfo),
			string(hyperv1.ValidReleaseImage), string(hyperv1.IgnitionEndpointAvailable),
			string(hyperv1.IgnitionServerValidReleaseInfo), string(hyperv1.InfrastructureReady),
		} {
			hc.Status.Conditions = append(hc.Status.Conditions, metav1.Condition{
				Type: t, Status: metav1.ConditionTrue, Reason: fmt.Sprintf("Reason%d", i),
				Message: fmt.Sprintf("message %d", i), LastTransitionTime: metav1.Now(),
			})
		}
		e := newEnv(hc)
		res, err := hostedcluster.NewStatusSyncer(e.client).SyncStatusFromHostedCluster(ctx, cr)
		s := e.finish("status_sync", res, err)
		for _, c := range cr.Status.Conditions {
			s.Conditions = append(s.Conditions, condition{c.Type, string(c.Status), c.Reason, c.Message, c.ObservedGeneration})
		}
		// Go mirrors in map order; compare sorted by type.
		sort.Slice(s.Conditions, func(i, j int) bool { return s.Conditions[i].Type < s.Conditions[j].Type })
		write(s)
	}

	// --- PullSecretCopyName / SSHKeyCopyName ------------------------------------------------
	{
		type names struct {
			Ref  string `json:"ref"`
			Pull string `json:"pull"`
			SSH  string `json:"ssh"`
		}
		var out []names
		for _, ref := range []string{"", "a", "pull-secret", "my-pull-secret", "ssh-key", "ünïcödé", strings.Repeat("x", 253)} {
			cr := baseCR()
			cr.Spec.PullSecretRef.Name = ref
			cr.Spec.SSHKeySecretRef.Name = ref
			out = append(out, names{ref, hostedcluster.PullSecretCopyName(cr), hostedcluster.SSHKeyCopyName(cr)})
		}
		b, err := json.MarshalIndent(out, "", "  ")
		must(err)
		must(os.WriteFile(filepath.Join(fixtureDir, "copy_names.json"), append(b, '\n'), 0o644))
	}
}
