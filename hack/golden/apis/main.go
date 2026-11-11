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

// Command apis generates the golden fixtures of crates/apis (Go encoding of every API type).
//
// For each type T: T.zero.json (json.Marshal(&T{})), T.filled.json (json.Marshal of a value
// filled recursively with deterministic non-zero data) and T.roundtrip.json
// (Unmarshal(filled) -> Marshal). Plus DPUFlavor.filled.yaml (sigs.k8s.io/yaml.Marshal of the
// cleaned DPUFlavor as embedded in ignition), and quantity/duration/ipnet tables.
//
// Usage: go run -mod=vendor ./hack/golden/apis [-out crates/apis/tests/fixtures]
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"time"

	dpuservicev1alpha1 "github.com/nvidia/doca-platform/api/dpuservice/v1alpha1"
	dpuoperatorv1alpha1 "github.com/nvidia/doca-platform/api/operator/v1alpha1"
	dpuprovisioningv1alpha1 "github.com/nvidia/doca-platform/api/provisioning/v1alpha1"
	configv1 "github.com/openshift/api/config/v1"
	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
	"github.com/openshift/hypershift/api/util/ipnet"
	metallbv1beta1 "go.universe.tf/metallb/api/v1beta1"
	apiextensionsv1 "k8s.io/apiextensions-apiserver/pkg/apis/apiextensions/v1"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/util/intstr"
	"sigs.k8s.io/yaml"

	provisioningv1alpha1 "github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/api/v1alpha1"
)

var types = []any{
	&provisioningv1alpha1.DPFHCPProvisioner{},
	&provisioningv1alpha1.DPFHCPProvisionerList{},
	&provisioningv1alpha1.DPFHCPProvisionerConfig{},
	&provisioningv1alpha1.DPFHCPProvisionerConfigList{},
	&dpuprovisioningv1alpha1.DPU{},
	&dpuprovisioningv1alpha1.DPUList{},
	&dpuprovisioningv1alpha1.DPUCluster{},
	&dpuprovisioningv1alpha1.DPUClusterList{},
	&dpuprovisioningv1alpha1.DPUFlavor{},
	&dpuprovisioningv1alpha1.DPUFlavorList{},
	&dpuservicev1alpha1.DPUDeployment{},
	&dpuservicev1alpha1.DPUDeploymentList{},
	&dpuservicev1alpha1.DPUServiceTemplate{},
	&dpuservicev1alpha1.DPUServiceTemplateList{},
	&dpuoperatorv1alpha1.DPFOperatorConfig{},
	&dpuoperatorv1alpha1.DPFOperatorConfigList{},
	&hyperv1.HostedCluster{},
	&hyperv1.HostedClusterList{},
	&hyperv1.NodePool{},
	&hyperv1.NodePoolList{},
	&metallbv1beta1.IPAddressPool{},
	&metallbv1beta1.IPAddressPoolList{},
	&metallbv1beta1.L2Advertisement{},
	&metallbv1beta1.L2AdvertisementList{},
	&configv1.ClusterVersion{},
	&configv1.ClusterVersionList{},
}

// filler fills a value with deterministic non-zero data.
type filler struct {
	n     int
	stack map[reflect.Type]bool
}

func (f *filler) next() int {
	f.n++
	return f.n
}

var (
	tTime        = reflect.TypeOf(metav1.Time{})
	tQuantity    = reflect.TypeOf(resource.Quantity{})
	tIntOrString = reflect.TypeOf(intstr.IntOrString{})
	tDuration    = reflect.TypeOf(metav1.Duration{})
	tRaw         = reflect.TypeOf(runtime.RawExtension{})
	tJSON        = reflect.TypeOf(apiextensionsv1.JSON{})
	tIPNet       = reflect.TypeOf(ipnet.IPNet{})
	tFieldsV1    = reflect.TypeOf(metav1.FieldsV1{})
)

