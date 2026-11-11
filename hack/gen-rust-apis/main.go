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

// Command gen-rust-apis is the post-processing stage of hack/gen-rust-apis.sh.
//
// kopium generates Rust structs (and their documentation) from the CRD schemas. CRD schemas
// do not carry the information needed to reproduce Go's encoding/json output exactly (Go
// field order, `omitempty`, pointer vs value fields, Go type names, Quantity vs IntOrString,
// custom marshalers), so this program re-shapes kopium's output against the upstream Go
// types using reflection:
//
//   - one Rust struct per Go struct type, named like the Go type, fields in Go order;
//   - serde attributes reproducing encoding/json (`omitempty`, nil -> null, null -> zero);
//   - documentation taken from the kopium output for the matching CRD field;
//   - `#[serde(flatten)] extra` on every struct to preserve unknown fields;
//   - Go constants of the API packages as `pub const SCREAMING_CASE`;
//   - `ctrlrt::impl_resource!` for every kind (group/plural/scope read from the CRD).
//
// It also cross-checks every Go field against the kopium (CRD) field and reports mismatches.
//
// Usage (from this module directory; see hack/gen-rust-apis.sh): go run . -crds DIR -kopium DIR -out DIR
//
// This is a standalone Go module: its go.mod pins the upstream API versions (doca-platform,
// hypershift, openshift/api, metallb, k8s.io/*) used both here and by controller-gen in
// hack/gen-rust-apis.sh and hack/gen-external-crds.sh.
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/constant"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"sort"
	"strings"
	"unicode"

	apiextensionsv1 "k8s.io/apiextensions-apiserver/pkg/apis/apiextensions/v1"
	"sigs.k8s.io/yaml"

	"golang.org/x/tools/go/packages"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	dpuoperatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	configv1 "github.com/openshift/api/config/v1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"
	corev1 "k8s.io/api/core/v1"
)

// root describes one kind that the operator uses.
type root struct {
	obj  any
	list any    // the matching <Kind>List type
	crd  string // CRD file name (in -crds dir)
}

var roots = []root{
	{&dpuprovisioningv1alpha1.DPU{}, &dpuprovisioningv1alpha1.DPUList{}, "provisioning.dpu.nvidia.com_dpus.yaml"},
	{&dpuprovisioningv1alpha1.DPUCluster{}, &dpuprovisioningv1alpha1.DPUClusterList{}, "provisioning.dpu.nvidia.com_dpuclusters.yaml"},
	{&dpuprovisioningv1alpha1.DPUFlavor{}, &dpuprovisioningv1alpha1.DPUFlavorList{}, "provisioning.dpu.nvidia.com_dpuflavors.yaml"},
	{&dpuservicev1alpha1.DPUDeployment{}, &dpuservicev1alpha1.DPUDeploymentList{}, "svc.dpu.nvidia.com_dpudeployments.yaml"},
	{&dpuservicev1alpha1.DPUServiceTemplate{}, &dpuservicev1alpha1.DPUServiceTemplateList{}, "svc.dpu.nvidia.com_dpuservicetemplates.yaml"},
	{&dpuoperatorv1alpha1.DPFOperatorConfig{}, &dpuoperatorv1alpha1.DPFOperatorConfigList{}, "operator.dpu.nvidia.com_dpfoperatorconfigs.yaml"},
	{&hyperv1.HostedCluster{}, &hyperv1.HostedClusterList{}, "hypershift.openshift.io_hostedclusters.yaml"},
	{&hyperv1.NodePool{}, &hyperv1.NodePoolList{}, "hypershift.openshift.io_nodepools.yaml"},
	{&metallbv1beta1.IPAddressPool{}, &metallbv1beta1.IPAddressPoolList{}, "metallb.io_ipaddresspools.yaml"},
	{&metallbv1beta1.L2Advertisement{}, &metallbv1beta1.L2AdvertisementList{}, "metallb.io_l2advertisements.yaml"},
	{&configv1.ClusterVersion{}, &configv1.ClusterVersionList{}, "config.openshift.io_clusterversions.yaml"},
}

// ownAPIEmbed is an external Go type embedded in this repository's own API
// (`apis::provisioning::v1alpha1`). The own API is hand-maintained in Rust
// (crates/apis/src/provisioning_types.rs) alongside its CRDs in config/crd/bases, so there
// are no Go types to walk; the external types it uses are listed here instead, in the
// order the own API references them. They are collected before the roots (this fixes
// their position in the generated modules) and documented from the matching kopium
// struct of the own CRD (kopium "" = collect only).
type ownAPIEmbed struct {
	typ    reflect.Type
	crd    string // own CRD file name (in -crds dir)
	kopium string // kopium struct for the field that embeds typ
}

var ownAPIEmbeds = []ownAPIEmbed{
	{reflect.TypeFor[corev1.LocalObjectReference](), "provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml", "DpfhcpProvisionerSshKeySecretRef"},
	{reflect.TypeFor[corev1.LocalObjectReference](), "provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml", "DpfhcpProvisionerPullSecretRef"},
	{reflect.TypeFor[hyperv1.AvailabilityPolicy](), "", ""},
	{reflect.TypeFor[hyperv1.ServiceNetworkEntry](), "provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml", "DpfhcpProvisionerNetworkingServiceNetwork"},
	{reflect.TypeFor[hyperv1.ClusterNetworkEntry](), "provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml", "DpfhcpProvisionerNetworkingClusterNetwork"},
	{reflect.TypeFor[hyperv1.MachineNetworkEntry](), "provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml", "DpfhcpProvisionerNetworkingMachineNetwork"},
	{reflect.TypeFor[corev1.ObjectReference](), "provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml", "DpfhcpProvisionerStatusHostedClusterRef"},
	{reflect.TypeFor[corev1.LocalObjectReference](), "provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml", "DpfhcpProvisionerStatusKubeConfigSecretRef"},
}

