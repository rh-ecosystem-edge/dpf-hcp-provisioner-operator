#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

echo "Installing dpu-agent..."

TIMEOUT=900 # 15 minutes
START=$(date +%s)

while true; do
    if ping -6 -c1 -W1 fe80::1%tmfifo_net0 &>/dev/null; then
        break
    fi
    ELAPSED=$(($(date +%s) - START))
    if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
        echo "ERROR: Timed out waiting for connectivity to host agent via tmfifo_net0"
        exit 1
    fi
    echo "Waiting for connectivity to host agent via tmfifo_net0... (${ELAPSED}s/${TIMEOUT}s)"
    sleep 1
done

install_error() {
    echo "ERROR: $1"
    /usr/local/bin/dpuagent-client.py send-error "DPUAgentInstallFailed" "$1"
    exit 1
}

cd /tmp
dnf download dpu-agent || install_error "dnf download dpu-agent failed"
rpm2cpio dpu-agent*.rpm | cpio -idm --no-absolute-filenames
[ -f opt/dpf/dpuagent ] || install_error "Failed to extract dpu-agent package"
mv opt/dpf/dpuagent /usr/local/bin/dpu-agent || install_error "Failed to move dpu-agent binary"
restorecon /usr/local/bin/dpu-agent || install_error "Failed to restorecon dpu-agent"
