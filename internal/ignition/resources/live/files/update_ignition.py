#!/usr/bin/python3

import base64
import json
import sys

IGNITION_FILE = sys.argv[1]

ENV_FILE = '/etc/dpf/environment'

ignition = json.load(open(IGNITION_FILE))

hostname: str = open('/etc/hostname').read().strip()

ignition['storage']['files'].append({
    'path': '/etc/hostname',
    'overwrite': True,
    'mode': 420,
    'contents': {
        'source': 'data:,' + hostname
    }
})

identity = open(ENV_FILE).read()
identity_b64 = base64.b64encode(identity.encode()).decode()
ignition['storage']['files'].append({
    'path': ENV_FILE,
    'overwrite': True,
    'mode': 420,
    'contents': {
        'source': 'data:;base64,' + identity_b64
    }
})

with open(IGNITION_FILE, 'w') as f:
    json.dump(ignition, f, indent=4)
