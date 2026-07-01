# OCP DPF Upgrade Procedure

An OpenShift DPF deployment consists of several independently versioned
components — the management cluster, the DPF operator, hosted clusters*, the
DPF HCP Provisioner, and the DPUs themselves. Each can be upgraded
individually, but they must be upgraded in a specific order and only between
compatible versions. This document describes the upgrade sequence,
per-component procedures, and known limitations.

*In an OCP DPF deployment, DPUs are managed as worker nodes of hosted
Kubernetes clusters. Each hosted cluster's control plane runs as a set of pods
on the management cluster, powered by HyperShift (part of MCE). The DPUs
join these hosted clusters and receive their workloads, networking
configuration, and OS updates through them.

## Limitations

<!-- TODO: move this to general doc, not upgrade doc -->
- It is recommended to install the management cluster as multi-arch. Using OCP
  DPF offline on a single-architecture cluster is not supported.

## Version Compatibility

Not every combination of component versions is supported. The key constraints:

- **DPF operator** currently supports exactly one OCP minor version for the
management cluster. Users must upgrade their management cluster to the
supported version before upgrading DPF. Future DPF releases may support older
OCP versions as well.

- **DPF operator** supports exactly one Kubernetes version for your hosted clusters.
The hosted control planes must be running a matching version.

- **DPF HCP Provisioner** is released per DPF version and supports the current
and immediately previous DPF version. Always upgrade to the matching
provisioner version before upgrading DPF.

- **HyperShift / MCE** does not support a newer hosted cluster version until
MCE itself is upgraded to the corresponding release. Upgrade MCE after
upgrading the management cluster and before upgrading your hosted clusters.

- **DPU firmware** must never be newer than the current DPF version. Upgrade
DPF before upgrading DPUs.


## Upgrade Order

Upgrades **must** follow this sequence:

1. Upgrade the provisioner
2. Upgrade the management cluster
3. Upgrade MCE
4. Upgrade your hosted clusters
5. Upgrade DPF
6. Upgrade DPUs on your hosted clusters

## Management Cluster Upgrade

- For the procedure, follow standard OCP upgrade docs.
- Only upgrade between version pairs tested for OCP DPF (supported matrix TBD).

### Known issue: UnexpectedAdmissionError on DPU hosts

After a management cluster upgrade, pods on DPU hosts may fail with
`UnexpectedAdmissionError` due to
[kubernetes#117955](https://github.com/kubernetes/kubernetes/issues/117955).
This will be handled automatically in a future version. To resolve manually:

```bash
oc get pods --all-namespaces --field-selector=status.phase=Failed \
  -o json | jq -r '.items[] | select(.status.reason=="UnexpectedAdmissionError") |
  "\(.metadata.namespace) \(.metadata.name)"' |
  while read ns name; do oc delete pod -n "$ns" "$name"; done
```

### Known issue: healthcheck failures on management cluster worker nodes

After a management cluster upgrade, worker nodes may report healthcheck
failures. This can be resolved by deleting the OVN-K pods running on the DPUs
of the affected worker nodes. A fix for this issue is in progress.

First, extract the hosted cluster kubeconfig from the management cluster:

```bash
oc get secret -n dpf-operator-system doca-admin-kubeconfig -ojson \
  | jq '.data."super-admin.conf" | @base64d' -r > /tmp/hcp-kubeconfig
```

Then delete the OVN-K node pods and let them be recreated:

```bash
KUBECONFIG=/tmp/hcp-kubeconfig \
  oc delete pods -n dpf-operator-system -l app.kubernetes.io/component=ovnkube-node
```

## Provisioner Upgrade

- <!-- Helm upgrade procedure — details needed. -->

## Hosted Clusters Upgrade

In order to upgrade your hosted clusters, update the `ocpReleaseImage` and
`machineOSURL` fields on your `DPFHCPProvisioner` CR:

```bash
oc patch dpfhcpprovisioner <name> -n clusters --type merge -p '{
  "spec": {
    "ocpReleaseImage": "quay.io/openshift-release-dev/ocp-release:<new-version>-multi",
    "machineOSURL": "<new-machine-os-url>"
  }
}'
```

The provisioner will upgrade the hosted control plane to match.

## DPF Upgrade

<!-- TODO: DPF upgrade procedure — Helm? NVIDIA docs? Red Hat docs? -->

## DPU Upgrade

In order to upgrade the DPUs, create a new BFB resource and update your
DPUDeployment to reference it.

1. Create a new BFB. We recommend naming it after the version you are
   upgrading to (e.g. `bf-bundle-4.22.2`). The BFB URL depends on the type of
   upgrade:
   - **z-stream upgrade** (e.g. 4.22.1 → 4.22.2): the BFB image is the same
     across z-stream versions within a minor release, so you may reuse the same
     `spec.url`. However, you must set a different `spec.fileName`.
   - **Minor version upgrade** (e.g. 4.22 → 4.23): use a new `spec.url`
     pointing to the BFB image for the new minor version.

   The actual OCP version running on the DPU will be determined by version
   currently running on the hosted cluster, not by the BFB image itself.

2. Update the DPUDeployment to point at the new BFB:

   ```bash
   oc patch dpudeployment <name> -n dpf-operator-system --type merge -p '{
     "spec": {
       "dpus": {
         "bfb": "<new-bfb-cr-name>"
       }
     }
   }'
   ```

   DPF will roll out the new firmware across DPUs. Do not delete the old BFB
   until the full fleet upgrade is complete.
