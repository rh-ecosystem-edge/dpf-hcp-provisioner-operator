#!/bin/bash
# Deploy HyperShift operator on management cluster for e2e testing.
# Follows the same pattern as openshift-dpf/scripts/tools.sh install_hypershift().
#
# Optional env vars:
#   HYPERSHIFT_IMAGE - HyperShift image (default: quay.io/hypershift/hypershift-operator:latest)
#   CONTAINER_COMMAND - Container runtime (default: podman)
#   KUBECONFIG - Path to kubeconfig (default: current context)
set -euo pipefail

HYPERSHIFT_DEPLOYMENT="operator"

echo "=== Deploying HyperShift operator ==="

# Check if already installed
if oc get deployment/${HYPERSHIFT_DEPLOYMENT} -n hypershift &>/dev/null; then
    echo "HyperShift operator already installed. Skipping deployment."
    exit 0
fi

HYPERSHIFT_IMAGE=${HYPERSHIFT_IMAGE:-quay.io/hypershift/hypershift-operator:latest}
echo "Using HyperShift image: ${HYPERSHIFT_IMAGE}"

# Check if hypershift binary is already available (e.g., in container)
if command -v hypershift &>/dev/null; then
    echo "Using existing hypershift binary: $(command -v hypershift)"
    HYPERSHIFT_BIN="hypershift"
else
    # Extract hypershift binary from container image (for local development)
    echo "Hypershift binary not found, extracting from container image..."

    CONTAINER_COMMAND=${CONTAINER_COMMAND:-podman}

    if ! command -v "${CONTAINER_COMMAND}" &>/dev/null; then
        echo "Error: ${CONTAINER_COMMAND} not found. Please install ${CONTAINER_COMMAND} or ensure hypershift binary is in PATH."
        exit 1
    fi

    HYPERSHIFT_BIN_DIR=$(mktemp -d)
    HYPERSHIFT_CONTAINER_ID=""

    cleanup() {
        if [[ -n "${HYPERSHIFT_CONTAINER_ID}" ]]; then
            "${CONTAINER_COMMAND}" rm -f "${HYPERSHIFT_CONTAINER_ID}" >/dev/null 2>&1 || true
        fi
        rm -rf "${HYPERSHIFT_BIN_DIR}"
    }
    trap cleanup EXIT

    echo "Extracting hypershift binary from ${HYPERSHIFT_IMAGE}..."
    HYPERSHIFT_CONTAINER_ID=$("${CONTAINER_COMMAND}" create --pull always "${HYPERSHIFT_IMAGE}")
    "${CONTAINER_COMMAND}" cp "${HYPERSHIFT_CONTAINER_ID}:/usr/bin/hypershift" "${HYPERSHIFT_BIN_DIR}/hypershift"
    "${CONTAINER_COMMAND}" rm -f "${HYPERSHIFT_CONTAINER_ID}"
    HYPERSHIFT_CONTAINER_ID=""  # Clear ID after successful removal
    chmod 0755 "${HYPERSHIFT_BIN_DIR}/hypershift"

    HYPERSHIFT_BIN="${HYPERSHIFT_BIN_DIR}/hypershift"
fi

# Install the HyperShift operator
echo "Installing HyperShift operator..."
"${HYPERSHIFT_BIN}" install --hypershift-image "${HYPERSHIFT_IMAGE}"

# Wait for operator to be ready
echo "Waiting for HyperShift operator deployment..."
oc wait deployment/${HYPERSHIFT_DEPLOYMENT} -n hypershift \
    --for=condition=Available \
    --timeout=5m

echo "Checking HyperShift operator status..."
oc -n hypershift get pods

echo "=== HyperShift operator deployed successfully ==="
