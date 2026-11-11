// Command zaplog generates golden fixtures for the Rust port of controller-runtime's zap
// logger (crates/ctrlrt-cmd). For each scenario it builds a logger exactly like
// cmd/main.go does (zap.Options{Development: true} + BindFlags + flag parsing +
// zap.New(zap.UseFlagOptions(&opts))) with a fixed clock, replays a list of logr calls and
// records the output.
//
// Usage: go run -mod=vendor ./hack/golden/zaplog <output dir>
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/go-logr/logr"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/klog/v2"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
)

// Op is one logr call.
type Op struct {
	// Names applied with WithName, in order.
	Names []string `json:"names,omitempty"`
	// WithValues key/values.
	Values []interface{} `json:"values,omitempty"`
	// "info" or "error".
	Kind string `json:"kind"`
	V    int    `json:"v,omitempty"`
	Msg  string `json:"msg"`
	// Error message for Error (nil => log.Error(nil, ...)).
	Err    *string       `json:"err,omitempty"`
	KVs    []interface{} `json:"kvs,omitempty"`
	Repeat int           `json:"repeat,omitempty"`
}

// Scenario is a logger configuration plus the calls to replay.
type Scenario struct {
	Name string `json:"name"`
	// Development is the zap.Options.Development value before flag parsing.
	Development bool     `json:"development"`
	Flags       []string `json:"flags"`
	// Clock as RFC3339Nano with a fixed zone offset in seconds.
	Time       string `json:"time"`
	TZOffset   int    `json:"tzOffset"`
	Ops        []Op   `json:"ops"`
	OutputFile string `json:"outputFile"`
}

type fixedClock struct{ t time.Time }

func (c fixedClock) Now() time.Time                         { return c.t }
func (c fixedClock) NewTicker(d time.Duration) *time.Ticker { return time.NewTicker(d) }

func s(v string) *string { return &v }

// convert turns the JSON-ish op values into the Go values the operator would log.
func convert(v interface{}) interface{} {
	switch x := v.(type) {
	case json.Number:
		if strings.ContainsAny(x.String(), ".eE") {
			f, _ := x.Float64()
			return f
		}
		i, _ := x.Int64()
		return int(i)
	case map[string]interface{}:
		if r, ok := x["$objref"]; ok {
			b, _ := json.Marshal(r)
			var ref corev1.ObjectReference
			_ = json.Unmarshal(b, &ref)
			return ref
		}
		if r, ok := x["$kref"]; ok {
			a := r.([]interface{})
			return klog.KRef(a[0].(string), a[1].(string))
		}
		if r, ok := x["$err"]; ok {
			return errors.New(r.(string))
		}
		if r, ok := x["$pod"]; ok {
			a := r.([]interface{})
			return &corev1.Pod{
				TypeMeta:   metav1.TypeMeta{APIVersion: "v1", Kind: "Pod"},
				ObjectMeta: metav1.ObjectMeta{Namespace: a[0].(string), Name: a[1].(string)},
			}
		}
		out := map[string]interface{}{}
		for k, e := range x {
			out[k] = convert(e)
		}
		return out
	case []interface{}:
		out := make([]interface{}, len(x))
		for i, e := range x {
			out[i] = convert(e)
		}
		return out
	default:
		return v
	}
}

func convertAll(in []interface{}) []interface{} {
	out := make([]interface{}, len(in))
	for i, e := range in {
		out[i] = convert(e)
	}
	return out
}

func run(sc Scenario) ([]byte, error) {
	opts := zap.Options{Development: sc.Development}
	fs := flag.NewFlagSet("zaplog", flag.ContinueOnError)
	opts.BindFlags(fs)
	if err := fs.Parse(sc.Flags); err != nil {
		return nil, err
	}
	t, err := time.Parse(time.RFC3339Nano, sc.Time)
	if err != nil {
		return nil, err
	}
	t = t.In(time.FixedZone("fixed", sc.TZOffset))
	var buf bytes.Buffer
	root := zap.New(zap.UseFlagOptions(&opts), zap.WriteTo(&buf), zap.RawZapOpts(zapClock(t)))
	for _, op := range sc.Ops {
		var l logr.Logger = root
		for _, n := range op.Names {
			l = l.WithName(n)
		}
		if len(op.Values) > 0 {
			l = l.WithValues(convertAll(op.Values)...)
		}
		n := op.Repeat
		if n == 0 {
			n = 1
		}
		for i := 0; i < n; i++ {
			switch op.Kind {
			case "info":
				l.V(op.V).Info(op.Msg, convertAll(op.KVs)...)
			case "error":
				var e error
				if op.Err != nil {
					e = errors.New(*op.Err)
				}
				l.Error(e, op.Msg, convertAll(op.KVs)...)
			default:
				return nil, fmt.Errorf("unknown op kind %q", op.Kind)
			}
		}
	}
	return buf.Bytes(), nil
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: zaplog <output dir>")
		os.Exit(1)
	}
	dir := os.Args[1]
	if err := os.MkdirAll(dir, 0o755); err != nil {
		panic(err)
	}
	scenarios := buildScenarios()
	for i := range scenarios {
		// Round-trip through JSON so Go sees exactly what the Rust test sees.
		b, _ := json.Marshal(scenarios[i])
		dec := json.NewDecoder(bytes.NewReader(b))
		dec.UseNumber()
		var sc Scenario
		if err := dec.Decode(&sc); err != nil {
			panic(err)
		}
		out, err := run(sc)
		if err != nil {
			panic(fmt.Sprintf("%s: %v", sc.Name, err))
		}
		sc.OutputFile = sc.Name + ".out"
		scenarios[i].OutputFile = sc.OutputFile
		if err := os.WriteFile(filepath.Join(dir, sc.OutputFile), out, 0o644); err != nil {
			panic(err)
		}
	}
	b, _ := json.MarshalIndent(scenarios, "", "  ")
	if err := os.WriteFile(filepath.Join(dir, "scenarios.json"), append(b, '\n'), 0o644); err != nil {
		panic(err)
	}
}
