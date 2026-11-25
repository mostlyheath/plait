# Change notes

A pull request that changes something a person using Plait will notice adds one
file here. Name it after the branch, for example `fix-worktree-warning.md`. Each
pull request has its own file, so pull requests never conflict over the
changelog.

The first line is the category: `Added`, `Changed`, `Fixed`, or `Removed`. The
rest is one short note in plain words, written for someone using Plait, not
for a reviewer:

```
Fixed
Creating a thread in its own worktree no longer warns that the worktree
changed on disk.
```

Docs, tests, CI and internal refactors need no note.

Before a release, a release-prep pull request runs
`python3 scripts/changelog.py collect <tag>`. That moves every note into a new
section at the top of [CHANGELOG.md](../CHANGELOG.md) and deletes the note
files. The release publishes that section as its notes, and it refuses to
publish without one. See [Releasing](../docs/RELEASING.md).
