#!/usr/bin/env python3
"""Write the update manifest that installed Plait apps and hosts check.

Usage: update_manifest.py <release-dir> <tag>

The manifest names each artifact's size and SHA-256. The release workflow
signs it with the update key (crates/plait-core/examples/update-key.rs);
clients verify that signature before trusting any field.
"""
import hashlib
import json
import re
import sys
from pathlib import Path

PLATFORM = "macos-arm64"
# Keep these aligned with host PROTOCOL_VERSION in crates/plait-core/src/host.rs
# and runtime/src/protocol.ts when the protocol or storage schema changes.
COMPATIBILITY = {"protocol": {"min": 1, "max": 6}, "storage_schema": 1}


def digest(path):
    hasher = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1 << 16), b""):
            hasher.update(block)
    return hasher.hexdigest()


def artifact(path):
    return {"name": path.name, "sha256": digest(path), "size": path.stat().st_size}


def build(release, tag):
    records = list(release.glob("Plait-Release-*.json"))
    if len(records) != 1:
        raise ValueError("Expected exactly one component release record")
    record = json.loads(records[0].read_text())
    version, revision = record["client"]["version"], record["revision"]
    if revision.endswith("-dirty"):
        raise ValueError("Releases must be built from a clean checkout")
    if not re.fullmatch(r"v" + re.escape(version) + r"(?:-preview\.[1-9][0-9]*)?", tag):
        raise ValueError("Release tag must match the packaged client version")
    if record["host"]["version"] != version:
        raise ValueError("Client and host versions differ")
    client = release / f"Plait-{version}-{revision}-{PLATFORM}.zip"
    host = release / f"Plait-Host-{version}-{revision}-{PLATFORM}.tar.gz"
    return {
        "schema_version": 1,
        "version": tag[1:],
        "tag": tag,
        "revision": revision,
        "host_build_id": record["host"]["build_id"],
        "artifacts": {f"client-{PLATFORM}": artifact(client), f"host-{PLATFORM}": artifact(host)},
    }


def build_component(release, tag, component):
    records = list(release.glob("Plait-Release-*.json"))
    if len(records) != 1:
        raise ValueError("Expected exactly one component release record")
    record = json.loads(records[0].read_text())
    version, revision = record[component]["version"], record["revision"]
    if revision.endswith("-dirty"):
        raise ValueError("Releases must be built from a clean checkout")
    if not re.fullmatch(r"v" + re.escape(version) + r"(?:-preview\.[1-9][0-9]*)?", tag):
        raise ValueError("Release tag must match the packaged component version")
    prefix, suffix = ("Plait", "zip") if component == "client" else ("Plait-Host", "tar.gz")
    path = release / f"{prefix}-{version}-{revision}-{PLATFORM}.{suffix}"
    manifest = {
        "schema_version": 2, "component": component, "version": tag[1:],
        "tag": tag, "revision": revision, "protocol": COMPATIBILITY["protocol"].copy(),
        "artifacts": {PLATFORM: artifact(path)},
    }
    if component == "host":
        manifest.update(build_id=record["host"]["build_id"], capabilities=[],
                        storage_schema=COMPATIBILITY["storage_schema"])
    return manifest


def build_client(release, tag):
    return build_component(release, tag, "client")


def build_host(release, tag):
    return build_component(release, tag, "host")


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    release = Path(sys.argv[1])
    tag = sys.argv[2]
    # Validate the record before deciding which component artifacts are present.
    records = list(release.glob("Plait-Release-*.json"))
    if len(records) != 1:
        raise ValueError("Expected exactly one component release record")
    record = json.loads(records[0].read_text())
    revision = record["revision"]
    client = release / f"Plait-{record['client']['version']}-{revision}-{PLATFORM}.zip"
    host = release / f"Plait-Host-{record['host']['version']}-{revision}-{PLATFORM}.tar.gz"
    manifests = {}
    if client.is_file():
        manifests["update-client.json"] = build_client(release, tag)
    if host.is_file():
        manifests["update-host.json"] = build_host(release, tag)
    if client.is_file() and host.is_file():
        manifests["update.json"] = build(release, tag)
    if not manifests:
        raise ValueError("No component artifacts found")
    for name, manifest in manifests.items():
        (release / name).write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
