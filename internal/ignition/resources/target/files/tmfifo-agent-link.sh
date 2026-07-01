#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

if [ "$DPUMode" = "zero-trust" ]; then
    echo "INFO: Zero-trust mode, skipping tmfifo host agent wait"
    exit 0
fi

START=$(date +%s)

echo "INFO: Waiting for IPv6 connectivity to host agent via tmfifo_net0..."
while ! ping -6 -c1 -W1 fe80::1%tmfifo_net0 &>/dev/null; do
    ELAPSED=$(( $(date +%s) - START ))
    echo "INFO: Still waiting for tmfifo_net0 connectivity (elapsed ${ELAPSED}s)..."
    sleep 5
done
echo "INFO: IPv6 connectivity established."

echo "INFO: Waiting for host agent HTTP server..."
while ! curl -s -o /dev/null --connect-timeout 5 --max-time 10 "http://[fe80::1%25tmfifo_net0]:11029/"; do
    ELAPSED=$(( $(date +%s) - START ))
    echo "INFO: Still waiting for host agent HTTP server (elapsed ${ELAPSED}s)..."
    sleep 5
done
echo "INFO: Host agent is reachable."

/usr/local/bin/dpuagent-client.py update-condition \
  HostAgentConnected True Connected "Host agent reachable via tmfifo_net0" || true