// module describes the Rust module generated for one Go package.
type module struct {
	file   string // generated/<file>.rs
	consts bool   // emit the package's exported constants
	doc    string
}

var modules = map[string]module{
	"github.com/nvidia/doca-platform/api/provisioning/v1alpha1":   {"dpf_provisioning_v1alpha1", true, "`github.com/nvidia/doca-platform/api/provisioning/v1alpha1`"},
	"github.com/nvidia/doca-platform/api/dpuservice/v1alpha1":     {"dpf_dpuservice_v1alpha1", true, "`github.com/nvidia/doca-platform/api/dpuservice/v1alpha1`"},
	"github.com/nvidia/doca-platform/api/operator/v1alpha1":       {"dpf_operator_v1alpha1", true, "`github.com/nvidia/doca-platform/api/operator/v1alpha1`"},
	"github.com/openshift/hypershift/api/hypershift/v1beta1":      {"hypershift_v1beta1", true, "`github.com/openshift/hypershift/api/hypershift/v1beta1`"},
	"go.universe.tf/metallb/api/v1beta1":                          {"metallb_v1beta1", true, "`go.universe.tf/metallb/api/v1beta1`"},
	"github.com/openshift/api/config/v1":                          {"openshift_config_v1", true, "`github.com/openshift/api/config/v1`"},
	"k8s.io/api/core/v1":                                          {"k8s_core_v1", false, "`k8s.io/api/core/v1` (Go-encoding mirrors of the types embedded in the CRDs above)"},
	"k8s.io/apimachinery/pkg/apis/meta/v1":                        {"k8s_meta_v1", false, "`k8s.io/apimachinery/pkg/apis/meta/v1` (Go-encoding mirrors of the types embedded in the CRDs above)"},
	"github.com/Mellanox/nic-configuration-operator/api/v1alpha1": {"nicconfig_v1alpha1", false, "`github.com/Mellanox/nic-configuration-operator/api/v1alpha1` (types embedded in DPUFlavor)"},
}

// Special (hand-written) leaf types.
type special int

const (
	spNone special = iota
	spTime
	spObjectMeta
	spCondition
	spDuration
	spQuantity
	spIntOrString
	spRawJSON
	spIPNet
	spListMeta
)

var specials = map[string]special{
	"k8s.io/apimachinery/pkg/apis/meta/v1.Time":                     spTime,
	"k8s.io/apimachinery/pkg/apis/meta/v1.ObjectMeta":               spObjectMeta,
	"k8s.io/apimachinery/pkg/apis/meta/v1.Condition":                spCondition,
	"k8s.io/apimachinery/pkg/apis/meta/v1.Duration":                 spDuration,
	"k8s.io/apimachinery/pkg/api/resource.Quantity":                 spQuantity,
	"k8s.io/apimachinery/pkg/util/intstr.IntOrString":               spIntOrString,
	"k8s.io/apimachinery/pkg/runtime.RawExtension":                  spRawJSON,
	"k8s.io/apiextensions-apiserver/pkg/apis/apiextensions/v1.JSON": spRawJSON,
	"github.com/openshift/hypershift/api/util/ipnet.IPNet":          spIPNet,
	"k8s.io/apimachinery/pkg/apis/meta/v1.ListMeta":                 spListMeta,
}

func fullName(t reflect.Type) string { return t.PkgPath() + "." + t.Name() }

func specialOf(t reflect.Type) special {
	if t.Name() == "" {
		return spNone
	}
	return specials[fullName(t)]
}

var (
	jsonMarshaler = reflect.TypeOf((*json.Marshaler)(nil)).Elem()
	textMarshaler = reflect.TypeOf((*interface{ MarshalText() ([]byte, error) })(nil)).Elem()
	jsonUnmarsh   = reflect.TypeOf((*json.Unmarshaler)(nil)).Elem()
)

func hasCustomEncoding(t reflect.Type) bool {
	pt := reflect.PointerTo(t)
	return t.Implements(jsonMarshaler) || pt.Implements(jsonMarshaler) ||
		t.Implements(textMarshaler) || pt.Implements(textMarshaler) || pt.Implements(jsonUnmarsh)
}

// goField is one JSON-visible field of a Go struct (embedded structs expanded).
type goField struct {
	goName    string // Go field name (dotted path for promoted fields)
	jsonName  string
	omitempty bool
	omitzero  bool
	typ       reflect.Type
}

