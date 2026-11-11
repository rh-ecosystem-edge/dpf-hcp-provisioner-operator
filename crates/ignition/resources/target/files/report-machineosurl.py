#!/usr/bin/env python3

import json
import subprocess
import sys

DPUAGENT_CLIENT = "/usr/local/bin/dpuagent-client.py"


def log(msg):
    print(msg, flush=True)


def update_condition(status, reason, message):
    try:
        subprocess.run(
            [DPUAGENT_CLIENT, "update-condition", "MachineOSURL", status, reason, message],
            check=False,
        )
    except OSError as e:
        log(f"WARN: failed to run dpuagent-client: {e}")


def booted_image_url():
    result = subprocess.run(
        ["rpm-ostree", "status", "--json"],
        check=True,
        capture_output=True,
        text=True,
    )
    data = json.loads(result.stdout)
    for dep in data.get("deployments", []):
        if dep.get("booted"):
            ref = dep.get("container-image-reference") or ""
            if ":" in ref:
                ref = ref.split(":", 1)[1]
            return ref
    return ""


def main():
    log("INFO: Reading booted container image from rpm-ostree status...")
    try:
        url = booted_image_url()
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as e:
        msg = f"Could not determine MachineOSURL from rpm-ostree status: {e}"
        log(f"ERROR: {msg}")
        update_condition("False", "LookupFailed", msg)
        return 1

    if not url:
        msg = "Could not determine MachineOSURL from rpm-ostree status"
        log(f"ERROR: {msg}")
        update_condition("False", "LookupFailed", msg)
        return 1

    log(f"INFO: Reporting MachineOSURL: {url}")
    update_condition("True", "Reported", url)
    return 0


if __name__ == "__main__":
    sys.exit(main())
