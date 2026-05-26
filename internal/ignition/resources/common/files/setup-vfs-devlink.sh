#!/bin/bash

TIMEOUT=600
START_TIME=$(date +%s)
LAST_ERROR=""
ACTION="${1:-}"

elapsed() {
  echo $(($(date +%s) - START_TIME))
}

check_timeout() {
  if [ "$(elapsed)" -ge "$TIMEOUT" ]; then
    echo "ERROR: setup-vfs-devlink ($ACTION) timed out after ${TIMEOUT}s: ${LAST_ERROR}"
    if [ "$ACTION" = "create-vfs" ]; then
      /usr/local/bin/dpuagent-client.py send-error "VFSetupTimedOut" \
        "setup-vfs-devlink ($ACTION) failed after ${TIMEOUT}s: ${LAST_ERROR}"
    fi
    exit 1
  fi
}

get_connectx_devices() {
  lspci -Dd 15b3: | grep ConnectX | awk '{print $1}'
}

get_steering_mode() {
  local dev=$1
  devlink dev param show "pci/${dev}" name flow_steering_mode 2>/dev/null | tail -1 | awk '{print $NF}'
}

set_steering_mode() {
  local dev=$1
  local mode=$2
  devlink dev param set "pci/${dev}" name flow_steering_mode value "${mode}" cmode runtime
}

# Returns 0 if all ConnectX devices are in switchdev mode, 1 otherwise.
verify_switchdev() {
  for dev in $(get_connectx_devices); do
    mode=$(devlink dev eswitch show "pci/${dev}" 2>/dev/null | awk '{print $3}')
    if [ "$mode" != "switchdev" ]; then
      LAST_ERROR="pci/${dev} not in switchdev mode (mode=${mode})"
      echo "WARN: ${LAST_ERROR}"
      return 1
    fi
  done
  return 0
}

do_switchdev() {
  echo "INFO: Configuring steering mode before switchdev..."
  for dev in $(get_connectx_devices); do
    steering=$(get_steering_mode "$dev")
    if [ "$steering" = "dmfs" ]; then
      echo "INFO: pci/${dev} steering mode is dmfs, setting to smfs..."
      mode=$(devlink dev eswitch show "pci/${dev}" 2>/dev/null | awk '{print $3}')
      if [ "$mode" != "legacy" ]; then
        devlink dev eswitch set "pci/${dev}" mode legacy
      fi
      if ! set_steering_mode "$dev" smfs; then
        LAST_ERROR="Failed to set steering mode smfs on ${dev}"
        echo "WARN: ${LAST_ERROR}"
      fi
    fi
  done

  echo "INFO: Setting eswitch mode to switchdev..."
  while ! verify_switchdev; do
    check_timeout

    for dev in $(get_connectx_devices); do
      if ! devlink dev eswitch set "pci/${dev}" mode switchdev; then
        LAST_ERROR="devlink switchdev on ${dev} failed"
        echo "WARN: ${LAST_ERROR}"
      fi
    done

    sleep 5
  done

  echo "INFO: Finished switchdev"
}

do_create_vfs() {
  echo "INFO: Verifying switchdev mode and creating VFs..."
  while true; do
    check_timeout

    if ! verify_switchdev; then
      sleep 5
      continue
    fi
    echo "INFO: All devices in switchdev mode"

    if ! /usr/local/bin/dpuagent-client.py configure-host-vfs; then
      LAST_ERROR="configure-host-vfs failed"
      echo "WARN: ${LAST_ERROR}"
      sleep 5
      continue
    fi
    echo "INFO: configure-host-vfs succeeded"

    vf_count=$(devlink port show 2>/dev/null | grep -c "flavour pcivf")
    if [ "$vf_count" -gt 0 ]; then
      echo "INFO: Found $vf_count VFs"
      break
    fi
    LAST_ERROR="no VFs found after configure-host-vfs"
    echo "WARN: ${LAST_ERROR}"
    sleep 5
  done

  echo "INFO: Waiting for default route..."
  while true; do
    check_timeout
    if ip route show default | grep -q default; then
      break
    fi
    sleep 5
  done
  echo "INFO: Default route available"

  echo "INFO: Finished create-vfs"
}

case "$ACTION" in
switchdev)
  do_switchdev
  ;;
create-vfs)
  do_create_vfs
  ;;
*)
  echo "Usage: $0 {switchdev|create-vfs}"
  exit 1
  ;;
esac
