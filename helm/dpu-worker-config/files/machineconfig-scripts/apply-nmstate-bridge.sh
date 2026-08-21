#!/bin/bash
set -e

BRIDGE_NAME="br-ex"
IP_HINT_FILE="/run/nodeip-configuration/primary-ip"
NODE_IP=""

read_node_ip() {
    if [[ ! -f "$IP_HINT_FILE" ]]; then
        echo "ERROR: IP hint file not found: $IP_HINT_FILE" >&2
        return 1
    fi

    NODE_IP=$(tr -d '[:space:]' < "$IP_HINT_FILE")

    if [[ -z "$NODE_IP" ]]; then
        echo "ERROR: IP hint file is empty: $IP_HINT_FILE" >&2
        return 1
    fi

    echo "INFO: Node IP from hint file: $NODE_IP"
}

wait_for_bridge_ip() {
    local bridge="$1"
    local timeout=120
    local interval=2
    local elapsed=0

    echo "INFO: Waiting up to ${timeout}s for $bridge to acquire $NODE_IP..."
    while (( elapsed < timeout )); do
        if ip -o addr show dev "$bridge" | grep -qw "$NODE_IP"; then
            echo "INFO: $bridge has $NODE_IP."
            ip addr show dev "$bridge"
            return 0
        fi
        sleep "$interval"
        elapsed=$(( elapsed + interval ))
    done

    echo "ERROR: $bridge did not acquire $NODE_IP within ${timeout}s." >&2
    return 1
}

set_bridge_rp_filter_loose() {
    local bridge="$1"
    echo "INFO: Setting rp_filter=2 (loose mode) on $bridge"
    sysctl -w "net.ipv4.conf.${bridge}.rp_filter=2"
}

# Validate that an existing bridge is healthy: has a port and acquires the node IP.
# Returns 0 if bridge is good, 1 if it needs recreation.
validate_existing_bridge() {
    if ! ip link show "$BRIDGE_NAME" &> /dev/null; then
        return 1
    fi

    echo "INFO: Bridge '$BRIDGE_NAME' exists, validating..."

    local port
    port=$(ip -j link show master "$BRIDGE_NAME" 2>/dev/null | jq -r '.[0].ifname // empty')
    if [[ -z "$port" ]]; then
        echo "WARN: Bridge '$BRIDGE_NAME' has no port attached, needs recreation." >&2
        return 1
    fi
    echo "INFO: Bridge port: $port"

    if wait_for_bridge_ip "$BRIDGE_NAME"; then
        set_bridge_rp_filter_loose "$BRIDGE_NAME"
        echo "INFO: Existing bridge '$BRIDGE_NAME' is healthy, keeping it."
        return 0
    fi

    echo "WARN: Bridge '$BRIDGE_NAME' failed IP validation, needs recreation." >&2
    return 1
}

cleanup_existing_bridge() {
    if ! ip link show "$BRIDGE_NAME" &> /dev/null; then
        return 0
    fi

    echo "INFO: Removing bridge '$BRIDGE_NAME'..."

    local absent_config cleanup_file
    absent_config=$(jq -n --arg br "$BRIDGE_NAME" '{
        "interfaces": [{
            "name": $br,
            "type": "linux-bridge",
            "state": "absent"
        }]
    }')

    cleanup_file=$(mktemp /tmp/br-dpu-cleanup.XXXXXX) || return 1
    printf '%s\n' "$absent_config" > "$cleanup_file"

    echo "--- Generated NMState cleanup state ---"
    cat "$cleanup_file"
    echo "----------------------------------------"

    if nmstatectl apply "$cleanup_file"; then
        echo "INFO: Bridge '$BRIDGE_NAME' removed successfully via nmstatectl."
    else
        echo "WARN: nmstatectl cleanup failed, attempting manual removal..." >&2
        nmcli connection delete "$BRIDGE_NAME" 2>/dev/null || true
        ip link set "$BRIDGE_NAME" down 2>/dev/null || true
        ip link delete "$BRIDGE_NAME" type bridge 2>/dev/null || true
        echo "INFO: Waiting 5s for network to stabilize after manual cleanup..."
        sleep 5
    fi

    rm -f "$cleanup_file"

    if ip link show "$BRIDGE_NAME" &> /dev/null; then
        echo "ERROR: Bridge '$BRIDGE_NAME' still exists after cleanup." >&2
        return 1
    fi
}

