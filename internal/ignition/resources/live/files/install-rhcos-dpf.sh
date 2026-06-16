#!/bin/bash

BLOCK_DEVICE=/dev/nvme0n1
IGNITION_FILE="/var/target.ign"
DPUFLAVOR_FILE="/etc/dpf/dpuflavor.json"

SECUREBOOT=false
NVCONFIG_CHANGED=false

log() {
    msg="[$(date +%H:%M:%S)] $*"
    echo "$msg"
    echo "$msg" >/dev/kmsg
    /usr/local/bin/bflog.sh "$msg"
}

error() {
    log "ERROR: $1 - $2"
    dpu_agent send-error "$1" "$2"
}

validate_identity() {
    if [ -z "$DPUName" ] || [ -z "$DPUNamespace" ] || [ -z "$DPUUID" ]; then
        error "Ignition" "Missing identity env vars (DPUName, DPUNamespace, DPUUID)"
        exit 1
    fi
    log "INFO: DPU identity: $DPUName ($DPUUID) in $DPUNamespace"
}

validate_ignition() {
    if [[ -f "$IGNITION_FILE" ]]; then
        log "INFO: Ignition file found at $IGNITION_FILE"
        if ! jq -e . "$IGNITION_FILE" >/dev/null 2>&1; then
            error "Ignition" "Ignition file is not a valid JSON."
            exit 1
        fi
    else
        error "Ignition" "Ignition file is missing, skipping installation."
        exit 1
    fi
}

update_ignition() {
    /usr/local/bin/update_ignition.py "$IGNITION_FILE"
    if [ $? -ne 0 ]; then
        error "Ignition" "Failed to update ignition file."
        exit 1
    fi
}

setup_RHCOS_EFI_record() {
    # Delete all previous Red Hat CoreOS EFI records
    while efibootmgr -v | grep -q "Red-Hat CoreOS GRUB"; do
        BOOTNUM=$(efibootmgr -v | grep "Red-Hat CoreOS GRUB" | awk '{print $1}' | sed 's/Boot\(....\)\*$/\1/' | head -n1)
        if [[ -n "$BOOTNUM" ]]; then
            efibootmgr -b "$BOOTNUM" -B
            log "INFO: Deleted previous RHCOS EFI record Boot$BOOTNUM"
        else
            break
        fi
    done

    efibootmgr -c -d "$BLOCK_DEVICE" -p 2 -l '\EFI\redhat\shimaa64.efi' -L "Red-Hat CoreOS GRUB"
    log "INFO: Created new RHCOS EFI record."
}

wait_for_host_agent() {
    log "INFO: Waiting for host agent connectivity..."
    until ping -6 -c1 -W1 fe80::1%tmfifo_net0 &>/dev/null; do
        log "INFO: Waiting for IPv6 connectivity to host agent..."
        sleep 1
    done
    log "INFO: IPv6 connectivity established."

    until curl -s -o /dev/null "http://[fe80::1%25tmfifo_net0]:11029/"; do
        log "INFO: Waiting for host agent HTTP server..."
        sleep 1
    done
    log "INFO: Host agent is reachable."
}

dpu_agent() {
    local TIMEOUT=300 # 5 minutes
    local START=$(date +%s)

    until /usr/local/bin/dpuagent-client.py "$@"; do
        local ELAPSED=$(($(date +%s) - START))
        if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
            log "ERROR: Timed out contacting dpu-agent on call $1" >&2
            exit 1
        fi

        log "WARN: dpuagent-client $1 failed, retrying in 5s..." >&2
        sleep 5
    done
}

get_devlist() {
    if [ "$SECUREBOOT" = "true" ]; then
        ls /dev/fwctl/fwctl* 2>/dev/null
    else
        lspci -d 15b3: | grep ConnectX | awk '{print $1}'
    fi
}

setup_secureboot() {
    if mokutil --sb-state 2>/dev/null | grep -q "SecureBoot enabled"; then
        SECUREBOOT=true
    else
        return 0
    fi

    log "INFO: SecureBoot enabled, loading fwctl modules"
    modprobe fwctl 2>/dev/null
    modprobe mlx5_fwctl 2>/dev/null
    udevadm settle

    local retries=5
    while [ $retries -gt 0 ] && ! ls /dev/fwctl/fwctl* &>/dev/null; do
        log "INFO: Waiting for fwctl devices to appear..."
        sleep 1
        retries=$((retries - 1))
    done
}

validate_hardware() {
    if [ ! -b "$BLOCK_DEVICE" ]; then
        error "Hardware" "Target block device $BLOCK_DEVICE not found"
        exit 1
    fi

    local dev_list=$(get_devlist)
    if [ -z "$dev_list" ]; then
        log "$(lspci -d 15b3:)"
        error "Hardware" "No devices found"
        exit 1
    fi
}

run_mstconfig() {
    if ! mstconfig "$@"; then
        error "NVConfig" "Failed: mstconfig $*"
        return 1
    fi
}

