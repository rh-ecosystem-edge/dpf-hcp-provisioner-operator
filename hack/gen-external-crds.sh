#!/usr/bin/env bash
# Regenerates the checked-in CRDs of the external APIs the operator consumes, with controller-gen
# over the Go API packages pinned in hack/gen-rust-apis/go.mod (fetched from the Go module proxy;
# nothing is vendored):
#
#   config/crd/external      doca-platform provisioning/v1alpha1 + dpuservice/v1alpha1 and
#                            hypershift v1beta1. Installed by the envtest suites
#                            (crates/controller, crates/manager smoke test, crates/ctrlrt-envtest),
#                            by `make test-e2e-kind` and by the DAST pipeline; also the input of
#                            hack/gen-rust-apis.sh.
#   test/e2e/testdata/crds   doca-platform provisioning, dpuservice and operator v1alpha1 and
#                            openshift config/v1. Installed by `make apply-crds-for-e2e`.
#
# To move to newer upstream API versions, bump them in hack/gen-rust-apis/go.mod
# (`go get ...` + `go mod tidy` in that directory), then run `make generate`.
set -euo pipefail

# Build environments (e.g. the OpenShift CI Go build root) may export GOFLAGS=-mod=vendor;
# the generator module is not vendored, so start from a clean GOFLAGS.
unset GOFLAGS

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

CONTROLLER_TOOLS_VERSION="${CONTROLLER_TOOLS_VERSION:-v0.18.0}"
TOOLS="${ROOT}/target/apis/tools"
CONTROLLER_GEN="${TOOLS}/bin/controller-gen"
GEN_MODULE="${ROOT}/hack/gen-rust-apis"

if [[ ! -x "${CONTROLLER_GEN}" ]] || ! "${CONTROLLER_GEN}" --version | grep -q "${CONTROLLER_TOOLS_VERSION}"; then
  GOBIN="${TOOLS}/bin" go install "sigs.k8s.io/controller-tools/cmd/controller-gen@${CONTROLLER_TOOLS_VERSION}"
fi

# gen_crds OUT_DIR PKG...: controller-gen crd for each package, resolved in hack/gen-rust-apis.
gen_crds() {
  local out="$1"
  shift
  rm -rf "${out}"
  mkdir -p "${out}"
  for pkg in "$@"; do
    (cd "${GEN_MODULE}" && GOFLAGS=-mod=readonly "${CONTROLLER_GEN}" crd paths="${pkg}" output:crd:dir="${out}")
  done
}

gen_crds "${ROOT}/config/crd/external" \
  github.com/nvidia/doca-platform/api/provisioning/v1alpha1/... \
  github.com/nvidia/doca-platform/api/dpuservice/v1alpha1/... \
  github.com/openshift/hypershift/api/hypershift/v1beta1/...

gen_crds "${ROOT}/test/e2e/testdata/crds" \
  github.com/nvidia/doca-platform/api/provisioning/v1alpha1/... \
  github.com/nvidia/doca-platform/api/dpuservice/v1alpha1/... \
  github.com/nvidia/doca-platform/api/operator/v1alpha1/... \
  github.com/openshift/api/config/v1/...

echo "generated config/crd/external and test/e2e/testdata/crds"
