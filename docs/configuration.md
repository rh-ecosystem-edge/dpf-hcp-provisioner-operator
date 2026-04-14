# Configuration Reference

## Helm Values

The full set of configurable values for the Helm chart (`helm/dpf-hcp-provisioner-operator/`).

### Image

| Value | Type | Default | Description |
|-------|------|--------|-------------|
| `image.repository` | `string` | `` | Operator container image repository |
| `image.tag` | `string` | `` | Image tag (defaults to `Chart.appVersion` if not set) |
| `image.pullPolicy` | `string` | `Always` | Image pull policy |

### General

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `nameOverride` | `string` | `dpf-hcp-provisioner-operator` | Override chart name in labels |
| `namespace` | `string` | `dpf-hcp-provisioner-system` | Namespace for operator deployment |
| `replicaCount` | `int` | `1` | Number of operator replicas |
| `logLevel` | `string` | `info` | Log verbosity: `debug`, `info`, `error` |

### Service Account

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `serviceAccount.name` | `string` | `""` (auto-generated) | Service account name |
| `serviceAccount.annotations` | `map` | `{}` | Annotations for the service account |

### Resources

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `resources.limits.cpu` | `string` | `500m` | CPU limit |
| `resources.limits.memory` | `string` | `512Mi` | Memory limit |
| `resources.requests.cpu` | `string` | `100m` | CPU request |
| `resources.requests.memory` | `string` | `128Mi` | Memory request |

### Leader Election

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `leaderElection.enabled` | `bool` | `true` | Enable leader election for HA deployments |

### Health Probes

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `healthProbe.port` | `int` | `8081` | Health probe port |
| `healthProbe.livenessProbe.initialDelaySeconds` | `int` | `15` | Liveness probe initial delay |
| `healthProbe.livenessProbe.periodSeconds` | `int` | `20` | Liveness probe interval |
| `healthProbe.readinessProbe.initialDelaySeconds` | `int` | `5` | Readiness probe initial delay |
| `healthProbe.readinessProbe.periodSeconds` | `int` | `10` | Readiness probe interval |

### Pod Placement

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `placement.target` | `string` | `master` | Placement preset: `master`, `worker`, or `custom` |
| `nodeSelector` | `map` | `{}` | Custom node selector (only used when `placement.target=custom`) |
| `tolerations` | `list` | `[]` | Custom tolerations (only used when `placement.target=custom`) |
| `affinity` | `map` | `{}` | Custom affinity rules (only used when `placement.target=custom`) |

When `placement.target` is `master` or `worker`, the chart automatically configures node selectors and tolerations. Use `custom` to provide your own scheduling rules.

### Security

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `podSecurityContext.runAsNonRoot` | `bool` | `true` | Run as non-root user |
| `podSecurityContext.seccompProfile.type` | `string` | `RuntimeDefault` | Seccomp profile |
| `securityContext.allowPrivilegeEscalation` | `bool` | `false` | Disallow privilege escalation |
| `securityContext.capabilities.drop` | `list` | `["ALL"]` | Drop all Linux capabilities |

### Operator Configuration CR

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `provisionerConfig` | `map` | `{}` | Override fields in the deployed DPFHCPProvisionerConfig singleton |

### Metadata

| Value | Type | Default | Description |
|-------|------|---------|-------------|
| `podAnnotations` | `map` | `{kubectl.kubernetes.io/default-container: manager}` | Pod annotations |
| `commonLabels` | `map` | `{}` | Labels applied to all resources |
| `commonAnnotations` | `map` | `{}` | Annotations applied to all resources |

## DPFHCPProvisionerConfig

The `DPFHCPProvisionerConfig` is a cluster-scoped singleton CR that controls operator-wide behavior. It must be named `default`.

The Helm chart automatically creates this CR with default values. You can customize it post-deployment:

```bash
kubectl patch dpfhcpconfig default --type merge -p '{"spec":{"blueFieldOCPLayerRepo":"quay.io/edge-infrastructure/bluefield-ocp"}}'
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `spec.blueFieldOCPLayerRepo` | `string` | `quay.io/edge-infrastructure/bluefield-ocp` | Container registry repository for BlueField RHCOS OCP image layers. The operator queries this repo to find an image tag matching the OCP version. The lookup is skipped when `machineOSURL` is provided in the DPFHCPProvisioner spec. |

### Behavior

- If the `DPFHCPProvisionerConfig` CR does not exist, the operator will not reconcile any `DPFHCPProvisioner` CRs
- The config is loaded on each reconciliation cycle, so changes take effect immediately
- Deleting the config CR effectively makes the operator idle
