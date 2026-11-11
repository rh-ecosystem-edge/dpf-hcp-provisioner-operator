# E2E Tests

End-to-end tests for the dpf-hcp-provisioner-operator. The suite is the Rust `e2e` crate
(`crates/e2e`, run with `cargo test -p e2e --test e2e_suite -- --ignored`); see
[crates/e2e/README.md](../../crates/e2e/README.md) for its layout, the Ginkgo-compatible runner
and the selection variables (`E2E_LABEL_FILTER`, `E2E_FOCUS`, `E2E_SUITE_TIMEOUT`,
`KIND_ONLY_TESTS`). This directory holds the data the suite uses: `scripts/` (HyperShift
deployment) and `testdata/crds/` (checked-in external CRDs, regenerated with
`make generate-external-crds`).

Tests are split into two groups using labels:

- **`ocp-required`** — tests that need a real OCP cluster with HyperShift (HostedCluster lifecycle,
  CSR approval, upgrades, cleanup). These validate the full DPFHCPProvisioner lifecycle.
- **Unlabeled** — tests that run on any Kubernetes cluster, including Kind. Currently covers
  DPUServiceTemplate controller lifecycle (creation, self-heal, deletion).

## Running on Kind

```bash
# Create a kind cluster (if you don't already have one).
# While it starts, kick off the image build in another terminal — it takes a while.
kind create cluster

# Build and load the operator image into kind
make docker-build IMG=localhost/dpf-hcp-provisioner:dev
podman save localhost/dpf-hcp-provisioner:dev | \
  podman exec -i kind-control-plane ctr -n k8s.io images import -

# Run the kind-compatible tests
export IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI=localhost/dpf-hcp-provisioner:dev
make test-e2e-kind
```

The `test-e2e-kind` target installs all required CRDs (DPF, HyperShift, OpenShift ClusterVersion),
deploys the operator via Helm, and runs tests filtered by `E2E_LABEL_FILTER='!ocp-required'`.

## Requirements

### OCP Cluster (for `ocp-required` tests)

An **OCP cluster** is required for the full test suite. The `ocp-required` tests
**cannot run on Kind or Minikube** because:

- **HyperShift operator** requires OCP-specific APIs and infrastructure to create real HostedClusters
- **HostedCluster provisioning** needs a functional API server with proper networking
- **CSR auto-approval** tests connect to the HostedCluster's API server
- **Ignition generation** downloads from the HostedCluster's ignition endpoint

The cluster must have:

- A **default StorageClass** (e.g., `gp3-csi` on AWS, `lvms-vg1` on SNO). Required for etcd PVCs.
  If no default StorageClass exists, set `ETCD_STORAGE_CLASS` explicitly.
- **Sufficient resources** for HyperShift control plane pods (at least 3 control-plane nodes recommended)
- **No DPF operator** running (its webhooks/finalizers can interfere with test stub CRs)

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `KUBECONFIG` | Yes (local) | - | Path to OCP cluster kubeconfig |
| `IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI` | Yes (local) | Injected in CI | Operator image pull spec |
| `MACHINE_OS_URL` | Yes | - | DPU machine OS image URL (skips BlueField OCP layer lookup) |
| `BASE_DOMAIN` | Yes | - | Cluster base domain for HostedCluster DNS records |
| `OPERATOR_HELM_CHART` | No | `helm/dpf-hcp-provisioner-operator` | Helm chart path or OCI URL |
| `HYPERSHIFT_IMAGE` | No | `quay.io/hypershift/hypershift-operator:latest` | HyperShift image for deployment |
| `ETCD_STORAGE_CLASS` | No | Auto-detected | StorageClass for etcd PVCs |
| `E2E_TEST_TIMEOUT` | No | `90m` | Overall test timeout (`make test-e2e`; passed to the suite as `E2E_SUITE_TIMEOUT`) |

## What the Tests Do

### Setup (BeforeSuite)

The test suite automatically:

