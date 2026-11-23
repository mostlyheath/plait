# Contributing to Plait

Read the [README](README.md), [architecture](docs/ARCHITECTURE.md),
[host-first plan](docs/HOST-FIRST.md), and [design guide](DESIGN.md). Current
work follows the [roadmap](docs/ROADMAP.md). The [host contract](docs/HOST.md)
and [remote guide](docs/REMOTE.md) describe implemented behavior and limits.

## Build and run

The native desktop requires an Apple Silicon Mac, Git, rustup, and full Xcode
with Metal. Runtime work also requires Node 22.19 or later and npm. Rustup uses
`rust-toolchain.toml`. Keep dependencies and lockfiles pinned unless the change
requires an update. If Metal is missing, run `xcodebuild -downloadComponent MetalToolchain`.

```sh
git clone https://github.com/heath0xFF/plait.git
cd plait
cargo build --locked --release
cargo run --locked --release -- /absolute/path/to/repository
```

Omit the path to restore the last checkout and thread. Agent work requires an
[installed host](docs/GUIDE.md#install-and-updates) or [development host](docs/HOST.md). For
packaging and installation from a checkout, see the [guide](docs/GUIDE.md).
See [Releasing](docs/RELEASING.md) for release procedures.

## Build Plait with Plait

Open the checkout in Plait, configure a provider or harness, and start a thread
with the desired behavior, constraints, and checks. Review the actual diff in
Changes and continue the thread for revisions. Plait can stage and commit the
result. Accepted work belongs to the host and continues when the window closes.

Protect existing changes: finish or commit them before asking an agent to edit
the same files, or use an optional per-thread worktree. Trusted commands run as
your user and can change existing work. Sandboxed commands have narrower access;
run steps they cannot perform separately. Approval mode and command policy are
separate choices. See the [host contract](docs/HOST.md) for their limits.

To prepare and install a source build on macOS:

```sh
bash scripts/prepare-self-host.sh
scripts/update-installed-macos.sh
```

The installer preserves credentials, threads, and drafts and can stage a host
update until active work finishes. Do not replace the host running a task during
that task. Inspect completed effects after interruptions; a host restart does
not replay interrupted effects.

## Changes and review

- Discuss substantial scope before starting. Work on a branch and open a small,
  focused pull request when the work is finished. `main` changes only through
  a pull request with passing checks; app changes merged to `main` publish the
  next preview. See [Releasing](docs/RELEASING.md).
- Describe the change, checks, results, and remaining verification limits;
  include screenshots for UI changes. Keep credentials, private transcripts,
  local settings, and build output out of Git.
- Changes that write a person's files, repository, or durable state read by an
  older build need an independent reader before merge. Report the review and
  its findings. Agree on changes to shared models, protocol, durable storage,
  or permission rules before implementation; test compatibility and failures.
- The person opening a pull request is responsible for every line and must be
  able to explain it. Name the authoring model and harness and any reviewing
  model. A different model or person may provide the independent review.
- Code authorship is limited to Claude Opus 5 or later, Claude Fable 5 or later,
  GPT-6 Astra, GPT-6.2 Astra, GPT-6.2 Sol, Gemini 4 Neon, or Gemini 4 Carbon.
  GLM-5.3 Flash may author simple prose, comments, tests, or a few lines in one
  place, excluding file or saved-state writes, protocol, permissions, sandbox,
  and Git writes. Other models may search, summarize, and review. Propose an
  addition to this list in an issue with examples of its pull requests. An
  agent must not open a pull request without a person requesting and owning it.

## Work split

| Area | Responsibility |
| --- | --- |
| `crates/plait` | GPUI presentation, input, focus, and background-work dispatch |
| `crates/plait-client` | Plain models, state transitions, client and recovery; no GPUI |
| `crates/plait-host` | Host lifetime, protocol, repository and process operations |
| `runtime/` | Pi integration, approvals, durable execution, and command policy |

State owns transitions; UI owns presentation. Keep models and Git independent
of GPUI, and filesystem and Git work off the render thread. The host owns
accepted work. Preserve one active writer per checkout, dirty work, exact-call
approval, and sandbox checks. Acknowledgements clear only the draft revision
sent; IME composition must not submit. Show real state, including failures and
unavailable actions. Sample state belongs only in test scenes.

Host-first ownership is being adopted; see the [plan](docs/HOST-FIRST.md) for
what remains planned. Git writes follow the allowlist in the
[roadmap](docs/ROADMAP.md). General history rewriting and an embedded terminal
are outside the current scope.

## Required checks

Run the required Rust checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Build `plait-host`, install runtime dependencies with `npm ci --ignore-scripts`
in `runtime/`, and run `npm run check` and `npm test` there. The shared CI entry
point is `bash scripts/check.sh`; it installs locked runtime packages, checks and
tests the applicable targets, and builds release binaries. Linux requires a C
compiler and linker. Linux tests trusted execution and sandbox refusal, but do
not establish command containment. `bash scripts/check.sh --require-command-sandbox`
requires a supported sandbox; see the [Linux sandbox decision](docs/LINUX-SANDBOX.md).

Test parsers, async result guards, and repository edge cases. After UI changes,
build and inspect wide and narrow native windows. `bash scripts/snapshots.sh
/tmp/plait-snapshots` produces test scenes for layout inspection; these do not
replace native interaction checks. See [verification](docs/VERIFICATION.md) for
coverage and limits. Do not run `scripts/acceptance-macos.sh` while a person is
using the Mac: it takes over the pointer and keyboard.

## Originality and licenses

Application code, assets, and design must be original. Do not copy, port, or
translate another application's code or use its distinctive UI as a template,
even when its license permits it. Compare observable outcomes and implement
requirements in Plait's own architecture.

Pin declared dependencies, inspect upstream source at the pinned revision before
using APIs, and retain licenses and notices. Preserve the provenance of
[Plait artwork](assets/branding/README.md) and
[harness logos](assets/harnesses/README.md); harness logos identify the harness,
not Plait. Contributions are under the [MIT license](LICENSE); dependencies
retain their own licenses.
