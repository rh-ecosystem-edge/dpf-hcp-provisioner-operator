#!/usr/bin/env bash
set -euo pipefail

umask 077

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 <installation-disk>" >&2
    exit 2
fi

install_disk=$1
mount_dir=
stage_dir=
archive_tmp=
archive_saved=false

cleanup() {
    local status=$?
    trap - EXIT

    if [[ -n "$archive_tmp" ]]; then
        rm -f -- "$archive_tmp" || status=1
    fi

    if [[ "$status" -eq 0 && "$archive_saved" == true ]]; then
        echo "Saved live boot journal to installed RHCOS /var/log/dpu-live-install-journal.tar.gz"
    fi

    exit "$status"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

udevadm settle

# Restrict the search to the installation disk: another disk may also have a
# partition labeled root.
root_partition=$(lsblk --raw --paths --noheadings --output PATH,PARTLABEL "$install_disk" | awk '$2 == "root" { print $1 }')
if [[ -z "$root_partition" || "$root_partition" == *$'\n'* ]]; then
    echo "Expected one root partition on $install_disk" >&2
    exit 1
fi

mount_dir=$(mktemp -d "${TMPDIR:-/tmp}/dpf-installed-rhcos.XXXXXX")
mount -o rw "$root_partition" "$mount_dir"

# OSTree keeps the persistent /var in the stateroot, outside its deployments.
stateroot_var="$mount_dir/ostree/deploy/rhcos/var"
if [[ ! -d "$stateroot_var" ]]; then
    echo "RHCOS OSTree stateroot not found on $root_partition" >&2
    exit 1
fi

log_dir="$stateroot_var/log"
mkdir -p "$log_dir"
# Stage the journal and archive on the live ISO. Only the completed archive is
# copied to the installed system; the live staging directory goes away on reboot.
stage_dir=$(mktemp -d "${TMPDIR:-/tmp}/dpf-live-journal.XXXXXX")
journalctl -b --no-pager --output=short-iso-precise > "$stage_dir/journalctl.log"
if [[ ! -s "$stage_dir/journalctl.log" ]]; then
    echo "The live boot journal is empty" >&2
    exit 1
fi

tar -czf "$stage_dir/dpu-live-install-journal.tar.gz" -C "$stage_dir" journalctl.log
archive_tmp=$(mktemp "$log_dir/.dpu-live-install-journal.XXXXXX")
cp "$stage_dir/dpu-live-install-journal.tar.gz" "$archive_tmp"
chmod 0600 "$archive_tmp"
mv -f "$archive_tmp" "$log_dir/dpu-live-install-journal.tar.gz"
archive_tmp=
archive_saved=true
