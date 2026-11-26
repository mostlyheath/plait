import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
BUILD = '1.2.3-' + 'a' * 20
REVISION = 'b' * 40
TAG = 'v1.2.3-preview.1'


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
        (self.bin / 'systemctl').write_text(
            '#!/bin/sh\necho "$*" >> "$HOME/systemctl-calls"\n'
            '[ "$2" = is-active ] && [ -n "${TEST_EXECSTART:-}" ] && exit 0\n'
            '[ "$2" = show ] && { echo "{ path=$TEST_EXECSTART ; }"; exit 0; }\n'
            '[ "$2" = is-active ] && exit 3\nexit 0\n')
        (self.bin / 'curl').write_text(
            '#!/bin/sh\nwhile [ "$#" -gt 0 ]; do\n'
            '  case "$1" in https://*) url=$1 ;; --output) shift; output=$1 ;; esac\n'
            '  shift\ndone\n'
            'case "$url" in\n'
            '  https://api.github.com/repos/mostlyheath/plait/*) asset=${url#*/mostlyheath/plait/} ;;\n'
            '  https://github.com/mostlyheath/plait/releases/download/*) asset=${url#*/plait/releases/} ;;\n'
            '  *) exit 1 ;;\nesac\ncp "$TEST_ASSETS/$asset" "$output"\n')
        for path in self.bin.iterdir():
            path.chmod(0o755)
        source = self.root / 'source' / BUILD
        (source / 'bin').mkdir(parents=True)
        host = source / 'bin/plait-host'
        host.write_text('#!/bin/sh\nprintf "%s\\n" "$@" >> "$HOME/calls"\n'
                        '[ "$1" = active-runs ] && echo "${TEST_ACTIVE_RUNS:-0}"\nexit 0\n')
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
        (self.assets / 'releases?per_page=1').write_text(json.dumps([{'tag_name': TAG}]))
        self.script = ROOT / "scripts/install.sh"
        self.env = {**os.environ, 'HOME': str(self.home), 'USER': 'test',
                    'PATH': str(self.bin) + ':/usr/bin:/bin', 'TEST_ASSETS': str(self.assets)}

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

    def source_link(self):
        source = self.root / 'checkout/target/release/plait-host'
        source.parent.mkdir(parents=True)
        source.write_text('source')
        link = self.home / '.local/bin/plait-host'
        link.parent.mkdir(parents=True)
        link.symlink_to(source)
        return link, source

    def test_source_migration_restarts_services(self):
        link, source = self.source_link()
        self.env['TEST_EXECSTART'] = str(source)
        tailnet = self.home / '.config/systemd/user/plait-host-tailnet.service'
        tailnet.parent.mkdir(parents=True)
        tailnet.write_text('unit')
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(link.resolve(), self.home / '.local/share/plait-host/releases' / BUILD / 'bin/plait-host')
        calls = (self.home / 'systemctl-calls').read_text()
        self.assertIn('--user restart plait-host.service', calls)
        self.assertIn('--user restart plait-host-tailnet.service', calls)
        self.assertIn('data in ~/.local/share/plait was kept', result.stdout)

    def test_source_migration_refuses_active_runs(self):
        link, source = self.source_link()
        self.env['TEST_ACTIVE_RUNS'] = '2'
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('2 active runs', result.stderr)
        self.assertEqual(link.resolve(), source)
        self.assertNotIn('restart', (self.home / 'systemctl-calls').read_text())

    def test_running_source_service_without_link_is_migrated(self):
        self.env['TEST_EXECSTART'] = str(self.root / 'checkout/target/release/plait-host')
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--user restart plait-host.service', (self.home / 'systemctl-calls').read_text())

    def test_unsupported_architecture(self):
        self.env['TEST_ARCH'] = 'riscv64'
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('x86_64 and arm64', result.stderr)
        self.assertFalse((self.home / '.local').exists())


if __name__ == '__main__':
    unittest.main()