func (f *filler) fill(v reflect.Value) {
	t := v.Type()
	switch t {
	case tTime:
		n := f.next()
		v.Set(reflect.ValueOf(metav1.NewTime(time.Date(2024, 1, 2, 3, 4, 5, 0, time.UTC).Add(time.Duration(n) * time.Minute))))
		return
	case tQuantity:
		n := f.next()
		// alternate binary/decimal/milli forms
		var q resource.Quantity
		switch n % 3 {
		case 0:
			q = resource.MustParse(fmt.Sprintf("%dMi", n))
		case 1:
			q = resource.MustParse(fmt.Sprintf("%dm", n*1500))
		default:
			q = resource.MustParse(fmt.Sprintf("%d", n*1000))
		}
		v.Set(reflect.ValueOf(q))
		return
	case tIntOrString:
		n := f.next()
		if n%2 == 0 {
			v.Set(reflect.ValueOf(intstr.FromInt32(int32(n))))
		} else {
			v.Set(reflect.ValueOf(intstr.FromString(fmt.Sprintf("%d%%", n))))
		}
		return
	case tDuration:
		n := f.next()
		v.Set(reflect.ValueOf(metav1.Duration{Duration: time.Duration(n)*time.Minute + 1500*time.Millisecond}))
		return
	case tRaw:
		n := f.next()
		v.Set(reflect.ValueOf(runtime.RawExtension{Raw: []byte(fmt.Sprintf(`{"raw":%d,"nested":{"b":[1,"x"]}}`, n))}))
		return
	case tJSON:
		n := f.next()
		v.Set(reflect.ValueOf(apiextensionsv1.JSON{Raw: []byte(fmt.Sprintf(`{"json":%d}`, n))}))
		return
	case tIPNet:
		n := f.next()
		v.Set(reflect.ValueOf(*ipnet.MustParseCIDR(fmt.Sprintf("10.%d.0.0/16", n%256))))
		return
	case tFieldsV1:
		n := f.next()
		v.Set(reflect.ValueOf(metav1.FieldsV1{Raw: []byte(fmt.Sprintf(`{"f:spec":{"f:n%d":{}}}`, n))}))
		return
	}
	switch t.Kind() {
	case reflect.String:
		v.SetString(fmt.Sprintf("s%d", f.next()))
	case reflect.Bool:
		v.SetBool(true)
	case reflect.Int, reflect.Int16, reflect.Int32, reflect.Int64:
		v.SetInt(int64(f.next()))
	case reflect.Int8:
		v.SetInt(int64(f.next() % 100))
	case reflect.Uint, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		v.SetUint(uint64(f.next()))
	case reflect.Uint8:
		v.SetUint(uint64(f.next() % 200))
	case reflect.Float32, reflect.Float64:
		v.SetFloat(float64(f.next()) + 0.5)
	case reflect.Interface:
		v.Set(reflect.ValueOf(fmt.Sprintf("iface%d", f.next())))
	case reflect.Pointer:
		if f.stack[t.Elem()] {
			return // recursive type: stop
		}
		p := reflect.New(t.Elem())
		f.fill(p.Elem())
		v.Set(p)
	case reflect.Slice:
		s := reflect.MakeSlice(t, 1, 1)
		f.fill(s.Index(0))
		v.Set(s)
	case reflect.Map:
		m := reflect.MakeMap(t)
		k := reflect.New(t.Key()).Elem()
		k.SetString(fmt.Sprintf("k%d", f.next()))
		e := reflect.New(t.Elem()).Elem()
		f.fill(e)
		m.SetMapIndex(k, e)
		v.Set(m)
	case reflect.Struct:
		f.stack[t] = true
		for i := 0; i < t.NumField(); i++ {
			if !t.Field(i).IsExported() {
				continue
			}
			f.fill(v.Field(i))
		}
		delete(f.stack, t)
	default:
		panic(fmt.Sprintf("unsupported kind %s (%s)", t.Kind(), t))
	}
}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func write(dir, name string, data []byte) {
	must(os.WriteFile(filepath.Join(dir, name), data, 0o644))
}

func main() {
	out := flag.String("out", "crates/apis/tests/fixtures", "output directory")
	flag.Parse()
	must(os.MkdirAll(*out, 0o755))

	for _, obj := range types {
		t := reflect.TypeOf(obj).Elem()
		name := t.Name()

		zero, err := json.Marshal(reflect.New(t).Interface())
		must(err)
		write(*out, name+".zero.json", zero)

		filledV := reflect.New(t)
		(&filler{stack: map[reflect.Type]bool{}}).fill(filledV.Elem())
		filled, err := json.Marshal(filledV.Interface())
		must(err)
		write(*out, name+".filled.json", filled)

		back := reflect.New(t)
		must(json.Unmarshal(filled, back.Interface()))
		rt, err := json.Marshal(back.Interface())
		must(err)
		write(*out, name+".roundtrip.json", rt)
	}

	// DPUFlavor as embedded in ignition (internal/ignition/dpuflavor.go cleanDPUFlavor):
	// TypeMeta + name/namespace + spec.
	flavor := &dpuprovisioningv1alpha1.DPUFlavor{}
	(&filler{stack: map[reflect.Type]bool{}}).fill(reflect.ValueOf(flavor).Elem())
	clean := &dpuprovisioningv1alpha1.DPUFlavor{
		TypeMeta:   flavor.TypeMeta,
		ObjectMeta: metav1.ObjectMeta{Name: flavor.Name, Namespace: flavor.Namespace},
		Spec:       flavor.Spec,
	}
	y, err := yaml.Marshal(clean)
	must(err)
	write(*out, "DPUFlavor.clean.yaml", y)
	j, err := json.Marshal(clean)
	must(err)
	write(*out, "DPUFlavor.clean.json", j)

	// resource.Quantity: ParseQuantity -> String()
	type qcase struct {
		In  string `json:"in"`
		Out string `json:"out,omitempty"`
		Err string `json:"err,omitempty"`
	}
	var qcases []qcase
	for _, in := range quantityInputs {
		q, err := resource.ParseQuantity(in)
		c := qcase{In: in}
		if err != nil {
			c.Err = err.Error()
		} else {
			c.Out = q.String()
		}
		qcases = append(qcases, c)
	}
	qj, err := json.MarshalIndent(qcases, "", "  ")
	must(err)
	write(*out, "quantity.json", append(qj, '\n'))

	// metav1.Duration / time.Duration: ParseDuration -> String()
	type dcase struct {
		In    string `json:"in"`
		Nanos int64  `json:"nanos"`
		Out   string `json:"out,omitempty"`
		Err   string `json:"err,omitempty"`
	}
	var dcases []dcase
	for _, in := range durationInputs {
		d, err := time.ParseDuration(in)
		c := dcase{In: in}
		if err != nil {
			c.Err = err.Error()
		} else {
			c.Nanos = int64(d)
			c.Out = d.String()
		}
		dcases = append(dcases, c)
	}
	dj, err := json.MarshalIndent(dcases, "", "  ")
	must(err)
	write(*out, "duration.json", append(dj, '\n'))

	// ipnet.IPNet: ParseCIDR -> MarshalJSON
	type icase struct {
		In   string          `json:"in"`
		JSON json.RawMessage `json:"json,omitempty"`
		Err  string          `json:"err,omitempty"`
	}
	var icases []icase
	for _, in := range ipnetInputs {
		n, err := ipnet.ParseCIDR(in)
		c := icase{In: in}
		if err != nil {
			c.Err = err.Error()
		} else {
			b, err := json.Marshal(n)
			must(err)
			c.JSON = b
		}
		icases = append(icases, c)
	}
	ij, err := json.MarshalIndent(icases, "", "  ")
	must(err)
	write(*out, "ipnet.json", append(ij, '\n'))
}

