"""Build a Finder-friendly disk image and the command-line installer's release index."""

import json
import os
from pathlib import Path
import re
import shutil

from release_macos import digest, run, write_json

ROOT = Path(__file__).resolve().parent.parent


def release_index(component_release, tag, app=None):
    version = component_release["client"]["version"]
    if not re.fullmatch(r"v" + re.escape(version) + r"(?:-preview\.[0-9]+)?", tag):
        raise ValueError("Release tag must match the packaged client version")
    revision = component_release["revision"]
    if not re.fullmatch(r"[0-9a-f]{12,40}", revision):
        raise ValueError("Only clean source builds can have a release index")
    downloads = {}
    for component, prefix in [("all", "Installer"), ("client", "Client"), ("host", "Host")]:
        matches = [item for item in component_release["artifacts"] if item["name"].startswith(f"Plait-{prefix}-")]
        if len(matches) != 1:
            raise ValueError("Expected exactly one package for each component")
        item = matches[0]
        if not re.fullmatch(r"Plait-(Installer|Client|Host)-[0-9a-fA-Z.-]+-macos-arm64\.pkg", item["name"]):
            raise ValueError("Invalid package filename")
        if not re.fullmatch(r"[0-9a-f]{64}", item["sha256"]):
            raise ValueError("Invalid package checksum")
        downloads[component] = dict(item)
    if app is not None:
        expected = f"Plait-{version}-{revision}-macos-arm64.zip"
        if app["name"] != expected or not re.fullmatch(r"[0-9a-f]{64}", app["sha256"]):
            raise ValueError("Invalid app archive metadata")
        downloads["app"] = dict(app)
    build_id = component_release["host"]["build_id"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-[0-9a-f]{20}", build_id):
        raise ValueError("Invalid host build identity")
    return {
        "schema_version": 1, "tag": tag, "revision": revision,
        "signing": "unsigned-preview", "host_build_id": build_id, "downloads": downloads,
    }


def verify_image(source, mount):
    """The mounted image holds exactly what was verified: files byte for byte,
    the app with an intact signature and the same host, and the shortcut."""
    if sorted(path.name for path in source.iterdir()) != sorted(path.name for path in mount.iterdir()):
        raise ValueError("Disk image contents differ from the verified installer")
    for path in source.iterdir():
        mounted = mount / path.name
        if path.is_symlink():
            if not mounted.is_symlink() or os.readlink(mounted) != os.readlink(path):
                raise ValueError(f"Disk image shortcut {path.name} differs")
        elif path.suffix == ".app":
            run("codesign", "--verify", "--deep", "--strict", mounted)
            host = "Contents/Resources/host.json"
            if digest(path / host) != digest(mounted / host):
                raise ValueError("Disk image app carries a different host")
        elif digest(path) != digest(mounted):
            raise ValueError(f"Disk image file {path.name} differs")


def build_distribution(installers, version, revision, work, archive, bundle):
    image_root = work / "disk-image"
    image_root.mkdir()
    # Only the app and an Applications shortcut: one thing to drag. The app sets
    # up its server on first launch; setup notes live in the guide.
    run("ditto", bundle, image_root / "Plait.app")
    (image_root / "Applications").symlink_to("/Applications")
    image = work / f"Plait-{version}-{revision}-macos-arm64.dmg"
    run("hdiutil", "create", "-volname", "Plait", "-srcfolder", image_root,
        "-fs", "HFS+", "-format", "UDZO", image)
    run("hdiutil", "verify", image)
    mount = work / "mounted-image"
    mount.mkdir()
    run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, image)
    try:
        verify_image(image_root, mount)
    finally:
        run("hdiutil", "detach", mount)
    script = work / "install.sh"
    shutil.copy2(ROOT / "scripts/install.sh", script)
    artifacts = [image, script]
    if not revision.endswith("-dirty"):
        component = next(path for path in installers if path.name.startswith("Plait-Release-"))
        tag = os.environ.get("PLAIT_RELEASE_TAG") or f"v{version}-preview.0"
        index = work / "release.json"
        app = {"name": archive.name, "sha256": digest(archive)}
        write_json(index, release_index(json.loads(component.read_text()), tag, app))
        artifacts.append(index)
    return artifacts
