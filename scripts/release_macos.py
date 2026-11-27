"""Build independent client/host packages without installing them on the build Mac."""

import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import subprocess
import tarfile
import urllib.request
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
HOST_BASE = Path("/Library/Application Support/Plait/Host")
AGENT_PATH = Path("/Library/LaunchAgents/dev.plait.host.plist")
AGENT_LABEL = "dev.plait.host"


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def manifest(component, version, revision):
    if component not in ("client", "host") or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Invalid component or version")
    if not re.fullmatch(r"[0-9a-f]{12,40}(-dirty)?", revision):
        raise ValueError("Invalid source revision")
    return {
        "schema_version": 1, "component": component, "version": version,
        "revision": revision,
        # Newest protocol spoken; both components still accept version 1.
        "protocol_version": 7,
        "update_policy": "manual, component-specific installer",
    }


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def normalize_modes(root):
    # Installed code is shared, but only the system administrator may change it.
    root.chmod(0o755)
    for path in root.rglob("*"):
        if not path.is_symlink():
            executable = path.is_dir() or bool(path.stat().st_mode & 0o111)
            path.chmod(0o755 if executable else 0o644)


def payload_digest(root):
    result = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        result.update(relative.encode() + b"\0")
        if path.is_symlink():
            # npm's relative bin links are valid; links outside the package are not.
            if path.readlink().is_absolute() or not path.exists() or not path.resolve().is_relative_to(root.resolve()):
                raise ValueError(f"Package symlink escapes its root or has no target: {relative}")
            result.update(b"link\0" + os.fsencode(path.readlink()) + b"\0")
        elif path.is_file():
            result.update(b"file\0" + f"{path.stat().st_mode & 0o777:o}\0".encode())
            result.update(digest(path).encode() + b"\0")
        elif path.is_dir():
            result.update(b"directory\0")
        else:
            raise ValueError(f"Special file in package: {relative}")
    return result.hexdigest()


