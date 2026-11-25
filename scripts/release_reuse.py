#!/usr/bin/env python3
"""Decide whether a push to main can reuse its pull request's check.

`release_reuse.py <sha> <before>` prints `reused=true` and `reused_run=<url>`
when the push is exactly one merged pull request, and a successful macOS check
of that request recorded the same Git tree as the pushed commit. Otherwise it
prints `reused=false`, and main runs the full check. Any error, missing
record, or ambiguity also prints `reused=false`: a doubt costs a test run,
never a skipped one.

Prints `key=value` lines for `$GITHUB_OUTPUT` and its reasons on stderr.
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ".github/workflows/macos.yml"
ARTIFACT = "tested-tree"
# A request rarely has more passing runs for its final commit than this.
CANDIDATES = 5


class NoReuse(Exception):
    pass


def merged_request(sha, repository, requests):
    """The one pull request into main whose merge made `sha`."""
    merged = [r for r in requests if r.get("merged_at") and r.get("merge_commit_sha") == sha and (r.get("base") or {}).get("ref") == "main"]
    if len(merged) != 1:
        raise NoReuse(f"{len(merged)} merged pull requests made this commit")
    request = merged[0]
    head = request.get("head") or {}
    # A fork's run is not reused: its workflow and artifacts come from outside this repository.
    if ((head.get("repo") or {}).get("full_name")) != repository:
        raise NoReuse("the pull request came from another repository")
    if not head.get("sha"):
        raise NoReuse("the pull request has no head commit")
    return request


def passing_runs(head, repository, runs):
    """This repository's successful macOS checks of the request's final commit, newest first."""
    passing = [
        run for run in runs
        if run.get("event") == "pull_request"
        and run.get("head_sha") == head
        and run.get("conclusion") == "success"
        and str(run.get("path", "")).split("@")[0] == WORKFLOW
        and ((run.get("head_repository") or {}).get("full_name")) == repository
    ]
    passing.sort(key=lambda run: (run.get("run_number", 0), run.get("run_attempt", 0)), reverse=True)
    return passing[:CANDIDATES]


def decide(sha, before, parent, tree, repository, requests, runs_for, tree_of):
    """Returns the URL of a run that tested `tree`, or raises NoReuse."""
    # A push of several commits was not one request's tested merge.
    if not before or parent != before:
        raise NoReuse("the push holds more than one commit")
    request = merged_request(sha, repository, requests)
    for run in passing_runs(request["head"]["sha"], repository, runs_for(request["head"]["sha"])):
        if tree_of(run) == tree:
            return run["html_url"]
    raise NoReuse(f"no passing check of #{request.get('number')} tested this tree")


def git(*args):
    return subprocess.run(["git", "-C", str(ROOT), *args], check=True, capture_output=True, text=True).stdout.strip()


def gh_json(path):
    return json.loads(subprocess.run(["gh", "api", path], check=True, capture_output=True, text=True, timeout=60).stdout)


def recorded_tree(run):
    with tempfile.TemporaryDirectory() as directory:
        result = subprocess.run(
            ["gh", "run", "download", str(run["id"]), "-n", ARTIFACT, "-D", directory],
            capture_output=True, text=True, timeout=120,
        )
        if result.returncode != 0:
            return None
        return (Path(directory) / ARTIFACT).read_text().strip()


def main(argv):
    if len(argv) != 3:
        raise NoReuse("usage: release_reuse.py <sha> <before>")
    sha, before = argv[1], argv[2]
    repository = os.environ["GITHUB_REPOSITORY"]
    url = decide(
        sha, before,
        parent=git("rev-parse", f"{sha}^"),
        tree=git("rev-parse", f"{sha}^{{tree}}"),
        repository=repository,
        requests=gh_json(f"repos/{repository}/commits/{sha}/pulls"),
        runs_for=lambda head: gh_json(
            f"repos/{repository}/actions/workflows/macos.yml/runs?event=pull_request&head_sha={head}&per_page=20"
        )["workflow_runs"],
        tree_of=recorded_tree,
    )
    return url


if __name__ == "__main__":
    try:
        url = main(sys.argv)
        print(f"Reusing the pull request's check: {url}", file=sys.stderr)
        print(f"reused=true\nreused_run={url}")
    except BaseException as error:  # noqa: BLE001 - any doubt runs the full check
        print(f"Running the full check: {error}", file=sys.stderr)
        print("reused=false")
