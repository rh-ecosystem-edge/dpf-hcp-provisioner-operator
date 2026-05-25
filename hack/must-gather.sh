#!/usr/bin/env bash

# Copyright 2025.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

set -euo pipefail

BASE_COLLECTION_PATH=/must-gather
mkdir -p "${BASE_COLLECTION_PATH}"

# Tee all output to a log file inside the artifact dir
exec 1> >(tee "${BASE_COLLECTION_PATH}/must-gather.log")
exec 2>&1

# ---------------------------------------------------------------------------
# Operator namespace auto-detection
# ---------------------------------------------------------------------------
OPERATOR_NAMESPACE=$(oc get deployment -A \
    -l app.kubernetes.io/name=dpf-hcp-provisioner-operator \
    -o jsonpath='{.items[0].metadata.namespace}' 2>/dev/null || true)

if [[ -z "${OPERATOR_NAMESPACE}" ]]; then
    echo "FATAL: could not detect operator namespace (no deployment with label app.kubernetes.io/name=dpf-hcp-provisioner-operator found)"
    exit 1
fi

# ---------------------------------------------------------------------------
# Time-based log filtering (per must-gather best practices)
# oc adm must-gather passes these when the user supplies --since / --since-time
# ---------------------------------------------------------------------------
log_collection_args=""
if [ -n "${MUST_GATHER_SINCE:-}" ]; then
    log_collection_args="--since=${MUST_GATHER_SINCE}"
fi
if [ -n "${MUST_GATHER_SINCE_TIME:-}" ]; then
    log_collection_args="--since-time=${MUST_GATHER_SINCE_TIME}"
fi

echo "# DPF HCP Provisioner Operator must-gather"
echo "# OPERATOR_NAMESPACE: ${OPERATOR_NAMESPACE}"
echo


inspect() {
    local inspect_err
    inspect_err="$(mktemp)"
    # shellcheck disable=SC2086
    if ! oc adm inspect ${log_collection_args} --dest-dir "${BASE_COLLECTION_PATH}" "$@" \
            >/dev/null 2>"${inspect_err}"; then
        echo "  [warn] inspect failed: $* :: $(<"${inspect_err}")"
    fi
    rm -f "${inspect_err}"
}

# ---------------------------------------------------------------------------
# Dynamic CRD discovery — one API call, split by scope.
# Automatically picks up new types added in future releases of doca-platform,
# HyperShift, or this operator.
# ---------------------------------------------------------------------------
CRD_LIST=$(oc get crd \
    -o jsonpath='{range .items[*]}{.metadata.name},{.spec.scope}{"\n"}{end}' 2>/dev/null \
    | grep -E '\.dpu\.nvidia\.com|\.dpu\.hcp\.io|\.hypershift\.openshift\.io' \
    || true)

readarray -t CRDS          < <(echo "${CRD_LIST}" | cut -d',' -f1 | grep -v '^$')
readarray -t CLUSTER_SCOPED_CRS < <(echo "${CRD_LIST}" | grep ',Cluster$'   | cut -d',' -f1 | grep -v '^$')
readarray -t ALL_NS_CRS    < <(echo "${CRD_LIST}" | grep ',Namespaced$' | cut -d',' -f1 | grep -v '^$')

