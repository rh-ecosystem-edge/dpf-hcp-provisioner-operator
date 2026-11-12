import base64
import io
import json
import runpy
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT = Path(__file__).parent / 'files' / 'update_ignition.py'
BOOTSTRAP_KUBECONFIG = '/var/lib/dpf/dpuagent/bootstrap-kubeconfig'


class UpdateIgnitionTest(unittest.TestCase):
    def test_replaces_existing_files_without_duplicates(self):
        with tempfile.TemporaryDirectory() as directory:
            ignition_path = Path(directory) / 'user.ign'
            ignition_path.write_text(json.dumps({
                'ignition': {'version': '3.4.0'},
                'storage': {'files': [
                    {'path': '/etc/hostname', 'contents': {'source': 'data:,old'}},
                    {'path': '/etc/dpf/environment', 'contents': {'source': 'data:,old'}},
                    {'path': BOOTSTRAP_KUBECONFIG, 'contents': {'source': 'data:,old'}},
                    {'path': '/etc/unrelated', 'contents': {'source': 'data:,keep'}},
                ]},
            }))

            real_open = open

            def open_file(path, *args, **kwargs):
                content = {
                    '/etc/hostname': 'new-host\n',
                    '/etc/dpf/environment': 'DPUName=new\n',
                    BOOTSTRAP_KUBECONFIG: 'kubeconfig\n',
                }
                if str(path) in content:
                    return io.StringIO(content[str(path)])
                return real_open(path, *args, **kwargs)

            with patch('builtins.open', side_effect=open_file), patch.object(sys, 'argv', [str(SCRIPT), str(ignition_path)]), patch('os.path.exists', return_value=True):
                runpy.run_path(str(SCRIPT))
                runpy.run_path(str(SCRIPT))

            files = json.loads(ignition_path.read_text())['storage']['files']
            by_path = {entry['path']: entry for entry in files}
            self.assertEqual(len(files), 4)
            self.assertEqual(len(by_path), 4)
            self.assertEqual(by_path['/etc/unrelated']['contents']['source'], 'data:,keep')
            self.assertEqual(by_path['/etc/hostname']['contents']['source'], 'data:,new-host')
            self.assertEqual(base64.b64decode(by_path['/etc/dpf/environment']['contents']['source'].split(',')[1]), b'DPUName=new\n')
            self.assertEqual(base64.b64decode(by_path[BOOTSTRAP_KUBECONFIG]['contents']['source'].split(',')[1]), b'kubeconfig')


if __name__ == '__main__':
    unittest.main()
