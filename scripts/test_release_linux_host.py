import json
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch

from package_linux_host import archive_name, build_host, check_elf, node_config, release_version
from rust_license_report import fallback_notices, generate_reports


class LinuxHostTest(unittest.TestCase):
    def test_build_stamp(self):
        self.assertEqual(release_version("v0.1.0-preview.58"), "0.1.0-preview.58")
        self.assertEqual(release_version("v1.2.3"), "1.2.3")
        for tag in ("v1.2.3-preview.0", "1.2.3", "v1.2.3-extra"):
            with self.assertRaises(ValueError):
                release_version(tag)
        with patch("package_linux_host.run") as run, patch.dict(
                "os.environ", {"PLAIT_RELEASE_VERSION": "stale"}):
            self.assertIsNone(build_host("123456789abc", None))
            env = run.call_args.kwargs["env"]
            self.assertEqual(env["PLAIT_BUILD_REVISION"], "123456789abc")
            self.assertNotIn("PLAIT_RELEASE_VERSION", env)
            self.assertEqual(build_host("123456789abc", "v0.1.0-preview.58"),
                             "0.1.0-preview.58")
            self.assertEqual(run.call_args.kwargs["env"]["PLAIT_RELEASE_VERSION"],
                             "0.1.0-preview.58")

    def test_license_report_uses_each_linux_target(self):
        root = Path(__file__).resolve().parent.parent
        for arch in ("aarch64", "x86_64"):
            triple = arch + "-unknown-linux-gnu"
            with self.subTest(arch=arch), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                with patch("rust_license_report.subprocess.run") as run:
                    run.return_value.returncode = 0
                    run.return_value.stdout = "Used by: example 1.0.0\nLicense texts"
                    run.return_value.stderr = ""
                    with patch("rust_license_report.rust_inventory", return_value="packages\nexample 1.0.0"):
                        generate_reports(root, triple, ["plait-host"], output)
                command = run.call_args.args[0]
                config = command[command.index("--config") + 1]
                self.assertEqual(tomllib.loads(config.read_text())["targets"], [triple])
                self.assertIn(root / "crates/plait-host/Cargo.toml", command)
                self.assertEqual((output / "host-rust-licenses.txt").read_text(), run.return_value.stdout)
                self.assertIn("--fail", command)

    def test_missing_inventory_notice_aborts_generation(self):
        root = Path(__file__).resolve().parent.parent
        with tempfile.TemporaryDirectory() as directory, patch("rust_license_report.subprocess.run") as run:
            run.return_value.returncode = 0
            run.return_value.stdout = "Other license texts"
            with patch("rust_license_report.rust_inventory", return_value="packages\nmissing 1"):
                with self.assertRaisesRegex(ValueError, "Missing Rust license report entries for missing 1"):
                    generate_reports(root, "aarch64-unknown-linux-gnu", ["plait-host"], Path(directory))
            self.assertFalse((Path(directory) / "host-rust-licenses.txt").exists())

    def test_generic_license_needs_a_verified_notice(self):
        root = Path(__file__).resolve().parent.parent
        message = "unable to find text for license 'MIT' for crate '{}', falling back to canonical text"
        crate = "lyon_geom 1.0.20"
        self.assertIn("Nicolas Silva", fallback_notices(root, message.format(crate), {crate})[0])
        with self.assertRaisesRegex(ValueError, "unknown 1.0"):
            fallback_notices(root, message.format("unknown 1.0"), {"unknown 1.0"})

    def test_only_workspace_member_versions_use_the_project_license(self):
        root = Path(__file__).resolve().parent.parent
        message = "unable to find text for license 'MIT' for crate '{}', falling back to canonical text"
        metadata = {"workspace_members": ["member"], "packages": [
            {"id": "member", "name": "example", "version": "2.0.0"},
            {"id": "registry", "name": "example", "version": "1.0.0"},
        ]}
        with patch("rust_license_report.subprocess.check_output", return_value=json.dumps(metadata)):
            crate = "example 2.0.0"
            self.assertIn((root / "LICENSE").read_text(), fallback_notices(root, message.format(crate), {crate})[0])
            with self.assertRaisesRegex(ValueError, "example 1.0.0"):
                fallback_notices(root, message.format("example 1.0.0"), {"example 1.0.0"})

    def test_node_config(self):
        path = Path(__file__).resolve().parent.parent / "packaging/linux/node.json"
        config, item = node_config(path, "x86_64")
        self.assertEqual(item["archive"], f"node-v{config['version']}-linux-x64.tar.xz")
        with tempfile.TemporaryDirectory() as directory:
            config["source"] = "https://example.com/"
            bad = Path(directory) / "node.json"
            bad.write_text(json.dumps(config))
            with self.assertRaises(ValueError):
                node_config(bad, "arm64")

    def test_name_and_elf(self):
        self.assertEqual(archive_name("0.1.0", "123456789abc", "arm64"),
                         "Plait-Host-0.1.0-123456789abc-linux-arm64.tar.gz")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "host"
            header = bytearray(20)
            header[:6] = b"\x7fELF\x02\x01"
            header[18:20] = (62).to_bytes(2, "little")
            path.write_bytes(header)
            check_elf(path, "x86_64")
            with self.assertRaises(ValueError):
                check_elf(path, "arm64")
            path.write_bytes(b"bad")
            with self.assertRaises(ValueError):
                check_elf(path, "x86_64")


if __name__ == "__main__":
    unittest.main()