def launch_agent(installed):
    return {
        "Label": AGENT_LABEL,
        "ProgramArguments": [str(installed / "bin/plait-host"), "run-user"],
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
        "ThrottleInterval": 10,
        # This service answers desktop requests, not discretionary background work.
        "ProcessType": "Standard",
        "EnvironmentVariables": {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin"},
    }


def platform_preflight():
    return '''if [[ ${3:-/} != / ]]; then
    echo 'Plait installs only on the current startup volume.' >&2
    exit 1
fi
version=$(/usr/bin/sw_vers -productVersion)
if [[ $(/usr/bin/uname -m) != arm64 || ${version%%.*} -lt 15 ]]; then
    echo 'Plait requires an Apple Silicon Mac with macOS 15 or later.' >&2
    exit 1
fi
'''


def host_preinstall(installed):
    return f'''#!/bin/bash
set -euo pipefail
{platform_preflight()}# Even an identical reinstall must not rewrite a live executable or sidecar file.
if [[ -e {shlex.quote(str(installed))} ]]; then
    echo 'This exact host build is already installed. Use the client-only package or a newer host build.' >&2
    exit 1
fi
'''


def host_postinstall(installed):
    executable = shlex.quote(str(installed / "bin/plait-host"))
    return f'''#!/bin/bash
set -euo pipefail
user=$(/usr/bin/stat -f %Su /dev/console)
uid=$(/usr/bin/stat -f %u /dev/console)
if [[ "$uid" == 0 || "$user" == loginwindow || "$user" == _mbsetupuser ]]; then
    echo 'Plait host will start at the next graphical login.'
    exit 0
fi
# An update must not cancel a run or replace a live sidecar's source files.
if /bin/launchctl print "gui/$uid/{AGENT_LABEL}" >/dev/null 2>&1; then
    echo 'Host update installed. Existing host is unchanged. Finish work, then log out and back in to activate.'
elif /usr/bin/sudo -H -u "$user" {executable} status >/dev/null 2>&1; then
    echo 'An existing unmanaged host is running. It was not stopped. Finish work and stop it before activating the installed host.'
elif ! /bin/launchctl asuser "$uid" /usr/bin/sudo -H -u "$user" /bin/launchctl bootstrap "gui/$uid" {shlex.quote(str(AGENT_PATH))}; then
    echo 'Host files installed. Log out and back in to start the user agent.'
fi
exit 0
'''


def client_preinstall():
    return '#!/bin/bash\nset -euo pipefail\n' + platform_preflight() + '''# Closing the client leaves the independent host and accepted work running.
if /usr/bin/pgrep -x '[Pp]lait' >/dev/null; then
    echo 'Quit the Plait client, then run this client installer again. Do not stop the host.' >&2
    exit 1
fi
'''


def verify_macho(binary):
    if subprocess.check_output(["lipo", "-archs", str(binary)], text=True).strip() != "arm64":
        raise ValueError(f"Expected arm64 executable: {binary}")
    for line in subprocess.check_output(["otool", "-L", str(binary)], text=True).splitlines()[1:]:
        dependency = line.strip().split(" (", 1)[0]
        if not dependency.startswith(("/System/Library/", "/usr/lib/")):
            raise ValueError(f"Unbundled dynamic dependency: {dependency}")
    run("codesign", "--verify", "--strict", binary)


def stage_font_notices(resources):
    destination = resources / "fonts"
    destination.mkdir()
    for name in ("OFL.txt", "README.md"):
        shutil.copy2(ROOT / "assets/fonts" / name, destination / name)


def stage_node(destination):
    config = json.loads((ROOT / "packaging/macos/node.json").read_text())
    archive_name = f"node-v{config['version']}-darwin-arm64.tar.xz"
    if config["archive"] != archive_name or config["source"] != f"https://nodejs.org/dist/v{config['version']}/":
        raise ValueError("Unexpected Node source")
    cache = ROOT / "target/release-inputs"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / archive_name
    if not archive.exists():
        temporary = archive.with_suffix(".download")
        try:
            with urllib.request.urlopen(config["source"] + archive_name, timeout=60) as response, temporary.open("wb") as output:
                shutil.copyfileobj(response, output)
            if digest(temporary) != config["sha256"]:
                raise ValueError("Downloaded Node checksum does not match the pinned release")
            temporary.replace(archive)
        finally:
            temporary.unlink(missing_ok=True)
    if digest(archive) != config["sha256"]:
        raise ValueError("Cached Node checksum does not match the pinned release")
    prefix = archive_name.removesuffix(".tar.xz")
    with tarfile.open(archive, "r:xz") as tar:
        for source, target in [("bin/node", "bin/node"), ("LICENSE", "NODE_LICENSE.txt")]:
            member = tar.getmember(f"{prefix}/{source}")
            if not member.isfile():
                raise ValueError("Expected regular Node distribution file")
            with tar.extractfile(member) as content, (destination / target).open("wb") as output:
                shutil.copyfileobj(content, output)
    (destination / "bin/node").chmod(0o755)
    verify_macho(destination / "bin/node")
    version = subprocess.check_output([str(destination / "bin/node"), "--version"], text=True).strip()
    if version != "v" + config["version"]:
        raise ValueError("Packaged Node reported the wrong version")
    return config["version"]


def copy_runtime_sources(runtime):
    """Keep runner imports adjacent to the packaged runtime entry point."""
    shutil.copytree(ROOT / "runtime/src", runtime / "src")


def stage_host(work, version, revision):
    host = work / "host-tree"
    (host / "bin").mkdir(parents=True)
    shutil.copy2(ROOT / "target/release/plait-host", host / "bin/plait-host")
    run("codesign", "--force", "--sign", "-", "--timestamp=none", host / "bin/plait-host")
    verify_macho(host / "bin/plait-host")
    node_version = stage_node(host)
    runtime = host / "runtime"
    copy_runtime_sources(runtime)
    for name in ["package.json", "package-lock.json"]:
        shutil.copy2(ROOT / "runtime" / name, runtime / name)
    environment = {**os.environ, "NPM_CONFIG_USERCONFIG": "/dev/null"}
    run("npm", "ci", "--omit=dev", "--ignore-scripts", cwd=runtime, env=environment)
    shutil.copy2(ROOT / "LICENSE", host / "LICENSE")
    shutil.copy2(ROOT / "Cargo.lock", host / "Cargo.lock")
    shutil.copy2(ROOT / "target/HOST_THIRD_PARTY_NOTICES.txt", host / "THIRD_PARTY_NOTICES.txt")
    normalize_modes(host)
    metadata = manifest("host", version, revision)
    runtime_package = json.loads((runtime / "package.json").read_text())
    metadata.update(release=os.environ.get("PLAIT_RELEASE_VERSION"), node_version=node_version, runtime_sdk_version=runtime_package["dependencies"]["@earendil-works/pi-coding-agent"], payload_sha256=payload_digest(host))
    metadata["build_id"] = f"{version}-{metadata['payload_sha256'][:20]}"
    write_json(host / "component.json", metadata)
    payload = work / "host-root"
    installed = HOST_BASE / "releases" / metadata["build_id"]
    target = payload / installed.relative_to("/")
    target.parent.mkdir(parents=True)
    shutil.move(host, target)
    write_json(payload / HOST_BASE.relative_to("/") / "installed.json", metadata)
    agent = payload / AGENT_PATH.relative_to("/")
    agent.parent.mkdir(parents=True, exist_ok=True)
    agent.write_bytes(plistlib.dumps(launch_agent(installed)))
    normalize_modes(payload)
    run("plutil", "-lint", agent)
    scripts = work / "host-scripts"
    scripts.mkdir()
    for name, content in [("preinstall", host_preinstall(installed)), ("postinstall", host_postinstall(installed))]:
        (scripts / name).write_text(content)
        (scripts / name).chmod(0o755)
    return payload, scripts, metadata


def embed_host(resources, payload, metadata):
    """Carry the host inside the app, so one app update also delivers its host.

    A data archive, not nested code: the app's signature covers it as a resource.
    """
    release = payload / HOST_BASE.relative_to("/") / "releases" / metadata["build_id"]
    with tarfile.open(resources / "Host.tar.gz", "w:gz", compresslevel=6) as archive:
        archive.add(release, arcname=metadata["build_id"])
    write_json(resources / "host.json", metadata)


def host_package_id(metadata):
    build_id = metadata["build_id"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-[0-9a-f]{20}", build_id):
        raise ValueError("Invalid immutable host build ID")
    return f"dev.plait.host.release.{build_id}"


def component_package(root, scripts, component, version, output, work):
    normalize_modes(root)
    identifier = "dev.plait.client"
    if component == "host":
        manifests = component_manifests(root)
        if len(manifests) != 1:
            raise ValueError("Expected one immutable host release")
        identifier = host_package_id(json.loads(manifests[0].read_text()))
    # A stable host receipt makes PackageKit delete the previous release on upgrade.
    args = ["pkgbuild", "--root", root, "--scripts", scripts, "--ownership", "recommended",
            "--identifier", identifier, "--version", version, "--install-location", "/"]
    if component == "client":
        components = work / "client-components.plist"
        components.write_bytes(plistlib.dumps([{
            "RootRelativeBundlePath": "Applications/Plait.app",
            "BundleIsRelocatable": False,
            "BundleIsVersionChecked": False,
            "BundleHasStrictIdentifier": True,
            "BundleOverwriteAction": "upgrade",
        }]))
        args.extend(["--component-plist", components])
    else:
        # Runtime dependencies must not be relocated through automatic bundle discovery.
        components = work / "host-components.plist"
        components.write_bytes(plistlib.dumps([]))
        args.extend(["--component-plist", components])
    run(*args, output)


def distribution(client_name, host_name, host_identifier):
    root = ET.Element("installer-gui-script", {"minSpecVersion": "1"})
    ET.SubElement(root, "title").text = "Plait — Client and Host"
    ET.SubElement(root, "options", {"customize": "always", "hostArchitectures": "arm64"})
    ET.SubElement(root, "welcome", {"file": "Welcome.html"})
    ET.SubElement(root, "conclusion", {"file": "Conclusion.html"})
    ET.SubElement(root, "domains", {"enable_localSystem": "true", "enable_currentUserHome": "false", "enable_anywhere": "false"})
    volume = ET.SubElement(root, "volume-check")
    allowed = ET.SubElement(volume, "allowed-os-versions")
    ET.SubElement(allowed, "os-version", {"min": "15.0"})
    outline = ET.SubElement(root, "choices-outline")
    for component, name, identifier in [("client", client_name, "dev.plait.client"), ("host", host_name, host_identifier)]:
        ET.SubElement(outline, "line", {"choice": component})
        choice = ET.SubElement(root, "choice", {"id": component, "title": f"Plait {component.title()}", "start_selected": "true"})
        ET.SubElement(choice, "pkg-ref", {"id": identifier})
        ET.SubElement(root, "pkg-ref", {"id": identifier}).text = name
    return ET.tostring(root, encoding="utf-8", xml_declaration=True)


def component_manifests(expanded):
    # npm packages may have unrelated component.json files of their own.
    return sorted(expanded.glob("**/Applications/Plait.app/Contents/Resources/component.json")) + sorted(
        expanded.glob("**/Library/Application Support/Plait/Host/releases/*/component.json")
    )


def build_installers(bundle, client_version, host_version, revision, work, staged=None):
    payload = work / "client-root"
    (payload / "Applications").mkdir(parents=True)
    shutil.copytree(bundle, payload / "Applications/Plait.app", symlinks=True)
    scripts = work / "client-scripts"
    scripts.mkdir()
    (scripts / "preinstall").write_text(client_preinstall())
    (scripts / "preinstall").chmod(0o755)
    client = work / f"Plait-Client-{client_version}-{revision}-macos-arm64.pkg"
    component_package(payload, scripts, "client", client_version, client, work)
    host_payload, host_scripts, metadata = staged or stage_host(work, host_version, revision)
    host = work / f"Plait-Host-{host_version}-{revision}-macos-arm64.pkg"
    component_package(host_payload, host_scripts, "host", host_version, host, work)
    resources = ROOT / "packaging/macos/installer"
    description = work / "Distribution.xml"
    description.write_bytes(distribution(client.name, host.name, host_package_id(metadata)))
    combined = work / f"Plait-Installer-{client_version}-{revision}-macos-arm64.pkg"
    run("productbuild", "--distribution", description, "--resources", resources, "--package-path", work, combined)
    for component in [client, host, combined]:
        expanded = work / (component.stem + "-expanded")
        run("pkgutil", "--expand-full", component, expanded)
        manifests = component_manifests(expanded)
        expected = {"client"} if component == client else {"host"} if component == host else {"client", "host"}
        if len(manifests) != len(expected) or {json.loads(path.read_text())["component"] for path in manifests} != expected:
            raise ValueError(f"Installer component manifest set is incorrect: {component.name}")
        for path in manifests:
            item = json.loads(path.read_text())
            if item["component"] == "host":
                tag = os.environ.get("PLAIT_RELEASE_TAG")
                if item["release"] != (tag[1:] if tag else None):
                    raise ValueError(f"Packaged host release does not match build tag: {component.name}")
                verify_macho(path.parent / "bin/plait-host")
                verify_macho(path.parent / "bin/node")
            else:
                run("codesign", "--verify", "--deep", "--strict", path.parent.parent.parent)
    release = work / f"Plait-Release-{revision}.json"
    write_json(release, {
        "schema_version": 1, "revision": revision,
        "client": manifest("client", client_version, revision), "host": metadata,
        "artifacts": [{"name": path.name, "sha256": digest(path)} for path in [client, host, combined]],
        "signing": "ad-hoc executables; unsigned installers; not notarized",
    })
    return [client, host, combined, release]
