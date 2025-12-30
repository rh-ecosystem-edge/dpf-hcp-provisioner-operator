# Helm Chart Summary

## Overview

This document provides a complete summary of the DPF-HCP Bridge Operator Helm chart.

## Chart Location

```
/home/lhadad/go/bin/src/github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/helm/dpf-hcp-bridge-operator/
```

## Directory Structure

```
helm/dpf-hcp-bridge-operator/
├── Chart.yaml                      # Chart metadata (name, version, description)
├── values.yaml                     # Default configuration values
├── values-production.yaml          # Production-optimized configuration
├── values-development.yaml         # Development-optimized configuration
├── README.md                       # Comprehensive documentation
├── INSTALL.md                      # Quick installation guide
├── CHANGELOG.md                    # Version history and changes
├── .helmignore                     # Files to exclude from package
│
├── crds/                           # Custom Resource Definitions
│   └── provisioning.dpu.hcp.io_dpfhcpbridges.yaml
│
├── templates/                      # Kubernetes resource templates
│   ├── _helpers.tpl               # Template helper functions
│   ├── namespace.yaml             # Namespace (dpf-hcp-bridge-system)
│   ├── serviceaccount.yaml        # ServiceAccount for operator
│   ├── clusterrole.yaml           # ClusterRole with RBAC permissions
│   ├── clusterrolebinding.yaml    # ClusterRoleBinding
│   ├── configmap-images.yaml      # ConfigMap for BlueField image mappings
│   ├── deployment.yaml            # Operator Deployment
│   └── NOTES.txt                  # Post-installation notes
│
└── examples/                       # Example CR and secret configurations
    ├── dpfhcpbridge-basic.yaml    # Basic DPFHCPBridge CR example
    ├── dpfhcpbridge-ha.yaml       # HA DPFHCPBridge CR example
    └── secrets-example.yaml       # Required secrets configuration
```

## Files and Purposes

### Core Chart Files

| File | Purpose |
|------|---------|
| `Chart.yaml` | Chart metadata including name, version (0.1.0), appVersion (v0.1.0), description, and keywords |
| `values.yaml` | Default configuration values with extensive inline documentation |
| `values-production.yaml` | Production-optimized values (HA, higher resources, node placement) |
| `values-development.yaml` | Development-optimized values (debug logging, lower resources) |
| `.helmignore` | Patterns for files to exclude from Helm package |

### Documentation Files

| File | Purpose |
|------|---------|
| `README.md` | Comprehensive documentation covering installation, configuration, usage, troubleshooting |
| `INSTALL.md` | Step-by-step quick installation guide with verification commands |
| `CHANGELOG.md` | Version history and changelog following semantic versioning |

### CRD Directory

| File | Purpose |
|------|---------|
| `crds/provisioning.dpu.hcp.io_dpfhcpbridges.yaml` | DPFHCPBridge Custom Resource Definition (copied from config/crd/bases/) |

### Template Directory

| File | Purpose |
|------|---------|
| `templates/_helpers.tpl` | Helm template helpers for labels, names, selectors, namespace, image |
| `templates/namespace.yaml` | Creates dpf-hcp-bridge-system namespace with labels |
| `templates/serviceaccount.yaml` | ServiceAccount for operator pod |
| `templates/clusterrole.yaml` | ClusterRole with RBAC permissions for DPFHCPBridge CRs |
| `templates/clusterrolebinding.yaml` | Binds ClusterRole to ServiceAccount |
| `templates/configmap-images.yaml` | ConfigMap (ocp-bluefield-images) for OCP-to-BlueField mappings |
| `templates/deployment.yaml` | Operator Deployment with health probes, security contexts, resources |
| `templates/NOTES.txt` | Post-installation notes displayed after `helm install` |

### Examples Directory

| File | Purpose |
|------|---------|
| `examples/dpfhcpbridge-basic.yaml` | Basic DPFHCPBridge CR example with minimal configuration |
| `examples/dpfhcpbridge-ha.yaml` | HA DPFHCPBridge CR example for production |
| `examples/secrets-example.yaml` | Example pull secret and SSH key secret configuration |

## Key Features

### 1. Configurable Deployment
- Image repository and tag customization
- Replica count (supports HA with leader election)
- Resource limits and requests
- Log level (debug, info, error)
- Node placement (nodeSelector, tolerations, affinity)

