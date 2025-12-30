# Quick Installation Guide

This guide provides step-by-step instructions for installing the DPF-HCP Bridge Operator using Helm.

## Prerequisites Checklist

Before installing, ensure you have:

- [ ] OpenShift 4.17+ cluster running (management cluster)
- [ ] `helm` CLI installed (version 3.x+)
- [ ] `oc` or `kubectl` CLI installed and configured
- [ ] MCE Operator installed
- [ ] DPF Operator installed
- [ ] HyperShift Operator installed
- [ ] ODF Operator installed
- [ ] MetalLB Operator installed (if using LoadBalancer services)

## Installation Steps

### Step 1: Verify Cluster Access

```bash
oc cluster-info
oc whoami
```

### Step 2: Install the Helm Chart

#### Option A: Install with Default Values

```bash
cd /path/to/dpf-hcp-bridge-operator
helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator
```

#### Option B: Install with Custom Values

Create a custom values file:

```bash
cat > my-values.yaml <<EOF
image:
  repository: quay.io/lhadad/dpf-hcp-bridge-operator
  tag: v0.1.0
  pullPolicy: IfNotPresent

resources:
  limits:
    cpu: 1000m
    memory: 1Gi
  requests:
    cpu: 200m
    memory: 256Mi

logLevel: debug

blueFieldImages:
  "4.19.0-ec.5": "<bluefield-container-image-url>"
  "4.18.0": "<bluefield-container-image-url>"
EOF

helm install dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator -f my-values.yaml
```

### Step 3: Verify Installation

Check the operator pod is running:

```bash
oc get pods -n dpf-hcp-bridge-system
```

Expected output:
```
NAME                                           READY   STATUS    RESTARTS   AGE
dpf-hcp-bridge-xxxxxxxxxx-xxxxx                1/1     Running   0          30s
```

Check the CRD is installed:

```bash
oc get crd dpfhcpbridges.provisioning.dpu.hcp.io
```

View operator logs:

```bash
oc logs -n dpf-hcp-bridge-system -l control-plane=controller-manager -f
```

### Step 4: Add BlueField Image Mappings (if not done via values.yaml)

If you didn't include `blueFieldImages` in your values.yaml, add them now:

```bash
oc edit configmap ocp-bluefield-images -n dpf-hcp-bridge-system
```

Add entries in the `data` section:

```yaml
data:
  "4.19.0-ec.5": "<bluefield-container-image-url>"
  "4.18.0": "<bluefield-container-image-url>"
```

### Step 5: Create Required Secrets

Before creating a DPFHCPBridge CR, create the pull secret and SSH key:

```bash
# Create namespace for your DPFHCPBridge CR
oc create namespace my-dpu-clusters

# Create pull secret
oc create secret generic my-pull-secret \
  --from-file=pullsecret=$HOME/.docker/config.json \
  -n my-dpu-clusters

# Create SSH public key secret
oc create secret generic my-ssh-key \
  --from-file=ssh-publickey=$HOME/.ssh/id_rsa.pub \
  -n my-dpu-clusters
```

### Step 6: Create Your First DPFHCPBridge CR

Create a DPFHCPBridge CR file:

```bash
cat > dpfhcpbridge-example.yaml <<EOF
apiVersion: provisioning.dpu.hcp.io/v1alpha1
kind: DPFHCPBridge
metadata:
  name: my-first-dpu-cluster
  namespace: my-dpu-clusters
spec:
  baseDomain: clusters.example.com
  dpuClusterRef:
    name: my-dpucluster
    namespace: dpu-system
  etcdStorageClass: ocs-storagecluster-ceph-rbd
  ocpReleaseImage: quay.io/openshift-release-dev/ocp-release:4.19.0-ec.5-x86_64
  pullSecretRef:
    name: my-pull-secret
  sshKeySecretRef:
    name: my-ssh-key
  virtualIP: 192.168.1.100
  controlPlaneAvailabilityPolicy: HighlyAvailable
EOF
```

Apply the CR:

```bash
oc apply -f dpfhcpbridge-example.yaml
```

### Step 7: Monitor the DPFHCPBridge Resource

Watch the status:

```bash
oc get dpfhcpbridge my-first-dpu-cluster -n my-dpu-clusters -w
```

Check detailed status:

```bash
oc describe dpfhcpbridge my-first-dpu-cluster -n my-dpu-clusters
```

View events:

```bash
oc get events -n my-dpu-clusters --sort-by='.lastTimestamp'
```

## Post-Installation

### Upgrade the Operator

To upgrade to a new version:

```bash
helm upgrade dpf-hcp-bridge ./helm/dpf-hcp-bridge-operator \
  --set image.tag=v0.2.0
```

### Uninstall the Operator

To remove the operator:

```bash
# Delete all DPFHCPBridge resources first
oc delete dpfhcpbridge --all -A

# Uninstall the Helm release
helm uninstall dpf-hcp-bridge

# Optionally delete the CRD
oc delete crd dpfhcpbridges.provisioning.dpu.hcp.io

# Optionally delete the namespace
oc delete namespace dpf-hcp-bridge-system
```

## Troubleshooting

### Operator Pod Not Starting

Check pod events:

```bash
oc describe pod -n dpf-hcp-bridge-system -l control-plane=controller-manager
```

Check pod logs:

```bash
oc logs -n dpf-hcp-bridge-system -l control-plane=controller-manager
```

### Image Pull Errors

Verify the image repository and tag:

```bash
oc get deployment -n dpf-hcp-bridge-system -o jsonpath='{.items[0].spec.template.spec.containers[0].image}'
```

### RBAC Issues

Verify ClusterRole and ClusterRoleBinding:

```bash
oc get clusterrole | grep dpf-hcp-bridge
oc get clusterrolebinding | grep dpf-hcp-bridge
```

### ConfigMap Not Found

Verify the ConfigMap exists:

```bash
oc get configmap ocp-bluefield-images -n dpf-hcp-bridge-system
```

## Support

For issues and questions:
- GitHub Issues: https://github.com/rh-ecosystem-edge/dpf-hcp-bridge-operator/issues
- Documentation: See README.md in this directory

## Next Steps

After successful installation:

1. Review the [README.md](README.md) for detailed configuration options
2. Check the [operator-prd.md](../../operator-prd.md) for architecture and design
3. Explore example DPFHCPBridge CRs in [config/samples/](../../config/samples/)
