#!/bin/bash
set -ex
CMD=$1
PF_TOTAL_SF=$2
PF_TRUSTED_SF=$3

case $CMD in
    setup) ;;
    *)
    echo "invalid first argument. ./configure-sfs.sh {setup}"
    exit 1
    ;;
esac

set_GUID_for_SF() {
    json_output=$(mlnx-sf -a show -j)

    # Iterate over each key in the JSON result
    echo "$json_output" | jq -c 'to_entries[]' | while read -r entry; do
        key=$(echo "$entry" | jq -r '.key')
        sf_netdev=$(echo "$entry" | jq -r '.value.sf_netdev')
        aux_dev=$(echo "$entry" | jq -r '.value.aux_dev')

        # Read the MAC address from the system file
        mac_address=$(cat /sys/class/net/"$sf_netdev"/address)

        # Update the MAC address using mlxdevm
        /opt/mellanox/iproute2/sbin/mlxdevm port function set "$key" hw_addr "$mac_address"

        # Unbind and bind the auxiliary device
        echo "$aux_dev" > /sys/bus/auxiliary/devices/"$aux_dev"/driver/unbind
        echo "$aux_dev" > /sys/bus/auxiliary/drivers/mlx5_core.sf/bind
    done
}

if [ "$CMD" = "setup" ]; then
    # Create SF on P0 for SFC
    # System SF(index 0) has been removed, so DPF will create SF from index 0
    for i in $(seq 0 $((PF_TOTAL_SF - 1 - $PF_TRUSTED_SF))); do
        # Create SFs with random mac, kernel will allocate random MAC for SF netdev
        /sbin/mlnx-sf --action create --device 0000:03:00.0 --sfnum ${i} || true
    done

    for i in $(seq 101 $((100 + $PF_TRUSTED_SF))); do
        /sbin/mlnx-sf --action create --device 0000:03:00.0 --sfnum ${i} -t || true
    done
    
    set_GUID_for_SF
fi
