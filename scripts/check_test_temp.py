"""Run a CI suite and report changes to its Plait temporary entries."""

from pathlib import Path
import subprocess
import sys
import tempfile


def snapshot(roots):
    return {path for root in roots for path in root.iterdir() if path.name.startswith("plait-")}


def check(command, roots):
    before = snapshot(roots)
    status = subprocess.run(command).returncode
    after = snapshot(roots)
    for path in sorted(after - before):
        print(f"Test left temporary entry {path}", file=sys.stderr)
    for path in sorted(before - after):
        print(f"Test removed pre-existing entry {path}", file=sys.stderr)
    return status or int(before != after)


if __name__ == "__main__":
    roots = {Path("/tmp").resolve(), Path(tempfile.gettempdir()).resolve()}
    sys.exit(check(sys.argv[1:], roots))