func jsonFields(t reflect.Type) []goField {
	var out []goField
	seen := map[string]bool{}
	var walk func(t reflect.Type, prefix string)
	walk = func(t reflect.Type, prefix string) {
		for i := 0; i < t.NumField(); i++ {
			f := t.Field(i)
			tag := f.Tag.Get("json")
			if tag == "-" {
				continue
			}
			name, opts, _ := strings.Cut(tag, ",")
			if f.Anonymous && name == "" {
				ft := f.Type
				if ft.Kind() == reflect.Pointer {
					panic(fmt.Sprintf("%s: embedded pointer %s not supported", t, f.Name))
				}
				if ft.Kind() != reflect.Struct {
					panic(fmt.Sprintf("%s: embedded non-struct %s not supported", t, f.Name))
				}
				if hasCustomEncoding(ft) {
					panic(fmt.Sprintf("%s: embedded custom-encoded %s not supported", t, f.Name))
				}
				walk(ft, prefix+f.Name+".")
				continue
			}
			if !f.IsExported() {
				continue
			}
			if name == "" {
				name = f.Name
			}
			if seen[name] {
				panic(fmt.Sprintf("%s: duplicate JSON field %q", t, name))
			}
			seen[name] = true
			out = append(out, goField{
				goName:    prefix + f.Name,
				jsonName:  name,
				omitempty: strings.Contains(","+opts+",", ",omitempty,"),
				omitzero:  strings.Contains(","+opts+",", ",omitzero,"),
				typ:       f.Type,
			})
		}
	}
	walk(t, "")
	return out
}

// ---------------------------------------------------------------------------------------
// kopium output parsing (documentation source + schema cross-check)
// ---------------------------------------------------------------------------------------

type kField struct {
	json     string
	rustType string
	doc      []string
}

type kStruct struct {
	name   string
	doc    []string
	fields map[string]*kField
	order  []string
}

type kFile map[string]*kStruct

var (
	reStruct = regexp.MustCompile(`^pub struct (\w+) \{`)
	reEnum   = regexp.MustCompile(`^pub enum (\w+) \{`)
	reField  = regexp.MustCompile(`^    pub (r#)?(\w+): (.+),$`)
	reRename = regexp.MustCompile(`rename = "([^"]+)"`)
)

func parseKopium(path string) kFile {
	f, err := os.Open(path)
	if err != nil {
		panic(err)
	}
	defer f.Close()
	out := kFile{}
	var doc []string
	var cur *kStruct
	inEnum := false
	rename := ""
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<24)
	for sc.Scan() {
		line := sc.Text()
		trim := strings.TrimSpace(line)
		switch {
		case strings.HasPrefix(trim, "///"):
			d := strings.TrimPrefix(trim, "///")
			d = strings.TrimPrefix(d, " ")
			doc = append(doc, strings.TrimRight(d, " \t"))
		case reStruct.MatchString(line):
			m := reStruct.FindStringSubmatch(line)
			cur = &kStruct{name: m[1], doc: doc, fields: map[string]*kField{}}
			out[m[1]] = cur
			doc = nil
		case reEnum.MatchString(line):
			inEnum = true
			doc = nil
		case line == "}":
			cur = nil
			inEnum = false
			doc = nil
		case inEnum:
			if !strings.HasPrefix(trim, "#") {
				doc = nil
			}
		case cur != nil && strings.HasPrefix(trim, "#[serde("):
			rename = ""
			if m := reRename.FindStringSubmatch(trim); m != nil {
				rename = m[1]
			}
		case cur != nil && reField.MatchString(line):
			m := reField.FindStringSubmatch(line)
			name := m[2]
			if rename != "" {
				name = rename
			}
			cur.fields[name] = &kField{json: name, rustType: m[3], doc: doc}
			cur.order = append(cur.order, name)
			doc = nil
			rename = ""
		case strings.HasPrefix(trim, "#["):
		default:
			doc = nil
		}
	}
	return out
}

// ---------------------------------------------------------------------------------------
// Type collection
// ---------------------------------------------------------------------------------------

type structInfo struct {
	t      reflect.Type
	doc    []string
	fdoc   map[string][]string
	kindOf *kindInfo // set for roots
}

type kindInfo struct {
	group, version, kind, plural string
	namespaced                   bool
}

type gen struct {
	structs  map[reflect.Type]*structInfo
	order    map[string][]reflect.Type // per Go package, discovery order
	aliases  map[string]map[string]reflect.Type
	warnings []string
	walked   map[string]bool
}

func (g *gen) warn(format string, a ...any) {
	g.warnings = append(g.warnings, fmt.Sprintf(format, a...))
}

func hasMod(p string) bool {
	_, ok := modules[p]
	return ok || strings.HasPrefix(p, "k8s.io/api/") || strings.HasPrefix(p, "github.com/openshift/api/")
}

func modFor(t reflect.Type) module {
	m, ok := modules[t.PkgPath()]
	if !ok {
		// Packages only reached through the API types (e.g. openshift operator/v1 types used
		// by hypershift): derive a module name from the import path.
		p := t.PkgPath()
		name := ""
		switch {
		case strings.HasPrefix(p, "k8s.io/api/"):
			name = "k8s_" + strings.TrimPrefix(p, "k8s.io/api/")
		case strings.HasPrefix(p, "github.com/openshift/api/"):
			name = "openshift_" + strings.TrimPrefix(p, "github.com/openshift/api/")
		default:
			panic(fmt.Sprintf("no Rust module mapping for Go package %q (type %s)", p, t))
		}
		name = strings.NewReplacer("/", "_", ".", "_", "-", "_").Replace(name)
		m = module{file: name, doc: "`" + p + "` (types embedded in the CRDs above)"}
		modules[p] = m
	}
	return m
}

