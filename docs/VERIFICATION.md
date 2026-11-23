# Verification

Automated checks cover important boundaries; they do not establish that every
installed-app interaction or live service works. Run checks directly from the
repository root. [CONTRIBUTING.md](../CONTRIBUTING.md) describes build
requirements and required checks.

## Automated suites

`bash scripts/check.sh` runs formatting, the acceptance script's windowless
checks, the headless dependency guard, Clippy, Rust tests, runtime type checks
and tests, and release builds. It installs runtime dependencies from the
lockfile without lifecycle scripts. On macOS it checks the workspace; on
Linux it checks `plait-client` and `plait-host`. Use
`bash scripts/check.sh --require-command-sandbox` on macOS to require the
real command sandbox tests. That gate fails on Linux while a containment
backend is unavailable.

Focused Rust checks on macOS:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

Rust tests cover parsers, protocol framing, state transitions and stale
asynchronous replies, recovery, Git reads and writes against temporary
repositories, checkout identity, leases, and filesystem edge cases. On Linux,
use `-p plait-client -p plait-host` in place of `--workspace` for Clippy and
tests.

For the runtime, first build `plait-host`, then run:

```sh
cargo build --locked -p plait-host
cd runtime
npm ci --ignore-scripts --omit=optional
npm run check
npm test
```

Runtime tests cover scripted host and agent behavior, approvals, cancellation,
recovery, dirty-file protection, records and undo, queues, worktrees, and
bounded output. macOS sandbox tests exercise real containment; Linux tests
cover trusted execution and refusal of sandboxed commands, not containment.
The credential-free Pi recovery probe runs separately:

```sh
cd experiments/pi-durable
npm ci --ignore-scripts
npm test
```

## Scenes and native checks

Headless GPUI scenes use injected test state to exercise views and interaction
in wide and narrow layouts. Export them with:

```sh
bash scripts/snapshots.sh /tmp/plait-snapshots
```

macOS uses the app's text shaping and renderer offscreen; Linux uses its own
fonts and software rendering. Scenes do not validate the native window,
input, accessibility bridge, or live provider. Build and inspect the native
app after UI changes. With Accessibility permission already granted,
`bash scripts/smoke-macos.sh` exercises accessible controls in disposable
repositories. See [UI checks](../CONTRIBUTING.md#required-checks).

`scripts/acceptance-macos.sh` drives a native window and collects observations.
It takes over the pointer and keyboard: do not run it while someone is using
the Mac. Its `--self-test` checks acceptance logic without driving a window.
Release CI runs native acceptance as an advisory job; it does not gate
publication. See [Releasing](RELEASING.md).

## CI

The Linux job runs the headless checks. The macOS check runs the workspace
and sandbox gate, Pi recovery probe, and release-script tests. Release jobs
build and test packages, including an installed-package smoke check; the
separate native acceptance job is advisory. Consult the workflow files for
conditions and the checks for a particular change.

## Performance checks

The planned host-first speed check compares local-socket Git status and diff
latency with the direct desktop path before that path is removed. The named
`scripts/speed-check.sh` is not present in this checkout; the check remains
planned. See [HOST-FIRST.md](HOST-FIRST.md#workspace-and-git).

`bash scripts/bench-frames.sh` measures headless CPU frame costs for generated
heavy scenes; `--debug` uses an unoptimized build. It excludes GPU
presentation, native input latency, and the installed app. It does not prove
that the application's frame or latency goals are met.

## Known gaps

- A live remote check from macOS to a Linux host over both SSH and Tailscale,
  including a run continuing while the client is closed, remains open.
- Linux has no validated command sandbox. Trusted execution tests do not
  establish containment.
- Real provider sign-in, token refresh, and live-model behavior require
  account-backed checks; scripted suites do not contact providers.
- Installed-app acceptance still needs coverage of first launch on a clean
  Mac, native pointer and keyboard interactions, IME, full VoiceOver,
  notifications, and sustained streaming. Headless scenes cannot prove these.
- Real remote Git credentials, signed commits, checkout filters, and worktree
  runs need installed-app exercise. Automated repository fixtures do not cover
  every configuration.
- CI runs on macOS 26; behavior on the minimum supported macOS version needs
  separate validation. The desktop is not validated as a native Linux app.
