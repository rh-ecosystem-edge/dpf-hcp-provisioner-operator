# E2E Tests (Rust)

End-to-end tests for the dpf-hcp-provisioner-operator — the Rust port of `test/e2e` and
`test/utils` (see `test/e2e/README.md` for the original). Go spec → Rust mapping:
[`TEST_MAPPING.md`](TEST_MAPPING.md).

Layout:

| Path | Contents |
|---|---|
| `src/utils.rs` | `test/utils/utils.go` (`run` = `utils.Run`, `get_project_dir`, `uncomment_code`, ...) |
| `src/ginkgo.rs` | Ginkgo-compatible runner: ordered containers, `BeforeAll`/`AfterAll`/`BeforeEach`/`AfterEach`, label filter, focus, suite timeout, `By`, `GinkgoWriter` |
| `src/gomega.rs` | Gomega matchers, `Eventually`, `Consistently` |
| `tests/e2e_suite/main.rs` | `e2e_suite_test.go`: `test_e2e` (= `TestE2E`), `BeforeSuite`, `AfterSuite` |
| `tests/e2e_suite/ci.rs`, `ci_helpers.rs` | `e2e_ci_test.go`, `e2e_ci_helpers_test.go` |
| `tests/e2e_suite/dpuservicetemplate.rs`, `dpuservicetemplate_helpers.rs` | `e2e_dpuservicetemplate_test.go`, `e2e_dpuservicetemplate_helpers_test.go` |
| `tests/e2e_suite/fake_ocp_on_kind.rs` | `e2e_fake_ocp_on_kind_test.go` |

Tests are split into two groups using labels, exactly as in Go:

- **`ocp-required`** — tests that need a real OCP cluster with HyperShift (HostedCluster lifecycle,
  CSR approval, upgrades, cleanup). These validate the full DPFHCPProvisioner lifecycle.
- **Unlabeled** — tests that run on any Kubernetes cluster, including Kind. Currently covers
  DPUServiceTemplate controller lifecycle (creation, self-heal, deletion).

## Gating

The whole suite is a single libtest test, `test_e2e`, marked `#[ignore]`: a plain `cargo test`
(or `cargo test -p e2e`) compiles it and runs only the unit tests of the helpers. Pass
`--ignored` to run it against the cluster in your kubeconfig. Inside the suite the Go selection
logic is reproduced unchanged:

| Go | Rust |
|---|---|
| `go test ./test/e2e/` | `cargo test -p e2e --test e2e_suite -- --ignored --nocapture` |
| `-ginkgo.label-filter='!ocp-required'` | `E2E_LABEL_FILTER='!ocp-required'` (labels, `!`, `&&`, `\|\|`, `,`, parentheses; case-insensitive) |
| `-ginkgo.focus=<regex>` | `E2E_FOCUS=<regex>` (matched against the full spec text) |
| `-ginkgo.timeout=90m` | `E2E_SUITE_TIMEOUT=90m` (Go duration; default `1h` like Ginkgo) |
| `KIND_ONLY_TESTS=true` | `KIND_ONLY_TESTS=true` (`testingOnOCP = false`: no HyperShift deploy, `placement.target=custom`, DPUServiceTemplate specs enabled, default base domain / storage class) |

All other environment variables (`KUBECONFIG`, `IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI`,
`MACHINE_OS_URL`, `BASE_DOMAIN`, `OPERATOR_HELM_CHART`, `OCP_RELEASE_IMAGE`,
`CONTROL_PLANE_AVAILABILITY`, `ETCD_STORAGE_CLASS`, `UPGRADE_OCP_RELEASE_IMAGE`,
`HYPERSHIFT_IMAGE`, `KIND_CLUSTER`) are read exactly where the Go suite reads them; a missing
required one fails `BeforeSuite` (or the spec) with the Go message.

Progress (`STEP: ...`, GinkgoWriter output, spec results and the final
`Ran X of Y Specs` summary) is written directly to stderr, so it streams even without
`--nocapture`.

