# DPF-HCP Bridge Operator Helm Chart

This Helm chart deploys the DPF-HCP Bridge Operator, which manages DPU (Data Processing Unit) clusters with HyperShift hosted control planes on NVIDIA BlueField DPUs.

## Description

The DPF-HCP Bridge Operator automates the provisioning and management of OpenShift clusters on BlueField DPU hardware by:

- Creating and managing HyperShift HostedCluster custom resources
- Validating DPUCluster references and cluster types
- Resolving OCP release images to BlueField-compatible container images
- Injecting kubeconfig credentials into DPUCluster CRs for cluster access
- Managing the full lifecycle of DPU-based OpenShift clusters

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

### Verification Commands

```bash
# Verify Helm is installed
helm version

# Verify OpenShift cluster access
oc cluster-info

# Check MCE Operator
oc get csv -n multicluster-engine | grep multicluster-engine

# Check DPF Operator
oc get csv -A | grep dpf-operator

# Check HyperShift Operator
oc get deployment -n hypershift

# Check ODF Operator
oc get csv -n openshift-storage | grep odf-operator

# Check MetalLB Operator (if using LoadBalancer)
oc get csv -n metallb-system | grep metallb-operator
```

## Installation

### Quick Start

Install the operator with default values:

```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator
```

### Custom Installation

Create a custom `my-values.yaml` file:

```yaml
# Custom image repository
image:
  repository: quay.io/myorg/dpf-hcp-bridge-operator
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

# Add BlueField image mappings
blueFieldImages:
  "4.19.0-ec.5": "<bluefield-container-image-url>"
  "4.18.0": "<bluefield-container-image-url>"
  "4.17.0": "<bluefield-container-image-url>"

# Node placement for operator pod
nodeSelector:
  node-role.kubernetes.io/master: ""

tolerations:
  - key: "node-role.kubernetes.io/master"
    operator: "Exists"
    effect: "NoSchedule"
```

Install with custom values:

```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator -f my-values.yaml
```

### Install in a Custom Namespace

By default, the operator is installed in the `dpf-hcp-bridge-system` namespace. To use a different namespace:

```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  --set namespace=my-custom-namespace
```

## Configuration

### Configuration Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `image.repository` | Operator container image repository | `quay.io/lhadad/dpf-hcp-bridge-operator` |
| `image.tag` | Operator image tag | `v0.1.0` |
| `image.pullPolicy` | Image pull policy | `IfNotPresent` |
| `replicaCount` | Number of operator replicas | `1` |
| `namespace` | Namespace for operator deployment | `dpf-hcp-bridge-system` |
| `serviceAccount.name` | Service account name | `dpf-hcp-bridge-operator-controller-manager` |
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
| `nodeSelector` | Node selector for pod placement (defaults to master nodes) | `{node-role.kubernetes.io/master: ""}` |
| `tolerations` | Tolerations for pod placement (allows scheduling on masters) | `[{key: node-role.kubernetes.io/master, operator: Exists, effect: NoSchedule}]` |
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
helm upgrade dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator -f values.yaml
```

#### Adding Mappings Directly to ConfigMap

```bash
kubectl edit configmap ocp-bluefield-images -n dpf-hcp-bridge-system
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
```

## Usage

### Creating a DPFHCPBridge Custom Resource

Once the operator is installed, you can create DPFHCPBridge CRs to provision DPU clusters.

#### Example: Basic DPFHCPBridge CR

```yaml
apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPBridge
metadata:
  name: prod-dpu-cluster
  namespace: default
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

#### Creating Required Secrets

Before creating a DPFHCPBridge CR, create the required secrets:

```bash
# Create pull secret
kubectl create secret generic my-pull-secret \
  --from-file=pullsecret=/path/to/pull-secret.json \
  -n default

# Create SSH key secret
kubectl create secret generic my-ssh-key \
  --from-file=ssh-publickey=/path/to/id_rsa.pub \
  -n default
```

#### Applying the CR

```bash
kubectl apply -f dpfhcpbridge.yaml
```

### Monitoring DPFHCPBridge Resources

```bash
# List all DPFHCPBridge resources
kubectl get dpfhcpbridge -A

# Describe a specific resource
kubectl describe dpfhcpbridge prod-dpu-cluster -n default

# Watch status changes
kubectl get dpfhcpbridge prod-dpu-cluster -n default -w

# Check events
kubectl get events -n default --field-selector involvedObject.name=prod-dpu-cluster
```