// collect registers t (a named struct) and everything reachable from it.
func (g *gen) collect(t reflect.Type) {
	switch t.Kind() {
	case reflect.Pointer, reflect.Slice, reflect.Array:
		g.collect(t.Elem())
		return
	case reflect.Map:
		if t.Key().Kind() != reflect.String {
			panic(fmt.Sprintf("map key %s not supported", t))
		}
		g.collectNamed(t.Key())
		g.collect(t.Elem())
		return
	case reflect.Struct:
	default:
		g.collectNamed(t)
		return
	}
	if specialOf(t) != spNone {
		return
	}
	if t.Name() == "" {
		panic(fmt.Sprintf("anonymous struct %s not supported", t))
	}
	if hasCustomEncoding(t) {
		panic(fmt.Sprintf("type %s has a custom JSON encoding; add it to specials", fullName(t)))
	}
	if _, ok := g.structs[t]; ok {
		return
	}
	modFor(t)
	g.structs[t] = &structInfo{t: t, fdoc: map[string][]string{}}
	g.order[t.PkgPath()] = append(g.order[t.PkgPath()], t)
	for _, f := range jsonFields(t) {
		g.collect(f.typ)
	}
}

// collectNamed records named scalar types (string enums etc.) as aliases.
func (g *gen) collectNamed(t reflect.Type) {
	if t.Name() == "" || t.PkgPath() == "" {
		return
	}
	switch t.Kind() {
	case reflect.String, reflect.Bool, reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Float32, reflect.Float64:
	case reflect.Interface:
		return
	default:
		panic(fmt.Sprintf("named type %s of kind %s not supported", fullName(t), t.Kind()))
	}
	if hasCustomEncoding(t) {
		panic(fmt.Sprintf("type %s has a custom JSON encoding; add it to specials", fullName(t)))
	}
	if !hasMod(t.PkgPath()) {
		return // named scalar from e.g. apimachinery (types.UID): use the plain Rust type
	}
	modFor(t)
	if g.aliases[t.PkgPath()] == nil {
		g.aliases[t.PkgPath()] = map[string]reflect.Type{}
	}
	g.aliases[t.PkgPath()][t.Name()] = t
}

// structElem returns the named generated struct reachable from t (through pointer/slice/map).
func structElem(t reflect.Type) reflect.Type {
	for {
		switch t.Kind() {
		case reflect.Pointer, reflect.Slice, reflect.Array, reflect.Map:
			t = t.Elem()
		case reflect.Struct:
			if specialOf(t) != spNone {
				return nil
			}
			return t
		default:
			return nil
		}
	}
}

var reIdent = regexp.MustCompile(`[A-Za-z_][A-Za-z0-9_]*`)

// docWalk attaches kopium documentation to Go struct t, walking kopium struct ks in parallel.
func (g *gen) docWalk(t reflect.Type, ks *kStruct, kf kFile, file string) {
	key := file + "|" + t.String() + "|" + ks.name
	if g.walked[key] {
		return
	}
	g.walked[key] = true
	si := g.structs[t]
	if len(si.doc) == 0 {
		si.doc = ks.doc
	}
	gfs := jsonFields(t)
	seen := map[string]bool{}
	for _, f := range gfs {
		seen[f.jsonName] = true
		kfld := ks.fields[f.jsonName]
		if kfld == nil {
			if f.jsonName != "apiVersion" && f.jsonName != "kind" && f.jsonName != "metadata" {
				g.warn("%s: Go field %s.%s (json %q) not present in CRD schema (kopium struct %s)", file, t, f.goName, f.jsonName, ks.name)
			}
			continue
		}
		if _, ok := si.fdoc[f.jsonName]; !ok && len(kfld.doc) > 0 {
			si.fdoc[f.jsonName] = kfld.doc
		}
		et := structElem(f.typ)
		if et == nil {
			continue
		}
		var nested *kStruct
		for _, id := range reIdent.FindAllString(kfld.rustType, -1) {
			if s, ok := kf[id]; ok {
				nested = s
				break
			}
		}
		if nested == nil {
			if !strings.Contains(kfld.rustType, "serde_json::Value") && !strings.Contains(kfld.rustType, "BTreeMap<String, String>") {
				g.warn("%s: no kopium struct for %s.%s (kopium type %s)", file, t, f.goName, kfld.rustType)
			}
			continue
		}
		g.docWalk(et, nested, kf, file)
	}
	for _, name := range ks.order {
		if !seen[name] {
			g.warn("%s: CRD field %s.%s not present in Go type %s", file, ks.name, name, t)
		}
	}
}

// ---------------------------------------------------------------------------------------
// Rust emission
// ---------------------------------------------------------------------------------------

func isUpper(r byte) bool { return r >= 'A' && r <= 'Z' }
func isLower(r byte) bool { return r >= 'a' && r <= 'z' }
func isDigit(r byte) bool { return r >= '0' && r <= '9' }

// words splits a Go/JSON identifier into words (acronym aware).
func words(s string) []string {
	var out []string
	start := 0
	for i := 1; i < len(s); i++ {
		p, c := s[i-1], s[i]
		split := false
		if c == '_' || c == '-' || c == '.' {
			out = append(out, s[start:i])
			start = i + 1
			i++
			continue
		}
		if isUpper(c) && (isLower(p) || isDigit(p)) {
			split = true
		}
		if isUpper(c) && isUpper(p) && i+1 < len(s) && isLower(s[i+1]) {
			split = true
			// plural acronym: "CIDRs", "IPs" (an `s` ending the word)
			if s[i+1] == 's' && (i+2 == len(s) || !isLower(s[i+2])) {
				split = false
			}
		}
		if split {
			out = append(out, s[start:i])
			start = i
		}
	}
	if start < len(s) {
		out = append(out, s[start:])
	}
	var res []string
	for _, w := range out {
		if w == "" {
			continue
		}
		// re-join compound words the generic rules split ("O"+"Auth", "I"+"Pv4", ...)
		if n := len(res); n > 0 && compoundWords[res[n-1]+w] {
			res[n-1] += w
			continue
		}
		res = append(res, w)
	}
	return res
}

