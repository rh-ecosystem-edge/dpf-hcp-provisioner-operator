#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

echo "Installing dpu-agent..."

TIMEOUT=1800 # 30 minutes
START=$(date +%s)

while true; do
    if ping -6 -c1 -W1 fe80::1%tmfifo_net0 &>/dev/null; then
        break
    fi
    ELAPSED=$(( $(date +%s) - START ))
    if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
        echo "ERROR: Timed out waiting for connectivity to host agent via tmfifo_net0"
        exit 1
    fi
    echo "Waiting for connectivity to host agent via tmfifo_net0... (${ELAPSED}s/${TIMEOUT}s)"
    sleep 1
done



cd /tmp
dnf download dpu-agent &&
  rpm2cpio dpu-agent*.rpm | cpio -idm --no-absolute-filenames &&
  mv opt/dpf/dpuagent /usr/local/bin/dpu-agent &&
  restorecon /usr/local/bin/dpu-agent