### Understanding Status

The DPFHCPBridge status provides detailed information about the provisioning process:

```bash
kubectl get dpfhcpbridge prod-dpu-cluster -n default -o jsonpath='{.status}' | jq
```

Key status fields:
- `phase`: Current lifecycle phase (Pending, Provisioning, Ready, Failed, Deleting)
- `conditions`: Detailed condition information
- `hostedClusterRef`: Reference to created HostedCluster
- `kubeConfigSecretRef`: Reference to kubeconfig secret
- `blueFieldContainerImageAvailable`: BlueField image resolution status

## Upgrading

### Upgrade the Operator

To upgrade the operator to a new version:

```bash
# Update image tag in values.yaml
image:
  tag: v0.2.0

# Upgrade the release
helm upgrade dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator -f values.yaml
```

### Upgrade with New Configuration

```bash
helm upgrade dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  --set image.tag=v0.2.0 \
  --set resources.limits.memory=1Gi
```

### Rollback

If an upgrade fails, rollback to the previous version:

```bash
helm rollback dpf-hcp-bridge
```

## Uninstallation

### Uninstall the Operator

```bash
helm uninstall dpf-hcp-bridge
```

**Note**: This will remove the operator but NOT the CRDs or existing DPFHCPBridge resources.

### Clean Up CRDs and Resources

To completely remove all DPFHCPBridge resources and CRDs:

```bash
# Delete all DPFHCPBridge resources
kubectl delete dpfhcpbridge --all -A

# Delete the CRD (this will also delete all CRs)
kubectl delete crd dpfhcpbridges.provisioning.dpu.hcp.io

# Uninstall Helm release
helm uninstall dpf-hcp-bridge

# Delete namespace (if desired)
kubectl delete namespace dpf-hcp-bridge-system
```

**WARNING**: Deleting DPFHCPBridge CRs will trigger cleanup of associated HostedClusters. Ensure you have backups before deleting production resources.

## Troubleshooting

### Operator Not Starting

Check pod status and logs:

```bash
kubectl get pods -n dpf-hcp-bridge-system
kubectl logs -n dpf-hcp-bridge-system -l control-plane=controller-manager
```

Common issues:
- Image pull errors: Verify image repository and credentials
- RBAC issues: Ensure ClusterRole and ClusterRoleBinding are created
- Resource limits: Increase CPU/memory if pod is being OOMKilled

### DPFHCPBridge Stuck in Pending

Check the resource status:

```bash
kubectl describe dpfhcpbridge <name> -n <namespace>
```

Common causes:
- Missing BlueField image mapping in ConfigMap
- Referenced DPUCluster not found
- Pull secret or SSH key secret missing
- Invalid spec fields

### BlueField Image Not Found

Verify the ConfigMap contains the mapping:

```bash
kubectl get configmap ocp-bluefield-images -n dpf-hcp-bridge-system -o yaml
```

Add the required mapping if missing.

### HostedCluster Creation Failed

Check HyperShift operator logs:

```bash
kubectl logs -n hypershift -l app=operator
```

Verify prerequisites:
- ODF operator is running
- Storage class exists
- MetalLB operator is configured (if using LoadBalancer)

### Kubeconfig Injection Failed

Verify:
- DPUCluster CR exists and is accessible
- Operator has permissions to update DPUCluster CRs
- HostedCluster kubeconfig secret was created

```bash
kubectl get hostedcluster -n clusters
kubectl get secret -n clusters
```

## Development

### Testing the Chart

Lint the chart:

```bash
helm lint ./helm/dpf-hcp-bridge-operator
```

Dry-run installation:

```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator --dry-run --debug
```

Template rendering:

```bash
helm template dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator
```

### Building Custom Images

If you need to build a custom operator image:

```bash
# Build the operator
make docker-build IMG=quay.io/myorg/dpf-hcp-bridge-operator:custom

# Push to registry
make docker-push IMG=quay.io/myorg/dpf-hcp-bridge-operator:custom

# Install with custom image
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  --set image.repository=quay.io/myorg/dpf-hcp-bridge-operator \
  --set image.tag=custom
```

## Support

- **Documentation**: [GitHub Repository](https://github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator)
- **Issues**: [GitHub Issues](https://github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/issues)
- **Community**: Red Hat Edge Infrastructure Team

## License

This operator is provided under the Apache License 2.0.
