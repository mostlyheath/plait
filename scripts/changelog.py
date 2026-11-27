#!/usr/bin/env python3
"""Collect change notes into CHANGELOG.md and read a release's section.

Each pull request with a change people will notice adds one file to
`changes/`, so parallel pull requests never edit the same lines. Its first
line is a category (Added, Changed, Fixed or Removed), and the rest is the
note, in plain words for someone using Plait.

`changelog.py collect <tag>` moves every note into a new section at the top
of CHANGELOG.md and deletes the note files. A release-prep pull request runs
it. `changelog.py section <tag>` prints that section for the release notes,
and fails if the section is missing or any notes still await collection.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CATEGORIES = ("Added", "Changed", "Fixed", "Removed")
HEADER = "# Changelog\n\nWhat changed in each Plait preview, newest first.\n"
TAG = re.compile(r"^v\d+\.\d+\.\d+(-preview\.\d+)?$")


class Invalid(Exception):
    pass


def parse_note(path):
    """A note's category and text; the file name is only for humans."""
    lines = path.read_text(encoding="utf-8").strip().splitlines()
    if len(lines) < 2:
        raise Invalid(f"{path.name}: expected a category line, then the note")
    category = lines[0].strip().rstrip(":")
    if category not in CATEGORIES:
        raise Invalid(f"{path.name}: category must be one of {', '.join(CATEGORIES)}")
    text = " ".join(line.strip() for line in lines[1:] if line.strip())
    if not text:
        raise Invalid(f"{path.name}: the note is empty")
    return category, text


def notes(directory):
    """Every note file except the README, in a stable order."""
    files = sorted(p for p in directory.glob("*.md") if p.name != "README.md")
    return [(path, *parse_note(path)) for path in files]


def render(tag, entries):
    lines = [f"## {tag}", ""]
    for category in CATEGORIES:
        items = [text for _, kind, text in entries if kind == category]
        if items:
            lines += [f"### {category}", ""] + [f"- {item}" for item in items] + [""]
    return "\n".join(lines)


def sections(text):
    """Map each `## <tag>` heading to its body, keeping the preamble apart."""
    parts = re.split(r"(?m)^## ", text)
    found = {}
    for part in parts[1:]:
        heading, _, body = part.partition("\n")
        found[heading.strip()] = body.strip()
    return parts[0], found


def collect(tag, root=ROOT):
    if not TAG.match(tag):
        raise Invalid(f"not a release tag: {tag}")
    directory = root / "changes"
    entries = notes(directory)
    if not entries:
        raise Invalid("no change notes to collect in changes/")
    changelog = root / "CHANGELOG.md"
    current = changelog.read_text(encoding="utf-8") if changelog.exists() else HEADER
    preamble, existing = sections(current)
    if tag in existing:
        raise Invalid(f"CHANGELOG.md already has a section for {tag}")
    rest = current[len(preamble):]
    changelog.write_text(preamble.rstrip() + "\n\n" + render(tag, entries) + "\n" + rest, encoding="utf-8")
    for path, _, _ in entries:
        path.unlink()
    return len(entries)


def section(tag, root=ROOT):
    changelog = root / "CHANGELOG.md"
    if not changelog.exists():
        raise Invalid("CHANGELOG.md is missing")
    _, found = sections(changelog.read_text(encoding="utf-8"))
    body = found.get(tag)
    if not body:
        raise Invalid(f"CHANGELOG.md has no section for {tag}; run the release-prep step first")
    if notes(root / "changes"):
        raise Invalid(f"add the outstanding change notes to the {tag} section before releasing")
    return body


def main(argv):
    if len(argv) != 3 or argv[1] not in ("collect", "section"):
        print("usage: changelog.py collect|section <tag>", file=sys.stderr)
        return 2
    try:
        if argv[1] == "collect":
            print(f"Collected {collect(argv[2])} notes into CHANGELOG.md")
        else:
            print(section(argv[2]))
    except Invalid as error:
        print(f"changelog: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
