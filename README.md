# DPF HCP Provisioner Operator

A Kubernetes operator that abstracts [HyperShift](https://github.com/openshift/hypershift) complexity for [NVIDIA DPF (DOCA Platform Framework)](https://github.com/nvidia/doca-platform). It orchestrates the full lifecycle of HostedClusters for DPU (Data Processing Unit) environments, treating the hosted control plane as a "black box" for end users.

The operator maintains a **1:1:1 relationship**: each `DPFHCPProvisioner` CR maps to exactly one `DPUCluster` and one `HostedCluster`.

## Key Features

- **HostedCluster lifecycle management** -- creates, updates, and deletes HostedClusters, NodePools, and associated secrets
- **Automatic CSR approval** -- approves Certificate Signing Requests from DPU worker nodes joining the hosted cluster
- **BlueField OCP layer image lookup** -- matches OCP release images to corresponding BlueField container images via container registry tag lookup (skipped when `machineOSURL` is provided)
- **Kubeconfig injection** -- extracts the HostedCluster kubeconfig and injects it into the DPUCluster CR
- **MetalLB configuration** -- deploys IPAddressPool and L2Advertisement resources for LoadBalancer service exposure
- **Ignition generation** -- generates BlueField-specific ignition configurations from HyperShift ignition for DPU node provisioning
- **Status translation** -- mirrors HostedCluster conditions to DPFHCPProvisioner status without exposing HyperShift internals

## Architecture

![Diagram](./docs/dpf_hcp_provisioner_operator_diagram.png)

### DPFHCPProvisioner Lifecycle Phases

| Phase | Description |
|-------|-------------|
| `Pending` | Initial state; validation of DPUCluster, secrets, and configuration in progress |
| `Provisioning` | HostedCluster and related resources are being created |
| `IgnitionGenerating` | BlueField-specific ignition configuration is being generated |
| `Ready` | HostedCluster is operational, kubeconfig injected, CSR auto-approval active |
| `Failed` | Permanent failure requiring user intervention |
| `Deleting` | Finalizer cleanup in progress (HostedCluster deletion, MetalLB cleanup) |

### Controllers

- **DPFHCPProvisionerReconciler** -- main controller that orchestrates the full provisioning lifecycle through modular feature handlers
- **CSRApprovalReconciler** -- secondary controller that watches for and auto-approves CSRs from DPU worker nodes in hosted clusters

## Prerequisites

The following operators and components must be installed on the management cluster:

| Component                                                                                              | Description |
|--------------------------------------------------------------------------------------------------------|-------------|
| OpenShift                                                                                              | Management cluster platform |
| [MCE Operator](https://docs.redhat.com/en/documentation/red_hat_advanced_cluster_management_for_kubernetes) | Multi-Cluster Engine for HyperShift support |
| [HyperShift Operator](https://github.com/openshift/hypershift)                                         | Manages HostedCluster resources |
| [DPF Operator](https://github.com/nvidia/doca-platform)                                                | Manages DPUCluster and DPU resources |
| [ODF Operator](https://docs.redhat.com/en/documentation/red_hat_openshift_data_foundation)             | Provides storage classes for etcd persistent volumes |
| [MetalLB Operator](https://metallb.universe.tf/)                                                       | Provides LoadBalancer services for control plane exposure |

## Custom Resources

### DPFHCPProvisioner

**API Group:** `provisioning.dpu.hcp.io/v1alpha1`
**Scope:** Namespaced
**Short Name:** `dpfhcp`

#### Spec Fields

| Field | Type | Required | Immutable | Default | Description |
|-------|------|----------|-----------|---------|-------------|
| `dpuClusterRef` | `{name, namespace}` | Yes | Yes | -- | Cross-namespace reference to the target DPUCluster |
| `baseDomain` | `string` | Yes | Yes | -- | Base domain for hosted cluster DNS (e.g., `clusters.example.com`) |
| `ocpReleaseImage` | `string` | Yes | No | -- | Full pull-spec URL for the OCP release image |
| `sshKeySecretRef` | `{name}` | Yes | Yes | -- | Secret containing SSH public key (`id_rsa.pub` key) |
| `pullSecretRef` | `{name}` | Yes | Yes | -- | Secret containing container registry pull secret (`.dockerconfigjson` key) |
| `etcdStorageClass` | `string` | No | Yes | -- | Storage class for etcd persistent volumes |
| `controlPlaneAvailabilityPolicy` | `string` | No | Yes | `HighlyAvailable` | `SingleReplica` or `HighlyAvailable` |
| `virtualIP` | `string` | No | Yes | -- | Virtual IP for LoadBalancer (required when `HighlyAvailable`) |
| `nodeSelector` | `map[string]string` | No | Yes | -- | Node selector for control plane pods (max 20 entries) |
| `flannelEnabled` | `bool` | No | Yes | `true` | Enable Flannel as the CNI plugin |
| `dpuDeploymentRef` | `{name, namespace}` | Yes | Yes | -- | Cross-namespace reference to DPUDeployment for ignition generation |
| `machineOSURL` | `string` | No | No | -- | Custom OS image URL for ignition configuration |

#### Status

- **`phase`** -- current lifecycle phase (see [DPFHCPProvisioner Lifecycle Phases](#dpfhcpprovisioner-lifecycle-phases))
- **`conditions`** -- array of conditions tracking operator state (Ready, HostedClusterAvailable, KubeConfigInjected, CSRAutoApprovalActive, SecretsValid, BlueFieldOCPLayerImageFound, MetalLBConfigured, IgnitionConfigured, etc.)
- **`hostedClusterRef`** -- reference to the created HostedCluster
- **`kubeConfigSecretRef`** -- reference to the injected kubeconfig Secret
- **`blueFieldOCPLayerImage`** -- matched BlueField OCP layer image URL

#### Example

```yaml
apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPProvisioner
metadata:
  name: dpfhcpprovisioner-sample
  namespace: dpf-hcp-provisioner-system
spec:
  dpuClusterRef:
    name: dpu-cluster-sample
    namespace: dpf-operator-system
  baseDomain: clusters.example.com
  ocpReleaseImage: quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-multi
  sshKeySecretRef:
    name: prod-ssh-key
  pullSecretRef:
    name: prod-pull-secret
  etcdStorageClass: ceph-rbd-retain
  dpuDeploymentRef:
    name: dpu-deployment-sample
    namespace: dpf-operator-system
  controlPlaneAvailabilityPolicy: HighlyAvailable
  virtualIP: 192.168.1.100
```

### DPFHCPProvisionerConfig

**API Group:** `provisioning.dpu.hcp.io/v1alpha1`
**Scope:** Cluster
**Short Name:** `dpfhcpconfig`

A cluster-scoped singleton (must be named `default`) that provides operator-wide configuration.

| Field | Type | Default | Description                                                      |
|-------|------|---------|------------------------------------------------------------------|
| `blueFieldOCPLayerRepo` | `string` | `quay.io/edge-infrastructure/bluefield-ocp` | Container registry for BlueField OCP layer images |

```yaml
apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPProvisionerConfig
metadata:
  name: default
spec:
  blueFieldOCPLayerRepo: quay.io/edge-infrastructure/bluefield-ocp
```

## Deployment

### Helm (Recommended)

```bash
helm install dpf-hcp-provisioner ./helm/dpf-hcp-provisioner-operator \
  --namespace dpf-hcp-provisioner-system \
  --create-namespace
```

For the full configuration reference, see [docs/configuration.md](docs/configuration.md).

## Getting Started

### 1. Create the operator namespace (if not using `--create-namespace`)

```bash
kubectl create namespace dpf-hcp-provisioner-system
```

### 2. Create the SSH key secret

```bash
kubectl create secret generic prod-ssh-key \
  --namespace dpf-hcp-provisioner-system \
  --from-file=id_rsa.pub=$HOME/.ssh/id_rsa.pub
```

### 3. Create the pull secret

```bash
kubectl create secret generic prod-pull-secret \
  --namespace dpf-hcp-provisioner-system \
  --from-file=.dockerconfigjson=$HOME/.docker/config.json \
  --type=kubernetes.io/dockerconfigjson
```

### 4. Create a DPFHCPProvisioner CR

```bash
kubectl apply -f config/samples/provisioning_v1alpha1_dpfhcpprovisioner.yaml
```

### 5. Monitor the provisioning status

```bash
# Watch the phase transitions
kubectl get dpfhcp -n dpf-hcp-provisioner-system -w

# Inspect detailed conditions
kubectl describe dpfhcp dpfhcpprovisioner-sample -n dpf-hcp-provisioner-system
```

### 6. Access the hosted cluster kubeconfig

Once the phase reaches `Ready`, the kubeconfig is injected into the DPUCluster CR:

```bash
kubectl get secret -n <dpucluster-namespace> -l app.kubernetes.io/managed-by=dpf-hcp-provisioner
```

## Troubleshooting

Check operator logs and CR conditions to diagnose issues:

```bash
# Operator logs
kubectl logs -n dpf-hcp-provisioner-system deployment/dpf-hcp-provisioner-operator -f

# CR conditions
kubectl describe dpfhcp <name> -n dpf-hcp-provisioner-system
```

For a full guide on common issues and debugging, see [docs/troubleshooting.md](docs/troubleshooting.md).

## Development

```bash
# Build the operator binary
make build

# Run unit tests
make test

# Run linter
make lint

# Build container image
make docker-build IMG=<registry/image:tag>

# Push container image
make docker-push IMG=<registry/image:tag>

# Generate CRDs and RBAC manifests
make manifests

# Generate DeepCopy methods
make generate

# Package and push Helm chart
make helm-package
make helm-push
```

### Development Environment with openshift-dpf

For developers looking to deploy, run, and test the operator in a full end-to-end environment, the [openshift-dpf](https://github.com/rh-ecosystem-edge/openshift-dpf) repository provides automation for setting up the complete DPF stack, including all prerequisites and the DPF HCP Provisioner Operator.

## License

Copyright 2025. Licensed under the Apache License, Version 2.0.
