#!/usr/bin/env python3
"""Package a built Linux host and its pinned runtime."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.request

from release_macos import ROOT, copy_runtime_sources, digest, manifest, normalize_modes, payload_digest, run, write_json


def node_config(path, arch):
    config = json.loads(path.read_text())
    version = config["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Invalid Node version")
    item = config["archives"][arch]
    suffix = {"x86_64": "x64", "arm64": "arm64"}[arch]
    if (config["source"] != f"https://nodejs.org/dist/v{version}/"
            or item["archive"] != f"node-v{version}-linux-{suffix}.tar.xz"
            or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])):
        raise ValueError("Unexpected Node source, archive, or checksum")
    return config, item


def archive_name(version, revision, arch):
    manifest("host", version, revision)
    if arch not in ("x86_64", "arm64"):
        raise ValueError("Invalid Linux architecture")
    return f"Plait-Host-{version}-{revision}-linux-{arch}.tar.gz"


def check_elf(path, arch):
    with path.open("rb") as file:
        header = file.read(20)
    machine = {"x86_64": 62, "arm64": 183}[arch]
    if (len(header) < 20 or header[:6] != b"\x7fELF\x02\x01"
            or int.from_bytes(header[18:20], "little") != machine):
        raise ValueError(f"Expected a 64-bit little-endian Linux {arch} ELF: {path}")


def stage_node(host, arch, supplied):
    config, item = node_config(ROOT / "packaging/linux/node.json", arch)
    archive = supplied
    if archive is None:
        archive = host.parent / item["archive"]
        with urllib.request.urlopen(config["source"] + item["archive"], timeout=60) as response, archive.open("wb") as output:
            shutil.copyfileobj(response, output)
    if digest(archive) != item["sha256"]:
        raise ValueError("Node archive checksum does not match the pinned release")
    prefix = item["archive"].removesuffix(".tar.xz")
    with tarfile.open(archive, "r:xz") as tar:
        for source, target in [("bin/node", "bin/node"), ("LICENSE", "NODE_LICENSE.txt")]:
            member = tar.getmember(f"{prefix}/{source}")
            if not member.isfile():
                raise ValueError("Expected regular Node distribution file")
            with tar.extractfile(member) as content, (host / target).open("wb") as output:
                shutil.copyfileobj(content, output)
    node = host / "bin/node"
    node.chmod(0o755)
    check_elf(node, arch)
    if subprocess.check_output([str(node), "--version"], text=True).strip() != "v" + config["version"]:
        raise ValueError("Packaged Node reported the wrong version")
    return config["version"]


def release_version(tag):
    if not tag:
        return None
    match = re.fullmatch(r"v([0-9]+\.[0-9]+\.[0-9]+(?:-preview\.[1-9][0-9]*)?)", tag)
    if not match:
        raise ValueError(f"Unrecognized release tag: {tag}")
    return match.group(1)


def build_host(revision, tag):
    version = release_version(tag)
    env = os.environ.copy()
    env["PLAIT_BUILD_REVISION"] = revision
    env.pop("PLAIT_RELEASE_VERSION", None)
    if version is not None:
        env["PLAIT_RELEASE_VERSION"] = version
    run("cargo", "build", "--release", "--locked", "-p", "plait-host", cwd=ROOT, env=env)
    return version


def package(arch, out, node_archive, allow_dirty, tag=None):
    revision = subprocess.check_output(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    if dirty:
        if not allow_dirty:
            raise ValueError("Refusing to package a dirty checkout; use --allow-dirty for testing")
        revision += "-dirty"
    with (ROOT / "crates/plait-host/Cargo.toml").open("rb") as file:
        version = tomllib.load(file)["package"]["version"]
    name = archive_name(version, revision, arch)
    release = build_host(revision, tag)
    binary = ROOT / "target/release/plait-host"
    check_elf(binary, arch)
    reported = json.loads(subprocess.check_output([str(binary), "version"], text=True))
    if reported.get("revision") != revision or reported.get("release") != release:
        raise ValueError("Host build version does not match packaging revision and release")
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as directory:
        host = Path(directory) / "host"
        (host / "bin").mkdir(parents=True)
        shutil.copy2(binary, host / "bin/plait-host")
        node_version = stage_node(host, arch, node_archive)
        runtime = host / "runtime"
        copy_runtime_sources(runtime)
        for filename in ("package.json", "package-lock.json"):
            shutil.copy2(ROOT / "runtime" / filename, runtime / filename)
        env = {**os.environ, "PATH": str(host / "bin") + os.pathsep + os.environ.get("PATH", ""), "NPM_CONFIG_USERCONFIG": "/dev/null"}
        run("npm", "ci", "--omit=dev", "--ignore-scripts", cwd=runtime, env=env)
        shutil.copy2(ROOT / "LICENSE", host / "LICENSE")
        shutil.copy2(ROOT / "Cargo.lock", host / "Cargo.lock")
        normalize_modes(host)
        metadata = manifest("host", version, revision)
        package_json = json.loads((runtime / "package.json").read_text())
        metadata.update(release=release, platform=f"linux-{arch}", node_version=node_version,
                        runtime_sdk_version=package_json["dependencies"]["@earendil-works/pi-coding-agent"],
                        payload_sha256=payload_digest(host))
        metadata["build_id"] = f"{version}-{metadata['payload_sha256'][:20]}"
        write_json(host / "component.json", metadata)
        target = out / name
        with tarfile.open(target, "w:gz", compresslevel=6) as tar:
            tar.add(host, arcname=metadata["build_id"])
        (out / (name + ".sha256")).write_text(f"{digest(target)}  {name}\n")
    print(target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", required=True, choices=("x86_64", "arm64"))
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--node-archive", type=Path)
    parser.add_argument("--allow-dirty", action="store_true")
    parser.add_argument("--release-tag", help="release tag to embed in the host")
    args = parser.parse_args()
    package(args.arch, args.out, args.node_archive, args.allow_dirty, args.release_tag)


if __name__ == "__main__":
    main()
