#!/bin/bash
# adapted from mellanox bfscripts

/usr/local/bin/bflog.sh "Linux up"
/usr/local/bin/bflog.sh "DPU is ready"

os_up_path="/sys/devices/platform/MLNXBF04:00/driver/os_up"
if [ ! -e "${os_up_path}" ]; then
  os_up_path="/sys/devices/platform/MLNXBF04:00/os_up"
fi

[ ! -e "${os_up_path}" ] && exit

echo 1 >"${os_up_path}"
