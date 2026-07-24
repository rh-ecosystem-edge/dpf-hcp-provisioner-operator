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
source /opt/mellanox/bfb/bmc

fw_condition() {
    is_zero_trust || /usr/local/bin/dpuagent-client.py update-condition "$@" || true
}

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

flint_device=$cx_pcidev

if mokutil --sb-state 2>/dev/null | grep -q "SecureBoot enabled"; then
    flint_device=$(grep -l "PCI_SLOT_NAME=$cx_pcidev" /sys/class/fwctl/*/device/uevent | awk -F/ '{print "/dev/fwctl/"$5}')
fi

PSID=$(mstflint -d $flint_device q | grep PSID | awk '{print $NF}')

log "INFO: Updating NIC firmware"
fw_condition NICFirmwareUpgraded False Upgrading "NIC firmware upgrade in progress"
nic_start=$(date +%s)
if ! update_nic_firmware; then
    ilog "ERROR: See /tmp/mlnx_fw_update.log for details"
    fw_error "NIC firmware update failed"
fi
nic_elapsed=$(($(date +%s) - nic_start))
fw_condition NICFirmwareUpgraded True Upgraded "NIC firmware upgrade completed in ${nic_elapsed}s"

# BMC Update
RC=0 # Variable used in /opt/mellanox/bfb/bmc
# The .pldm golden image update path has no version check and would redundantly
# flash on every boot. Disable since BMC/CEC firmware updates are sufficient.
UPDATE_DPU_GOLDEN_IMAGE="no"
UPDATE_NIC_FW_GOLDEN_IMAGE="no"
# # New BMC Credentials
BMC_USER="firmware_updater"
BMC_PASSWORD="$(tr -dc 'A-Za-z0-9' </dev/urandom | head -c 4)-$(tr -dc 'A-Za-z0-9' </dev/urandom | head -c 4)_$(tr -dc '0-9' </dev/urandom | head -c 2)$(tr -dc 'a-z' </dev/urandom | head -c 1)$(tr -dc 'A-Z' </dev/urandom | head -c 1)"
# # BMC Firmware Update
BMC_REBOOT="yes"
CEC_REBOOT="yes"
USER_ID=8

pre_bmc_components_update() {
    ipmitool user set name $USER_ID $BMC_USER
    ipmitool user set password $USER_ID $BMC_PASSWORD
    ipmitool user enable $USER_ID
    ipmitool channel setaccess 1 $USER_ID ipmi=on
    ipmitool user priv $USER_ID 0x4 1
}

_bmc_cleaned=0
post_bmc_components_update() {
    if [ "$_bmc_cleaned" = "1" ]; then return; fi
    _bmc_cleaned=1
    ipmitool user set name $USER_ID "" 2>/dev/null || true
    if ip link show vlan4040 &>/dev/null; then
        log "INFO: Removing vlan4040 interface"
        ip link set dev vlan4040 down 2>/dev/null || true
        ip link del vlan4040 2>/dev/null || true
    fi
}
trap post_bmc_components_update EXIT

fw_condition BMCFirmwareUpgraded False Upgrading "BMC firmware upgrade in progress"
bmc_start=$(date +%s)
bmc_rc=0
bmc_components_update || bmc_rc=$?
bmc_elapsed=$(($(date +%s) - bmc_start))
if [ "$bmc_rc" -eq 0 ]; then
    fw_condition BMCFirmwareUpgraded True Upgraded "BMC firmware upgrade completed in ${bmc_elapsed}s"
else
    log "WARNING: BMC components update returned rc=$bmc_rc"
    fw_condition BMCFirmwareUpgraded False Failed "BMC firmware upgrade failed after ${bmc_elapsed}s"
fi

mkdir -p /var/dpf
echo "$current_ver" >"$STAMP"
log "INFO: Firmware upgrade complete (doca-runtime $current_ver)"
