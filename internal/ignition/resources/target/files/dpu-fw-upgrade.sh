#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

LOG="/tmp/dpu-fw-upgrade.log"
rshimlog=$(which bfrshlog 2>/dev/null || true)

rlog() {
    msg=$(echo "$*" | sed 's/INFO://;s/ERROR:/ERR/;s/WARNING:/WARN/')
    if [ -n "$rshimlog" ]; then
        $rshimlog "$msg"
    fi
}

ilog() {
    echo "$*"
    msg="[$(date +%H:%M:%S)] $*"
    echo "$msg" >>$LOG
    echo "$msg" >/dev/kmsg 2>/dev/null || true
}

log() {
    ilog "$*"
    rlog "$*"
}

function_exists() {
    declare -f -F "$1" >/dev/null
    return $?
}

STAMP="/var/dpf/fw-installed-version"

current_ver=$(rpm -q --qf '%{VERSION}-%{RELEASE}' doca-runtime 2>/dev/null || echo "unknown")

if [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$current_ver" ]; then
    log "INFO: Firmware already upgraded for doca-runtime $current_ver, skipping"
    exit 0
fi

unmount_partition() { true; }
update_progress() { true; }
bind_partitions() { true; }

log "INFO: Sourcing firmware scripts"
source /opt/mellanox/bfb/atf-uefi
source /opt/mellanox/bfb/nic-fw

fw_error() {
    log "ERROR: $1"
    /usr/local/bin/dpuagent-client.py send-error "FirmwareUpgradeFailed" "$1"
    exit 1
}

log "INFO: Updating ATF/UEFI"
if ! update_atf_uefi; then
    fw_error "ATF/UEFI update failed"
fi

cx_pcidev=$(lspci -nD 2>/dev/null | grep 15b3:a2d[26c] | awk '{print $1}' | head -1)
cx_dev_id=$(lspci -nD -s ${cx_pcidev} 2>/dev/null | awk -F ':' '{print strtonum("0x" $NF)}')
PSID=$(mstflint -d $cx_pcidev q | grep PSID | awk '{print $NF}')

log "INFO: Updating NIC firmware"
if ! update_nic_firmware; then
    ilog "ERROR: See /tmp/mlnx_fw_update.log for details"
    fw_error "NIC firmware update failed"
fi

mkdir -p /var/dpf
echo "$current_ver" >"$STAMP"
log "INFO: Firmware upgrade complete (doca-runtime $current_ver)"
