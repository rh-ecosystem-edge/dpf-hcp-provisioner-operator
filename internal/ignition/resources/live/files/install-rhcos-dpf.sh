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

validate_identity() {
    if [ -z "$DPUName" ] || [ -z "$DPUNamespace" ] || [ -z "$DPUUID" ]; then
        log "ERROR: Missing identity env vars (DPUName, DPUNamespace, DPUUID)"
        exit 1
    fi
    log "INFO: DPU identity: $DPUName ($DPUUID) in $DPUNamespace"
}

validate_ignition() {
    if [[ -f "$IGNITION_FILE" ]]; then
        log "INFO: Ignition file found at $IGNITION_FILE"
        if ! jq -e . "$IGNITION_FILE" >/dev/null 2>&1; then
            log "ERROR: Ignition file is not a valid JSON."
            exit 1
        fi
    else
        log "ERROR: Ignition file is missing, skipping installation."
        exit 1
    fi
}

update_ignition() {
    /usr/local/bin/update_ignition.py "$IGNITION_FILE"
    if [ $? -ne 0 ]; then
        log "ERROR: Failed to update ignition file."
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
    until /usr/local/bin/dpuagent-client.py "$1"; do
        log "WARN: dpuagent-client $1 failed, retrying in 5s..."
        sleep 5
    done
}

set_nvconfig() {
    local nvconfig_params=$(jq -r '.spec.nvconfig[].parameters[]' "$DPUFLAVOR_FILE" | tr '\n' ' ')
    if [ -z "$nvconfig_params" ]; then
        log "INFO: No nvconfig parameters found, skipping"
        return
    fi

    local pcie_dev_list=$(lspci -d 15b3: | grep ConnectX | awk '{print $1}')
    for dev in ${pcie_dev_list}; do
        log "INFO: Saving NVConfig query to /tmp/nvconfig-${dev}.json"
        if ! mstconfig -d ${dev} -j /tmp/nvconfig-${dev}.json query; then
            log "ERROR: Failed to query NVConfig on dev ${dev}"
            exit 1
        fi

        log "INFO: Setting NVConfig on dev ${dev}: ${nvconfig_params}"
        if ! mstconfig -d ${dev} -y set ${nvconfig_params}; then
            log "ERROR: Failed to set NVConfig on dev ${dev}"
            exit 1
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
        log "ERROR: Failed to install Red Hat CoreOS."
        exit 1
    fi

    sync
}

wait_for_host_reboot_if_required() {
    for f in /tmp/nvconfig-*.json; do
        [ -f "$f" ] || continue
        local cfg=$(jq '.[].tlv_configuration' "$f")
        local sriov_en=$(echo "$cfg" | jq -r '.SRIOV_EN.next_value')
        local num_of_vfs=$(echo "$cfg" | jq -r '.NUM_OF_VFS.next_value')

        if [ "$sriov_en" != "True(1)" ] || [ "$num_of_vfs" = "0" ]; then
            log "INFO: Host reboot required, signaling host agent"
            dpu_agent update-host-reboot
            log "INFO: Waiting for host reboot..."
            sleep infinity
        fi
    done
    log "INFO: No host reboot required."
}

call_configure_host_vfs() {
    dpu_agent configure-host-vfs
}

validate_identity
validate_ignition
update_ignition
wait_for_host_agent
set_nvconfig

install_rhcos
setup_RHCOS_EFI_record
sync

log "INFO: Installation complete."

wait_for_host_reboot_if_required

call_configure_host_vfs

log "INFO: Waiting for 10 seconds before rebooting"
sleep 10
log "INFO: Rebooting..."
reboot
