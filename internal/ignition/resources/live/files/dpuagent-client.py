#!/usr/bin/env python3

import json
import os
import sys
import urllib.request
from datetime import datetime, timezone

HOSTAGENT_IPV6 = "fe80::1"
HOSTAGENT_PORT = 11029
HOSTAGENT_IFACE = "tmfifo_net0"
HOSTAGENT_URL = f"http://[{HOSTAGENT_IPV6}%25{HOSTAGENT_IFACE}]:{HOSTAGENT_PORT}"
REQUEST_TIMEOUT = 60


DPU_NAME = os.getenv("DPUName")
DPU_NAMESPACE = os.getenv("DPUNamespace")
DPU_UID = os.getenv("DPUUID")


def base_request(method, path, payload):
    data = json.dumps(payload).encode()
    req = urllib.request.Request(
        f"{HOSTAGENT_URL}{path}",
        data=data,
        headers={"Content-Type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT) as resp:
        return resp.read()


def configure_host_vfs():
    return base_request("POST", "/configure-host-vfs", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
    })


def update_host_reboot():
    return base_request("POST", "/update-status", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "dpuUID": DPU_UID,
        "agentStatus": {
            "rebootMethod": "SystemLevelReset"
        },
    })


def update_time():
    return base_request("POST", "/update-status", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "agentStatus": {
            "lastStartupTime": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
        },
    })


COMMANDS = {
    "configure-host-vfs": configure_host_vfs,
    "update-host-reboot": update_host_reboot,
    "update-time": update_time,
}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        print(f"Usage: {sys.argv[0]} <{'|'.join(COMMANDS)}>")
        sys.exit(1)
    result = COMMANDS[sys.argv[1]]()
    if result:
        print(result.decode())