var quantityInputs = []string{
	"0", "1", "-1", "+1", "01", "00", "-0", "1.", ".5", "-.5", "1.0", "1.5", "1.500", "0.1", "0.0001m",
	"0.0000000001", "1n", "1u", "1m", "1000m", "1500m", "1k", "1000", "1000000", "1M", "1G", "1T", "1P",
	"1E", "1000E", "10E", "1Ki", "1024", "1024Ki", "1024Mi", "1Gi", "0.5Gi", "1.5Gi", "500Mi", "100Mi",
	"8Mi", "16Mi", "1000Ki", "1023Ki", "2048Ki", "1e3", "1E3", "1e-3", "12e6", "1e0", "1.5e3", "1e+3",
	"100Ei", "8Ei", "7Ei", "123456789012345678", "1234567890123456789", "12345678901234567890",
	"9223372036854775807", "9223372036854775808Ki", "0.5", "0.333", "3.14159", "-1Gi", "-1.5Gi",
	"-1500m", "2.5", "250", "1Mi", "5Gi", "20Gi", "100m", "250m", "0.25", "64Ki", "32Mi",
	"", "1.2.3", "1Q", "1Ki5", "abc", "e3", "1e", "1ee3", "1Kii", "--1", "1 ", " 1", "1e1000", "1.Gi",
	"0.000000000001Gi", "1500Mi", "1536Mi", "0.1Ki", "1024m", "2048m",
}

var durationInputs = []string{
	"0", "-0", "+0", "5s", "30s", "1478s", "-5s", "+5s", "5.0s", "5.6s", "5.s", ".5s", "1.004s", "1.0040s",
	"100.00100s", "10ns", "11us", "12µs", "12μs", "13ms", "14s", "15m", "16h", "3h30m", "10.5s4m",
	"-2m3.4s", "1h2m3s4ms5us6ns", "39h9m14.425s", "52763797000ns", "0.3333333333333333333h",
	"9007199254740993ns", "9223372036854775807ns", "9223372036854775.807us", "9223372036854s775ms807us",
	"-9223372036854775808ns", "-9223372036854775.808us", "0.100000000000000000000h",
	"0.830103483285477580700h", "", "3", "-", "s", ".", "-.", ".s", "+.s", "1d", "1\x01s", "1\ts",
	"1é", "9223372036854775808ns", "9223372036854775.808us",
	"9223372036854ms775us808ns", "-9223372036854775809ns", "1h0m0.5s", "61m", "3601s",
	"1.5h", "2h45m", "1us", "1500ns",
}

var ipnetInputs = []string{
	"172.31.0.0/16", "10.132.0.0/14", "10.0.0.5/8", "2001:db8::/64", "2001:db8:0:0:1:0:0:1/128",
	"::ffff:10.0.0.1/104", "0.0.0.0/0", "1.2.3.4/08", "10.0.0.0", "10.0.0.0/33", "010.0.0.0/8",
	"fe80::1%eth0/64", "x/8", "1.2.3.4/", "::/0", "fd00::1/48", "192.168.1.100/32",
}