var compoundWords = map[string]bool{
	"OAuth": true, "IPv4": true, "IPv6": true, "QoS": true, "VSphere": true, "S390X": true,
}

func snake(s string) string { return strings.ToLower(strings.Join(words(s), "_")) }

func screaming(s string) string { return strings.ToUpper(strings.Join(words(s), "_")) }

var rustKeywords = map[string]bool{
	"as": true, "break": true, "const": true, "continue": true, "crate": true, "else": true, "enum": true,
	"extern": true, "false": true, "fn": true, "for": true, "if": true, "impl": true, "in": true, "let": true,
	"loop": true, "match": true, "mod": true, "move": true, "mut": true, "pub": true, "ref": true,
	"return": true, "self": true, "Self": true, "static": true, "struct": true, "super": true, "trait": true,
	"true": true, "type": true, "unsafe": true, "use": true, "where": true, "while": true, "async": true,
	"await": true, "dyn": true, "abstract": true, "become": true, "box": true, "do": true, "final": true,
	"macro": true, "override": true, "priv": true, "typeof": true, "unsized": true, "virtual": true,
	"yield": true, "try": true, "gen": true,
}

func rustField(json string) string {
	n := snake(json)
	if rustKeywords[n] || n == "extra" { // `extra` is reserved for unknown fields
		return n + "_"
	}
	return n
}

// typePath returns the Rust path of named Go type t as seen from module `from`.
func typePath(t reflect.Type, from string) string {
	m := modFor(t)
	if m.file == from {
		return t.Name()
	}
	return "crate::generated::" + m.file + "::" + t.Name()
}

func scalarRust(k reflect.Kind) string {
	switch k {
	case reflect.String:
		return "String"
	case reflect.Bool:
		return "bool"
	case reflect.Int, reflect.Int64:
		return "i64"
	case reflect.Int32:
		return "i32"
	case reflect.Int16:
		return "i16"
	case reflect.Int8:
		return "i8"
	case reflect.Uint, reflect.Uint64:
		return "u64"
	case reflect.Uint32:
		return "u32"
	case reflect.Uint16:
		return "u16"
	case reflect.Uint8:
		return "u8"
	case reflect.Float64:
		return "f64"
	case reflect.Float32:
		return "f32"
	}
	return ""
}

type rtype struct {
	rust    string
	option  bool // top-level Rust Option
	goSerde bool // needs crate::go::{ser,de}
	// zeroSkippable: Go omitempty can omit this value (non-struct)
	zeroSkippable bool
}

// rustType maps a Go type to its Rust representation. `stack` holds the structs being
// emitted (to box recursive pointers).
func (g *gen) rustType(t reflect.Type, from string, owner reflect.Type) rtype {
	switch t.Kind() {
	case reflect.Pointer:
		e := t.Elem()
		inner := g.rustType(e, from, owner)
		if inner.option {
			// *metav1.Time is Option<Time> already.
			inner.zeroSkippable = true
			return inner
		}
		r := inner.rust
		if e == owner {
			r = "Box<" + r + ">"
		}
		return rtype{rust: "Option<" + r + ">", option: true, goSerde: inner.goSerde, zeroSkippable: true}
	case reflect.Slice, reflect.Array:
		if t.Elem().Kind() == reflect.Uint8 {
			panic(fmt.Sprintf("[]byte field (%s) not supported", t))
		}
		inner := g.rustType(t.Elem(), from, owner)
		return rtype{rust: "Vec<" + inner.rust + ">", goSerde: inner.goSerde, zeroSkippable: true}
	case reflect.Map:
		inner := g.rustType(t.Elem(), from, owner)
		return rtype{rust: "BTreeMap<String, " + inner.rust + ">", goSerde: inner.goSerde, zeroSkippable: true}
	case reflect.Interface:
		return rtype{rust: "serde_json::Value", zeroSkippable: true}
	case reflect.Struct:
		switch specialOf(t) {
		case spTime:
			return rtype{rust: "Option<Time>", option: true, goSerde: true}
		case spObjectMeta:
			return rtype{rust: "ObjectMeta", goSerde: true}
		case spListMeta:
			return rtype{rust: "ListMeta", goSerde: true}
		case spCondition:
			return rtype{rust: "Condition", goSerde: true}
		case spDuration:
			return rtype{rust: "crate::go::Duration"}
		case spQuantity:
			return rtype{rust: "Quantity", goSerde: true}
		case spIntOrString:
			return rtype{rust: "IntOrString"}
		case spRawJSON:
			return rtype{rust: "crate::go::Json"}
		case spIPNet:
			return rtype{rust: "crate::ipnet::IPNet"}
		}
		return rtype{rust: typePath(t, from)}
	}
	s := scalarRust(t.Kind())
	if s == "" {
		panic(fmt.Sprintf("unsupported Go kind %s (%s)", t.Kind(), t))
	}
	if t.Name() != "" && t.PkgPath() != "" && hasMod(t.PkgPath()) {
		s = typePath(t, from)
	}
	return rtype{rust: s, zeroSkippable: true}
}

