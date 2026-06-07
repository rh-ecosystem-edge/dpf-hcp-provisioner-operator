#!/bin/bash

# Monitors PF0 and PF1 link state and reports changes to the DPU object
# via dpuagent-client.py → host agent → DPU.status.agentStatus.conditions.

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

POLL_INTERVAL=${PF_MONITOR_INTERVAL:-30}

# Discover PF interfaces by type (not by name).
# Uses devlink port flavour as the primary method, with sysfs fallback.
find_pf_interfaces() {
    local -a pfs=()

    while IFS= read -r line; do
        local netdev
        netdev=$(echo "$line" | sed -n 's/.*netdev \([^ ]*\).*/\1/p')
        if [ -n "$netdev" ]; then
            pfs+=("$netdev")
        fi
    done < <(devlink port show 2>/dev/null | grep "flavour physical")

    if [ ${#pfs[@]} -eq 0 ]; then
        for iface in /sys/class/net/*; do
            local name
            name=$(basename "$iface")
            if [ -f "$iface/device/sriov_totalvfs" ] && [ ! -L "$iface/device/physfn" ]; then
                pfs+=("$name")
            fi
        done
    fi

    printf '%s\n' "${pfs[@]}" | sort
}

get_operstate() {
    cat "/sys/class/net/${1}/operstate" 2>/dev/null || echo "unknown"
}

report_status() {
    local pf_label=$1
    local iface=$2
    local state=$3
    /usr/local/bin/dpuagent-client.py update-pf-status "$pf_label" "$state" "$iface" \
        "${pf_label} interface ${iface} operstate is ${state}" || true
}

declare -A PF_IFACES
declare -A LAST_STATES

echo "INFO: pf-monitor starting (poll every ${POLL_INTERVAL}s)..."

# Wait until at least one PF interface appears (the NIC driver may not be ready at boot).
while true; do
    mapfile -t pfs < <(find_pf_interfaces)
    if [ ${#pfs[@]} -gt 0 ]; then
        break
    fi
    echo "WARN: No PF interfaces found yet, retrying in ${POLL_INTERVAL}s..."
    sleep "$POLL_INTERVAL"
done

for i in "${!pfs[@]}"; do
    PF_IFACES["PF${i}"]="${pfs[$i]}"
done

for label in "${!PF_IFACES[@]}"; do
    echo "INFO: Monitoring ${label} on interface ${PF_IFACES[$label]}"
done

while true; do
    for label in "${!PF_IFACES[@]}"; do
        iface="${PF_IFACES[$label]}"
        state=$(get_operstate "$iface")

        if [ "$state" != "${LAST_STATES[$label]}" ]; then
            echo "INFO: ${label} (${iface}) state changed: ${LAST_STATES[$label]:-<init>} -> ${state}"
            report_status "$label" "$iface" "$state"
            LAST_STATES[$label]="$state"
        fi
    done

    sleep "$POLL_INTERVAL"
done
