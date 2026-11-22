# Working on Plait

- Read README.md and docs/ARCHITECTURE.md. Stay inside the current milestone.
- The desktop is Rust + GPUI. TypeScript is for the runtime sidecar and probes.
  No web UI, embedded terminal, or mandatory worktrees.
- Keep models and Git free of GPUI. State owns transitions; UI owns presentation.
- Run all filesystem and Git work on the background executor, never in render.
- Git reads through the host stay hardened: disable optional locks, external diff,
  textconv, and filters. Git writes are limited to what docs/ROADMAP.md agrees
  (staging by file or hunk, unstaging, committing, push, publish, fetch,
  fast-forward pull, confirmed, undoable discard, creating and switching
  branches, restoring the files a turn changed when it is undone, and adding
  or removing an optional per-thread worktree). Normal development commits are
  permitted.
- Show only real data in the app: no sample threads or placeholder machines.
  Test scenes may inject sample state, never in a shipped view. Do not imply execution.
- Build original application code and design. Do not copy or port competitor code,
  assets, or distinctive UI. Compare user outcomes, not implementation details.
  Declared, pinned dependencies are permitted; retain their licenses.
- This repository is public. Keep docs current and reader-facing; omit personal
  workstation details, session handoffs, private transcripts, and agent work logs.
  Separate shipped behavior, planned work, and verification limits. Keep provenance.
- Pin dependencies. Check upstream source at the pinned revision before using APIs.
- Use small functions, explicit errors, and short comments explaining why.
- Test parsers, async result guards, and repository edge cases.
- Run cargo fmt --all -- --check, cargo clippy --workspace --all-targets -- -D warnings,
  and cargo test --workspace. Build and inspect the native app after UI changes.
- Run builds and tests directly.
- Do not run `scripts/acceptance-macos.sh` while a person is using this Mac: it
  takes over the pointer and keyboard. It runs on CI for each release build.
- Work on a branch. `main` changes only through a pull request whose checks
  passed; merging one that changes the app publishes the next preview. See
  docs/RELEASING.md.
- Read CONTRIBUTING.md for the work split. Agree before changing shared contracts;
  docs/ROADMAP.md records what is agreed for the current milestone.