func writeDoc(b *strings.Builder, indent string, doc []string) {
	// trim trailing empty lines
	for len(doc) > 0 && strings.TrimSpace(doc[len(doc)-1]) == "" {
		doc = doc[:len(doc)-1]
	}
	for _, d := range doc {
		if d == "" {
			fmt.Fprintf(b, "%s///\n", indent)
		} else {
			fmt.Fprintf(b, "%s/// %s\n", indent, d)
		}
	}
}

func (g *gen) emitStruct(b *strings.Builder, si *structInfo, from string) {
	t := si.t
	writeDoc(b, "", si.doc)
	if len(si.doc) > 0 {
		b.WriteString("///\n")
	}
	fmt.Fprintf(b, "/// Go: `%s.%s`\n", t.PkgPath(), t.Name())
	b.WriteString("#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]\n")
	b.WriteString("#[serde(default)]\n")
	fmt.Fprintf(b, "pub struct %s {\n", t.Name())
	used := map[string]bool{"extra": true}
	for _, f := range jsonFields(t) {
		rt := g.rustType(f.typ, from, t)
		if !f.omitempty && (f.typ.Kind() == reflect.Slice || f.typ.Kind() == reflect.Map) {
			// Go encodes a nil slice/map without omitempty as `null`: keep nil vs empty apart.
			rt.rust = "Option<" + rt.rust + ">"
			rt.option = true
		}
		name := rustField(f.jsonName)
		if used[name] {
			panic(fmt.Sprintf("%s: duplicate Rust field name %s", t, name))
		}
		used[name] = true
		doc := si.fdoc[f.jsonName]
		writeDoc(b, "    ", doc)
		if len(doc) > 0 {
			b.WriteString("    ///\n")
		}
		tag := "`" + f.jsonName
		if f.omitempty {
			tag += ",omitempty"
		}
		if f.omitzero {
			tag += ",omitzero"
		}
		tag += "`"
		fmt.Fprintf(b, "    /// Go: `%s %s` json:%s\n", f.goName, goTypeString(f.typ), tag)
		var attrs []string
		if name != f.jsonName {
			attrs = append(attrs, fmt.Sprintf("rename = %q", f.jsonName))
		}
		switch {
		case (f.omitempty || f.omitzero) && rt.zeroSkippable:
			attrs = append(attrs, `skip_serializing_if = "crate::go::is_empty"`)
		case f.omitzero && specialOf(f.typ) == spTime:
			// Go 1.24 `omitzero`: metav1.Time has IsZero()
			attrs = append(attrs, `skip_serializing_if = "crate::go::is_zero_time"`)
		case f.omitzero && f.typ.Kind() == reflect.Struct && specialOf(f.typ) == spNone:
			// Go 1.24 `omitzero` on a struct: omitted when every field is zero
			attrs = append(attrs, `skip_serializing_if = "crate::go::is_default"`)
		case f.omitzero:
			panic(fmt.Sprintf("%s.%s: omitzero on %s not supported", t, f.goName, f.typ))
		}
		if rt.goSerde {
			attrs = append(attrs, `serialize_with = "crate::go::ser"`, `deserialize_with = "crate::go::de"`)
		} else if !rt.option {
			attrs = append(attrs, `deserialize_with = "crate::go::null_default"`)
		}
		if len(attrs) > 0 {
			fmt.Fprintf(b, "    #[serde(%s)]\n", strings.Join(attrs, ", "))
		}
		fmt.Fprintf(b, "    pub %s: %s,\n", name, rt.rust)
	}
	b.WriteString("    /// Fields unknown to this version of the API, preserved on read-modify-write.\n")
	b.WriteString("    #[serde(flatten)]\n")
	b.WriteString("    pub extra: serde_json::Map<String, serde_json::Value>,\n")
	b.WriteString("}\n\n")
	if k := si.kindOf; k != nil {
		scope := "Namespaced"
		if !k.namespaced {
			scope = "Cluster"
		}
		fmt.Fprintf(b, "ctrlrt::impl_resource!(%s, %q, %q, %q, %q, %s);\n\n", t.Name(), k.group, k.version, k.kind, k.plural, scope)
	}
}

func goTypeString(t reflect.Type) string {
	s := t.String()
	return strings.ReplaceAll(s, "`", "")
}

// ---------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------

type goConst struct {
	name  string
	value string // Rust literal
	rtype string
	doc   []string
	gtype string
}

