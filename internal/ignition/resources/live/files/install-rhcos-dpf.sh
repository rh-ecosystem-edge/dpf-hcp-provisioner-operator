#!/bin/bash

TARGET_DEVICE=/dev/nvme0n1
IGNITION_FILE="/var/target.ign"
DPUFLAVOR_FILE="/etc/dpf/dpuflavor.json"

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

    efibootmgr -c -d "$TARGET_DEVICE" -p 2 -l '\EFI\redhat\shimaa64.efi' -L "Red-Hat CoreOS GRUB"
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

NVCONFIG_CHANGED=false

run_mstconfig() {
    if ! mstconfig "$@"; then
        error "NVConfig" "Failed: mstconfig $*"
        return 1
    fi
}

set_nvconfig() {
    local pcie_dev_list=$(lspci -d 15b3: | grep ConnectX | awk '{print $1}')
    for dev in ${pcie_dev_list}; do
        local query_output
        if ! query_output=$(mstconfig -d ${dev} -e q SRIOV_EN NUM_OF_VFS 2>&1); then
            error "NVConfig" "Failed to query NVConfig on dev ${dev}"
            exit 1
        fi

        local sriov_en=$(echo "$query_output" | grep SRIOV_EN | awk '{print $(NF-1)}')
        local current_vfs=$(echo "$query_output" | grep NUM_OF_VFS | awk '{print $(NF-1)}')

        if [ "$sriov_en" != "True(1)" ] || [ "$current_vfs" -le 0 ]; then
            log "INFO: Setting NVConfig on dev ${dev}: SRIOV_EN=1 NUM_OF_VFS=1 (was SRIOV_EN=${sriov_en}, NUM_OF_VFS=${current_vfs})"
            run_mstconfig -d ${dev} -y reset
            run_mstconfig -d ${dev} -y set SRIOV_EN=1 NUM_OF_VFS=1
            NVCONFIG_CHANGED=true
        else
            log "INFO: NVConfig on dev ${dev} already correct, skipping"
        fi
    done
    log "INFO: Finished setting nvconfig parameters"
}

install_rhcos() {
    log "INFO: Installing Red Hat CoreOS on $TARGET_DEVICE with ignition file $IGNITION_FILE"

    KERNEL_PARAMETERS="console=hvc0 console=ttyAMA0 earlycon=pl011,0x13010000 ignore_loglevel modprobe.blacklist=mlxbf_pmc"
    FLAVOR_KARGS=$(jq -r .spec.grub.kernelParameters[] "$DPUFLAVOR_FILE")

    for param in $FLAVOR_KARGS; do
        case " $KERNEL_PARAMETERS " in
        *" $param "*) ;;
        *) KERNEL_PARAMETERS="$KERNEL_PARAMETERS $param" ;;
        esac
    done

    coreos-installer install "$TARGET_DEVICE" \
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
