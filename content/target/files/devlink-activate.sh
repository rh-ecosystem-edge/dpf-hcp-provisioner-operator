#!/bin/bash

pcie_dev_list=$(lspci -Dd 15b3: | grep ConnectX | awk '{print $1}')

for dev in ${pcie_dev_list}; do
  echo "activate devlink on dev ${dev}"
  devlink dev eswitch set pci/${dev} mode switchdev
done
echo "Finished activating devlink"
