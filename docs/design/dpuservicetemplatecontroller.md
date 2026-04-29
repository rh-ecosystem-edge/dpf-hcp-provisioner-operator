# DPUServiceTemplate Controller

## Purpose

Creates and continuously reconciles three `DPUServiceTemplate` resources (OVN,
DTS, HBN) in every namespace which contains at-least one `DPFHCPProvisioner.
During the PoC stages these were applied manually via shell scripts in the
[openshift-dpf](https://github.com/rh-ecosystem-edge/openshift-dpf)
post-installation automation.

The primary reason these should be managed by a controller as opposed to being
created by a user is OVN-K version synchronization. The `ovnkube-node-dpu-host`
DaemonSet on the host is managed by Cluster Network Operator. During a
management cluster upgrade, that operator updates the `DaemonSet` to a new
OVN-K version. That version should match the version of the OVN-K which is
running on the DPU (running via a `DPUService`).

This controller watches the DaemonSet and, once the rollout completes (all pods
updated and available), updates the OVN `DPUServiceTemplate` to match that
version, ensuring the OVN-K version on the DPU stays in sync with the host.

Templates are named after their respective service only (no DPUCluster suffix),
since all DPUClusters use the same templates. The templates are created in
every namespace that contains a DPUCluster.

## Templates Created

| Name | Service | Chart |
|---|---|---|
| `ovn` | OVN Kubernetes | `ovn-kubernetes-chart` |
| `doca-telemetry-service` | DOCA Telemetry Service | `doca-telemetry` |
| `hbn` | Host-Based Networking | `doca-hbn` |

## Implementation Details

### DPF version-based defaults

Chart versions and image repos are **not** user-configurable. These values are
difficult for users to determine on their own and they move in lockstep with
DPF releases, so they are hardcoded in a Go map (`dpfVersionDefaults` in
`defaults.go`) keyed by DPF **major** version. At runtime, the controller reads
`DPFOperatorConfig.status.version` from
`dpfoperatorconfig/dpf-operator-system`, extracts the major version, and looks
up the corresponding defaults.

### Cleanup

Once there are no more DPFHCPProvisioners in a namespace, the templates of that
namespace are deleted.


### Reconcile ordering

Runs after validations + MetalLB configuration, before secret copying and
HostedCluster creation. The `DPUServiceTemplatesConfigured` condition gates the
`Ready` condition.

## Files

| File | Purpose |
|---|---|
| `defaults.go` | DPF version-to-defaults map, version lookup |
| `dpuservicetemplate.go` | Manager, template creation, OVN image resolution |
| `cleanup_handler.go` | Cleanup handler |
