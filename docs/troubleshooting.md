# Troubleshooting

This guide covers common issues when using the DPF HCP Provisioner Operator and how to diagnose them.

## Viewing Operator Logs

```bash
kubectl logs -n dpf-hcp-provisioner-system deployment/dpf-hcp-provisioner-operator -f
```

For more verbose output, redeploy with `logLevel: debug` in Helm values.

## Inspecting CR Status

```bash
# Quick status overview
kubectl get dpfhcp -n <namespace>

# Detailed conditions
kubectl describe dpfhcp <name> -n <namespace>

# JSON output for scripting
kubectl get dpfhcp <name> -n <namespace> -o jsonpath='{.status.conditions}' | jq .
```

## Common Issues

### DPUCluster Not Found

**Condition:** `DPUClusterMissing = True`

**Symptoms:** CR stays in `Pending` phase.

**Cause:** The `dpuClusterRef` points to a DPUCluster that doesn't exist or is in a different namespace than specified.

**Resolution:**
1. Verify the DPUCluster exists: `kubectl get dpucluster -n <namespace>`
2. Ensure the `name` and `namespace` in `dpuClusterRef` match exactly
3. Check RBAC -- the operator needs `get` permissions on DPUCluster resources in the target namespace

### DPUCluster Type Invalid

**Condition:** `ClusterTypeValid = False`

**Symptoms:** CR stays in `Pending` phase.

**Cause:** The referenced DPUCluster is not of a supported type for HyperShift provisioning.

**Resolution:** Ensure the DPUCluster is configured with a type compatible with HyperShift-based provisioning.

### DPUCluster Already In Use

**Condition:** `DPUClusterInUse = True`

**Symptoms:** CR stays in `Pending` phase.

**Cause:** Another DPFHCPProvisioner is already using the same DPUCluster. Each DPUCluster can only be referenced by one DPFHCPProvisioner (1:1 mapping).

**Resolution:** Use a different DPUCluster or delete the existing DPFHCPProvisioner that references it.

### Secrets Validation Failure

**Condition:** `SecretsValid = False`

**Symptoms:** CR stays in `Pending` phase.

**Cause:** SSH key or pull secret is missing, or has incorrect format.

**Resolution:**
1. Verify the SSH key secret exists and contains the `id_rsa.pub` key:
   ```bash
   kubectl get secret <ssh-secret-name> -n <provisioner-namespace> -o jsonpath='{.data}' | jq 'keys'
   ```
2. Verify the pull secret exists and contains `.dockerconfigjson`:
   ```bash
   kubectl get secret <pull-secret-name> -n <provisioner-namespace> -o jsonpath='{.data}' | jq 'keys'
   ```
3. Ensure both secrets are in the same namespace as the DPFHCPProvisioner CR

### BlueField OCP Layer Image Lookup Failure

**Condition:** `BlueFieldOCPLayerImageFound = False`

**Symptoms:** CR stays in `Failed` phase.

**Cause:** The operator cannot find a BlueField container image tag matching the OCP version in the configured registry.

**Resolution:**
1. Check the DPFHCPProvisionerConfig for the correct registry:
   ```bash
   kubectl get dpfhcpconfig default -o yaml
   ```
2. Verify the OCP version extracted from `ocpReleaseImage` has a matching tag in the BlueField OCP layer image repository
3. If the BlueField OCP layer image is known, set `machineOSURL` in the DPFHCPProvisioner spec to skip the lookup

### HostedCluster Stuck in WaitingForControlPlane

**Conditions:** `HostedClusterAvailable = False`, `HostedClusterProgressing = True`

**Symptoms:** CR stays in `WaitingForControlPlane` phase for an extended period.

**Resolution:**
1. Check the HostedCluster directly:
   ```bash
   kubectl get hostedcluster -A
   kubectl describe hostedcluster <name> -n <namespace>
   ```
2. Look for issues with:
   - The OCP release image (invalid or inaccessible)
   - etcd storage (storage class not available)
   - Node scheduling (control plane nodes not available or have resource pressure)
   - Network connectivity (DNS resolution, API accessibility)
3. Check HyperShift operator logs for more details

### MetalLB Configuration Issues

**Condition:** `MetalLBConfigured = False`

**Symptoms:** Services are not accessible via LoadBalancer.

**Resolution:**
1. Verify MetalLB operator is installed and running
2. Check that the `virtualIP` in the spec is a valid, routable IP on the management cluster network
3. Inspect MetalLB resources:
   ```bash
   kubectl get ipaddresspool -A
   kubectl get l2advertisement -A
   ```

### CSR Approval Not Working

**Condition:** `CSRAutoApprovalActive = False`

**Symptoms:** DPU worker nodes cannot join the hosted cluster.

**Cause:** The operator cannot connect to the hosted cluster to watch for CSRs.

**Resolution:**
1. Ensure the HostedCluster is in `Ready` state
2. Check the hosted cluster is reachable from the management cluster
3. Look for `CSRApproval` controller errors in operator logs:
   ```bash
   kubectl logs -n dpf-hcp-provisioner-system deployment/dpf-hcp-provisioner-operator | grep -i csr
   ```

### Ignition Generation Failure

**Condition:** `IgnitionConfigured = False` with reason `IgnitionGenerationFailed` or `MachineOSURLMissing`

**Symptoms:** CR enters `Failed` phase. Note: transient invalidation reasons (e.g., `ReleaseImageUpdated`, `OperatorRestarted`, `ConfigMapDeleted`) do **not** cause the `Failed` phase — the CR stays in `GeneratingIgnition` and the controller retries automatically.

**Resolution:**
1. Verify the `dpuDeploymentRef` points to a valid DPUDeployment
2. Check that the DPUDeployment has the required DPU flavor information
3. If using a custom `machineOSURL`, verify the URL is accessible
4. Check operator logs for ignition generation errors

## Deleting a DPFHCPProvisioner

Deletion triggers finalizer cleanup that removes the HostedCluster, MetalLB resources, and injected kubeconfig in order. If the CR is stuck in `Deleting` phase:

1. Check conditions for cleanup status:
   ```bash
   kubectl get dpfhcp <name> -n <namespace> -o jsonpath='{.status.conditions}' | jq '.[] | select(.type=="HostedClusterCleanup")'
   ```
2. Check if the HostedCluster is being deleted:
   ```bash
   kubectl get hostedcluster -A
   ```
3. If the HostedCluster itself is stuck deleting, investigate HyperShift operator logs

## Events

The operator emits Kubernetes events on the DPFHCPProvisioner CR for key lifecycle actions. View them with:

```bash
kubectl get events -n <namespace> --field-selector involvedObject.name=<dpfhcp-name>
```
