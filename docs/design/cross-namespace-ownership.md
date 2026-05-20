# Cross-namespace ownership

Several controllers create resources in the `DPUCluster`'s namespace, which may
differ from the provisioner's namespace. Kubernetes OwnerReferences don't work
cross-namespace, so ownership is tracked via labels:

- `dpfhcpprovisioner.dpu.hcp.io/name`
- `dpfhcpprovisioner.dpu.hcp.io/namespace`

These labels serve two purposes:
1. Prevent one provisioner from overwriting another provisioner's resources (ownership check via `common.IsOwnedByProvisioner`)
2. Identify which resources to delete during finalizer cleanup

Any controller that manages resources outside the provisioner's own namespace
uses this pattern. The shared ownership check lives in
`internal/common/ownership.go`.
