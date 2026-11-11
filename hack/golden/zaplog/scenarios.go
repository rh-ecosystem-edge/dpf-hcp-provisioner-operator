package main

import (
	"time"

	"go.uber.org/zap"
)

func zapClock(t time.Time) zap.Option { return zap.WithClock(fixedClock{t}) }

func commonOps() []Op {
	return []Op{
		{Kind: "info", Msg: "starting manager"},
		{Kind: "info", Names: []string{"setup"}, Msg: "disabling http/2"},
		{Kind: "info", Names: []string{"setup"}, Msg: "Initializing metrics certificate watcher using provided certificates",
			KVs: []interface{}{"metrics-cert-path", "/tmp/certs", "metrics-cert-name", "tls.crt", "metrics-cert-key", "tls.key"}},
		{Kind: "info", Names: []string{"controller-runtime", "metrics"}, Msg: "Serving metrics server",
			KVs: []interface{}{"bindAddress", ":8443", "secure", true}},
		{Kind: "info", Names: []string{"a", "b"}, Values: []interface{}{"controller", "dpfhcpprovisioner", "reconcileID", "abc-123"},
			Msg: "Reconciling", KVs: []interface{}{"DPFHCPProvisioner", map[string]interface{}{"$kref": []interface{}{"ns1", "name1"}}, "namespace", "ns1", "name", "name1"}},
		{Kind: "info", V: 1, Names: []string{"events"}, Msg: "Created HostedCluster",
			KVs: []interface{}{"type", "Normal", "object", map[string]interface{}{"$objref": map[string]interface{}{
				"kind": "DPFHCPProvisioner", "namespace": "ns", "name": "p1", "uid": "1234-5678",
				"apiVersion": "provisioning.dpu.hcp.io/v1alpha1", "resourceVersion": "42"}}, "reason", "HostedClusterCreated"}},
		{Kind: "info", V: 1, Msg: "v1 line", KVs: []interface{}{"int", 42, "neg", -7, "float", 1.5, "float2", 100.0, "big", 1e21, "small", 1e-7, "nil", nil, "bool", false}},
		{Kind: "info", V: 2, Msg: "v2 line", KVs: []interface{}{"k", "v"}},
		{Kind: "info", V: 3, Msg: "v3 line"},
		{Kind: "info", Msg: "special \"chars\"\n\ttab", KVs: []interface{}{"str", "quote\" backslash\\ nl\n cr\r tab\t bell\x07 del\x7f <>&   ünïcödé 日本", "k\"ey", "v"}},
		{Kind: "info", Msg: "nested", KVs: []interface{}{
			"map", map[string]interface{}{"b": 1, "a": "x <>&\b\f", "c": []interface{}{1, "two", 3.5, nil, true, map[string]interface{}{"z": 1e-7, "y": 1e21, "x": 0.1}}},
			"list", []interface{}{"a", "b"},
			"empty", map[string]interface{}{},
			"emptyList", []interface{}{},
		}},
		{Kind: "info", Msg: "error as value", KVs: []interface{}{"err", map[string]interface{}{"$err": "boom"}}},
		{Kind: "error", Names: []string{"setup"}, Msg: "unable to start manager", Err: s("failed to get restconfig: no configuration has been provided")},
		{Kind: "error", Msg: "nil error", KVs: []interface{}{"controller", "CSRApproval"}},
		{Kind: "error", Names: []string{"setup"}, Values: []interface{}{"wv", 1}, Msg: "to initialize metrics certificate watcher", Err: s("open /x/tls.crt: no such file or directory"),
			KVs: []interface{}{"error", map[string]interface{}{"$err": "open /x/tls.crt: no such file or directory"}}},
		{Kind: "info", Msg: ""},
	}
}

func buildScenarios() []Scenario {
	const t = "2024-03-05T07:08:09.123456789Z"
	var out []Scenario
	add := func(name string, dev bool, tz int, flags ...string) {
		out = append(out, Scenario{Name: name, Development: dev, Flags: flags, Time: t, TZOffset: tz, Ops: commonOps()})
	}
	// Stack traces are Go specific: disable them (panic level) for byte comparisons.
	add("dev-default", true, 0, "--zap-stacktrace-level=panic")
	add("dev-tz", true, 2*3600+30*60, "--zap-stacktrace-level=panic")
	add("dev-negtz", true, -5*3600, "--zap-stacktrace-level=panic", "--zap-time-encoding=iso8601")
	add("dev-level-info", true, 0, "--zap-stacktrace-level=panic", "--zap-log-level=info")
	add("dev-level-3", true, 0, "--zap-stacktrace-level=panic", "--zap-log-level=3")
	add("dev-level-error", true, 0, "--zap-stacktrace-level=panic", "--zap-log-level=error")
	add("dev-level-panic", true, 0, "--zap-stacktrace-level=panic", "--zap-log-level=PANIC")
	add("dev-json", true, 0, "--zap-stacktrace-level=panic", "--zap-encoder=json")
	add("prod-default", false, 0, "--zap-stacktrace-level=panic")
	add("prod-devel-flag", true, 0, "--zap-stacktrace-level=panic", "--zap-devel=false")
	add("prod-console", false, 0, "--zap-stacktrace-level=panic", "--zap-encoder=console")
	add("prod-level-2", false, 0, "--zap-stacktrace-level=panic", "--zap-log-level=2")
	for _, te := range []string{"epoch", "millis", "nanos", "iso8601", "rfc3339", "rfc3339nano"} {
		add("dev-time-"+te, true, 0, "--zap-stacktrace-level=panic", "--zap-time-encoding="+te)
		add("prod-time-"+te, false, 0, "--zap-stacktrace-level=panic", "--zap-time-encoding="+te)
	}
	// Kubernetes objects in production (non-verbose KubeAwareEncoder).
	out = append(out, Scenario{Name: "prod-kube-object", Development: false, Flags: []string{"--zap-stacktrace-level=panic"}, Time: t,
		Ops: []Op{{Kind: "info", Msg: "object", KVs: []interface{}{"pod", map[string]interface{}{"$pod": []interface{}{"ns", "p"}}}}},
	})
	out = append(out, Scenario{Name: "prod-kube-object-console", Development: false, Flags: []string{"--zap-stacktrace-level=panic", "--zap-encoder=console"}, Time: t,
		Ops: []Op{{Kind: "info", Msg: "object", KVs: []interface{}{"pod", map[string]interface{}{"$pod": []interface{}{"ns", "p"}}}}},
	})
	// Production sampling: first 100 per second, then every 100th.
	out = append(out, Scenario{Name: "prod-sampling", Development: false, Flags: []string{"--zap-stacktrace-level=panic"}, Time: t,
		Ops: []Op{
			{Kind: "info", Msg: "repeated", Repeat: 250},
			{Kind: "error", Msg: "repeated error", Err: s("e"), Repeat: 101},
			{Kind: "info", Msg: "other"},
		},
	})
	// Stack traces enabled (default dev stacktrace level = warn): structure only.
	out = append(out, Scenario{Name: "dev-stacktrace", Development: true, Time: t,
		Ops: []Op{{Kind: "info", Msg: "no stack"}, {Kind: "error", Msg: "with stack", Err: s("boom")}},
	})
	out = append(out, Scenario{Name: "prod-stacktrace", Development: false, Time: t,
		Ops: []Op{{Kind: "info", Msg: "no stack"}, {Kind: "error", Msg: "with stack", Err: s("boom")}},
	})
	return out
}
