#!/usr/bin/python3

import base64
import json
import os
import sys

IGNITION_FILE = sys.argv[1]
ENV_FILE = '/etc/dpf/environment'
BOOTSTRAP_KUBECONFIG = '/var/lib/dpf/dpuagent/bootstrap-kubeconfig'
BFB_REGISTRY_FILE = '/etc/dpf/bfb-registry'

ignition = json.load(open(IGNITION_FILE))


def add_file(path, mode, source):
    files = ignition.setdefault('storage', {}).setdefault('files', [])
    if any(f.get('path') == path for f in files):
        return
    files.append({
        'path': path,
        'overwrite': True,
        'mode': mode,
        'contents': {
            'source': source
        }
    })


hostname: str = open('/etc/hostname').read().strip()
add_file('/etc/hostname', 420, 'data:,' + hostname)

identity = open(ENV_FILE).read()
identity_b64 = base64.b64encode(identity.encode()).decode()
add_file(ENV_FILE, 420, 'data:;base64,' + identity_b64)

if os.path.exists(BOOTSTRAP_KUBECONFIG):
    kubeconfig = open(BOOTSTRAP_KUBECONFIG).read().strip()
    if kubeconfig:
        kc_b64 = base64.b64encode(kubeconfig.encode()).decode()
        add_file(BOOTSTRAP_KUBECONFIG, 384, 'data:;base64,' + kc_b64)

if os.path.exists(BFB_REGISTRY_FILE):
    bfb_registry = open(BFB_REGISTRY_FILE).read()
    if bfb_registry.strip():
        bfb_registry_b64 = base64.b64encode(bfb_registry.encode()).decode()
        ignition['storage']['files'].append({
            'path': BFB_REGISTRY_FILE,
            'overwrite': True,
            'mode': 420,
            'contents': {
                'source': 'data:;base64,' + bfb_registry_b64
            }
        })

with open(IGNITION_FILE, 'w') as f:
    json.dump(ignition, f, indent=4)