## Running on Kind

```bash
# Create a kind cluster (if you don't already have one).
kind create cluster

# Build and load the operator image into kind
make docker-build IMG=localhost/dpf-hcp-provisioner:dev
podman save localhost/dpf-hcp-provisioner:dev | \
  podman exec -i kind-control-plane ctr -n k8s.io images import -

# Run the kind-compatible tests
export IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI=localhost/dpf-hcp-provisioner:dev
make manifests
KIND_ONLY_TESTS=true E2E_LABEL_FILTER='!ocp-required' E2E_SUITE_TIMEOUT=10m \
  cargo test -p e2e --test e2e_suite -- --ignored --nocapture
```

`BeforeSuite` itself runs `make apply-crds-for-e2e` (all required CRDs: DPF, HyperShift,
OpenShift ClusterVersion) and deploys the operator via Helm.

## Running on OCP

Requirements are the same as for the Go suite (see `test/e2e/README.md`): an OCP cluster with a
default StorageClass (or `ETCD_STORAGE_CLASS`), enough resources for HyperShift control planes,
and no DPF operator running.

```bash
export KUBECONFIG=/path/to/kubeconfig
export IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI=quay.io/redhat-user-workloads/dpf-hcp-provisioner-tenant/dpf-hcp-provisioner:latest
export MACHINE_OS_URL=quay.io/eelgaev/rhcos-bfb:4.22.0-ec.2
export BASE_DOMAIN=your-cluster.example.com
E2E_SUITE_TIMEOUT=90m cargo test -p e2e --test e2e_suite -- --ignored --nocapture
```

Optional: `OPERATOR_HELM_CHART=oci://quay.io/redhat-user-workloads/dpf-hcp-provisioner-tenant/chart`,
`UPGRADE_OCP_RELEASE_IMAGE=...` (enables the upgrade spec), `E2E_FOCUS='CSR Auto-Approval'`.

## Makefile targets

The suite shells out to the same targets as the Go suite (`make e2e-deploy-hypershift`,
`make apply-crds-for-e2e`) and runs `helm`, `ssh-keygen` and `openssl` with the same arguments.
`utils::run` runs commands from the repository root (and `chdir`s there, like Go).
Recipes equivalent to `Makefile.e2e`:

```make
test-e2e:
	E2E_SUITE_TIMEOUT=$(E2E_TEST_TIMEOUT) cargo test -p e2e --test e2e_suite -- --ignored --nocapture

test-e2e-kind: manifests apply-crds-for-e2e
	KIND_ONLY_TESTS=true E2E_LABEL_FILTER='!ocp-required' E2E_SUITE_TIMEOUT=$(E2E_KIND_TIMEOUT) \
		cargo test -p e2e --test e2e_suite -- --ignored --nocapture
```

## Differences from the Go suite

- Top-level containers run in declaration order (`DPFHCPProvisioner E2E`, then
  `DPUServiceTemplate E2E`); Ginkgo shuffles top-level containers with a random seed by default.
  Specs inside each `Ordered` container keep Ginkgo's semantics exactly (see `src/ginkgo.rs`).
- Failure messages follow Gomega's layout (description, `Expected ... to equal ...`,
  `Timed out after Xs.`) but without code locations. Places where the Go code would panic
  (nil pointer, failed type assertion) fail the spec with a `[PANICKED]` message instead.
- The label filter supports labels, `!`, `&&`, `||`, `,` and parentheses (no `/regex/` or set
  operators, which the Makefile does not use).
- After the suite timeout, cleanup nodes get Ginkgo's default 30s grace period each.
- `GetProjectDir` strips `/crates/e2e` (the crate directory) instead of `/test/e2e`.
- The runtime scheme registration in `BeforeSuite` has no Rust equivalent (types carry their
  GVK); the unused `kubernetes.Clientset` built by `getHCClient` is not created.
