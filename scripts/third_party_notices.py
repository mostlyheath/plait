"""Combine per-target Cargo inventories and license reports with the staged host's licenses."""

import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

from runtime_payload import binary_inventory, is_javascript

ROOT = Path(__file__).resolve().parent.parent


def rust_inventory(root, component, triple):
    tree = subprocess.check_output(
        ["cargo", "tree", "--locked", "-p", component, "--target", triple,
         "--edges", "normal,no-proc-macro", "--prefix", "none", "--format", "{p}"], cwd=root, text=True)
    inventory = set()
    for line in tree.splitlines():
        name, version, *_ = line.split()
        inventory.add(f"{name} {version.removeprefix('v')}")
    return "\n".join([f"Rust packages in {component}", *sorted(inventory)])


def rust_notices(root, component, triple="aarch64-apple-darwin", report_dir=None):
    report = "host-rust-licenses.txt" if component == "plait-host" else "client-rust-licenses.txt"
    directory = report_dir or root / "target"
    return rust_inventory(root, component, triple) + "\n\n" + read_package_file(directory, directory / report)


def read_package_file(package, path):
    if not path.resolve().is_relative_to(package.resolve()):
        raise ValueError(f"License file outside package: {path}")
    text = path.read_text(encoding="utf-8")
    if not text.strip():
        raise ValueError(f"Empty package file: {path}")
    return text


def fallback_notice(package, manifest, license_id):
    """Some npm tarballs declare a license but omit the upstream license file."""
    if (manifest["name"].startswith("@earendil-works/") and manifest["version"] == "1.5.2"
            and license_id == "MIT"):
        # From https://github.com/earendil-works/pi/blob/v1.5.2/LICENSE
        return read_package_file(ROOT, ROOT / "packaging/macos/LICENSE-pi")
    if (manifest["name"], manifest["version"]) == ("data-uri-to-buffer", "4.0.1"):
        readme = package / "README.md"
        match = re.search(r"(?im)^#{0,6} *licen[cs]e *$.*", read_package_file(package, readme), re.DOTALL)
        if match and all(term in match.group() for term in ("Copyright (c)", "Permission is hereby granted", "THE SOFTWARE IS PROVIDED")):
            return match.group()
    if (manifest["name"], manifest["version"], license_id) == ("proxy-agent-negotiate", "1.1.0", "MIT"):
        # TooTallNate/proxy-agents f633521 extracted both proxy agents' implementation.
        # src/index.ts is unchanged at release 065d1ffc. Both source packages carry
        # the same notice; this copy is from packages/http-proxy-agent/LICENSE there.
        return read_package_file(ROOT, ROOT / "packaging/macos/LICENSE-proxy-agent-negotiate")
    if (manifest["name"], manifest["version"], license_id) == ("standardwebhooks", "1.1.1", "MIT"):
        # npm gitHead, standard-webhooks/standard-webhooks @ b4d2c14, libraries/LICENSE
        return read_package_file(ROOT, ROOT / "packaging/macos/LICENSE-standardwebhooks")
    if license_id == "Apache-2.0" and (manifest["name"], manifest["version"]) in {
        ("@aws-sdk/credential-provider-http", "3.972.86"),
        ("@aws-sdk/credential-provider-login", "3.972.92"),
        ("@aws-sdk/nested-clients", "3.998.3"),
    }:
        return read_package_file(package.parent, package.parent / "core/LICENSE")
    raise ValueError(f"No license text found for {manifest['name']} ({license_id})")


def package_notices(package):
    for parent, directories, files in os.walk(package):
        directories[:] = sorted(name for name in directories if name != "node_modules")
        for name in sorted(files):
            path = Path(parent) / name
            if (name.lower().startswith(("license", "licence", "copying", "notice", "copyright"))
                    or any(part.lower() in {"third_party_notices", "licenses", "licences"}
                           for part in path.relative_to(package).parts[:-1])
                    or name.lower().replace("-", "_").startswith("third_party_notice")):
                yield path