1. **Deploys HyperShift operator** on the management cluster (`make e2e-deploy-hypershift`)
2. **Installs the external CRDs** (DPF, HyperShift, OpenShift config) checked in under
   `test/e2e/testdata/crds` and `config/crd/external` (`make apply-crds-for-e2e`)
3. **Deploys the operator** via Helm chart with the specified image

### Test Resources Created (BeforeAll)

In the test namespace (`dpf-hcp-provisioner-system`):
- SSH key secret (generated)
- Pull secret (copied from `openshift-config/pull-secret`)

In a separate namespace (`dpf-e2e-dpucluster`):
- DPUCluster stub CR (`type: static`)
- DPFOperatorConfig CR
- DPUFlavor stub CR
- DPUDeployment stub CR (referencing the DPUFlavor)

### Test Cases

**HostedCluster Lifecycle:**
- Creates DPFHCPProvisioner CR and waits for it to reach Ready (~15-25 min)
- Verifies HostedCluster and NodePool are created
- Validates ignition ConfigMap contains valid Ignition JSON
- Validates kubeconfig is injected into DPUCluster namespace
- Verifies the `openshift-doca-hugepages-holder` namespace and reservation DaemonSet request 500Mi of `hugepages-2Mi` per node
- Validates all expected status conditions

**CSR Auto-Approval:**
- Creates DPU stub CRs and programmatic CSRs using kubectl `--as` impersonation
- Verifies bootstrap CSRs are approved for valid DPU nodes
- Verifies serving CSRs are approved (with Node existence check)
- Verifies CSRs for unknown hostnames are rejected

**Cleanup:**
- Deletes the DPFHCPProvisioner CR and verifies finalizer cleanup
- Verifies HostedCluster and related resources are deleted

**Validation Errors:**
- Verifies operator fails with missing DPUCluster reference
- Verifies operator fails with invalid secret references

### Teardown (AfterAll / AfterSuite)

- Deletes all test resources and the DPUCluster namespace
- Uninstalls the operator via Helm

## Running Locally

### 1. Provision an OCP cluster

Use [openshift-dpf](https://github.com/rh-ecosystem-edge/openshift-dpf) to create a cluster:

```bash
cd openshift-dpf
make verify-files check-cluster create-vms prepare-manifests cluster-install update-etc-hosts kubeconfig
```

### 2. Run the tests

```bash
export KUBECONFIG=/path/to/kubeconfig
export IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI=quay.io/redhat-user-workloads/dpf-hcp-provisioner-tenant/dpf-hcp-provisioner:latest
export MACHINE_OS_URL=quay.io/eelgaev/rhcos-bfb:4.22.0-ec.2
export BASE_DOMAIN=your-cluster.example.com
make test-e2e
```

### 3. Using a pre-built chart from registry

```bash
export OPERATOR_HELM_CHART=oci://quay.io/redhat-user-workloads/dpf-hcp-provisioner-tenant/chart
```

## CI Integration

In CI (openshift/release), the e2e workflow:

1. Provisions an AWS OCP cluster via IPI
2. Builds the operator image from the PR
3. Runs `make test-e2e` from the `src` image
4. `IMAGE_DPF_HCP_PROVISIONER_OPERATOR_CI` is injected via the workflow `dependencies` block

Trigger on a PR: `/test dpf-hcp-provisioner-operator-e2e-aws-single-replica`

## Makefile Targets

| Target | Description |
|--------|-------------|
| `make test-e2e` | Run the full e2e test suite (OCP cluster required) |
| `make test-e2e-kind` | Run kind-compatible e2e tests (no OCP required) |
| `make e2e-deploy-hypershift` | Deploy HyperShift operator (idempotent) |
| `make apply-crds-for-e2e` | Install the checked-in external CRDs and wait for them |
| `make generate-external-crds` | Regenerate the checked-in external CRDs (needs Go; see `hack/gen-external-crds.sh`) |