func loadConsts(pkgPaths []string) map[string][]goConst {
	cfg := &packages.Config{Mode: packages.NeedName | packages.NeedTypes | packages.NeedSyntax | packages.NeedTypesInfo}
	pkgs, err := packages.Load(cfg, pkgPaths...)
	if err != nil {
		panic(err)
	}
	out := map[string][]goConst{}
	for _, p := range pkgs {
		if len(p.Errors) > 0 {
			panic(fmt.Sprintf("%s: %v", p.PkgPath, p.Errors))
		}
		docs := map[string][]string{}
		varVals := map[string]constant.Value{} // exported vars initialized with constants
		for _, f := range p.Syntax {
			for _, d := range f.Decls {
				gd, ok := d.(*ast.GenDecl)
				if !ok {
					continue
				}
				for _, s := range gd.Specs {
					vs, ok := s.(*ast.ValueSpec)
					if !ok {
						continue
					}
					if gd.Tok == token.VAR && len(vs.Values) == len(vs.Names) {
						for i, n := range vs.Names {
							if tv, ok := p.TypesInfo.Types[vs.Values[i]]; ok && tv.Value != nil {
								varVals[n.Name] = tv.Value
							}
						}
					}
					var cg *ast.CommentGroup = vs.Doc
					if cg == nil && len(gd.Specs) == 1 {
						cg = gd.Doc
					}
					if cg == nil {
						continue
					}
					text := strings.TrimRight(cg.Text(), "\n")
					for _, n := range vs.Names {
						docs[n.Name] = strings.Split(text, "\n")
					}
				}
			}
		}
		scope := p.Types.Scope()
		for _, name := range scope.Names() {
			obj := scope.Lookup(name)
			if !obj.Exported() {
				continue
			}
			var v constant.Value
			switch o := obj.(type) {
			case *types.Const:
				v = o.Val()
			case *types.Var:
				// Go "constants" declared as package vars (e.g. hyperv1.APIServer).
				v = varVals[name]
			}
			if v == nil {
				continue
			}
			gc := goConst{name: name, doc: docs[name], gtype: obj.Type().String()}
			if _, isVar := obj.(*types.Var); isVar {
				gc.gtype = "var " + gc.gtype
			}
			basic, _ := obj.Type().Underlying().(*types.Basic)
			if basic == nil {
				continue
			}
			switch {
			case basic.Info()&types.IsString != 0:
				gc.rtype = "&str"
				gc.value = rustString(constant.StringVal(v))
			case basic.Info()&types.IsInteger != 0:
				gc.rtype = "i64"
				switch basic.Kind() {
				case types.Int32:
					gc.rtype = "i32"
				case types.Int16:
					gc.rtype = "i16"
				case types.Int8:
					gc.rtype = "i8"
				case types.Uint8:
					gc.rtype = "u8"
				case types.Uint16:
					gc.rtype = "u16"
				case types.Uint32:
					gc.rtype = "u32"
				case types.Uint64, types.Uint:
					gc.rtype = "u64"
				}
				gc.value = v.ExactString()
			case basic.Info()&types.IsBoolean != 0:
				gc.rtype = "bool"
				gc.value = v.ExactString()
			case basic.Info()&types.IsFloat != 0:
				gc.rtype = "f64"
				f, _ := constant.Float64Val(v)
				gc.value = fmt.Sprintf("%v_f64", f)
			default:
				continue
			}
			out[p.PkgPath] = append(out[p.PkgPath], gc)
		}
	}
	return out
}

func rustString(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\n':
			b.WriteString(`\n`)
		case '\t':
			b.WriteString(`\t`)
		case '\r':
			b.WriteString(`\r`)
		default:
			if r < 0x20 || !unicode.IsPrint(r) {
				fmt.Fprintf(&b, `\u{%x}`, r)
			} else {
				b.WriteRune(r)
			}
		}
	}
	b.WriteByte('"')
	return b.String()
}

// ---------------------------------------------------------------------------------------