get_nodeip_hint_interface() {
    local iface timeout=60 interval=5 elapsed=0

    echo "INFO: Waiting up to ${timeout}s for an interface (other than $BRIDGE_NAME) to have $NODE_IP..." >&2
    while (( elapsed < timeout )); do
        iface=$(ip -j addr | jq -r --arg ip "$NODE_IP" --arg br "$BRIDGE_NAME" \
            'first(.[] | select(any(.addr_info[]; .local==$ip) and .ifname!=$br)) | .ifname')

        if [[ -n "${iface}" && "${iface}" != "null" ]]; then
            echo "INFO: Found interface $iface with $NODE_IP." >&2
            echo "${iface}"
            return 0
        fi
        sleep "$interval"
        elapsed=$(( elapsed + interval ))
    done

    echo "ERROR: No interface found with IP $NODE_IP within ${timeout}s" >&2
    return 1
}

apply_linux_bridge() {
    local iface="$1"
    local bridge="$BRIDGE_NAME"

    if [ -z "$iface" ]; then
        echo "ERROR: No physical interface matches the Node IP in $IP_HINT_FILE." >&2
        exit 1
    fi

    local iface_mtu
    iface_mtu=$(ip -j link show "$iface" | jq -r '.[0].mtu')
    echo "INFO: Target interface: $iface (MTU: $iface_mtu)"

    local routes_json
    routes_json=$(nmstatectl show --json | jq -c --arg phys "$iface" \
        '[.routes.config // [] | .[] | select(.["next-hop-interface"] == $phys)]')

    local config_file
    config_file=$(mktemp /tmp/br-dpu-config.XXXXXX) || exit 1

    echo "INFO: Generating NMState desired state..."
    nmstatectl show "$iface" --json | jq \
        --arg br "$bridge" \
        --arg phys "$iface" \
        --argjson iface_mtu "$iface_mtu" \
        --argjson phys_routes "$routes_json" \
    '
    .interfaces[0] as $p |
    {
        "interfaces": [
            ({
                "name": $br,
                "type": "linux-bridge",
                "state": "up",
                "mtu": $iface_mtu,
                "mac-address": $p."mac-address",
                "ipv4": ($p.ipv4 | del(.forwarding)),
                "ipv6": ($p.ipv6 | del(.forwarding)),
                "bridge": {
                    "options": { "stp": { "enabled": false } },
                    "port": [{ "name": $phys }]
                }
            }),
            ({
                "name": $phys,
                "type": $p.type,
                "state": "up",
                "mtu": $iface_mtu,
                "ipv4": { "enabled": false },
                "ipv6": { "enabled": false }
            } + if $p["link-aggregation"] then
                    { "link-aggregation": $p["link-aggregation"] }
                else {} end)
        ]
    }
    + if ($phys_routes | length) > 0 then
        { "routes": { "config": [$phys_routes[] | .["next-hop-interface"] = $br] } }
      else {} end
    ' > "$config_file"

    echo "--- Generated NMState desired state ---"
    cat "$config_file"
    echo "---------------------------------------"

    echo "INFO: Applying configuration via nmstatectl..."
    if nmstatectl apply "$config_file"; then
        echo "SUCCESS: Bridge $bridge created successfully."
        wait_for_bridge_ip "$bridge"
        ip addr show "$bridge"

        set_bridge_rp_filter_loose "$bridge"
    else
        echo "ERROR: Failed to apply NMState configuration." >&2
        rm -f "$config_file"
        exit 1
    fi

    rm -f "$config_file"
}

# --- Main ---
read_node_ip

if validate_existing_bridge; then
    exit 0
fi

echo "INFO: Bridge needs (re)creation..."
cleanup_existing_bridge || exit 1
SELECTED_IFACE=$(get_nodeip_hint_interface)
apply_linux_bridge "$SELECTED_IFACE"
