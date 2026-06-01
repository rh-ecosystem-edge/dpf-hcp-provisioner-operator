# Image Caching

The operator includes an **opportunistic image caching** feature that mirrors
container images (RHCOS BFB images) from external registries to the OpenShift
internal image registry. When active, the ignition configuration uses the
internal URL, removing the dependency on external registry availability during
DPU provisioning.

Caching is fully automatic — there is no feature gate or flag.
If the internal registry is available and properly configured, the operator
uses it. If it is not, the operator silently falls back to the external URL.

## Enabling the Internal Registry

By default, OpenShift deploys the internal image registry in a `Removed` state
on some platforms (e.g. bare-metal). To enable it:

### 1. Set the Registry to Managed

```bash
oc patch configs.imageregistry.operator.openshift.io cluster \
  --type merge --patch '{"spec":{"managementState":"Managed"}}'
```

### 2. Expose the Default Route

The operator discovers the registry through its external route.
Enable it if not already exposed:

```bash
oc patch configs.imageregistry.operator.openshift.io cluster \
  --type merge --patch '{"spec":{"defaultRoute":true}}'
```

### 3. Configure Storage (if needed)

The registry needs persistent storage. On SNO / bare-metal clusters
that use `emptyDir` by default, this is acceptable for non-production use:

```bash
oc patch configs.imageregistry.operator.openshift.io cluster \
  --type merge --patch '{"spec":{"storage":{"emptyDir":{}}}}'
```

For production, use a PVC-backed storage.

### 4. Verify

```bash
# Registry pods should be Running
oc get pods -n openshift-image-registry -l docker-registry=default

# Route should exist with a hostname
oc get route default-route -n openshift-image-registry
```

## RBAC Requirements

The operator's ServiceAccount must have permission to push images to the
internal registry. Grant the `registry-editor` role in the operator namespace:

```bash
oc policy add-role-to-user registry-editor \
  system:serviceaccount:dpf-hcp-provisioner-system:dpf-hcp-provisioner-operator \
  -n dpf-hcp-provisioner-system
```

Alternatively, the `system:image-builder` role also works:

```bash
oc policy add-role-to-user system:image-builder \
  system:serviceaccount:dpf-hcp-provisioner-system:dpf-hcp-provisioner-operator \
  -n dpf-hcp-provisioner-system
```

> **Note:** This RoleBinding should be set up by the deployment mechanism
> (Helm chart or OLM). It is not managed by the operator itself.

## How It Works

When a `DPFHCPProvisioner` CR is reconciled, the operator:

1. **Checks registry availability** — reads
   `configs.imageregistry.operator.openshift.io/cluster` for
   `managementState: Managed` and looks for the `default-route` Route in
   `openshift-image-registry`.
2. **Resolves the source image** — uses `spec.machineOSURL` if set,
   otherwise falls back to `status.blueFieldOCPLayerImage`.
3. **Compares digests** — if a cached copy already exists, the operator
   compares upstream and cached image digests via HEAD requests. If they
   match, caching is skipped.
4. **Mirrors the image** — pulls from the external registry using the CR's
   `pullSecretRef` and pushes to the internal registry.
5. **Updates status** — sets `status.cachedMachineOSURL` to the internal
   registry URL and the `ImageCached` condition to `True`.
6. **Generates ignition** — `generateIgnition()` uses the cached URL
   instead of the external one.

### Target URL Format

```
<route-hostname>/<operator-namespace>/<repo-name>:<tag>
```

For example:
```
default-route-openshift-image-registry.apps.cluster.example.com/dpf-hcp-provisioner-system/rhcos-bfb:4.22.0
```

## Failure Handling

Caching is designed to **never block provisioning**:

- If the internal registry is unavailable → caching is skipped, external URL
  is used.
- If image pull or push fails → the operator retries up to **5 times** with
  exponential backoff (30s → 1m → 2m → 5m → 10m).
- After 5 consecutive failures → the `ImageCached` condition is set to
  `CacheFailed` and the operator proceeds with the external URL.

## Observing Status

### Conditions

The `ImageCached` condition on the `DPFHCPProvisioner` CR reports caching
status:

```bash
oc get dpfhcpprovisioner <name> -o jsonpath='{.status.conditions[?(@.type=="ImageCached")]}'
```

| Reason | Meaning |
|--------|---------|
| `ImageCached` | Image is cached and up to date |
| `CachingInProgress` | Mirror operation is running |
| `RegistryNotAvailable` | Internal registry is not deployed |
| `RegistryConfigNotManaged` | Registry `managementState` is not `Managed` |
| `RegistryRouteNotExposed` | `default-route` not found or has no hostname |
| `ImagePullFailed` | Failed to pull from external registry |
| `ImagePushFailed` | Failed to push to internal registry |
| `RegistryAuthFailed` | Authentication to registry failed |
| `CacheFailed` | Max retries exceeded, falling back to external URL |

### Cached URL

```bash
oc get dpfhcpprovisioner <name> -o jsonpath='{.status.cachedMachineOSURL}'
```

### Events

The operator emits Kubernetes events for caching activity:

```bash
oc get events --field-selector involvedObject.kind=DPFHCPProvisioner | grep -i cache
```

## Disabling Caching

To disable caching, set the internal registry to `Removed`:

```bash
oc patch configs.imageregistry.operator.openshift.io cluster \
  --type merge --patch '{"spec":{"managementState":"Removed"}}'
```

The operator will detect the change on the next reconciliation and fall back
to the external URL.
