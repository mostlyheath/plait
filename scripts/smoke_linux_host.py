#!/usr/bin/env python3
"""Exercise an archived Linux host with its bundled runtime."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tarfile
import tempfile
import time

from release_macos import digest


def request(store, operation):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(10)
        client.connect(str(store / "host.sock"))
        client.sendall((json.dumps({"op": operation}) + "\n").encode())
        with client.makefile("rb") as stream:
            reply = json.loads(stream.readline())
    if not reply.get("ok"):
        raise ValueError(f"Host {operation} failed: {reply}")
    return reply["result"]


def main(archive):
    checksum = archive.with_name(archive.name + ".sha256").read_text().strip()
    if checksum != f"{digest(archive)}  {archive.name}":
        raise ValueError("Archive checksum mismatch")
    with tempfile.TemporaryDirectory(prefix="plait-", dir="/tmp") as directory:
        root = Path(directory)
        with tarfile.open(archive, "r:gz") as tar:
            members = tar.getmembers()
            names = {member.name.split("/")[0] for member in members}
            if len(names) != 1:
                raise ValueError("Expected one archive root")
            tar.extractall(root, filter="data")
        build_id = names.pop()
        host = root / build_id
        metadata = json.loads((host / "component.json").read_text())
        if metadata["build_id"] != build_id:
            raise ValueError("Archive root does not match build ID")
        binary = host / "bin/plait-host"
        env = {**os.environ, "PATH": str(host / "bin")}
        version = json.loads(subprocess.check_output([str(binary), "version"], env=env, text=True))
        if version["component"] != "host" or version["version"] != metadata["version"] or version["revision"] != metadata["revision"]:
            raise ValueError("Host version does not match component metadata")
        if metadata["release"] != version["release"]:
            raise ValueError("Archived host release does not match the binary")
        store = root / "store"
        process = subprocess.Popen([str(binary), "run", "--store", str(store)], env=env)
        try:
            deadline = time.monotonic() + 30
            while not (store / "host.sock").exists():
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("Host failed to start")
                time.sleep(0.1)
            request(store, "status")
            info = request(store, "host_info")
            if info["release"] != version["release"] or info["build_id"] != build_id:
                raise ValueError("Running host identity does not match the archive")
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    print(f"ok: {Path(archive).name} answered status with only its own Node on PATH")

if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    main(Path(sys.argv[1]).resolve())