# Usage: query_nvconfig <dev> <PARAM1> [PARAM2 ...]
# Sets NVCONFIG_<PARAM> variables in the caller's scope.
query_nvconfig() {
    local dev=$1; shift
    local query_output
    if ! query_output=$(mstconfig -d ${dev} -e q "$@" 2>&1); then
        error "NVConfig" "Failed to query NVConfig on dev ${dev}"
        exit 1
    fi
    local param
    for param in "$@"; do
        local value
        value=$(echo "$query_output" | grep "$param" | awk '{print $(NF-1)}')
        printf -v "NVCONFIG_${param}" '%s' "$value"
    done
}

set_nvconfig() {
    local dpuflavor_vfs
    dpuflavor_vfs=$(jq -r '[.spec.nvconfig[].parameters[] | select(startswith("NUM_OF_VFS="))] | first // "NUM_OF_VFS=1"' "$DPUFLAVOR_FILE" | cut -d= -f2)
    log "INFO: Desired NUM_OF_VFS=${dpuflavor_vfs} (from DPUFlavor)"

    local dev_list
    dev_list=$(get_devlist)

    for dev in ${dev_list}; do
        query_nvconfig "${dev}" SRIOV_EN NUM_OF_VFS PF_NUM_OF_VF_VALID

        log "INFO: dev ${dev}: SRIOV_EN=${NVCONFIG_SRIOV_EN} NUM_OF_VFS=${NVCONFIG_NUM_OF_VFS} PF_NUM_OF_VF_VALID=${NVCONFIG_PF_NUM_OF_VF_VALID}"

        if [ "$NVCONFIG_PF_NUM_OF_VF_VALID" == "True(1)" ] || [ "$NVCONFIG_SRIOV_EN" != "True(1)" ]; then
            log "INFO: Resetting NVConfig on dev ${dev} (PF_NUM_OF_VF_VALID=${NVCONFIG_PF_NUM_OF_VF_VALID}, SRIOV_EN=${NVCONFIG_SRIOV_EN})"
            run_mstconfig -d "${dev}" -y reset
            run_mstconfig -d "${dev}" -y set SRIOV_EN=1 PF_NUM_OF_VF_VALID=0
            NVCONFIG_CHANGED=true

            query_nvconfig "${dev}" NUM_OF_VFS
        fi

        if [ "$NVCONFIG_NUM_OF_VFS" -ne "$dpuflavor_vfs" ]; then
            log "INFO: Setting NUM_OF_VFS=${dpuflavor_vfs} on dev ${dev} (current: ${NVCONFIG_NUM_OF_VFS})"
            run_mstconfig -d "${dev}" -y set NUM_OF_VFS="${dpuflavor_vfs}"
            NVCONFIG_CHANGED=true
        else
            log "INFO: NVConfig on dev ${dev} already correct (NUM_OF_VFS=${NVCONFIG_NUM_OF_VFS})"
        fi
    done
    log "INFO: Finished setting nvconfig parameters"
}

install_rhcos() {
    log "INFO: Installing Red Hat CoreOS on $BLOCK_DEVICE with ignition file $IGNITION_FILE"

    KERNEL_PARAMETERS="console=hvc0 console=ttyAMA0 earlycon=pl011,0x13010000 ignore_loglevel"
    FLAVOR_KARGS=$(jq -r .spec.grub.kernelParameters[] "$DPUFLAVOR_FILE")

    for param in $FLAVOR_KARGS; do
        case " $KERNEL_PARAMETERS " in
        *" $param "*) ;;
        *) KERNEL_PARAMETERS="$KERNEL_PARAMETERS $param" ;;
        esac
    done

    coreos-installer install "$BLOCK_DEVICE" \
        --append-karg "$KERNEL_PARAMETERS" \
        --ignition-file "$IGNITION_FILE" \
        --offline

    if [ $? -ne 0 ]; then
        error "RHCOSInstallation" "Failed to install Red Hat CoreOS."
        exit 1
    fi
}

wait_for_host_reboot_if_required() {
    if [ "$NVCONFIG_CHANGED" = "true" ]; then
        log "INFO: Host reboot required (NVConfig was changed), signaling host agent"
        dpu_agent trigger-reboot PowerCycle

        while true; do
            sleep 60
            log "INFO: Waiting for host power cycle, retrying..."
            dpu_agent trigger-reboot PowerCycle
        done
    fi
    log "INFO: No host reboot required."
}

setup_secureboot
validate_hardware

validate_identity
validate_ignition
update_ignition
wait_for_host_agent
dpu_agent update-reboot-method-discovery
set_nvconfig

install_rhcos
setup_RHCOS_EFI_record
sync

log "INFO: Installation complete."

dpu_agent update-time

log "INFO: Waiting for DPU phase to reach 'DPU Config'..."

until [ "$(dpu_agent get-dpu-phase)" = "DPU Config" ]; do
    sleep 5
done
wait_for_host_reboot_if_required


log "INFO: Waiting for 10 seconds before rebooting"
sleep 10
log "INFO: Rebooting..."
reboot