### 2. Security
- Non-root container execution
- Seccomp profile (RuntimeDefault)
- Dropped all capabilities
- No privilege escalation
- Follows Pod Security Standards (restricted)

### 3. RBAC
- ServiceAccount for operator
- ClusterRole with minimal required permissions
- ClusterRoleBinding for authorization

### 4. BlueField Image Mappings
- ConfigMap for OCP-to-BlueField image version mappings
- Configurable via values.yaml or direct ConfigMap edit
- Initially empty with helpful comments

### 5. Health Probes
- Liveness probe on /healthz endpoint
- Readiness probe on /readyz endpoint
- Configurable delays and periods

### 6. Post-Installation
- NOTES.txt displays helpful next steps
- Links to documentation
- Instructions for adding BlueField image mappings
- Example CR creation

## Installation Commands

### Basic Installation
```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator
```

### Production Installation
```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  -f helm/dpf-hcp-bridge-operator/values-production.yaml
```

### Development Installation
```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  -f helm/dpf-hcp-bridge-operator/values-development.yaml
```

### Custom Installation
```bash
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  --set image.tag=v0.2.0 \
  --set replicaCount=2 \
  --set logLevel=debug
```

## Upgrade Commands

```bash
# Upgrade with new image version
helm upgrade dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  --set image.tag=v0.2.0

# Upgrade with production values
helm upgrade dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  -f helm/dpf-hcp-bridge-operator/values-production.yaml

# Rollback to previous version
helm rollback dpf-hcp-bridge
```

## Verification Commands

```bash
# Verify chart structure
tree helm/dpf-hcp-bridge-operator

# Lint chart (requires helm installed)
helm lint ./helm/dpf-hcp-bridge-operator

# Dry-run installation
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator --dry-run --debug

# Template rendering
helm template dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator

# Show computed values
helm get values dpf-hcp-bridge
```

## Configuration Values

### Image Configuration
```yaml
image:
  repository: quay.io/lhadad/dpf-hcp-bridge-operator
  tag: v0.1.0
  pullPolicy: IfNotPresent
```

### Resources
```yaml
resources:
  limits:
    cpu: 500m
    memory: 512Mi
  requests:
    cpu: 100m
    memory: 128Mi
```

### BlueField Images
```yaml
blueFieldImages:
  "4.19.0-ec.5": "<bluefield-container-image-url>"
  "4.18.0": "<bluefield-container-image-url>"
```

### High Availability
```yaml
replicaCount: 2
leaderElection:
  enabled: true
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

## Resources Created by Chart

When installed, the Helm chart creates the following Kubernetes resources:

1. **Namespace**: `dpf-hcp-bridge-system` (or custom via values.namespace)
2. **ServiceAccount**: `dpf-hcp-bridge-operator-controller-manager`
3. **ClusterRole**: `<release-name>-dpf-hcp-bridge-operator-manager-role`
4. **ClusterRoleBinding**: `<release-name>-dpf-hcp-bridge-operator-manager-rolebinding`
5. **ConfigMap**: `ocp-bluefield-images` (for BlueField image mappings)
6. **Deployment**: `<release-name>-dpf-hcp-bridge-operator-controller-manager`
7. **CRD**: `dpfhcpbridges.provisioning.dpu.hcp.io`

## Prerequisites

The following must be installed on the management cluster before deploying this chart:

1. **Helm** (3.x+)
2. **OpenShift** (4.17+)
3. **MCE Operator**
4. **DPF Operator**
5. **HyperShift Operator**
6. **ODF Operator**
7. **MetalLB Operator** (if using LoadBalancer services)

## Next Steps

After installing the Helm chart:

1. Verify operator is running
2. Add BlueField image mappings to ConfigMap
3. Create required secrets (pull secret, SSH key)
4. Create DPFHCPBridge CR
5. Monitor CR status and operator logs

See INSTALL.md for detailed step-by-step instructions.

## Support

- **Repository**: https://github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator
- **Issues**: https://github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/issues
- **Documentation**: See README.md in helm/dpf-hcp-bridge-operator/

## Chart Version Information

- **Chart Version**: 0.1.0
- **Operator Version**: v0.1.0
- **API Version**: v2 (Helm 3)
- **Kubernetes Version**: 1.28+ (OpenShift 4.17+)
