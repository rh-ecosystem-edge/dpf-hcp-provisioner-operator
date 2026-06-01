#!/usr/bin/env python3

import json
import os
import sys
import urllib.error
import urllib.parse
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
    try:
        with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT) as resp:
            body = resp.read().decode()
            print(f"[{resp.status}] {body}")
            return body
    except urllib.error.HTTPError as e:
        body = e.read().decode()
        print(f"[{e.code}] {body}", file=sys.stderr)
        sys.exit(1)


def base_get(path, params):
    qs = urllib.parse.urlencode(params)
    req = urllib.request.Request(f"{HOSTAGENT_URL}{path}?{qs}", method="GET")
    try:
        with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT) as resp:
            return resp.read().decode()
    except urllib.error.HTTPError as e:
        body = e.read().decode()
        print(f"[{e.code}] {body}", file=sys.stderr)
        sys.exit(1)


def get_dpu():
    body = base_get("/get-object", {
        "group": "provisioning.dpu.nvidia.com",
        "version": "v1alpha1",
        "kind": "DPU",
        "namespace": DPU_NAMESPACE,
        "name": DPU_NAME,
    })
    return json.loads(body)


def get_dpu_phase():
    phase = get_dpu()["status"]["phase"]
    print(phase)
    return phase


def configure_host_vfs(vf_count):
    return base_request("POST", "/configure-host-vfs", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "dpuUID": DPU_UID,
        "vfCount": vf_count,
    })


def update_reboot_method_discovery():
    return base_request("POST", "/update-status", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "dpuUID": DPU_UID,
        "agentStatus": {
            "conditions": [{
                "type": "RebootMethodDiscovery",
                "status": "True",
                "reason": "SystemLevelReset",
                "message": "Reboot method discovered during live RHCOS installation",
                "lastTransitionTime": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            }],
        },
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


def update_nvconfig_applied():
    boot_id = open("/proc/sys/kernel/random/boot_id").read().strip()
    return base_request("POST", "/update-status", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "dpuUID": DPU_UID,
        "agentStatus": {
            "initialBootID": boot_id,
            "conditions": [{
                "type": "NVConfigApplied",
                "status": "True",
                "reason": "AppliedByInstallScript",
                "message": "NVConfig applied during live RHCOS installation",
                "lastTransitionTime": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            }],
        },
    })


def trigger_reboot(reboot_method):
    return base_request("POST", "/trigger-reboot", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "dpuUID": DPU_UID,
        "rebootMethod": reboot_method,
    })


def update_time():
    return base_request("POST", "/update-status", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "dpuUID": DPU_UID,
        "agentStatus": {
            "lastStartupTime": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
        },
    })


def send_error(reason, message):
    return base_request("POST", "/update-status", {
        "dpuName": DPU_NAME,
        "dpuNamespace": DPU_NAMESPACE,
        "dpuUID": DPU_UID,
        "agentStatus": {
            "conditions": [{
                "type": "InstallError",
                "status": "True",
                "reason": reason,
                "message": message[:4096],
                "lastTransitionTime": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            }],
        },
    })


COMMANDS = {
    "get-dpu-phase": get_dpu_phase,
    "configure-host-vfs": lambda: configure_host_vfs(int(sys.argv[2])),
    "update-reboot-method-discovery": update_reboot_method_discovery,
    "update-host-reboot": update_host_reboot,
    "update-nvconfig-applied": update_nvconfig_applied,
    "trigger-reboot": lambda: trigger_reboot(sys.argv[2]),
    "update-time": update_time,
    "send-error": lambda: send_error(sys.argv[2], sys.argv[3]),
}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        print(f"Usage: {sys.argv[0]} <{'|'.join(COMMANDS)}>")
        sys.exit(1)
    COMMANDS[sys.argv[1]]()
