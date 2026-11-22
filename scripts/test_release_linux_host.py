import json
from pathlib import Path
import tempfile
import unittest

from package_linux_host import archive_name, check_elf, node_config


class LinuxHostTest(unittest.TestCase):
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
