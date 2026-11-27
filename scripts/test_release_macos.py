import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

from release_macos import (
    ROOT, copy_runtime_sources, AGENT_LABEL, HOST_BASE, client_preinstall, component_manifests, distribution,
    host_package_id, host_postinstall, host_preinstall, launch_agent, manifest, normalize_modes, payload_digest, platform_preflight, stage_font_notices, stage_host,
)


class ReleaseTests(unittest.TestCase):
    def test_staged_host_release_uses_only_the_build_tag(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target/release").mkdir(parents=True)
            (root / "target/release/plait-host").touch()
            (root / "target/HOST_THIRD_PARTY_NOTICES.txt").touch()
            for name in ("runtime", "LICENSE", "Cargo.lock"):
                (root / name).symlink_to(ROOT / name)
            # Capture the environment at the first build command on either platform.
            shell = """
                uname() { if [[ $1 == -s ]]; then echo Darwin; else echo arm64; fi; }
                cargo() { printenv PLAIT_RELEASE_VERSION || true; exit 0; }
                source "$0"
            """
            for tag, expected in (("", None), ("v0.1.0-preview.83", "0.1.0-preview.83"), ("v0.1.0", "0.1.0")):
                with self.subTest(tag=tag):
                    output = subprocess.check_output(
                        ["bash", "-c", shell, str(ROOT / "scripts/build-macos.sh")],
                        env=dict(os.environ, PLAIT_RELEASE_TAG=tag, PLAIT_RELEASE_VERSION="stale"),
                        text=True)
                    build_env = {"PLAIT_RELEASE_VERSION": output.strip()} if output else {}
                    with patch.dict(os.environ, build_env, clear=True), patch("release_macos.ROOT", root), \
                            patch("release_macos.run"), patch("release_macos.verify_macho"), \
                            patch("release_macos.host_notices", return_value="notices"), \
                            patch("release_macos.stage_node", return_value="22"):
                        payload, _, _ = stage_host(root / (tag or "untagged"), "0.1.0", "a" * 12)
                    component, = component_manifests(payload)
                    self.assertEqual(json.loads(component.read_text())["release"], expected)

    def test_client_stages_complete_bundled_font_notices(self):
        with tempfile.TemporaryDirectory() as directory:
            resources = Path(directory)
            stage_font_notices(resources)
            for name in ("OFL.txt", "README.md"):
                self.assertEqual((ROOT / "assets/fonts" / name).read_bytes(),
                                 (resources / "fonts" / name).read_bytes())

    def test_runtime_package_contains_every_runner_source(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory) / "runtime"
            copy_runtime_sources(runtime)
            source = ROOT / "runtime/src/runner"
            expected = {path.relative_to(source) for path in source.rglob("*") if path.is_file()}
            self.assertTrue({Path("main.ts"), Path("pi-runner.ts")} <= expected)
            self.assertEqual(expected, {path.relative_to(runtime / "src/runner") for path in (runtime / "src/runner").rglob("*") if path.is_file()})
            self.assertFalse((runtime / "test").exists())

    def test_component_contracts_are_separate_and_inputs_are_bounded(self):
        for component in ["client", "host"]:
            value = manifest(component, "0.1.0", "a" * 12)
            self.assertEqual(value["component"], component)
            self.assertEqual(value["protocol_version"], 7)
        for component, version, revision in [
            ("other", "0.1.0", "a" * 12),
            ("host", "../escape", "a" * 12),
            ("host", "0.1.0", "x; rm -rf anything"),
        ]:
            with self.assertRaises(ValueError):
                manifest(component, version, revision)

    def test_payload_identity_ignores_timestamps_but_includes_content_and_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            file = root / "program"
            file.write_text("one")
            first = payload_digest(root)
            file.touch()
            self.assertEqual(first, payload_digest(root))
            file.write_text("two")
            self.assertNotEqual(first, payload_digest(root))
            second = payload_digest(root)
            file.chmod(0o755)
            self.assertNotEqual(second, payload_digest(root))

    def test_package_links_cannot_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target").write_text("safe")
            (root / "link").symlink_to("target")
            payload_digest(root)
            (root / "link").unlink()
            (root / "link").symlink_to("/etc/passwd")
            with self.assertRaisesRegex(ValueError, "escapes"):
                payload_digest(root)

    def test_user_agent_pins_an_immutable_release_without_a_store_override(self):
        installed = HOST_BASE / "releases/0.1.0-aabbccdd"
        agent = plistlib.loads(plistlib.dumps(launch_agent(installed)))
        self.assertEqual(agent["Label"], AGENT_LABEL)
        self.assertEqual(agent["ProgramArguments"], [str(installed / "bin/plait-host"), "run-user"])
        self.assertNotIn("UserName", agent)
        self.assertNotIn("PLAIT_HOST_STORE", agent["EnvironmentVariables"])
        self.assertEqual(agent["KeepAlive"], {"SuccessfulExit": False})
        self.assertEqual(agent["ProcessType"], "Standard")

    def test_host_updates_never_force_restart_and_client_package_does_not_touch_host(self):
        script = host_postinstall(HOST_BASE / "releases/0.1.0-aabb")
        for forbidden in ["killall", "pkill", "bootout", "kickstart", "rm -rf"]:
            self.assertNotIn(forbidden, script)
        self.assertIn("launchctl print", script)
        self.assertIn("launchctl bootstrap", script)
        self.assertNotIn("launchctl", client_preinstall())

    def test_combined_installer_contains_independently_selectable_components(self):
        host_id = host_package_id({"build_id": "0.1.0-" + "a" * 20})
        document = ET.fromstring(distribution("client.pkg", "host.pkg", host_id))
        choices = document.findall("choice")
        self.assertEqual([choice.get("id") for choice in choices], ["client", "host"])
        self.assertEqual([choice.get("start_selected") for choice in choices], ["true", "true"])
        self.assertEqual([ref.text for ref in document.findall("pkg-ref")], ["client.pkg", "host.pkg"])
        self.assertEqual([ref.get("id") for ref in document.findall("pkg-ref")], ["dev.plait.client", host_id])
        self.assertEqual(document.find("volume-check/allowed-os-versions/os-version").get("min"), "15.0")

    def test_existing_host_release_is_rejected_without_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            installed = Path(directory) / "release with spaces"
            script = host_preinstall(installed).replace('/usr/bin/sw_vers -productVersion', 'printf 15.0').replace('/usr/bin/uname -m', 'printf arm64')
            self.assertEqual(subprocess.run(["bash"], input=script, text=True, capture_output=True).returncode, 0)
            installed.mkdir()
            marker = installed / "marker"
            marker.write_text("active release")
            refused = subprocess.run(["bash"], input=script, text=True, capture_output=True)
            self.assertEqual(refused.returncode, 1)
            self.assertIn("already installed", refused.stderr)
            self.assertEqual(marker.read_text(), "active release")

    def test_preflight_rejects_wrong_platform_and_non_startup_volumes(self):
        for architecture, version, volume, expected in [
            ('arm64', '15.0', '/', 0), ('x86_64', '15.0', '/', 1),
            ('arm64', '14.0', '/', 1), ('arm64', '15.0', '/Volumes/Other', 1),
        ]:
            script = platform_preflight().replace('/usr/bin/sw_vers -productVersion', f'printf {version}').replace('/usr/bin/uname -m', f'printf {architecture}')
            result = subprocess.run(['bash', '-s', '--', 'package', '/', volume], input=script, text=True, capture_output=True)
            self.assertEqual(result.returncode, expected)

    def test_package_modes_do_not_allow_group_or_other_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = root / "data"
            code = root / "program"
            data.write_text("data")
            code.write_text("code")
            data.chmod(0o666)
            code.chmod(0o777)
            normalize_modes(root)
            self.assertEqual(data.stat().st_mode & 0o777, 0o644)
            self.assertEqual(code.stat().st_mode & 0o777, 0o755)
            self.assertEqual(root.stat().st_mode & 0o777, 0o755)

    def test_installer_manifests_exclude_npm_component_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            client = root / 'client.pkg/Payload/Applications/Plait.app/Contents/Resources/component.json'
            host = root / 'host.pkg/Payload/Library/Application Support/Plait/Host/releases/version/component.json'
            unrelated = host.parent / 'runtime/node_modules/extend/component.json'
            for path in [client, host, unrelated]:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('{}')
            self.assertEqual(component_manifests(root), [client, host])

    def test_immutable_host_releases_have_distinct_package_receipts(self):
        first = host_package_id({'build_id': '0.1.0-' + 'a' * 20})
        second = host_package_id({'build_id': '0.1.0-' + 'b' * 20})
        self.assertNotEqual(first, second)
        self.assertEqual(first, host_package_id({'build_id': '0.1.0-' + 'a' * 20}))
        with self.assertRaises(ValueError):
            host_package_id({'build_id': '../escape'})

    def test_node_download_is_pinned(self):
        path = Path(__file__).resolve().parent.parent / "packaging/macos/node.json"
        node = json.loads(path.read_text())
        self.assertEqual(node["archive"], f"node-v{node['version']}-darwin-arm64.tar.xz")
        self.assertRegex(node["sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(node["source"], f"https://nodejs.org/dist/v{node['version']}/")


if __name__ == "__main__":
    unittest.main()
