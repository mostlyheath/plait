import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from package_linux_host import archive_name, build_host, check_elf, node_config, package, release_version


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

    def test_archive_records_the_release_selected_for_the_build(self):
        source = Path(__file__).resolve().parent.parent
        revision = "123456789abc"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target/release").mkdir(parents=True)
            (root / "target/release/plait-host").touch()
            for name in ("crates", "runtime", "LICENSE", "Cargo.lock"):
                (root / name).symlink_to(source / name)
            for tag, expected in ((None, None), ("v0.1.0-preview.58", "0.1.0-preview.58")):
                with self.subTest(tag=tag):
                    out = root / (tag or "untagged")
                    reported = json.dumps({"revision": revision, "release": expected})
                    with patch("package_linux_host.ROOT", root), patch("package_linux_host.run"), \
                            patch("package_linux_host.check_elf"), patch("package_linux_host.stage_node", return_value="22"), \
                            patch("package_linux_host.subprocess.check_output", side_effect=[revision, "", reported]), \
                            patch.dict("os.environ", {"PLAIT_RELEASE_VERSION": "stale"}):
                        package("x86_64", out, None, False, tag)
                    archive, = out.glob("*.tar.gz")
                    with tarfile.open(archive) as tar:
                        component, = (item for item in tar if Path(item.name).name == "component.json")
                        self.assertEqual(json.load(tar.extractfile(component))["release"], expected)

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