func main() {
	crdDir := flag.String("crds", "", "directory with CRD YAML files")
	kopiumDir := flag.String("kopium", "", "directory with kopium output (<crd file>.rs)")
	outDir := flag.String("out", "", "output directory (crates/apis/src/generated)")
	flag.Parse()
	if *crdDir == "" || *kopiumDir == "" || *outDir == "" {
		flag.Usage()
		os.Exit(2)
	}

	g := &gen{
		structs: map[reflect.Type]*structInfo{},
		order:   map[string][]reflect.Type{},
		aliases: map[string]map[string]reflect.Type{},
		walked:  map[string]bool{},
	}
	for _, e := range ownAPIEmbeds {
		g.collect(e.typ)
	}
	for _, e := range ownAPIEmbeds {
		if e.kopium == "" {
			continue
		}
		kf := parseKopium(filepath.Join(*kopiumDir, strings.TrimSuffix(e.crd, ".yaml")+".rs"))
		ks := kf[e.kopium]
		if ks == nil {
			panic(fmt.Sprintf("%s: no kopium struct %s for %s", e.crd, e.kopium, e.typ))
		}
		g.docWalk(e.typ, ks, kf, e.crd)
	}
	for _, r := range roots {
		t := reflect.TypeOf(r.obj).Elem()
		g.collect(t)
		lt := reflect.TypeOf(r.list).Elem()
		g.collect(lt)
		g.structs[lt].doc = []string{t.Name() + "List is a list of " + t.Name() + "."}

		raw, err := os.ReadFile(filepath.Join(*crdDir, r.crd))
		if err != nil {
			panic(err)
		}
		crd := &apiextensionsv1.CustomResourceDefinition{}
		if err := yaml.Unmarshal(raw, crd); err != nil {
			panic(err)
		}
		if crd.Spec.Names.Kind != t.Name() {
			panic(fmt.Sprintf("%s: kind %s != Go type %s", r.crd, crd.Spec.Names.Kind, t.Name()))
		}
		gv := ""
		for _, v := range crd.Spec.Versions {
			if strings.HasSuffix(t.PkgPath(), "/"+v.Name) {
				gv = v.Name
				if v.Schema != nil && v.Schema.OpenAPIV3Schema != nil {
					g.structs[t].doc = strings.Split(strings.TrimRight(v.Schema.OpenAPIV3Schema.Description, "\n"), "\n")
				}
			}
		}
		if gv == "" {
			panic(fmt.Sprintf("%s: no CRD version matching package %s", r.crd, t.PkgPath()))
		}
		g.structs[t].kindOf = &kindInfo{
			group: crd.Spec.Group, version: gv, kind: crd.Spec.Names.Kind,
			plural: crd.Spec.Names.Plural, namespaced: crd.Spec.Scope == apiextensionsv1.NamespaceScoped,
		}

		kf := parseKopium(filepath.Join(*kopiumDir, strings.TrimSuffix(r.crd, ".yaml")+".rs"))
		for _, f := range jsonFields(t) {
			st := structElem(f.typ)
			if st == nil {
				continue
			}
			var ks *kStruct
			for name, s := range kf {
				if strings.EqualFold(name, t.Name()+f.jsonName) {
					ks = s
				}
			}
			if ks == nil {
				g.warn("%s: no kopium root struct for %s.%s", r.crd, t.Name(), f.jsonName)
				continue
			}
			if len(g.structs[t].fdoc[f.jsonName]) == 0 {
				g.structs[t].fdoc[f.jsonName] = ks.doc
			}
			g.docWalk(st, ks, kf, r.crd)
		}
	}

	var constPkgs []string
	for p, m := range modules {
		if m.consts {
			constPkgs = append(constPkgs, p)
		}
	}
	sort.Strings(constPkgs)
	consts := loadConsts(constPkgs)

	if err := os.MkdirAll(*outDir, 0o755); err != nil {
		panic(err)
	}
	var files []string
	var pkgs []string
	for p := range modules {
		pkgs = append(pkgs, p)
	}
	sort.Strings(pkgs)
	for _, p := range pkgs {
		m := modules[p]
		if len(g.order[p]) == 0 && len(consts[p]) == 0 {
			continue
		}
		files = append(files, m.file)
		var b strings.Builder
		b.WriteString("// Code generated by hack/gen-rust-apis.sh (kopium + hack/gen-rust-apis). DO NOT EDIT.\n\n")
		fmt.Fprintf(&b, "//! Generated mirror of %s.\n\n", m.doc)
		b.WriteString("#![allow(unused_imports, clippy::all, rustdoc::all)]\n\n")
		b.WriteString("use std::collections::BTreeMap;\n\n")
		b.WriteString("use k8s_openapi::apimachinery::pkg::api::resource::Quantity;\n")
		b.WriteString("use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ListMeta, ObjectMeta, Time};\n")
		b.WriteString("use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;\n")
		b.WriteString("use serde::{Deserialize, Serialize};\n\n")

		// aliases
		var aliasNames []string
		for n := range g.aliases[p] {
			aliasNames = append(aliasNames, n)
		}
		sort.Strings(aliasNames)
		for _, n := range aliasNames {
			t := g.aliases[p][n]
			fmt.Fprintf(&b, "/// Go: `type %s %s`\npub type %s = %s;\n", n, t.Kind(), n, scalarRust(t.Kind()))
		}
		if len(aliasNames) > 0 {
			b.WriteString("\n")
		}

		// constants
		if cs := consts[p]; len(cs) > 0 {
			names := map[string]string{}
			for _, c := range cs {
				rn := screaming(c.name)
				if prev, ok := names[rn]; ok {
					panic(fmt.Sprintf("%s: constants %s and %s both map to %s", p, prev, c.name, rn))
				}
				names[rn] = c.name
				writeDoc(&b, "", c.doc)
				if len(c.doc) > 0 {
					b.WriteString("///\n")
				}
				fmt.Fprintf(&b, "/// Go: `%s` (`%s`)\n", c.name, c.gtype)
				fmt.Fprintf(&b, "pub const %s: %s = %s;\n", rn, c.rtype, c.value)
			}
			b.WriteString("\n")
		}

		for _, t := range g.order[p] {
			g.emitStruct(&b, g.structs[t], m.file)
		}
		if err := os.WriteFile(filepath.Join(*outDir, m.file+".rs"), []byte(b.String()), 0o644); err != nil {
			panic(err)
		}
	}

	var b strings.Builder
	b.WriteString("// Code generated by hack/gen-rust-apis.sh. DO NOT EDIT.\n\n")
	b.WriteString("//! Types generated from the CRDs (kopium) and the vendored Go API types.\n")
	b.WriteString("//! Use them through the re-exporting modules at the crate root.\n\n")
	sort.Strings(files)
	for _, f := range files {
		fmt.Fprintf(&b, "pub mod %s;\n", f)
	}
	if err := os.WriteFile(filepath.Join(*outDir, "mod.rs"), []byte(b.String()), 0o644); err != nil {
		panic(err)
	}

	sort.Strings(g.warnings)
	var wb strings.Builder
	wb.WriteString("# Schema cross-check report (Go types vs CRD/kopium). Generated; do not edit.\n")
	for _, w := range g.warnings {
		wb.WriteString(w + "\n")
	}
	if err := os.WriteFile(filepath.Join(*outDir, "crosscheck.txt"), []byte(wb.String()), 0o644); err != nil {
		panic(err)
	}
	fmt.Fprintf(os.Stderr, "generated %d modules, %d structs, %d cross-check notes\n", len(files), len(g.structs), len(g.warnings))
}
