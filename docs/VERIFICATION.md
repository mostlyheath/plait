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

The `bubbles` and `turns-line` scenes draw the same request and reply. Additional bubble scenes cover wrapping, narrow windows, attachments, streaming, folded turns, Find and selection. Headless tests check that both styles expose the authors and select the same text across their messages.

The `work-*` conversation scenes cover finished and running work, failure, a direct answer, Find inside a fold, and the appearance preference. The `closing-line-*` scenes cover outcomes, file lists, Undo refusal, a narrow column, and the older-host fallback. Native checks include opening work from the bottom of the conversation, following a recorded file through Review, and moving through the controls with the keyboard.

The `quote-selection-*` scenes show the action beside Copy message and the appended quote in the draft. A headless test activates the action with a pointer and a rebound key, checks the caret, and verifies that an acknowledgement for the earlier draft cannot clear the quote.

The `header-*` scenes cover the title and tabs settings, tab overflow, thread states, project views, drafts, and both traffic-light insets. Pixel inspection, native interaction and frame timings for the header are pending. Exercise tab shortcuts in both settings, reorder and close a tab with the pointer, and check truncation with a long title and branch. Check window dragging, the system double-click action and both transitions into and out of full screen with the sidebar shown and hidden.

macOS uses the app's text shaping and renderer offscreen; Linux uses its own
fonts and software rendering. Scenes do not validate the native window,
input, accessibility bridge, or live provider. Build and inspect the native
app after UI changes. With Accessibility permission already granted,
`bash scripts/smoke-macos.sh` exercises accessible controls in disposable
repositories. See [UI checks](../CONTRIBUTING.md#required-checks).

The `running-mark` scenes show the same resting ring in the sidebar, tab,
turn state, and running tool. Tests check that all four marks switch between
animated and still sprites when Reduce Motion changes and queue no view
redraw callbacks. On Macs with a Metal device, the test also compares the
visible starting poses pixel for pixel. The pinned GPUI rotation
test checks that a sprite advances while its owning view’s render count
stays constant. In a native
build, observe an active turn and toggle macOS Reduce Motion while it is
working. The ring should stand still with Reduce Motion on and resume
turning with it off.

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

- The message box, changed-files strip, and approval card have wide and narrow
  test scenes. Native focus, input-method composition, VoiceOver, and the cost
  of drawing the box shadow while typing and scrolling still need inspection.
- Headless tests cover all three bundled IBM Plex Sans weights, tabular
  figures, and switching between Plex Sans and System. They check that both
  font choices fit the Appearance controls at 1380 × 860 and 900 × 600.
  The `interface-font-*` scenes also show the conversation and sidebar at
  both sizes. Pixel review, native window inspection on a Mac without a
  separate Plex Sans installation, and frame benchmarks remain open.
- The shared controls draw a hollow 1.5-point blue inset focus outline.
  An offscreen Metal test uses Tab and checks the outline and surface pixels on
  Panel, Raised, and Navigation in both defaults. It requires a Metal device.
  Native visual review and the specified 1-point exterior gap remain open.
- The sheet and sidebar foot have headless scenes for both sidebar positions and for
  connected, connecting, offline, and update-available states. Native checks cover
  the sheet corners, window dragging, and the foot controls by pointer and keyboard.
  Those checks and frame measurements are pending.
- Sidebar card scenes cover the state words, the worktree branch, missing
  metadata, selection and hover, inline rename, both menus, archived threads,
  folded and missing projects, and scrolling 300 threads across six projects.
  Native inspection in both themes and widths, VoiceOver announcements, and
  comparison of the d1–d8 frame cases are pending.
- The earlier desktop layout (sidebar, status line, message box, This turn,
  folded turns, Work view) was released without inspection in the native app.
  Its scenes were rendered and looked at on Linux only. See
  [#130](https://github.com/mostlyheath/plait/issues/130).
- Some tests depend on timing and fail now and then on unrelated changes; one
  such failure left a test fixture running and hung a release build. Passing
  runs are not yet evidence that these tests are sound. See
  [#128](https://github.com/mostlyheath/plait/issues/128).
- The installed-runtime suite runs only in a release build, so a break that
  only it catches is found when releasing, not at merge. Runtime tests that
  depend on macOS paths are not caught by Linux runs.
- Host rollback after the protocol 7 store change was tested by writing a
  store as an older host would, not by running a previous host binary. One
  recovery case after a rollback is known and open; see
  [#131](https://github.com/mostlyheath/plait/issues/131).
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

Panel interaction checks cover opening each view, switching focus for Find, resizing and resetting the divider, reopening a tab, and returning to the conversation on narrow sheets. Check the saved width again after restarting the native app.
