import json
import sys
from unittest.mock import patch
import tempfile
import unittest
from pathlib import Path

from update_manifest import build, build_client, build_host, main


class UpdateManifestTest(unittest.TestCase):
    def release(self, root, revision="1eb61f1259dd"):
        record = {
            "revision": revision,
            "client": {"version": "0.1.0"},
            "host": {"version": "0.1.0", "build_id": "0.1.0-014483b69f94cba19406"},
        }
        (root / f"Plait-Release-{revision}.json").write_text(json.dumps(record))
        (root / f"Plait-0.1.0-{revision}-macos-arm64.zip").write_bytes(b"client")
        (root / f"Plait-Host-0.1.0-{revision}-macos-arm64.tar.gz").write_bytes(b"host!")

    def test_manifest_names_both_components_with_digests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.release(root)
            manifest = build(root, "v0.1.0-preview.3")
            self.assertEqual(manifest["version"], "0.1.0-preview.3")
            self.assertEqual(manifest["host_build_id"], "0.1.0-014483b69f94cba19406")
            client = manifest["artifacts"]["client-macos-arm64"]
            self.assertEqual(client["name"], "Plait-0.1.0-1eb61f1259dd-macos-arm64.zip")
            self.assertEqual(client["size"], 6)
            self.assertEqual(len(client["sha256"]), 64)
            self.assertEqual(manifest["artifacts"]["host-macos-arm64"]["size"], 5)

    def test_tags_must_match_the_build_and_dirty_builds_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.release(root)
            for tag in ["v0.2.0", "0.1.0", "v0.1.0-beta.1", "v0.1.0-preview.0"]:
                with self.assertRaises(ValueError, msg=tag):
                    build(root, tag)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.release(root, "1eb61f1259dd-dirty")
            with self.assertRaises(ValueError):
                build(root, "v0.1.0")

    def test_component_shapes_and_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.release(root)
            client = build_client(root, "v0.1.0-preview.3")
            host = build_host(root, "v0.1.0-preview.3")
            common = {"schema_version": 2, "version": "0.1.0-preview.3",
                      "tag": "v0.1.0-preview.3", "revision": "1eb61f1259dd",
                      "protocol": {"min": 1, "max": 6}}
            legacy = build(root, "v0.1.0-preview.3")
            self.assertEqual(client, {**common, "component": "client", "artifacts": {
                "macos-arm64": legacy["artifacts"]["client-macos-arm64"]}})
            self.assertEqual(host, {**common, "component": "host", "build_id": legacy["host_build_id"],
                "capabilities": [], "storage_schema": 1, "artifacts": {
                "macos-arm64": legacy["artifacts"]["host-macos-arm64"]}})
            for builder in (build_client, build_host):
                with self.assertRaises(ValueError):
                    builder(root, "v0.2.0")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.release(root, "123-dirty")
            for builder in (build_client, build_host):
                with self.assertRaises(ValueError):
                    builder(root, "v0.1.0")

    def test_single_component_directories(self):
        for missing, expected in (("Plait-Host-*", "client"), ("Plait-0*", "host")):
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.release(root)
                next(root.glob(missing)).unlink()
                with patch.object(sys, "argv", ["update_manifest.py", directory, "v0.1.0"]):
                    main()
                self.assertTrue((root / f"update-{expected}.json").is_file())
                self.assertFalse((root / "update.json").exists())
                other = "host" if expected == "client" else "client"
                self.assertFalse((root / f"update-{other}.json").exists())


if __name__ == "__main__":
    unittest.main()
