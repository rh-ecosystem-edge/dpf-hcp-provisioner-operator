#!/usr/bin/env bash
# Regenerates crates/apis/src/generated from the CRDs of every API kind the operator uses.
#
#   1. collect CRD YAMLs:
#        - config/crd/bases                      (this repo's api/v1alpha1)
#        - internal/controller/testdata/crds     (doca-platform provisioning/dpuservice, hypershift)
#        - controller-gen over the vendored Go types for doca-platform operator, metallb and
#          openshift config/v1 (no CRDs for those are checked in)
#   2. run kopium on every CRD (structure + documentation)
#   3. post-process with hack/gen-rust-apis (Go): re-shape the kopium output against the
#      vendored Go types so that serde reproduces Go's encoding/json byte for byte
#      (field order, omitempty, nil -> null, Go type names), add unknown-field preservation,
#      Go constants and ctrlrt::impl_resource! registrations.
#   4. rustfmt
#
# Tools are installed into target/apis/tools (kopium via cargo, controller-gen via go).
# See crates/apis/GENERATION.md for the list of post-processing overrides.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

KOPIUM_VERSION="${KOPIUM_VERSION:-0.24.1}"
CONTROLLER_TOOLS_VERSION="${CONTROLLER_TOOLS_VERSION:-v0.18.0}"
TOOLS="${ROOT}/target/apis/tools"
KOPIUM="${TOOLS}/bin/kopium"
CONTROLLER_GEN="${TOOLS}/bin/controller-gen"
OUT="${ROOT}/crates/apis/src/generated"

if [[ ! -x "${KOPIUM}" ]] || ! "${KOPIUM}" --version | grep -q "${KOPIUM_VERSION}"; then
  CARGO_TARGET_DIR="${ROOT}/target/apis" cargo install kopium --locked --version "${KOPIUM_VERSION}" --root "${TOOLS}"
fi
if [[ ! -x "${CONTROLLER_GEN}" ]]; then
  GOBIN="${TOOLS}/bin" go install "sigs.k8s.io/controller-tools/cmd/controller-gen@${CONTROLLER_TOOLS_VERSION}"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
mkdir -p "${WORK}/crds" "${WORK}/kopium"

CRDS=(
  config/crd/bases/provisioning.dpu.hcp.io_dpfhcpprovisioners.yaml
  config/crd/bases/provisioning.dpu.hcp.io_dpfhcpprovisionerconfigs.yaml
  internal/controller/testdata/crds/provisioning.dpu.nvidia.com_dpus.yaml
  internal/controller/testdata/crds/provisioning.dpu.nvidia.com_dpuclusters.yaml
  internal/controller/testdata/crds/provisioning.dpu.nvidia.com_dpuflavors.yaml
  internal/controller/testdata/crds/svc.dpu.nvidia.com_dpudeployments.yaml
  internal/controller/testdata/crds/svc.dpu.nvidia.com_dpuservicetemplates.yaml
  internal/controller/testdata/crds/hypershift.openshift.io_hostedclusters.yaml
  internal/controller/testdata/crds/hypershift.openshift.io_nodepools.yaml
)
cp "${CRDS[@]}" "${WORK}/crds/"

GEN="${WORK}/controller-gen"
mkdir -p "${GEN}"
for pkg in \
  github.com/nvidia/doca-platform/api/operator/v1alpha1/... \
  go.universe.tf/metallb/api/v1beta1/... \
  github.com/openshift/api/config/v1; do
  "${CONTROLLER_GEN}" crd paths="${pkg}" output:crd:dir="${GEN}"
done
cp "${GEN}/operator.dpu.nvidia.com_dpfoperatorconfigs.yaml" \
   "${GEN}/metallb.io_ipaddresspools.yaml" \
   "${GEN}/metallb.io_l2advertisements.yaml" \
   "${GEN}/config.openshift.io_clusterversions.yaml" \
   "${WORK}/crds/"

for crd in "${WORK}"/crds/*.yaml; do
  name="$(basename "${crd}" .yaml)"
  "${KOPIUM}" -f "${crd}" --hide-kube --docs --derive Default --derive PartialEq --no-condition --no-object-reference > "${WORK}/kopium/${name}.rs"
done

rm -rf "${OUT}"
go run -mod=vendor ./hack/gen-rust-apis -crds "${WORK}/crds" -kopium "${WORK}/kopium" -out "${OUT}"
rustfmt --edition 2024 "${OUT}"/*.rs
echo "generated ${OUT}"
