#!/bin/bash

# TODO: Implement rshim logging
log() {
    msg="[$(date +%H:%M:%S)] $*"
    echo "$msg"
    echo "$msg" >/dev/kmsg
    /usr/local/bin/bflog.sh "$msg"
}

IGNITION_FILE="/var/target.ign"
source /etc/bf.env

default_device=/dev/mmcblk0
if [ -b /dev/nvme0n1 ]; then
    default_device="/dev/$(
        cd /sys/block
        /bin/ls -1d nvme* | sort -n | tail -1
    )"
fi
device=${device:-"$default_device"}

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
    /usr/local/bin/update_ignition.py $IGNITION_FILE
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

    efibootmgr -c -d "$device" -p 2 -l '\EFI\redhat\shimaa64.efi' -L "Red-Hat CoreOS GRUB"
    log "INFO: Created new RHCOS EFI record."
}

install_rhcos() {
    log "INFO: Installing Red Hat CoreOS on $device with ignition file $IGNITION_FILE"

    coreos-installer install "$device" \
        --append-karg "console=hvc0 console=ttyAMA0 earlycon=pl011,0x13010000 ignore_loglevel modprobe.blacklist=mlxbf_pmc $KERNEL_PARAMETERS" \
        --ignition-file "$IGNITION_FILE" \
        --offline

    if [ $? -ne 0 ]; then
        log "ERROR: Failed to install Red Hat CoreOS."
        exit 1
    fi

    sync
}

validate_ignition
update_ignition
install_rhcos
setup_RHCOS_EFI_record
sync

log "INFO: Installation complete, waiting for 10 seconds before signaling"
sleep 10

# Hack to skip a reboot after installation
for i in {1..3}; do
    /usr/local/bin/bfupsignal.sh
    sleep 120
done
