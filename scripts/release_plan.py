#!/usr/bin/env python3
"""Decide what a CI run does.

A change is tested on its pull request and again when it reaches main. A
merge releases nothing: a person starts a release by hand. `pull-request
<base>` says whether the request needs the tests at all; `merged <before>`
says the same for a push to main; `main`, used by a release, says whether main
holds code that ships and has not been released, and which preview it would be.

Prints `key=value` lines for `$GITHUB_OUTPUT`.
"""
import fnmatch
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Prose changes nothing a test reads.
PROSE = ("*.md", "docs/*", "LICENSE")

# Paths that never reach an installed app or host. Anything else ships.
UNSHIPPED = PROSE + (
    ".github/*",
    "experiments/*",
    "runtime/test/*",
    "scripts/acceptance/*",
    "scripts/acceptance-macos.sh",
    "scripts/bench-*.sh",
    "scripts/check.sh",
    "scripts/install-nextest.sh",
    "scripts/probe-*.mjs",
    "scripts/readme-screenshots.sh",
    "scripts/release_plan.py",
    "scripts/release_reuse.py",
    "scripts/snapshots.sh",
    "scripts/test_*.py",
)


def only(paths, patterns):
    """Whether every path matches one of the patterns. `*` crosses folders."""
    return all(any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns) for path in paths)


def needs_tests(paths):
    """A pull request is tested unless it is known to change only prose."""
    return paths is None or not paths or not only(paths, PROSE)


def ships(paths):
    """Whether these changes reach an installed app. Unknown changes do."""
    return paths is None or (bool(paths) and not only(paths, UNSHIPPED))


def previews(version, tags):
    """The preview numbers already used for this version, dead tags included."""
    pattern = re.compile(rf"^v{re.escape(version)}-preview\.(\d+)$")
    return sorted(int(match[1]) for match in map(pattern.match, tags) if match)


def last_tag(version, tags):
    used = previews(version, tags)
    return f"v{version}-preview.{used[-1]}" if used else None


def next_tag(version, tags):
    used = previews(version, tags)
    return f"v{version}-preview.{used[-1] + 1 if used else 1}"


def git(*args):
    return subprocess.run(["git", "-C", str(ROOT), *args], check=True, capture_output=True, text=True).stdout


def changed(since):
    """Paths that differ between `since` and HEAD, or None when that is unknown."""
    try:
        return [line for line in git("diff", "--name-only", since, "HEAD").splitlines() if line]
    except subprocess.CalledProcessError:
        return None


def cargo_version():
    manifest = (ROOT / "crates/plait/Cargo.toml").read_text()
    return re.search(r'^version = "([^"]+)"', manifest, re.MULTILINE)[1]


def main(argv):
    if argv[:1] == ["pull-request"] and len(argv) == 2:
        print(f"test={str(needs_tests(changed(argv[1]))).lower()}")
        print("release=false")
        return 0
    if argv[:1] == ["merged"] and len(argv) == 2:
        print(f"test={str(needs_tests(changed(argv[1]))).lower()}")
        print("release=false")
        return 0
    if argv == ["main"]:
        version = cargo_version()
        tags = git("tag", "--list").splitlines()
        previous = last_tag(version, tags)
        release = ships(changed(previous)) if previous else True
        print("test=false")
        print(f"release={str(release).lower()}")
        print(f"tag={next_tag(version, tags)}")
        print(f"previous={previous or ''}")
        return 0
    print("Usage: release_plan.py pull-request <base> | merged <before> | main", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
