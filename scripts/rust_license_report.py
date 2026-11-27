"""Generate release license texts with the pinned cargo-about for the requested target."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

from third_party_notices import rust_inventory


# cargo-about 0.9.3 logs fallback per crate before grouping identical license texts.
FALLBACK = re.compile(r"unable to find text for license '([^']+)' for crate '([^']+)', falling back to canonical text")


def fallback_notices(root, diagnostics, inventory):
    missing = sorted({(crate, license_id) for license_id, crate in FALLBACK.findall(diagnostics)
                      if crate in inventory})
    if not missing:
        return []
    directory = root / "packaging/rust-notices"
    records = json.loads((directory / "sources.json").read_text())
    by_crate = {crate: record for record in records for crate in record["crates"]}
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=root))
    members = set(metadata["workspace_members"])
    workspace = {f"{item['name']} {item['version']}" for item in metadata["packages"]
                 if item["id"] in members}
    sections = {}
    for crate, license_id in missing:
        if crate in workspace and license_id == "MIT":
            text = (root / "LICENSE").read_text()
        else:
            record = by_crate.get(crate)
            if not record or license_id != "MIT":
                raise ValueError(f"Supply a checksum-pinned {license_id} notice for {crate}")
            data = (directory / record["file"]).read_bytes()
            if not data.strip() or hashlib.sha256(data).hexdigest() != record["sha256"]:
                raise ValueError(f"Invalid pinned notice for {crate}")
            text = data.decode("utf-8")
        sections.setdefault(text, []).append(crate)
    return [", ".join(crates) + "\n" + text for text, crates in sections.items()]


def generate_reports(root, triple, components, output):
    target = root / "target"
    tools = target / "build-tools"
    subprocess.run(["cargo", "install", "cargo-about", "--locked", "--version", "0.9.3",
                    "--features", "cli", "--root", tools, "--target-dir", target / "build-tools-compile"],
                   cwd=root, check=True)
    packaging = root / "packaging/macos"
    config = output / f"about-{triple}.toml"
    config.write_text(f"targets = {json.dumps([triple])}\n" + (packaging / "about.toml").read_text())
    for component in components:
        name = {"plait-host": "host", "plait": "client"}[component]
        report = output / f"{name}-rust-licenses.txt"
        result = subprocess.run([tools / "bin/cargo-about", "-L", "debug", "generate", "--locked", "--fail",
                                 "--manifest-path", root / f"crates/{component}/Cargo.toml", "--config", config,
                                 packaging / "notices.hbs"], cwd=root, capture_output=True, text=True)
        if result.returncode:
            sys.stderr.write(result.stderr)
            result.check_returncode()
        inventory = set(rust_inventory(root, component, triple).splitlines()[1:])
        represented = set(re.findall(r"^Used by: (\S+ \S+)$", result.stdout, re.MULTILINE))
        omitted = inventory - represented
        if omitted:
            raise ValueError("Missing Rust license report entries for " + ", ".join(sorted(omitted)))
        extra = fallback_notices(root, result.stderr, inventory)
        if not result.stdout.strip():
            raise ValueError(f"Empty Rust license report for {component}")
        report.write_text("\n\n".join([result.stdout, *extra]), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("triple")
    parser.add_argument("components", nargs="+", choices=("plait", "plait-host"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    generate_reports(root, args.triple, args.components, root / "target")
