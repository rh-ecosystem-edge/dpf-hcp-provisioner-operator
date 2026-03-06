#!/bin/bash
set -e

pcie_dev_list=$(lspci -d 15b3: | grep ConnectX | awk '{print $1}')

for dev in ${pcie_dev_list}; do
  echo "set NVConfig on dev ${dev}"
  mstconfig -d ${dev} -y set $@
done
echo "Finished setting nvconfig parameters"