# An empty owner identifies a notice vendored with the release tools.
COMPONENT_LICENSES = {
    "Photon dependency notices": ("", "LICENSE-photon-dependencies"),
    # image-rs/color_quant @ 6c302d2, LICENSE and src/lib.rs
    "color_quant and NeuQuant": ("", "LICENSE-color_quant"),
    # quickjs-wasi 3.6.2 pins quickjs-ng @ 6d46d07d04041b40f4f49eaa7fdebe44c314c699.
    "QuickJS-NG": ("", "LICENSE-quickjs-ng"),
    "WASI libraries": ("", "LICENSE-wasi-libraries"),
    "memchr": ("", "LICENSE-milo-memchr"),
    # Undici 8.12.3 deps/llhttp/include/llhttp.h identifies llhttp release/v9.3.1.
    "llhttp": ("", "LICENSE-llhttp"),
    "llhttp SIMD": ("", "LICENSE-llhttp"),
    "Milo": ("undici", "lib/milo/LICENSE.md"),
    "Milo SIMD": ("undici", "lib/milo/LICENSE.md"),
    "long.js i64 arithmetic": ("", "LICENSE-long-header"),
}


JS_DATA = ROOT / "packaging/js-notices"
JS_BUNDLES = json.loads((JS_DATA / "bundles.json").read_text())
JS_NOTICES = json.loads((JS_DATA / "notices.json").read_text())
JS_SOURCES = json.loads((JS_DATA / "sources.json").read_text())
PNPM_MODULE = re.compile(rb"node_modules/\.pnpm/((?:@[\w.-]+\+)?[\w.-]+)@([\w.-]+?)(?:_|/node_modules/)")


def inventory_digest(files):
    # JSON arrays and escaping keep unusual filenames and package labels unambiguous.
    rows = sorted((path, checksum, sorted(components)) for path, checksum, components in files)
    return hashlib.sha256(json.dumps(rows, separators=(",", ":")).encode("utf-8")).hexdigest()


def bundle_components(record, checksum, data, origin):
    if record is None:
        return []
    components = record["packages"]
    named = {name.decode().replace("+", "/") + " " + version.decode()
             for name, version in PNPM_MODULE.findall(data)}
    if checksum != record["sha256"] or not components or named.difference(components):
        raise ValueError(f"Review bundled JavaScript notices for {origin}")
    return components


# Preserve common minifier license banners and esbuild's grouped notices.
JS_NOTICE = re.compile(rb"/\*(?:[!*]|\s*@license\b|\s*Bundled license information:).*?\*/", re.DOTALL)
BANNER_PACKAGE = re.compile(rb"/\*[!*]\s*\*?\s*(@?[\w./-]+)\s+v(\d+\.\d+\.\d+(?:-[\w.-]+)?)\b")
LICENSE_REFERENCE = re.compile(rb"^/\*!\s*For license information please see (.+?)\s*\*/$", re.IGNORECASE)


def bundled_notices(package, manifest, referenced, checked):
    owner = f"{manifest['name']} {manifest['version']}"
    bundles = JS_BUNDLES.get(owner, {})
    sources = []
    seen = set()
    for parent, directories, files in os.walk(package):
        directories[:] = sorted(name for name in directories if name != "node_modules")
        for name in sorted(files):
            path = Path(parent) / name
            if not is_javascript(path):
                continue
            if not path.resolve().is_relative_to(package.resolve()):
                raise ValueError(f"JavaScript outside package: {path}")
            checked.add(path)
            relative = path.relative_to(package).as_posix()
            data = path.read_bytes()
            checksum = hashlib.sha256(data).hexdigest()
            record = bundles.get(relative)
            origin = f"{manifest['name']}/{relative}"
            components = bundle_components(record, checksum, data, origin)
            sources.append((relative, checksum, components))
            for component in components:
                referenced.setdefault(component, set()).add(origin)
            for match in JS_NOTICE.finditer(data):
                notice = match.group()
                banner = BANNER_PACKAGE.match(notice)
                if banner:
                    name, version = (part.decode("ascii").lower() for part in banner.groups())
                    if name == "protobuf.js":
                        name = "protobufjs"
                    referenced.setdefault(f"{name} {version}", set()).add(origin)
                if (notice.startswith(b"/**") and not banner
                        and not re.search(rb"copyright|@license|@preserve", notice, re.IGNORECASE)):
                    continue
                reference = LICENSE_REFERENCE.match(notice)
                key = (path.parent if reference else None, notice)
                if key not in seen:
                    seen.add(key)
                    yield f"{path.relative_to(package)}\n{notice.decode('utf-8')}"
                    if reference:
                        source = path.parent / reference[1].decode("utf-8")
                        yield read_package_file(package, source)

    if inventory_digest(sources) != JS_SOURCES.get(owner, inventory_digest([])):
        raise ValueError(f"Review bundled JavaScript notices for {owner}: "
                         "file or component list differs from reviewed inventory")


