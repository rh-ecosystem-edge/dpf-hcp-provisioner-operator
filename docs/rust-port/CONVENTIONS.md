# Rust port — conventions

The operator is being ported from Go to Rust with **100% functional compatibility**.
The Go sources stay in the tree as the reference implementation (and as the generator of
golden fixtures) until the port is complete.

## Layout

Cargo workspace at the repo root, one crate per Go package under `crates/`:

| Crate | Go package |
|---|---|
| `ctrlrt` | controller-runtime / apimachinery core (client facade, errors, conditions, controllerutil, recorder, logger, controller builder) |
| `ctrlrt-fake` | `controller-runtime/pkg/client/fake` (+ `client-go/kubernetes/fake` needs) |
| `ctrlrt-runtime` | real client, informer cache, controllers, work queue, manager, leader election, health probes, event recorder |
| `ctrlrt-cmd` | zap-compatible logging, Go `flag`-compatible CLI parsing, metrics server (authn/authz filter), cert watcher |
| `ctrlrt-envtest` | `controller-runtime/pkg/envtest` |
| `gocompat` | byte-compatible Go stdlib behaviour: `encoding/json`, `sigs.k8s.io/yaml`, `compress/gzip`, `net/url`, semver |
| `registry` | `go-containerregistry` subset (`name`, `authn`, `remote`, `mutate.Extract`) |
| `apis` | `api/v1alpha1` + external APIs (doca-platform, hypershift, metallb, openshift config) |
| `common`, `dpfdefaults`, `ignition`, `hostedclient` | `internal/...` |
| `ctrl-<name>` | `internal/controller/<name>` |
| `controller` | `internal/controller` (main reconciler) |
| `manager` | `cmd/main.go` (binary `manager`) |
| `e2e` | `test/e2e`, `test/utils` |

## Rules for every crate

1. **Exact behaviour.** Port logic line by line. Error strings, condition reasons/messages,
   event reasons/messages, log messages and keys, label/annotation keys, requeue durations
   must be identical to Go. Use `ctrlrt::wrapf!(err, "context: {}", err)` wherever Go uses
   `fmt.Errorf("context: %w", err)`, and `anyhow::anyhow!` for `errors.New`/`fmt.Errorf`
   without `%w`. Always render errors with `{}` (the message already contains the chain).
2. **Context & logging.** Go `ctx context.Context` → `ctx: &ctrlrt::Ctx`.
   `log := logf.FromContext(ctx)` → `let log = &ctx.log;`
   `log.Info("msg", "k", v)` → `log.info("msg", kv!["k" => v]);`
   `log.V(1).Info(...)` → `log.v(1).info(...)`; `log.Error(err, ...)` → `log.error(&err, ...)`.
3. **Client.** `client.Client` → `ctrlrt::Client` (clone it). `c.Get(ctx, key, &obj)` →
   `let obj: T = c.get(&key).await?` (or `get_into`). `Create/Update/UpdateStatus` take
   `&mut obj` and write the server response back, like Go.
   `apierrors.IsNotFound(err)` → `ctrlrt::error::is_not_found(&err)` (works through wraps).
4. **Types.** External API structs mirror the Go structs *and their JSON tags*: a field
   is serialized iff Go would serialize it (`omitempty` → skip when zero/None/empty; no
   `omitempty` → always serialize, zero value included; non-pointer `metav1.Time` → `null`
   when zero). Unknown fields are preserved via `#[serde(flatten)] extra`.
5. **Tests.** Every Go test must have a Rust counterpart: each Ginkgo `It`/`Entry` and each
   `t.Run` case. Name Rust tests after the Go spec path in snake_case. Keep all assertions
   (same expected values/messages). Each crate keeps a `TEST_MAPPING.md` listing
   `Go test → Rust test` so coverage can be audited. Unit tests that used the
   controller-runtime fake client use `ctrlrt-fake`; envtest suites use `ctrlrt-envtest`.
6. **Golden differential tests.** Where bytes matter (JSON/YAML/gzip/ignition/objects sent
   to the API server), generate fixtures from the Go implementation with a Go program at
   `hack/golden/<name>/main.go` (package `main` inside the Go module, may import
   `internal/...`; run `go run -mod=vendor ./hack/golden/<name>`), store them under
   `crates/<crate>/tests/fixtures/`, and assert byte equality in Rust.
7. **Builds.** Use a private target dir: `CARGO_TARGET_DIR=target/<your-crate>`.
   Only touch your own crate(s) (and `hack/golden/<your-name>`). Add third-party deps in your
   crate's `Cargo.toml` with explicit versions. Do not run state-changing git commands.
   If another crate needs a change, describe it in your final report instead of editing it.
8. **Static binary.** Pure-Rust or freestanding-C dependencies only (rustls + ring, no
   OpenSSL, no system libs). `cargo build --release --target x86_64-unknown-linux-musl`
   must keep working.
9. No `unsafe`. No panics on runtime paths (Go code that panics at init on embedded data may
   panic in a `LazyLock`, as Go does).
