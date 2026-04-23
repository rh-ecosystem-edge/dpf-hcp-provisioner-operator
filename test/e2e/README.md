# E2E Tests

End-to-end tests for the dpf-hcp-provisioner-operator. These tests validate the full
DPFHCPProvisioner lifecycle on a real OpenShift (OCP) cluster with HyperShift.

## Requirements

### Cluster

An **OCP cluster** is required. These tests **cannot run on Kind or Minikube** because:

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
| `E2E_TEST_TIMEOUT` | No | `90m` | Overall test timeout |

## What the Tests Do

### Setup (BeforeSuite)

The test suite automatically:

1. **Deploys HyperShift operator** on the management cluster (`make e2e-deploy-hypershift`)
2. **Generates and installs DPF CRDs** from vendored nvidia/doca-platform types (`make e2e-install-dpf-crds`)
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
export ETCD_STORAGE_CLASS=lvms-vg1
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
| `make test-e2e` | Run the full e2e test suite |
| `make e2e-deploy-hypershift` | Deploy HyperShift operator (idempotent) |
| `make e2e-generate-dpf-crds` | Generate DPF CRDs from vendored types |
| `make e2e-install-dpf-crds` | Generate, install, and wait for DPF CRDs |
