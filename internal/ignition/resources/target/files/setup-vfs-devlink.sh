#!/bin/bash

echo "INFO: Waiting for host agent connectivity..."
until curl -s -o /dev/null "http://[fe80::1%25tmfifo_net0]:11029/"; do
  echo "INFO: Waiting for host agent HTTP server..."
  sleep 5
done
echo "INFO: Host agent is reachable."

TIMEOUT=600
START_TIME=$(date +%s)
LAST_ERROR=""

VF_READY=false
until $VF_READY; do
  ELAPSED=$(($(date +%s) - START_TIME))
  if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
    echo "ERROR: setup-vfs-devlink timed out after ${TIMEOUT}s"
    /usr/local/bin/dpuagent-client.py send-error "VFSetupTimedOut" "setup-vfs-devlink failed after ${TIMEOUT}s: ${LAST_ERROR}"
    exit 1
  fi

  /usr/local/bin/dpuagent-client.py configure-host-vfs || {
    LAST_ERROR="configure-host-vfs failed"
    echo "WARN: ${LAST_ERROR}"
    continue
  }

  sleep 30

  pcie_dev_list=$(lspci -Dd 15b3: | grep ConnectX | awk '{print $1}')
  failed=false
  for dev in ${pcie_dev_list}; do
    if ! devlink dev eswitch set pci/${dev} mode switchdev; then
      LAST_ERROR="devlink switchdev on ${dev} failed"
      echo "WARN: ${LAST_ERROR}"
      failed=true
      break
    fi
  done
  if $failed; then
    continue
  fi

  vf_count=$(devlink port show 2>/dev/null | grep -c "flavour pcivf")
  if [ "$vf_count" -gt 0 ]; then
    echo "INFO: Found $vf_count VFs"
    VF_READY=true
  else
    LAST_ERROR="no VFs found after devlink switchdev"
    echo "WARN: ${LAST_ERROR}"
  fi
done

echo "Finished activating devlink"
