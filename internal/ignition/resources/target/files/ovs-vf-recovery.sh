#!/bin/bash
#
# OVS VF Representor Recovery (WORKAROUND for https://partners.nvidia.com/Bug/ViewBug/6443100)
#
# When the x86 host reboots while the DPU stays up, VF representors are
# destroyed and recreated. OVS-DOCA/DPDK may fail to re-attach representors
# that were previously opened, leaving them in a permanent error state.
#
# This script polls OVS every POLL_INTERVAL seconds for DPDK interfaces
# with errors and takes recovery actions:
#   1. "Error attaching device" → restart openvswitch
#   2. "No such device" → restart the ovnkube-controller container via crictl

set -uo pipefail

COOLDOWN_SECONDS="${COOLDOWN_SECONDS:-300}"
POLL_INTERVAL="${POLL_INTERVAL:-30}"
LOGGER_TAG="ovs-vf-recovery"

OVS_STAMP_FILE="${OVS_STAMP_FILE:-/run/ovs-vf-recovery-ovs.stamp}"
OVS_ERROR_PATTERN="Error attaching device"

OVNK_STAMP_FILE="${OVNK_STAMP_FILE:-/run/ovs-vf-recovery-ovnk.stamp}"
OVNK_ERROR_PATTERN="No such device"
OVNK_CONTAINER_NAME="doca-ovnkube-controller"

# ---------------------------------------------------------------------------
# Utility functions
# ---------------------------------------------------------------------------

log() { echo "[$(date -Iseconds)] [$LOGGER_TAG] $*"; }

# cooldown_ok STAMP_FILE
# Returns 0 (true) if enough time has passed since last action, 1 otherwise.
cooldown_ok() {
    local stamp_file="$1"
    if [ ! -f "$stamp_file" ]; then
        log "No stamp file ($stamp_file), first action."
        return 0
    fi
    local last now elapsed
    last=$(cat "$stamp_file")
    now=$(date +%s)
    elapsed=$(( now - last ))
    log "Cooldown check ($stamp_file): elapsed=${elapsed}s, required=${COOLDOWN_SECONDS}s"
    if [ "$elapsed" -lt "$COOLDOWN_SECONDS" ]; then
        log "Cooldown active, skipping."
        return 1
    fi
    log "Cooldown expired, proceeding."
    return 0
}

# mark_cooldown STAMP_FILE
mark_cooldown() {
    date +%s > "$1"
}

# ---------------------------------------------------------------------------
# Error detection
# ---------------------------------------------------------------------------

# find_matching_errors PATTERN
# Filters the global $ovs_output for interfaces matching PATTERN.
# Sets: match_count, match_names
find_matching_errors() {
    local pattern="$1"
    match_count=$(echo "$ovs_output" \
      | jq "[.data[] | select(.[1] | tostring | test(\"$pattern\"))] | length" 2>&1)
    local rc=$?
    if [ "$rc" -ne 0 ]; then
        log "ERROR: jq failed (rc=$rc) for pattern '$pattern': $match_count"
        match_count=0
        match_names=""
        return 1
    fi
    if [ "${match_count:-0}" -gt 0 ]; then
        match_names=$(echo "$ovs_output" \
          | jq -r "[.data[] | select(.[1] | tostring | test(\"$pattern\")) | .[0]] | join(\", \")" 2>/dev/null)
    else
        match_names=""
    fi
    return 0
}

# ---------------------------------------------------------------------------
# Recovery handlers
# ---------------------------------------------------------------------------

# handle_ovs_attach_errors
# Restarts the full openvswitch stack when VF representors fail to attach.
handle_ovs_attach_errors() {
    find_matching_errors "$OVS_ERROR_PATTERN"
    if [ "${match_count:-0}" -eq 0 ]; then
        return
    fi

    log "Found ${match_count} interface(s) with attach errors: ${match_names:-unknown}"

    if ! cooldown_ok "$OVS_STAMP_FILE"; then
        return
    fi

    mark_cooldown "$OVS_STAMP_FILE"
    log "Restarting openvswitch to recover ${match_count} broken interface(s): ${match_names:-unknown}"
    systemctl restart openvswitch
    local rc=$?
    log "systemctl restart openvswitch rc=$rc"
    if [ "$rc" -ne 0 ]; then
        log "WARNING: openvswitch restart failed (rc=$rc)"
    fi
    log "Waiting 60s for openvswitch to fully restart."
    sleep 60
}

# handle_ovnk_device_errors
# Stops the ovnkube-controller container (kubelet will restart it) when the
# OVNK management port reports "No such device".
handle_ovnk_device_errors() {
    find_matching_errors "$OVNK_ERROR_PATTERN"
    if [ "${match_count:-0}" -eq 0 ]; then
        return
    fi

    log "Found ${match_count} interface(s) with OVNK errors: ${match_names:-unknown}"

    if ! cooldown_ok "$OVNK_STAMP_FILE"; then
        return
    fi

    mark_cooldown "$OVNK_STAMP_FILE"

    local container_id
    container_id=$(crictl ps --name "$OVNK_CONTAINER_NAME" -q 2>&1)
    if [ -z "$container_id" ]; then
        log "OVNK: container '$OVNK_CONTAINER_NAME' not found, skipping restart."
        return
    fi
    log "OVNK: stopping container $OVNK_CONTAINER_NAME (id=$container_id), kubelet will restart it."
    crictl stop "$container_id"
    local rc=$?
    log "OVNK: crictl stop rc=$rc"
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

log "Starting — polling every ${POLL_INTERVAL}s for DPDK errors."
log "Config: COOLDOWN_SECONDS=$COOLDOWN_SECONDS"
log "Config: OVS pattern='$OVS_ERROR_PATTERN' stamp=$OVS_STAMP_FILE"
log "Config: OVNK pattern='$OVNK_ERROR_PATTERN' container=$OVNK_CONTAINER_NAME stamp=$OVNK_STAMP_FILE"

poll_count=0

while true; do
    sleep "$POLL_INTERVAL"
    poll_count=$((poll_count + 1))

    log "Poll #${poll_count}: querying OVS..."

    ovs_output=$(ovs-vsctl --format=json --columns=name,error,type find Interface type=dpdk 'error!=[]' 2>&1)
    ovs_rc=$?
    log "Poll #${poll_count}: ovs-vsctl rc=$ovs_rc"

    if [ "$ovs_rc" -ne 0 ]; then
        log "ERROR: ovs-vsctl failed (rc=$ovs_rc): $ovs_output"
        continue
    fi

    total_error_ifaces=$(echo "$ovs_output" | jq '.data | length' 2>/dev/null)
    log "Poll #${poll_count}: OVS returned ${total_error_ifaces:-0} interface(s) with any error"

    if [ "${total_error_ifaces:-0}" -eq 0 ]; then
        log "Poll #${poll_count}: no errors, all healthy."
        continue
    fi

    # Debug: log column layout and first error row.
    headings=$(echo "$ovs_output" | jq -c '.headings' 2>/dev/null)
    log "Poll #${poll_count}: columns=$headings"
    first_row=$(echo "$ovs_output" | jq -c '.data[0]' 2>/dev/null)
    log "Poll #${poll_count}: first row=$first_row"

    # Run each recovery handler. Add new handlers here.
    handle_ovs_attach_errors
    handle_ovnk_device_errors

    log "Poll #${poll_count}: recovery cycle complete."
done
