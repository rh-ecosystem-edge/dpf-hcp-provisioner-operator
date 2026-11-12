#!/usr/bin/python3

import base64
import json
import os
import sys

IGNITION_FILE = sys.argv[1]
ENV_FILE = '/etc/dpf/environment'
BOOTSTRAP_KUBECONFIG = '/var/lib/dpf/dpuagent/bootstrap-kubeconfig'

with open(IGNITION_FILE) as f:
    ignition = json.load(f)

with open('/etc/hostname') as f:
    hostname = f.read().strip()

def replace_file(path, mode, source):
    files = ignition['storage']['files']
    files[:] = [entry for entry in files if entry['path'] != path]
    files.append({
        'path': path,
        'overwrite': True,
        'mode': mode,
        'contents': {'source': source},
    })


replace_file('/etc/hostname', 420, 'data:,' + hostname)

with open(ENV_FILE) as f:
    identity = f.read()
identity_b64 = base64.b64encode(identity.encode()).decode()
replace_file(ENV_FILE, 420, 'data:;base64,' + identity_b64)

if os.path.exists(BOOTSTRAP_KUBECONFIG):
    with open(BOOTSTRAP_KUBECONFIG) as f:
        kubeconfig = f.read().strip()
    if kubeconfig:
        kc_b64 = base64.b64encode(kubeconfig.encode()).decode()
        replace_file(BOOTSTRAP_KUBECONFIG, 384, 'data:;base64,' + kc_b64)

with open(IGNITION_FILE, 'w') as f:
    json.dump(ignition, f, indent=4)
