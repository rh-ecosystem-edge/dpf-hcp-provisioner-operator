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

CONTAINER_COMMAND=${CONTAINER_COMMAND:-podman}

# Extract hypershift binary to a local directory (no sudo required)
HYPERSHIFT_BIN_DIR=$(mktemp -d)
echo "Extracting hypershift binary from ${HYPERSHIFT_IMAGE}..."
${CONTAINER_COMMAND} cp $(${CONTAINER_COMMAND} create --name hypershift --rm --pull always ${HYPERSHIFT_IMAGE}):/usr/bin/hypershift "${HYPERSHIFT_BIN_DIR}/hypershift"
${CONTAINER_COMMAND} rm -f hypershift
chmod 0755 "${HYPERSHIFT_BIN_DIR}/hypershift"

# Install the HyperShift operator
echo "Installing HyperShift operator..."
"${HYPERSHIFT_BIN_DIR}/hypershift" install --hypershift-image ${HYPERSHIFT_IMAGE}

# Clean up binary
rm -rf "${HYPERSHIFT_BIN_DIR}"

# Wait for operator to be ready
echo "Waiting for HyperShift operator deployment..."
oc wait deployment/${HYPERSHIFT_DEPLOYMENT} -n hypershift \
    --for=condition=Available \
    --timeout=5m

echo "Checking HyperShift operator status..."
oc -n hypershift get pods

echo "=== HyperShift operator deployed successfully ==="
