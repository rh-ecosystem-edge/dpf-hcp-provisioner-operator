# DPUServiceTemplate Controller

## Purpose

Creates and continuously reconciles three `DPUServiceTemplate` resources (OVN,
DTS, HBN) in every namespace referenced by a `DPFHCPProvisioner`'s `DPUClusterRef`.
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
since within a namespace all DPUClusters share the same templates. The
templates are created in every namespace referenced by a `DPFHCPProvisioner`'s
DPUClusterRef.

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
DPF releases, so they are maintained in a JSON file (`dpuservicetemplate_values.json`)
keyed by DPF **major.minor** version (e.g. `"26.4"`). At runtime, the controller reads
`DPFOperatorConfig.status.version` from
`dpfoperatorconfig/dpf-operator-system`, extracts the major.minor version, and looks
up the corresponding defaults.

### Value overrides via ConfigMap

The hardcoded defaults can be overridden at runtime by creating a ConfigMap
named `dpuservicetemplate-overrides` in the operator namespace. The ConfigMap
holds an `overrides.json` key with a JSON object keyed by DPF major.minor version.
Any fields set in the override are merged on top of the hardcoded values. If
the ConfigMap doesn't exist, the hardcoded values are used as-is.

This is primarily useful for development and testing of different versions of
the DPU service components. The
[openshift-dpf](https://github.com/rh-ecosystem-edge/openshift-dpf) automation
repo uses this ConfigMap to apply the component versions set through its
various env var configurations.

### OVN image architecture resolution

The management cluster's OVN DaemonSet provides an x86 image, but DPU workloads
run on aarch64. For single-arch release images, the controller resolves the
corresponding aarch64 OVN image from the OCP release payload. Multi-arch release
images are used directly.

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
| `controller.go` | Reconciler, cleanup, template lifecycle |
| `dpuservicetemplate.go` | Manager, template creation, OVN image resolution |
| `dpuservicetemplatevalues.go` | DPF version-to-defaults map |
| `release_image.go` | Release payload image extraction |
