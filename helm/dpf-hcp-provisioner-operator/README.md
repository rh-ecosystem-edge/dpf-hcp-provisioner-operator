# DPF-HCP provisioner Operator Helm Chart

This Helm chart deploys the DPF-HCP provisioner Operator, which manages DPU (Data Processing Unit) clusters with HyperShift hosted control planes on NVIDIA BlueField DPUs.

## Table of Contents

- [Description](#description)
- [Quick Start](#quick-start)
- [Prerequisites](#prerequisites)
- [Installation](#installation)
  - [Install with Default Values](#install-with-default-values)
  - [Install with Custom Values](#install-with-custom-values)
  - [Install in a Custom Namespace](#install-in-a-custom-namespace)
- [Configuration](#configuration)
  - [Configuration Parameters](#configuration-parameters)
  - [BlueField Image Mappings](#bluefield-image-mappings)
  - [Resource Requirements](#resource-requirements)
  - [High Availability](#high-availability)
  - [Node Placement](#node-placement)
- [Usage](#usage)
  - [Creating Required Secrets](#creating-required-secrets)
  - [Creating a DPFHCPProvisioner CR](#creating-a-dpfhcpprovisioner-cr)
  - [Monitoring DPFHCPProvisioner Resources](#monitoring-dpfhcpprovisioner-resources)
  - [Understanding Status](#understanding-status)
- [Upgrading](#upgrading)
- [Uninstallation](#uninstallation)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [Support](#support)

## Description

The DPF-HCP provisioner Operator addresses this challenge by completely abstracting Hypershift objects and their management from DPU users. It treats the hosted cluster as a "black box," providing a simplified interface through the DPFHCPProvisioner custom resource that maintains a 1:1:1 relationship with DPUCluster and HostedCluster resources. By automating the full lifecycle management, BlueField container image mapping and validation, CSR approvals, and status translation, the operator minimizes manual user actions and eliminates the need to understand Hypershift internals.

## Quick Start

Install the operator with a single command:

```bash
helm install dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.1.0 \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

Verify installation:

```bash
kubectl get pods -n dpf-hcp-provisioner-system
```

Expected output:
```
NAME                                           READY   STATUS    RESTARTS   AGE
dpf-hcp-provisioner-operator-xxxxxxxxxx-xxxxx       1/1     Running   0          30s
```

## Prerequisites

Before installing this Helm chart, ensure the following are installed and configured on your **management cluster** (OpenShift):

| Component | Version | Purpose | Documentation |
|-----------|---------|---------|---------------|
| **Helm** | 3.x+ | Package manager for Kubernetes | [helm.sh](https://helm.sh) |
| **OpenShift** | 4.17+ | Management cluster platform | [openshift.com](https://www.openshift.com) |
| **MCE Operator** | Latest | Multi-Cluster Engine for hosted control plane management | [MCE Docs](https://access.redhat.com/documentation/en-us/red_hat_advanced_cluster_management) |
| **DPF Operator** | Latest | Manages DPUCluster CRs | DPF Documentation |
| **HyperShift Operator** | Latest | Provides HostedCluster CRD and control plane management | [HyperShift Docs](https://hypershift-docs.netlify.app/) |
| **ODF Operator** | Latest | OpenShift Data Foundation for persistent storage (etcd volumes) | [ODF Docs](https://access.redhat.com/documentation/en-us/red_hat_openshift_data_foundation) |
| **MetalLB Operator** | Latest | Load balancer for exposing services (required if using LoadBalancer service type) | [MetalLB Docs](https://metallb.universe.tf/) |

## Installation

### Install with Default Values

Install the operator from the OCI registry:

```bash
helm install dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.1.0 \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

### Install with Custom Values

Create a custom `my-values.yaml` file:

```yaml
# Custom image repository
image:
  repository: quay.io/myorg/dpf-hcp-provisioner-operator
  tag: v0.2.0
  pullPolicy: Always

# Increase resources for production
resources:
  limits:
    cpu: 1000m
    memory: 1Gi
  requests:
    cpu: 200m
    memory: 256Mi

# Set log level
logLevel: debug

# Add BlueField image mappings
blueFieldImages:
  "4.19.0-ec.5": "<bluefield-container-image-url>"
  "4.18.0": "<bluefield-container-image-url>"
  "4.17.0": "<bluefield-container-image-url>"

# Node placement for operator pod
placement:
  target: master
```

Install with custom values:

```bash
helm install dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.1.0 \
  --values my-values.yaml \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

### Install in a Custom Namespace

By default, the operator is installed in the `dpf-hcp-provisioner-system` namespace. To use a different namespace:

```bash
helm install dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.1.0 \
  --set namespace=my-custom-namespace \
  --namespace my-custom-namespace --create-namespace
```

## Configuration

### Configuration Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `image.repository` | Operator container image repository | `quay.io/lhadad/dpf-hcp-provisioner-operator` |
| `image.tag` | Operator image tag | `v0.1.0` |
| `image.pullPolicy` | Image pull policy | `Always` |
| `replicaCount` | Number of operator replicas | `1` |
| `namespace` | Namespace for operator deployment | `dpf-hcp-provisioner-system` |
| `serviceAccount.name` | Service account name (empty = auto-generate from release name) | `""` (uses release name) |
| `serviceAccount.annotations` | Service account annotations | `{}` |
| `resources.limits.cpu` | CPU limit | `500m` |
| `resources.limits.memory` | Memory limit | `512Mi` |
| `resources.requests.cpu` | CPU request | `100m` |
| `resources.requests.memory` | Memory request | `128Mi` |
| `logLevel` | Logging level (debug, info, error) | `info` |
| `leaderElection.enabled` | Enable leader election | `true` |
| `healthProbe.port` | Health probe port | `8081` |
| `healthProbe.livenessProbe.initialDelaySeconds` | Liveness probe initial delay | `15` |
| `healthProbe.livenessProbe.periodSeconds` | Liveness probe period | `20` |
| `healthProbe.readinessProbe.initialDelaySeconds` | Readiness probe initial delay | `5` |
| `healthProbe.readinessProbe.periodSeconds` | Readiness probe period | `10` |
| `placement.target` | Target node type (master, worker, custom) | `master` |
| `nodeSelector` | Node selector for pod placement (used when placement.target=custom) | `{}` |
| `tolerations` | Tolerations for pod placement (used when placement.target=custom) | `[]` |
| `affinity` | Affinity rules for pod placement | `{}` |
| `blueFieldImages` | OCP-to-BlueField image mappings | `{}` |
| `commonLabels` | Additional labels for all resources | `{}` |
| `commonAnnotations` | Additional annotations for all resources | `{}` |

### BlueField Image Mappings

The operator requires a mapping between OCP release images and BlueField-compatible container images. This mapping is stored in a ConfigMap named `ocp-bluefield-images`.

#### Adding Mappings via values.yaml

```yaml
blueFieldImages:
  "4.19.0-ec.5": "<bluefield-container-image-url>"
  "4.18.0": "<bluefield-container-image-url>"
  "4.17.0": "<bluefield-container-image-url>"
```

Then upgrade the release:

```bash
helm upgrade dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.1.0 \
  --values values.yaml \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

#### Adding Mappings Directly to ConfigMap

```bash
kubectl edit configmap ocp-bluefield-images -n dpf-hcp-provisioner-system
```

Add mappings in the `data` section:

```yaml
data:
  "4.19.0-ec.5": "<bluefield-container-image-url>"
  "4.18.0": "<bluefield-container-image-url>"
```

### Resource Requirements

For production environments, consider increasing resource limits:

```yaml
resources:
  limits:
    cpu: 1000m
    memory: 1Gi
  requests:
    cpu: 200m
    memory: 256Mi
```

### High Availability

The operator supports leader election by default. For high availability, increase replica count:

```yaml
replicaCount: 2
leaderElection:
  enabled: true

# Spread replicas across zones
affinity:
  podAntiAffinity:
    preferredDuringSchedulingIgnoredDuringExecution:
      - weight: 100
        podAffinityTerm:
          labelSelector:
            matchExpressions:
              - key: control-plane
                operator: In
                values:
                  - controller-manager
          topologyKey: topology.kubernetes.io/zone
```

### Node Placement

Control where the operator pod runs using the `placement.target` parameter:

```yaml
# Run on control plane nodes (default)
placement:
  target: master

# Run on worker nodes
placement:
  target: worker

# Custom placement with manual nodeSelector/tolerations
placement:
  target: custom

nodeSelector:
  custom-label: custom-value

tolerations:
  - key: "custom-taint"
    operator: "Exists"
    effect: "NoSchedule"
```

## Usage

### Creating Required Secrets

Before creating a DPFHCPProvisioner CR, create the required secrets in the **same namespace** as your DPFHCPProvisioner CR:

```bash
# Create namespace for your DPFHCPProvisioner CR (if it doesn't exist)
kubectl create namespace my-dpu-clusters

# Create pull secret (must be in same namespace as DPFHCPProvisioner CR)
kubectl create secret generic my-pull-secret \
  --from-file=.dockerconfigjson=/path/to/pull-secret.json \
  --namespace my-dpu-clusters

# Create SSH key secret (must be in same namespace as DPFHCPProvisioner CR)
kubectl create secret generic my-ssh-key \
  --from-file=id_rsa.pub=/path/to/id_rsa.pub \
  --namespace my-dpu-clusters
```

### Creating a DPFHCPProvisioner CR

Once the operator is installed and secrets are created, you can create DPFHCPProvisioner CRs to provision DPU clusters.

#### Example: Basic DPFHCPProvisioner CR

```yaml
apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPProvisioner
metadata:
  name: prod-dpu-cluster
  namespace: my-dpu-clusters
spec:
  # Base domain for cluster DNS
  baseDomain: clusters.example.com

  # Reference to existing DPUCluster CR
  dpuClusterRef:
    name: my-dpucluster
    namespace: dpu-clusters

  # Storage class for etcd volumes
  etcdStorageClass: ocs-storagecluster-ceph-rbd

  # OCP release image
  ocpReleaseImage: quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-x86_64

  # Pull secret reference (must exist in same namespace)
  pullSecretRef:
    name: my-pull-secret

  # SSH key reference (must exist in same namespace)
  sshKeySecretRef:
    name: my-ssh-key

  # Virtual IP for load balancer (required for HighlyAvailable control plane)
  virtualIP: 192.168.1.100

  # Control plane availability policy
  controlPlaneAvailabilityPolicy: HighlyAvailable
```

#### Applying the CR

```bash
kubectl apply -f dpfhcpprovisioner.yaml
```

### Monitoring DPFHCPProvisioner Resources

```bash
# List all DPFHCPProvisioner resources
kubectl get dpfhcpprovisioner -A

# Short form using alias
kubectl get dpfhcp -A

# Describe a specific resource
kubectl describe dpfhcpprovisioner prod-dpu-cluster -n my-dpu-clusters

# Watch status changes
kubectl get dpfhcpprovisioner prod-dpu-cluster -n my-dpu-clusters -w

# Check events
kubectl get events -n my-dpu-clusters --field-selector involvedObject.name=prod-dpu-cluster

# View operator logs
kubectl logs -n dpf-hcp-provisioner-system -l control-plane=controller-manager -f
```

### Understanding Status

The DPFHCPProvisioner status provides detailed information about the provisioning process:

```bash
kubectl get dpfhcpprovisioner prod-dpu-cluster -n my-dpu-clusters -o jsonpath='{.status}' | jq
```

Key status fields:
- `phase`: Current lifecycle phase (Pending, Provisioning, Ready, Failed, Deleting)
- `conditions`: Detailed condition information organized by category:
  - **DPFHCPProvisioner-specific conditions:**
    - `Ready`: Overall operational status of the DPFHCPProvisioner
    - `KubeConfigInjected`: Kubeconfig successfully injected into DPUCluster CR
    - `HostedClusterCleanup`: Status of HostedCluster deletion during finalizer cleanup
  - **Validation conditions:**
    - `SecretsValid`: Required secrets (pull secret, SSH key) are valid
    - `BlueFieldImageResolved`: BlueField container image successfully resolved
    - `DPUClusterMissing`: Referenced DPUCluster exists
    - `ClusterTypeValid`: DPUCluster type is supported
    - `DPUClusterInUse`: DPUCluster is not already in use by another DPFHCPProvisioner
  - **HostedCluster conditions (mirrored):**
    - `HostedClusterAvailable`: HostedCluster has a healthy control plane
    - `HostedClusterProgressing`: HostedCluster is attempting deployment or upgrade
    - `HostedClusterDegraded`: HostedCluster is encountering errors
    - `ValidReleaseImage`: Release image in spec is valid for HostedCluster
    - `ValidReleaseInfo`: Release contains all required HyperShift images
    - `IgnitionEndpointAvailable`: Ignition server is available
    - `IgnitionServerValidReleaseInfo`: Release has local ignition provider images
- `hostedClusterRef`: Reference to created HostedCluster
- `kubeConfigSecretRef`: Reference to kubeconfig secret in DPUCluster namespace
- `blueFieldContainerImage`: Resolved BlueField container image URL

## Upgrading

### Upgrade to a New Version

To upgrade the operator to a new version:

```bash
helm upgrade dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.2.0 \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

### Upgrade with Custom Values

```bash
helm upgrade dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.2.0 \
  --values my-values.yaml \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

### Upgrade with Override Parameters

```bash
helm upgrade dpf-hcp-provisioner-operator oci://quay.io/lhadad/charts/dpf-hcp-provisioner-operator \
  --version 0.2.0 \
  --set resources.limits.memory=1Gi \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

### Rollback

If an upgrade fails, rollback to the previous version:

```bash
helm rollback dpf-hcp-provisioner-operator --namespace dpf-hcp-provisioner-system
```

## Uninstallation

### Uninstall the Operator

```bash
helm uninstall dpf-hcp-provisioner-operator --namespace dpf-hcp-provisioner-system
```

**Note**: This will remove the operator but NOT the CRDs or existing DPFHCPProvisioner resources.

### Clean Up CRDs and Resources

To completely remove all DPFHCPProvisioner resources and CRDs:

```bash
# Delete all DPFHCPProvisioner resources first
kubectl delete dpfhcpprovisioner --all -A

# Uninstall Helm release
helm uninstall dpf-hcp-provisioner-operator --namespace dpf-hcp-provisioner-system

# Optionally delete the CRD
kubectl delete crd dpfhcpprovisioners.provisioning.dpu.hcp.io

# Optionally delete namespace
kubectl delete namespace dpf-hcp-provisioner-system
```

**WARNING**: Deleting DPFHCPProvisioner CRs will trigger cleanup of associated HostedClusters and NodePools. Ensure you have backups before deleting production resources.

## Troubleshooting

### Operator Not Starting

Check pod status and logs:

```bash
kubectl get pods -n dpf-hcp-provisioner-system
kubectl describe pod -n dpf-hcp-provisioner-system -l control-plane=controller-manager
kubectl logs -n dpf-hcp-provisioner-system -l control-plane=controller-manager
```

### DPFHCPProvisioner Stuck in Pending

The Pending phase is the initial validation state. Check the resource status and conditions:

```bash
kubectl describe dpfhcpprovisioner <name> -n <namespace>
kubectl get dpfhcpprovisioner <name> -n <namespace> -o jsonpath='{.status.conditions}' | jq
```

If stuck in Pending for more than a few seconds, check the operator logs:

```bash
kubectl logs -n dpf-hcp-provisioner-system -l control-plane=controller-manager
```

### DPFHCPProvisioner in Failed Phase

Check the resource conditions to identify the validation failure:

```bash
kubectl describe dpfhcpprovisioner <name> -n <namespace>
```

Common causes:
- **Missing BlueField image mapping**: Check ConfigMap `ocp-bluefield-images`
- **Referenced DPUCluster not found**: Verify DPUCluster exists
- **Pull secret or SSH key secret missing**: Verify secrets exist in same namespace
- **Invalid spec fields**: Check validation errors in conditions

### BlueField Image Not Found

Verify the ConfigMap contains the mapping:

```bash
kubectl get configmap ocp-bluefield-images -n dpf-hcp-provisioner-system -o yaml
```

Add the required mapping if missing:

```bash
kubectl edit configmap ocp-bluefield-images -n dpf-hcp-provisioner-system
```

### HostedCluster Creation Failed

Check HyperShift operator logs:

```bash
kubectl logs -n hypershift -l app=operator
```

Verify prerequisites:
- ODF operator is running
- Storage class exists (`kubectl get storageclass`)
- MetalLB operator is configured (if using LoadBalancer)
- Virtual IP is routable in management cluster network

### Kubeconfig Injection Failed

Verify:
- DPUCluster CR exists and is accessible
- Operator has permissions to update DPUCluster CRs in target namespace
- HostedCluster kubeconfig secret was created

```bash
kubectl get dpucluster -n <dpucluster-namespace>
kubectl get secret -n <dpfhcpprovisioner-namespace> | grep kubeconfig
```

## Development

### Install from Local Source

For development, you can install from local chart source:

```bash
# Clone the repository
git clone https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator
cd dpf-hcp-provisioner-operator

# Install from local chart
helm install dpf-hcp-provisioner-operator ./helm/dpf-hcp-provisioner-operator \
  --namespace dpf-hcp-provisioner-system --create-namespace

# Or with development values
helm install dpf-hcp-provisioner-operator ./helm/dpf-hcp-provisioner-operator \
  --values helm/dpf-hcp-provisioner-operator/values-development.yaml \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

### Testing the Chart

Lint the chart:

```bash
helm lint ./helm/dpf-hcp-provisioner-operator
```

Dry-run installation:

```bash
helm install dpf-hcp-provisioner-operator ./helm/dpf-hcp-provisioner-operator --dry-run --debug
```

Template rendering:

```bash
helm template dpf-hcp-provisioner-operator ./helm/dpf-hcp-provisioner-operator
```

### Building Custom Images

If you need to build a custom operator image:

```bash
# Build the operator
make docker-build IMG=quay.io/myorg/dpf-hcp-provisioner-operator:custom

# Push to registry
make docker-push IMG=quay.io/myorg/dpf-hcp-provisioner-operator:custom

# Install from local chart with custom image
helm install dpf-hcp-provisioner-operator ./helm/dpf-hcp-provisioner-operator \
  --set image.repository=quay.io/myorg/dpf-hcp-provisioner-operator \
  --set image.tag=custom \
  --namespace dpf-hcp-provisioner-system --create-namespace
```

### Using Development Values

The chart includes pre-configured values files for different environments:

**values-development.yaml**:
- Image tag: `latest`
- Pull policy: `Always`
- Log level: `debug`
- Lower resource limits (50m CPU, 128Mi memory)
- Single replica

**values-production.yaml**:
- Image tag: `v0.1.0` (specific version)
- Pull policy: `IfNotPresent`
- Log level: `info`
- Higher resource limits (200m CPU, 256Mi memory)
- 2 replicas with zone anti-affinity

Install with specific environment:

```bash
# Development
helm install dpf-hcp-provisioner-operator ./helm/dpf-hcp-provisioner-operator \
  --values helm/dpf-hcp-provisioner-operator/values-development.yaml

# Production
helm install dpf-hcp-provisioner-operator ./helm/dpf-hcp-provisioner-operator \
  --values helm/dpf-hcp-provisioner-operator/values-production.yaml
```

## Support

- **Documentation**: [GitHub Repository](https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator)
- **Issues**: [GitHub Issues](https://github.com/rh-ecosystem-edge/dpf-hcp-provisioner-operator/issues)
- **Community**: Red Hat Ecosystem Edge Team

## License

This operator is provided under the Apache License 2.0.
