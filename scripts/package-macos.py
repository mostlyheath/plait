#!/usr/bin/env python3
"""Package and verify the development build. Run through build-macos.sh."""

import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tempfile

from release_macos import build_installers, embed_host, manifest, stage_font_notices, stage_host, write_json
from distribution_macos import build_distribution

ROOT = Path(__file__).resolve().parent.parent
BUNDLE_ID = "dev.plait.desktop"


def verify_bundle(bundle):
    with (bundle / "Contents/Info.plist").open("rb") as file:
        info = plistlib.load(file)
    assert info["CFBundleIdentifier"] == BUNDLE_ID
    assert info["CFBundleExecutable"] == "plait"
    binary = bundle / "Contents/MacOS/plait"
    assert os.access(binary, os.X_OK), "Bundle executable is not executable"
    assert subprocess.check_output(["lipo", "-archs", str(binary)], text=True).strip() == "arm64"
    links = subprocess.check_output(["otool", "-L", str(binary)], text=True).splitlines()[1:]
    for line in links:
        dependency = line.strip().split(" (", 1)[0]
        assert dependency.startswith(("/System/Library/", "/usr/lib/")), f"Unbundled dependency: {dependency}"
    assert (bundle / "Contents/Resources/LICENSE").read_text().startswith("MIT License")
    for name in ("OFL.txt", "README.md"):
        assert (bundle / "Contents/Resources/fonts" / name).read_bytes() == (ROOT / "assets/fonts" / name).read_bytes()
    host = json.loads((bundle / "Contents/Resources/host.json").read_text())
    assert host["component"] == "host" and (bundle / "Contents/Resources/Host.tar.gz").stat().st_size > 0
    assert info["CFBundleIconFile"] == "Plait.icns"
    assert (bundle / "Contents/Resources/Plait.icns").read_bytes()[:4] == b"icns"
    notices = (bundle / "Contents/Resources/THIRD_PARTY_NOTICES.txt").read_text()
    assert "gpui" in notices and "Apache License" in notices, "Dependency notices are missing"
    subprocess.run(["plutil", "-lint", str(bundle / "Contents/Info.plist")], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(bundle)], check=True)


def main():
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("Packaging requires an Apple Silicon Mac.")
    metadata = json.loads((ROOT / "target/macos-package-metadata.json").read_text())
    version = next(package["version"] for package in metadata["packages"] if package["name"] == "plait")
    host_version = next(package["version"] for package in metadata["packages"] if package["name"] == "plait-host")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise SystemExit("The bundle needs a numeric major.minor.patch Cargo version.")
    revision = subprocess.check_output(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT, text=True).strip()
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT):
        revision += "-dirty"
    dist = ROOT / "dist"
    if dist.is_symlink() or (dist / "Plait.app").is_symlink():
        raise SystemExit("Refusing to replace a symlinked package output.")
    dist.mkdir(exist_ok=True)
    archive_name = f"Plait-{version}-{revision}-macos-arm64.zip"
    with tempfile.TemporaryDirectory(prefix="plait-package-", dir=ROOT / "target") as work:
        work = Path(work)
        bundle = work / "Plait.app"
        contents = bundle / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        resources = contents / "Resources"
        resources.mkdir()
        shutil.copy2(ROOT / "target/release/plait", contents / "MacOS/plait")
        shutil.copy2(ROOT / "LICENSE", resources / "LICENSE")
        shutil.copy2(ROOT / "Cargo.lock", resources / "Cargo.lock")
        shutil.copy2(ROOT / "scripts/update-installed-macos.sh", resources / "install-user.sh")
        shutil.copy2(ROOT / "target/THIRD_PARTY_NOTICES.txt", resources / "THIRD_PARTY_NOTICES.txt")
        stage_font_notices(resources)
        iconset = work / "Plait.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = str(size * scale)
                suffix = "@2x" if scale == 2 else ""
                subprocess.run([
                    "sips", "-z", pixels, pixels, str(ROOT / "assets/branding/app-icon/plait-app-icon-1024.png"),
                    "--out", str(iconset / f"icon_{size}x{size}{suffix}.png"),
                ], check=True, stdout=subprocess.DEVNULL)
        subprocess.run(["iconutil", "-c", "icns", "-o", str(resources / "Plait.icns"), str(iconset)], check=True)
        info = {
            "CFBundleIdentifier": BUNDLE_ID,
            "CFBundleName": "Plait",
            "CFBundleDisplayName": "Plait",
            "CFBundleExecutable": "plait",
            "CFBundleIconFile": "Plait.icns",
            "CFBundlePackageType": "APPL",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "LSMinimumSystemVersion": "15.0",
            "NSHighResolutionCapable": True,
            "NSPrincipalClass": "NSApplication",
            # Opening a file or folder in Rex sends it an Apple event. Without
            # this sentence macOS refuses the event instead of asking.
            "NSAppleEventsUsageDescription": (
                "Plait opens files and folders in the editor and terminal "
                "you chose in Settings."
            ),
            "PlaitRevision": revision,
            "PlaitBuildChannel": "development",
        }
        write_json(resources / "component.json", manifest("client", version, revision))
        staged = stage_host(work, host_version, revision)
        embed_host(resources, staged[0], staged[2])
        with (contents / "Info.plist").open("wb") as file:
            plistlib.dump(info, file)
        # Ad-hoc signing permits local use; it is not a notarized Developer ID release.
        subprocess.run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(bundle)], check=True)
        verify_bundle(bundle)
        archive = work / archive_name
        subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(bundle), str(archive)], check=True)
        unpacked = work / "unpacked"
        subprocess.run(["ditto", "-x", "-k", str(archive), str(unpacked)], check=True)
        verify_bundle(unpacked / "Plait.app")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        installers = build_installers(bundle, version, host_version, revision, work, staged)
        # The same host archive, alone, for `plait-host update` on machines without the app.
        host_archive = work / f"Plait-Host-{host_version}-{revision}-macos-arm64.tar.gz"
        shutil.copy2(resources / "Host.tar.gz", host_archive)
        installers.append(host_archive)
        installers += build_distribution(installers, version, revision, work, archive, bundle)
        # Only replace generated output after both the bundle and its archive pass verification.
        destination = dist / "Plait.app"
        if destination.exists():
            shutil.rmtree(destination)
        shutil.move(str(bundle), destination)
        os.replace(archive, dist / archive_name)
        (dist / f"{archive_name}.sha256").write_text(f"{digest}  {archive_name}\n")
        for artifact in installers:
            checksum = hashlib.sha256(artifact.read_bytes()).hexdigest()
            name = artifact.name
            os.replace(artifact, dist / name)
            (dist / f"{name}.sha256").write_text(f"{checksum}  {name}\n")
            print(f"Installer/release artifact: {dist / name}")
    print(f"Development app: {dist / 'Plait.app'}")
    print(f"Archive: {dist / archive_name}")
    print("Ad-hoc app/host; unsigned installers. Not notarized. Use component packages for manual independent updates.")


if __name__ == "__main__":
    main()