def npm_notices(runtime):
    runtime = runtime.resolve()
    packages = json.loads((runtime / "package-lock.json").read_text())["packages"]
    sources = {}
    for path, record in binary_inventory(runtime):
        entry = packages.get("node_modules/" + record.package, {})
        if entry.get("version") != record.version or entry.get("dev"):
            raise ValueError(f"Missing locked package notice for {path}")
        yield f"Binary: {path} — {record.package} {record.version}"
        for component in record.components:
            yield f"{component} (embedded in {path})"
            source = COMPONENT_LICENSES[component]
            sources.setdefault(source, set()).add(component)
    for (owner, filename), components in sources.items():
        base = runtime / "node_modules" / owner if owner else ROOT / "packaging/macos"
        yield ", ".join(sorted(components))
        yield read_package_file(base, base / filename)
    installed = set()
    referenced = {}
    checked = set()
    for location, entry in sorted(packages.items()):
        if not location or entry.get("dev"):
            continue
        package = runtime / location
        if not package.resolve().is_relative_to((runtime / "node_modules").resolve()):
            raise ValueError(f"Package is outside node_modules: {location}")
        if not package.is_dir() and entry.get("optional"):
            continue  # npm omits optional binaries for other platforms.
        manifest = json.loads(read_package_file(package, package / "package.json"))
        if manifest["version"] != entry["version"]:
            raise ValueError(f"Installed version differs from lockfile at {location}")
        license_id = entry.get("license") or manifest.get("license") or "See package files"
        yield f"npm: {manifest['name']} {entry['version']} ({license_id})"
        if manifest.get("author"):
            yield "Author: " + str(manifest["author"])
        installed.add(f"{manifest['name']} {manifest['version']}")
        yield from bundled_notices(package, manifest, referenced, checked)
        files = list(package_notices(package))
        primary = any(p.relative_to(package).parts[0].lower().startswith(("license", "licence", "copying"))
                      for p in files)
        if str(license_id).startswith("SEE LICENSE IN "):
            files.append(package / license_id[len("SEE LICENSE IN "):])
        elif not primary:
            yield fallback_notice(package, manifest, license_id)
        for path in dict.fromkeys(files):
            yield str(path.relative_to(package))
            yield read_package_file(package, path)

    for parent, _, files in os.walk(runtime):
        for name in files:
            path = Path(parent) / name
            if path in checked or not is_javascript(path):
                continue
            if path.parent.name == ".bin" and path.is_symlink() and path.resolve() in checked:
                continue
            raise ValueError(f"Unclassified staged JavaScript: {path.relative_to(runtime)}")

    texts = {}
    for component, paths in sorted(referenced.items()):
        yield f"{component} (embedded in {', '.join(sorted(paths))})"
        if component in installed:
            continue
        sources = JS_NOTICES.get(component)
        if not sources:
            raise ValueError(f"Missing bundled JavaScript notice for {component}")
        for source in sources:
            texts.setdefault(source, []).append(component)
    for source, components in sorted(texts.items()):
        yield ", ".join(components)
        yield read_package_file(ROOT / "packaging", ROOT / "packaging" / source)


def host_notices(root, host, node_version, triple="aarch64-apple-darwin", report_dir=None):
    sections = [rust_notices(root, "plait-host", triple, report_dir),
                f"Node.js {node_version}", (host / "NODE_LICENSE.txt").read_text()]
    sections.extend(npm_notices(host / "runtime"))
    return "\n\n".join(sections) + "\n"


def app_notices(root, host_text):
    assets = root / "assets"
    sections = [rust_notices(root, "plait"), host_text]
    for name in ("fonts/README.md", "fonts/OFL.txt", "themes/base16/README.md",
                 "themes/base16/LICENSE", "harnesses/README.md", "harnesses/LICENSE-lobe-icons"):
        sections.append((assets / name).read_text())
    for scheme in sorted((assets / "themes/base16").glob("*.yaml")):
        credit = [line for line in scheme.read_text().splitlines() if line.startswith(("name:", "author:"))]
        sections.append("\n".join(credit))
    sections.extend(["Stillwater themes — https://github.com/heath0xFF/stillwater",
                     (assets / "themes/LICENSE-stillwater").read_text()])
    return "\n\n".join(sections) + "\n"


def main(argv=None, root=ROOT):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("host", type=Path, help="staged macOS host with Node and installed runtime")
    parser.add_argument("output", type=Path, help="destination for the combined app notices")
    args = parser.parse_args(argv)
    version = json.loads((args.host / "component.json").read_text())["node_version"]
    args.output.write_text(app_notices(root, host_notices(root, args.host, version)), encoding="utf-8")


if __name__ == "__main__":
    main()