if [[ ${#CRDS[@]} -eq 0 ]]; then
    echo "  [warn] no relevant CRDs found on cluster"
fi

# ---------------------------------------------------------------------------
# Collection functions
# ---------------------------------------------------------------------------

function get_crds() {
    echo "Collecting CRD definitions (${#CRDS[@]} types found)..."
    for crd in "${CRDS[@]}"; do
        echo "  crd/${crd}"
        inspect "crd/${crd}"
    done
}

function get_cluster_scoped_crs() {
    echo
    echo "Collecting cluster-scoped CR instances (${#CLUSTER_SCOPED_CRS[@]} types)..."
    for cr in "${CLUSTER_SCOPED_CRS[@]}"; do
        echo "  ${cr}"
        inspect "${cr}"
    done
}

function get_all_ns_crs() {
    echo
    echo "Collecting namespaced CR instances across all namespaces (${#ALL_NS_CRS[@]} types)..."
    for cr in "${ALL_NS_CRS[@]}"; do
        echo "  ${cr}"
        inspect "${cr}" --all-namespaces
    done
}

function get_provisioner_and_hypershift_operators_namespaces() {
    echo
    echo "Collecting DPF HCP Provisioner operator and HyperShift namespaces..."
    echo "  ns/${OPERATOR_NAMESPACE} (DPF HCP Provisioner Operator)"
    inspect "ns/${OPERATOR_NAMESPACE}"
    echo "  ns/hypershift (HyperShift Operator)"
    inspect "ns/hypershift"
}

function get_dpf_operator_namespace() {
    echo
    echo "Collecting DPF operator namespace (DPFOperatorConfig, DPUCluster, DPUFlavor, BFcfg CMs)..."

    local DPF_NS oc_rc=0
    DPF_NS=$(oc get dpfoperatorconfigs --all-namespaces \
        -o jsonpath='{.items[0].metadata.namespace}' 2>&1) || oc_rc=$?

    if [[ ${oc_rc} -ne 0 ]]; then
        echo "  [warn] failed to query DPFOperatorConfig: ${DPF_NS}"
        return
    fi
    if [[ -z "${DPF_NS}" ]]; then
        echo "  [warn] DPFOperatorConfig not found — skipping DPF operator namespace collection"
        return
    fi

    echo "  DPF operator namespace: ${DPF_NS}"
    inspect "ns/${DPF_NS}"
}

function get_dpfhcpprovisioner_and_hcp_namespaces() {
    echo
    echo "Collecting DPFHCPProvisioner CR namespaces and their HCP namespaces..."

    local PROVISIONERS oc_rc=0
    PROVISIONERS=$(oc get dpfhcpprovisioners --all-namespaces \
        -o jsonpath='{range .items[*]}{.metadata.namespace},{.metadata.name}{"\n"}{end}' \
        2>&1) || oc_rc=$?

    if [[ ${oc_rc} -ne 0 ]]; then
        echo "  [warn] failed to query DPFHCPProvisioner CRs: ${PROVISIONERS}"
        return
    fi
    if [[ -z "${PROVISIONERS}" ]]; then
        echo "  [warn] no DPFHCPProvisioner CRs found"
        return
    fi

    while IFS=',' read -r CR_NS CR_NAME; do
        [[ -z "${CR_NS}" || -z "${CR_NAME}" ]] && continue

        echo
        echo "  >> DPFHCPProvisioner ${CR_NS}/${CR_NAME}"

        # CR namespace: DPFHCPProvisioner, HostedCluster, NodePools, BFcfg CMs, Secrets, pod logs
        inspect "ns/${CR_NS}"

        # HCP namespace: <cr-namespace>-<cr-name>
        local HCP_NS="${CR_NS}-${CR_NAME}"
        if oc get namespace "${HCP_NS}" &>/dev/null; then
            echo "     HCP namespace: ${HCP_NS}"
            inspect "ns/${HCP_NS}"
        else
            echo "     [warn] HCP namespace '${HCP_NS}' not found (cluster not yet provisioned?)"
        fi

    done <<< "${PROVISIONERS}"
}

# ---------------------------------------------------------------------------
# Run collection
# ---------------------------------------------------------------------------
get_crds
get_cluster_scoped_crs
get_all_ns_crs
get_provisioner_and_hypershift_operators_namespaces
get_dpf_operator_namespace
get_dpfhcpprovisioner_and_hcp_namespaces

echo
echo "Done. All data written to ${BASE_COLLECTION_PATH}"

# force disk flush so all data is accessible in the copy container
sync
exit 0
