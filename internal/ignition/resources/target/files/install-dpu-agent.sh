#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

if [ -f /usr/local/bin/dpu-agent ]; then
    echo "dpu-agent already installed, skipping"
    exit 0
fi

echo "Installing dpu-agent..."

DPUFLAVOR_FILE="/etc/dpf/dpuflavor.json"
DPU_MODE=$(jq -r '.spec.dpuMode // "dpu"' "$DPUFLAVOR_FILE" 2>/dev/null || echo "dpu")
is_zero_trust() { [ "$DPU_MODE" = "zero-trust" ]; }

install_error() {
    echo "ERROR: $1"
    is_zero_trust || /usr/local/bin/dpuagent-client.py send-error "DPUAgentInstallFailed" "$1"
    exit 1
}

if is_zero_trust; then
    echo "Zero-trust mode: downloading dpu-agent from bfb-registry at $BFBRegistryURL"

    if [ -z "$BFBRegistryURL" ]; then
        install_error "Zero-trust mode requires BFBRegistryURL"
    fi

    TIMEOUT=300
    START=$(date +%s)
    while true; do
        if curl -sf --max-time 5 "$BFBRegistryURL/rpm/repodata/repomd.xml" >/dev/null 2>&1; then
            break
        fi
        ELAPSED=$(($(date +%s) - START))
        if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
            install_error "Timed out waiting for bfb-registry at $BFBRegistryURL"
        fi
        echo "Waiting for bfb-registry connectivity... (${ELAPSED}s/${TIMEOUT}s)"
        sleep 5
    done

    cd /tmp
    dnf download dpu-agent --disablerepo=* --repofrompath=zt-agentrepo,"$BFBRegistryURL/rpm/" || install_error "dnf download dpu-agent from bfb-registry failed"
else
    TIMEOUT=900
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

    cd /tmp
    dnf download dpu-agent --disablerepo=* --enablerepo=agentrepo || install_error "dnf download dpu-agent failed"
fi

rpm2cpio dpu-agent*.rpm | cpio -idm --no-absolute-filenames
[ -f opt/dpf/dpuagent ] || install_error "Failed to extract dpu-agent package"
mv opt/dpf/dpuagent /usr/local/bin/dpu-agent || install_error "Failed to move dpu-agent binary"
restorecon /usr/local/bin/dpu-agent || install_error "Failed to restorecon dpu-agent"
