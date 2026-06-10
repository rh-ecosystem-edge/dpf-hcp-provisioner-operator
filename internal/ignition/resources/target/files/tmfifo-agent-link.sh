#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

DPUFLAVOR_FILE="/etc/dpf/dpuflavor.json"
DPU_MODE=$(jq -r '.spec.dpuMode // "dpu"' "$DPUFLAVOR_FILE" 2>/dev/null || echo "dpu")
if [ "$DPU_MODE" = "zero-trust" ]; then
    echo "INFO: Zero-trust mode, skipping tmfifo host agent wait"
    exit 0
fi

TIMEOUT=900
START=$(date +%s)

echo "INFO: Waiting for IPv6 connectivity to host agent via tmfifo_net0..."
while true; do
    if ping -6 -c1 -W1 fe80::1%tmfifo_net0 &>/dev/null; then
        break
    fi
    ELAPSED=$(($(date +%s) - START))
    if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
        echo "ERROR: Timed out waiting for connectivity to host agent via tmfifo_net0"
        /usr/local/bin/dpuagent-client.py send-error "HostAgentUnreachable" "Timed out after ${TIMEOUT}s waiting for tmfifo_net0 connectivity"
        exit 1
    fi
    sleep 1
done
echo "INFO: IPv6 connectivity established."

echo "INFO: Waiting for host agent HTTP server..."
while true; do
    if curl -s -o /dev/null --connect-timeout 5 --max-time 10 "http://[fe80::1%25tmfifo_net0]:11029/"; then
        break
    fi
    ELAPSED=$(($(date +%s) - START))
    if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
        echo "ERROR: Timed out waiting for host agent HTTP server"
        /usr/local/bin/dpuagent-client.py send-error "HostAgentUnreachable" "Timed out after ${TIMEOUT}s waiting for host agent HTTP"
        exit 1
    fi
    sleep 1
done
echo "INFO: Host agent is reachable."
