#!/bin/bash
# adapted from mellanox bfscripts

rshlog_path="/sys/devices/platform/MLNXBF04:00/driver/rsh_log"
if [ ! -e "${rshlog_path}" ]; then
  rshlog_path="/sys/devices/platform/MLNXBF04:00/rsh_log"
fi

[ ! -e "${rshlog_path}" ] && exit

echo "$*" >"${rshlog_path}"
