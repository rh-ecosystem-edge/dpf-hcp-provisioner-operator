# Rust port — conventions

The operator was ported from Go to Rust with **100% functional compatibility**. The port is
complete and the Go implementation (`api/`, `cmd/`, `internal/`, `test/e2e/*.go`,
`test/utils/`, the golden generators under `hack/golden/`, `go.mod`/`vendor/`) has been
removed from the tree; it remains available in the git history (last commit containing it:
`fead352`). Code comments that say "port of `internal/...`" record that provenance.

The only Go code left is `hack/gen-rust-apis` (a standalone Go module that pins the upstream
API versions and drives the API type / external CRD generation; see
`crates/apis/GENERATION.md`).

## Layout

Cargo workspace at the repo root, one crate per former Go package under `crates/`:

| Crate | Former Go package |
|---|---|
| `ctrlrt` | controller-runtime / apimachinery core (client facade, errors, conditions, controllerutil, recorder, logger, controller builder) |
| `ctrlrt-fake` | `controller-runtime/pkg/client/fake` (+ `client-go/kubernetes/fake` needs) |
| `ctrlrt-runtime` | real client, informer cache, controllers, work queue, manager, leader election, health probes, event recorder |
| `ctrlrt-cmd` | zap-compatible logging, Go `flag`-compatible CLI parsing, metrics server (authn/authz filter), cert watcher |
| `ctrlrt-envtest` | `controller-runtime/pkg/envtest` |
| `gocompat` | byte-compatible Go stdlib behaviour: `encoding/json`, `sigs.k8s.io/yaml`, `compress/gzip`, `net/url`, semver |
| `registry` | `go-containerregistry` subset (`name`, `authn`, `remote`, `mutate.Extract`) |
| `apis` | `api/v1alpha1` (now hand-maintained: `src/provisioning_types.rs`, kept in sync with `config/crd/bases`) + generated external APIs (doca-platform, hypershift, metallb, openshift config) |
| `common`, `dpfdefaults`, `ignition`, `hostedclient` | `internal/...` |
| `ctrl-<name>` | `internal/controller/<name>` |
| `controller` | `internal/controller` (main reconciler) |
| `manager` | `cmd/main.go` (binary `manager`) |
| `e2e` | `test/e2e`, `test/utils` |

Runtime data that the Go code embedded lives next to the crate that embeds it:
`crates/dpfdefaults/dpfversions.json`, `crates/ignition/resources/<provider>/{files,systemd}`.
External CRDs used by the envtest suites and e2e are checked in under `config/crd/external` and
`test/e2e/testdata/crds`.

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
5. **Tests.** Every Go test has a Rust counterpart: each Ginkgo `It`/`Entry` and each
   `t.Run` case, named after the Go spec path in snake_case, with the same assertions
   (same expected values/messages). Each crate's `TEST_MAPPING.md` records the
   `Go test → Rust test` mapping made during the port (the Go tests themselves are gone);
   keep it when renaming tests, and add new tests as plain Rust tests. Unit tests that used the
   controller-runtime fake client use `ctrlrt-fake`; envtest suites use `ctrlrt-envtest`.
6. **Golden fixtures (frozen).** Where bytes matter (JSON/YAML/gzip/ignition/objects sent
   to the API server), the Rust tests assert byte equality against fixtures under
   `crates/<crate>/tests/fixtures/`. They were produced from the Go implementation by the
   generators in `hack/golden/<name>/main.go`, which were removed together with the Go code:
   the fixtures (and `crates/gocompat/src/unicode_tables.rs`, generated from Go's Unicode
   tables) are now **frozen references** of the Go behaviour and cannot be regenerated.
   Do not "fix" a failing golden test by rewriting the fixture unless the behaviour change is
   intentional; then edit the fixture deliberately (by hand or from the Rust output), review the
   diff, and note the intended deviation from the former Go behaviour in `DEVIATIONS.md`. The
   generators can be recovered from the git history (`git show fead352:hack/golden/<name>/main.go`)
   if a Go reference is ever needed again.
7. **Builds.** Use a private target dir: `CARGO_TARGET_DIR=target/<your-crate>`.
   Add third-party deps in the crate's `Cargo.toml` with explicit versions.
8. **Static binary.** Pure-Rust or freestanding-C dependencies only (rustls + ring, no
   OpenSSL, no system libs). `cargo build --release --target x86_64-unknown-linux-musl`
   must keep working.
9. No `unsafe`. No panics on runtime paths (Go code that panics at init on embedded data may
   panic in a `LazyLock`, as Go does).
