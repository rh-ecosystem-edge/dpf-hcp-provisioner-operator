# Rust port — known deviations from the Go implementation

The port targets 100% behavioural compatibility. Byte-level equivalence with the Go code is
enforced by golden fixtures generated from the Go sources (`hack/golden/*`) for everything the
operator writes: objects sent to the API server, ignition payloads (incl. gzip bytes),
DPUServiceTemplate helm values, condition/event/error strings, CLI usage/errors and log lines.
An end-to-end differential run of the Go and Rust `manager` binaries against the same envtest
API server produces identical CR status (`crates/manager/tests/smoke.rs` with
`SMOKE_MANAGER_BIN=<go binary>`).

The remaining differences are listed below. None changes reconciliation results; most are
unreachable in practice or only affect diagnostic text.

## Process / runtime

| Area | Go | Rust | Impact |
|---|---|---|---|
| Stack traces in error logs | Go frames | Rust frames (same zap layout) | log text only |
| Go runtime metrics (`go_*`) | exported | not exported (process_* and all controller-runtime/workqueue/rest-client metrics are) | metrics only |
| Metrics self-signed cert key | RSA-2048 | ECDSA P-256 (ring cannot generate RSA) | none (same CN/SANs/validity) |
| Metrics endpoint content types | text/protobuf negotiation | always text format | protobuf scrapers fall back to text |
| Network error wording | Go `net` errors | Go-like mapping for common cases (refused, reset, TLS unknown authority/hostname, DNS, timeouts); rare errors keep the Rust wording | condition/log text on network failures |
| Registry/HTTP calls | cancelled via `ctx` | cancelled on drop; registry calls have no ctx | none for reconcile results |
| Cached list ordering | random map order | deterministic (namespace, name) | none |
| Webhook server | constructed, never started (no webhooks) | not constructed; cert watcher still validated/started as in Go | none |
| Unknown fields on read-modify-write | dropped (Go structs) | preserved (`extra` maps) | safer; ignition DPUFlavor output clears them to match Go bytes |

## Library-level behaviour

| Area | Difference |
|---|---|
| `equality.Semantic` | implemented on JSON documents with nil≡empty normalisation; a nil pointer-to-struct and a pointer to an all-`omitempty` empty struct compare equal |
| Map-iteration-dependent messages | where Go picks a "random" element (status mirror append order, CSR missing-usage message, ignition partition collisions), Rust uses a stable order |
| Nil-pointer panics in Go | reproduced: the reconciler re-raises Go's `runtime error: invalid memory address or nil pointer dereference`, recovered by the runtime exactly like controller-runtime (`panic: … [recovered]`) |
| `resource.MustParse` panic on invalid hugepage size | returns an error with the same message (unreachable: CRD enum restricts the value) |
| Kubeconfig parse errors | kube-rs wording instead of clientcmd wording |
| YAML parsing | `gocompat::yaml` parses what yaml.v2 emits + JSON; anchors/tags/multi-doc are rejected (only used in tests) |
| x509 edge cases | invalid UTCTime/GeneralizedTime subject values: same accept/reject decisions, different error wording (logs only) |
| Ignition validation | Go reports the first of several colliding partition numbers in map order; Rust reports the first in document order |

## Build / CI

* `Containerfile` uses a musl-native Rust builder (`docker.io/library/rust:1.99-alpine3.24`)
  because RHEL/UBI Rust packages ship no musl target and UBI lacks `glibc-static`; the runtime
  image is unchanged (`ubi10-micro`).
* Konflux pipelines prefetch `cargo` dependencies instead of `gomod`
  (`.tekton/dpf-hcp-provisioner-rhel10-operator-*.yaml`). This needs validation in Konflux
  (hermeto cargo support and allowance of the builder image).
* `.ci-operator.yaml` (OpenShift CI build root) still references the Go build-root image; the
  CI job definitions live in openshift/release and must be updated there to provide a Rust
  toolchain for `make test`/`make lint`.
* `make manifests`/`make generate` still use controller-gen on the Go API types, and the
  golden generators under `hack/golden` import the Go packages. Removing the Go tree would
  make the CRD YAML in `config/crd/bases` and `config/rbac` hand-maintained sources.
