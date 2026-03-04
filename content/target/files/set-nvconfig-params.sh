#!/bin/bash
set -e
for dev in /dev/mst/*; do
  echo "set NVConfig on dev ${dev}"
  mlxconfig -d ${dev} -y set $@
done
echo "Finished setting nvconfig parameters"
