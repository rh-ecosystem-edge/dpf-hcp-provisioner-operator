#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

if [ -f /usr/local/bin/dpu-agent ]; then
    echo "dpu-agent already installed, skipping"
    exit 0
fi

echo "Installing dpu-agent..."

install_error() {
    echo "ERROR: $1"
    /usr/local/bin/dpuagent-client.py send-error "DPUAgentInstallFailed" "$1"
    exit 1
}

cd /tmp
dnf download dpu-agent --disablerepo=* --enablerepo=agentrepo || install_error "dnf download dpu-agent failed"
rpm2cpio dpu-agent*.rpm | cpio -idm --no-absolute-filenames
[ -f opt/dpf/dpuagent ] || install_error "Failed to extract dpu-agent package"
mv opt/dpf/dpuagent /usr/local/bin/dpu-agent || install_error "Failed to move dpu-agent binary"
restorecon /usr/local/bin/dpu-agent || install_error "Failed to restorecon dpu-agent"
