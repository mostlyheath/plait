import functools
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parent.parent
BUILD = '1.2.3-' + 'a' * 20
REVISION = 'b' * 40
TAG = 'v1.2.3-preview.1'


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


class HostInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / 'home'
        self.home.mkdir()
        self.assets = self.root / 'assets'
        self.assets.mkdir()
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        (self.bin / 'uname').write_text('#!/bin/sh\n[ "$1" = -s ] && echo Linux || echo "${TEST_ARCH:-x86_64}"\n')
        (self.bin / 'loginctl').write_text('#!/bin/sh\necho Linger=no\n')
        for path in self.bin.iterdir():
            path.chmod(0o755)
        source = self.root / 'source' / BUILD
        (source / 'bin').mkdir(parents=True)
        host = source / 'bin/plait-host'
        host.write_text('#!/bin/sh\nprintf "%s\\n" "$@" >> "$HOME/calls"\n')
        host.chmod(0o755)
        (source / 'bin/node').write_text('node')
        (source / 'runtime').mkdir()
        (source / 'component.json').write_text('{}')
        self.name = f'Plait-Host-1.2.3-{REVISION[:12]}-linux-x86_64.tar.gz'
        self.release = self.assets / 'download' / TAG
        self.release.mkdir(parents=True)
        with tarfile.open(self.release / self.name, 'w:gz') as archive:
            archive.add(source, arcname=BUILD)
        self.digest = hashlib.sha256((self.release / self.name).read_bytes()).hexdigest()
        self.manifest = {'schema_version': 2, 'component': 'host', 'tag': TAG, 'build_id': BUILD,
                         'artifacts': {'linux-x86_64': {'name': self.name, 'sha256': self.digest}}}
        self.write_manifest()
        (self.assets / 'releases').write_text(json.dumps([{'tag_name': TAG}]))
        handler = functools.partial(QuietHandler, directory=str(self.assets))
        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        address = f'http://127.0.0.1:{self.server.server_port}'
        script = (ROOT / 'scripts/install.sh').read_text()
        script = script.replace('https://api.github.com/repos/$repository', address)
        script = script.replace('https://github.com/$repository/releases/download', address + '/download')
        # The fixture server is plain HTTP on loopback; the real script refuses anything but HTTPS.
        script = script.replace("--proto '=https' --proto-redir '=https' ", '')
        self.script = self.root / 'install.sh'
        self.script.write_text(script)
        self.env = {**os.environ, 'HOME': str(self.home), 'USER': 'test',
                    'PATH': str(self.bin) + ':/usr/bin:/bin'}

    def write_manifest(self):
        (self.release / 'update-host-linux-x86_64.json').write_text(json.dumps(self.manifest))

    def run_install(self):
        return subprocess.run(['bash', str(self.script), '--host'], env=self.env,
                              text=True, capture_output=True)

    def test_success_and_repeat_keep_existing_release(self):
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        installed = self.home / '.local/share/plait-host/releases' / BUILD
        self.assertEqual((self.home / '.local/bin/plait-host').resolve(), installed / 'bin/plait-host')
        self.assertIn('publisher signature', result.stdout)
        self.assertIn('enable-linger', result.stdout)
        self.assertIn('not on PATH', result.stdout)
        (installed / 'marker').write_text('kept')
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((installed / 'marker').read_text(), 'kept')
        self.assertEqual((self.home / 'calls').read_text().splitlines(), ['install-service'] * 2)

    def test_checksum_mismatch_preserves_existing_install(self):
        old = self.home / '.local/share/plait-host/releases/old'
        old.mkdir(parents=True)
        (old / 'marker').write_text('untouched')
        self.manifest['artifacts']['linux-x86_64']['sha256'] = '0' * 64
        self.write_manifest()
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('checksum', result.stderr)
        self.assertEqual((old / 'marker').read_text(), 'untouched')
        self.assertFalse((old.parent / BUILD).exists())
        self.assertFalse((self.home / '.local/bin/plait-host').exists())

    def test_another_installed_build_is_left_to_the_updater(self):
        old = self.home / '.local/share/plait-host/releases' / ('1.2.2-' + 'c' * 20) / 'bin'
        old.mkdir(parents=True)
        (self.home / '.local/bin').mkdir(parents=True)
        (self.home / '.local/bin/plait-host').symlink_to(old / 'plait-host')
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('plait-host update', result.stderr)
        self.assertFalse((old.parent.parent / BUILD).exists())

    def test_unsupported_architecture(self):
        self.env['TEST_ARCH'] = 'riscv64'
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('x86_64 and arm64', result.stderr)
        self.assertFalse((self.home / '.local').exists())


if __name__ == '__main__':
    unittest.main()
