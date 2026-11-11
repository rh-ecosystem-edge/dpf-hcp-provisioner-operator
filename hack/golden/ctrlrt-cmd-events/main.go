// Command ctrlrt-cmd-events generates golden fixtures for the Rust port of client-go's
// record.EventCorrelator (crates/ctrlrt-cmd/src/events.rs): a sequence of events (built
// like recorderImpl.makeEvent at fixed times) is fed through the correlator with a fake
// clock; for each step the correlate result (skip / event / patch) is recorded, and a
// simulated API server answers creates/patches and calls UpdateState like recordToSink.
//
// Usage: go run -mod=vendor ./hack/golden/ctrlrt-cmd-events > crates/ctrlrt-cmd/tests/fixtures/events/correlator.json
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"time"

	v1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/record"
	"k8s.io/client-go/tools/record/util"
)

type fakeClock struct{ t time.Time }

func (c *fakeClock) Now() time.Time                  { return c.t }
func (c *fakeClock) Since(t time.Time) time.Duration { return c.t.Sub(t) }

// Step is one recorder.Event call.
type Step struct {
	// Seconds since base time (fractional allowed).
	At        float64 `json:"at"`
	Kind      string  `json:"kind"`
	Namespace string  `json:"namespace"`
	Name      string  `json:"name"`
	UID       string  `json:"uid"`
	Type      string  `json:"type"`
	Reason    string  `json:"reason"`
	Message   string  `json:"message"`
	Component string  `json:"component"`
}

// Result is the correlator output for a step.
type Result struct {
	Step   Step            `json:"step"`
	Skip   bool            `json:"skip"`
	Event  json.RawMessage `json:"event,omitempty"`
	Patch  string          `json:"patch,omitempty"`
	Action string          `json:"action,omitempty"`
}

var base = time.Date(2024, 3, 5, 7, 8, 9, 123456789, time.UTC)

func steps() []Step {
	var out []Step
	add := func(at float64, name, reason, msg string) {
		out = append(out, Step{At: at, Kind: "DPFHCPProvisioner", Namespace: "ns", Name: name, UID: "uid-" + name,
			Type: "Normal", Reason: reason, Message: msg, Component: "dpfhcpprovisioner-controller"})
	}
	// de-duplication
	add(0, "p1", "Created", "created thing")
	add(1, "p1", "Created", "created thing")
	add(2.5, "p1", "Created", "created thing")
	// aggregation: 10 distinct messages with the same reason
	for i := 1; i <= 10; i++ {
		add(3+float64(i), "p1", "Sync", "sync message "+strconv.Itoa(i))
	}
	add(20, "p1", "Sync", "sync message 11")
	add(21, "p1", "Sync", "sync message 12")
	// spam filter: 25 burst per object/type, then 1 per 300s
	for i := 1; i <= 30; i++ {
		add(100, "p2", "R"+strconv.Itoa(i), "spam")
	}
	add(100+301, "p2", "Late", "spam")
	add(100+301, "p2", "Late2", "spam")
	// aggregation window reset after 10 minutes
	for i := 1; i <= 9; i++ {
		add(1000, "p3", "Agg", "agg "+strconv.Itoa(i))
	}
	add(1000+601, "p3", "Agg", "agg 10")
	// cluster-scoped object, warning type
	out = append(out, Step{At: 2000, Kind: "Node", Name: "node-1", UID: "n1", Type: "Warning", Reason: "Bad", Message: "bad node",
		Component: "csr-approval-controller"})
	out = append(out, Step{At: 2001, Kind: "Node", Name: "node-1", UID: "n1", Type: "Warning", Reason: "Bad", Message: "bad node",
		Component: "csr-approval-controller"})
	return out
}

func makeEvent(s Step, t time.Time) *v1.Event {
	ref := &v1.ObjectReference{Kind: s.Kind, Namespace: s.Namespace, Name: s.Name, UID: types.UID(s.UID),
		APIVersion: "provisioning.dpu.hcp.io/v1alpha1", ResourceVersion: "7"}
	mt := metav1.Time{Time: t}
	namespace := ref.Namespace
	if namespace == "" {
		namespace = metav1.NamespaceDefault
	}
	ev := &v1.Event{
		ObjectMeta: metav1.ObjectMeta{
			Name:      util.GenerateEventName(ref.Name, mt.UnixNano()),
			Namespace: namespace,
		},
		InvolvedObject: *ref,
		Reason:         s.Reason,
		Message:        s.Message,
		FirstTimestamp: mt,
		LastTimestamp:  mt,
		Count:          1,
		Type:           s.Type,
	}
	ev.Source = v1.EventSource{Component: s.Component}
	ev.ReportingInstance = ""
	ev.ReportingController = s.Component
	return ev
}

func main() {
	clk := &fakeClock{t: base}
	c := record.NewEventCorrelatorWithOptions(record.CorrelatorOptions{Clock: clk})
	rv := 100
	var results []Result
	for _, s := range steps() {
		clk.t = base.Add(time.Duration(s.At * float64(time.Second)))
		ev := makeEvent(s, clk.t)
		res, err := c.EventCorrelate(ev)
		if err != nil {
			panic(err)
		}
		r := Result{Step: s, Skip: res.Skip}
		if !res.Skip {
			b, _ := json.Marshal(res.Event)
			r.Event = b
			r.Patch = string(res.Patch)
			// simulated API server
			newEvent := *res.Event
			rv++
			newEvent.ResourceVersion = strconv.Itoa(rv)
			if res.Event.Count > 1 {
				r.Action = "patch"
			} else {
				r.Action = "create"
			}
			c.UpdateState(&newEvent)
		}
		results = append(results, r)
	}
	b, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		panic(err)
	}
	fmt.Fprintln(os.Stdout, string(b))
}
