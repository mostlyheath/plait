# Verification

Passing development tests, installed-package checks, and live-model acceptance
are different kinds of evidence. This page records their scope. For commands
and prerequisites, see [Contributing](../CONTRIBUTING.md#required-checks).

## Release baseline

- **Release:** [v0.1.0-preview.2](https://github.com/heath0xFF/plait/releases/tag/v0.1.0-preview.2)
- **Source:** [`5316950`](https://github.com/heath0xFF/plait/tree/5316950cf69035ed99e4c4d35369eb0b4282d589)
- **Platform:** Apple Silicon, macOS 26 CI

The [release workflow](https://github.com/heath0xFF/plait/actions/runs/43888424840)
passed before publication. [Linux CI](https://github.com/heath0xFF/plait/actions/runs/43888417201)
also passed for that source commit. These results do not certify later changes
or runtime behavior on older macOS versions.

| Check | Evidence |
| --- | --- |
| Rust workspace | Formatting, strict Clippy, 90 Mac tests, release builds |
| Production runtime | 105 tests from source and the same 105 with the installed host, Node, and sidecar |
| Durable runtime recovery | Two separate credential-free process-crash probes |
| Packaging | 21 tests, native `plutil`, package contents, and read-only DMG mount/content verification |
| Installed host | Standard service scheduling; three isolated first-start checks within 15 seconds; no developer Node path |
| Independent updates | Client replacement and staged host update preserve active work; activation follows cleanup |
| Native recovery | Actual installed client is terminated during an accepted command, then reopens the same thread and unsent draft |
| Public downloads | All 19 assets downloaded; all eight checksum files, release discovery, and component metadata checked |

The staged host upgrade uses a labeled fixture with unchanged executable code.
It does not establish compatibility with unknown future protocols or storage
migrations. Checksums detect corruption, not publisher identity. The preview is
**unsigned and not notarized**; see [Mac installation](MACOS.md).

## Protocol version 3

[Protocol version 3](HOST.md#protocol-version-3) is implemented in the host,
runtime, and desktop core, after the release baseline above. Its evidence is
development tests only:

- Runtime tests drive the real host binary and sidecar with a scripted provider
  in disposable stores and repositories. They cover history paging and
  oversized text, thread titles, listing and archive, the thread-list
  subscription, model and thinking changes, queued and steering inputs across
  Stop and restart, usage and reasoning, the turn limit, and retry without a
  repeated tool effect.
- Recorded version 1 and 2 replies are compared byte for byte with the current
  host, including for a thread that used version 3 features and a store saved
  before version 3.
- Rust tests cover negotiation from 3 to 2 to 1, request shapes, and the
  thread-list reader, and decode replies recorded from the real host with the
  client's strict types.

Not verified: a live model, a real provider's reasoning or usage figures, the
installed package with a live provider. Desktop integration is covered by the
conversation, composer, and navigation checks below. Thread-list backpressure relies on the code path shared with `watch` and
has no test of its own.

## Recovery and startup regressions

The desktop journal releases its writer lock when its final owner is dropped.
Regression tests cover a duplicated descriptor and a child held before exec.
They check immediate reopen, saved data, and exclusive ownership. Host and command
leases retain their separate lifetimes. See [the recovery fix](https://github.com/heath0xFF/plait/pull/20).

The installed host uses standard service scheduling. Its startup gate creates
three isolated LaunchAgents with separate stores, empty credential homes, and
no auto-restart. Each must answer within the existing 15-second bound. The
[startup correction](https://github.com/heath0xFF/plait/pull/21) also includes a controlled comparison with the retired background policy. These
are service-readiness checks, not native UI latency or live-account acceptance.

## Installed coding check

`runtime/test/dogfood.macos.test.ts` connects the production runtime to a scripted
loopback provider through Pi's real decoder. In a disposable copy of Plait source,
it approves a refactor and regression test, runs offline locked Cargo tests,
disconnects, retries the accepted request, restarts the host, and completes a
follow-up. Assertions cover Git diffs, the unchanged index and pre-existing dirty
work, stable tool/message identities, and absence of repeated execution.

The installed-client gate launches the actual native app against that host. It
terminates the client process during the command, then reopens it without a checkout argument.
The restored draft is supplied by the test, not typed by a person. Reopening
neither submits the draft nor calls the provider again.

Inspected native captures from the release:

- [Accepted command running](https://github.com/heath0xFF/plait/releases/download/v0.1.0-preview.2/installed-client-active.png)
- [Reopened thread, code block, and saved draft](https://github.com/heath0xFF/plait/releases/download/v0.1.0-preview.2/installed-client-coding.png)

These checks establish real tool effects and native process recovery, not
live-model quality, personal-account authentication, or keyboard/IME behavior.
The response labels the scripted provider. No sample-view code ships in the app.

## Coverage boundaries

- **Rust/core:** Git parsing and repository edge cases, unchanged index/worktree
  after reads, stale async results, protocol framing, peer identity, Unicode,
  transcript updates, and private recovery journals.
- **Runtime:** approvals and revocation, dirty-file protection, exact postimages,
  request deduplication, host/sidecar failure, cancellation, and bounded output.
- **macOS sandbox:** denied outside writes, network, sockets, signals, and cache
  access; permitted private build output and scratch IPC. This is not a general
  process container; see the [host contract](HOST.md#safety-boundary).
- **Linux:** headless development and explicit command rejection. Scripted
  lifecycle fixtures are not a working Linux containment backend.
- **Headless UI:** wide/narrow layouts, scroll-state retention, and recovery
  guards with injected test state, on macOS and Linux. Exported macOS images use
  the app's Metal renderer and Core Text offscreen; Linux images use Linux fonts
  and software rendering. Neither validates the native window, input, focus
  that follows input, VoiceOver, or latency. Debug test builds run a prepaint
  accessibility collector over headless scenes to catch duplicate node IDs and
  missing required nodes across frames; that checks Plait's element identities,
  not the native AccessKit or macOS accessibility bridge. Conversation text
  selection, copy, link hover, and the jump to the newest row are exercised by
  pointer and key events sent through GPUI's dispatch in these scenes, not by
  a native pointer.

Project navigation has headless scenes for wide and narrow grouping, unread and
approval states, archived threads, inline rename, search, empty results, thread
actions, paused updates, and the older-host fallback. A dispatched UI test switches
checkouts while preserving each draft and rejecting the previous watch. Core
tests cover listing identity guards, activity tracking, preference migration, and
serialized saves. Delayed management replies cannot dismiss a newer rename or
overwrite a newer live summary; reconnect and checkout changes release pending
controls whose old replies are discarded. System notification delivery and
notification-click activation are not exercised by these tests.

The optional headless PNG export is ignored in the normal Rust suite and runs
explicitly when requested; CI does not run it on either platform. See
[UI checks](../CONTRIBUTING.md#ui-checks).

## Provider sign-in

The [provider sign-in operations](HOST.md#provider-sign-in) landed after the
release baseline and have development evidence only:

- Scripted providers cover the sign-in states, cancellation, expiry, bounds,
  key storage, and the absence of secrets in every reply.
- Pi's real registry and credential store, in a disposable agent directory with
  outbound requests blocked, cover key storage and file modes, literal keys, and
  `models.json` edits. Connection checks use a local server.
- The host binary with a scripted sidecar covers a reload during an active run,
  operations over the socket, and sign-in cleanup at shutdown. The production
  sidecar entry is started once against an empty disposable Pi directory.

No test contacts a provider. Pi's real sign-in flows for Anthropic, OpenAI, and
the others, including the browser redirect to the loopback listener and token
refresh afterward, have not been run through the host. They need acceptance
with real accounts through Settings → Providers.

## Settings interaction coverage

Source builds have wide and narrow scenes for provider lists, browser and device
sign-in, optional code entry, failed sign-in, masked API keys, endpoint model
selection, command policy, and Trusted confirmation. These inject host replies;
they do not authenticate to a real provider or save a real credential.

Shortcut scenes send keys through GPUI dispatch to prove that recording intercepts
an existing command, conflicts name it, and rebinding removes the old shortcut.
Secret-field checks keep clipboard contents unchanged and prove that a sent value
cannot return through Undo. Dispatched Tab and Shift-Tab checks traverse endpoint
fields and reach the composer from adjacent controls. Endpoint results only
describe the address that was checked. Pure client tests cover request encoding,
tolerant replies, sign-in states, endpoint validation, and policy copy.

An isolated native source build was inspected against a disposable version 3 host
and checkout. All five Settings tabs, provider entry with masked accessibility
values, endpoint field traversal through Save, shortcut conflict cancellation and
rebinding, command-palette search and dispatch, and cancelling Trusted confirmation
with Return were checked. This exposed and resolved missing field tab stops and
duplicate Settings heading accessibility IDs. The pinned headless test platform
does not activate accessibility trees, so PNG exports cannot cover those IDs.

No real sign-in, credential save, endpoint request, or model run was performed in
that native pass. Exact minimum-size native scrolling, IME, token refresh,
notification delivery, and full VoiceOver acceptance remain separate checks.

### Settings by scope

Source builds split settings by scope: a thread's under its message box, a
project's on its own page, and the app's in Settings. Desktop tests check that
Project settings… for a project that is not active makes it the active one and
then opens its page, that it opens nothing while a thread is being created,
that the same project opens without changing checkout, that the menu bar's
**Project Settings…** opens the active project's page, and that Cmd-comma over
that page opens Settings rather than closing it. A Pi thread's run-settings
menu lists only its three approval choices, even when the project's commands
are trusted. A unit test keeps the Settings tabs' order and their element IDs
in step. A core test keeps the explanation of the default for edits and
commands from promising a sandbox: Settings describes no one project, and the
project's own page says what its commands can reach. Wide and narrow scenes render Settings → New threads (including the
default harness), Settings → Agents, the project's page with the command
policy and its Pi setup (`project-settings`, `agent-settings`), the Sandboxed,
Trusted, and confirmation states there (`policy-*`), and the project's sidebar
menu (`navigation-project-actions`). All of it uses injected host replies.

None of this has been checked in the installed app: not the menu bar item, the
sidebar menu with a real pointer, the header while a settings page is open, a
real policy change saved from the new page, or VoiceOver on either page.

## Paged conversation history

Source builds with a version 3 host have core tests for history/live seams,
duplicate IDs, full fields replacing clipped tails, late pages against newer
snapshots, reasoning-only rows, UTF-16 continuations, explicit oversized text
parts, memory eviction, and retained list identity when prepending. A scripted
owner-only socket test delivers history after the selected thread changes and
checks that the reply is discarded.

The scene walk includes full stored text, loaded older history, loading, text
parts, collapsed and expanded reasoning, selectable reasoning, live thinking,
retry, and ended run states at both window sizes. It dispatches wheel and key
events, selects and copies reasoning, and checks that prepending preserves the
list's item and pixel offset. These are windowless GPUI checks. Native trackpad
feel during page delivery, VoiceOver disclosure behavior, and live-provider
history timing still need native acceptance. No provider calls are needed by
these tests.

Turn records have core tests: a record attaches to the answer that closes a
turn and counts only the paths its finished edits changed, a turn with no tool
calls has a record with nothing to count, a running turn and a turn whose
request is outside the loaded rows have none, and a record moves into its
answer's row when a later message ends the rows. Scenes draw the turn line,
the closing nodes, and the state line in both themes and at both window sizes,
and the scrolled-back scenes show the line broken above the state. Three
things are not exercised by a test or a scene: the row being measured again
when its record moves, the broken line while older history pages are being
read, and a failure message long enough to wrap in the state line.

## Known gaps

The [native acceptance check](#native-acceptance-check) below is implemented,
but **has not yet been run against a window**. Its windowless checks do not
close any of these native gaps.

- Gatekeeper approval, installed authentication, and a live-model source change
  on a user-managed Mac need separate acceptance. CI does not replace them.
- Drag-and-drop installation: first-launch setup's decisions (fresh Mac,
  running or manual host, installed-but-stopped release, source build and
  development store, DMG and translocated launch, legacy install, missing
  bundled host) and its symbolic-link refusal are unit tests in
  `plait-core::update::setup`. The v0.1.0-preview.26 release build made the
  new DMG and checked the mounted image: the app's code signature, its bundled
  host, and the Applications shortcut. Registering the LaunchAgent on first
  launch, the cross-launch lock, and the setup line in the app have not run on
  a Mac with no host. A real downloaded DMG, dragged to Applications with
  quarantine intact and opened on such a Mac, is still needed.
- The bundle targets macOS 15+, but CI currently runs on macOS 26. Older-system
  runtime behavior, Intel macOS, and a native Linux client are not validated.
- Full VoiceOver, keyboard focus, IME, and sustained typing/streaming checks
  remain open for the installed release.
- Text selection, link clicks, and hover-revealed copy controls have not been
  checked with a native pointer in the installed app. Links inside conversation
  text are not reachable from the keyboard.
- The released preview has bounded conversation/tool previews. Source builds have
  a paged history view, with native and live-provider history acceptance still
  open. There is no published ADEMark result.
- v0.1.0-preview.7's desktop changes (wrapping tool rows, resizable side
  panels, Cmd-L, the run-settings menu and per-thread approval) are covered by
  development tests and headless scene snapshots. Dragging a panel edge, hover
  text, and the menu have not been checked with a native pointer in the
  installed app.
- Git staging, unstaging, committing, and History (after preview.7) are covered
  by core tests against real temporary repositories (renames, deletions, an
  unborn branch, hooks, filters, a held `index.lock`, paging) and by scene
  snapshots of Changes and History. They have not been exercised with a native
  pointer in the installed app, with Git LFS, or with signed commits.
- The changed-files list is covered by a host test of `changed_files` (a
  written file is listed, a read one is not, unknown threads and version 2
  requests are refused), by Git tests of the line counts (staged and unstaged
  together, deleted, new, binary, ignored, and clean files, and a repository
  without commits), and by a desktop test that counts real files, hides the
  card, and opens a file in Changes. It has not been watched live in the
  installed app.
- The line of controls under the composer is covered by unit tests of its
  texts (the Permissions hover leads with what the mode allows; a harness
  using another mode, and a Pi run under another approval mode than its
  control names, are each one amber sentence) and by scenes of the idle,
  running, reported-mode, and new-thread composers. Scenes do not draw hover
  text, so the sentences moved into hover are checked as text only, and none
  of this has been checked in the installed app.
- The Deny once and Allow once shortcuts are covered by action-table tests
  (both defaults need Command and Option, take no composer or other default
  key, and match the table in [macOS](MACOS.md#application-shortcuts)) and by an
  isolated desktop test that dispatches keys through GPUI. They answer when the
  card's buttons would, and do nothing with no approval, while disconnected,
  with a control request in flight, once answered, with arguments that cannot
  be approved, behind Settings, Changes, or an open command palette, or for
  another thread. A repeated keystroke with no release between answers
  nothing, and a rebound key answers. No host runs in that test, so it checks
  that an answer was sent, not the host's reply. The keys have not been
  pressed in the installed app: real key repeat, key release while Command is
  held, the menu items, and choosing either command in the palette are
  unchecked there.
- Asking about diff lines is covered by unit tests of the quote (line ranges,
  removed-only picks, header lines left out, fences longer than any backtick
  run, long picks summarized) and a desktop test that picks lines in a real
  diff and quotes them into a composer. Native pointer clicks on line numbers
  have not been exercised in the installed app.
- Diffs that start at the change are covered by unit tests of the row mapping
  in `plait-core::patch` (a modified, new, deleted, renamed, mode-changed, and
  binary file, a quoted path, a commit with several files, a file with no
  hunks, and a removed line that reads like a header), by a test against a
  real repository whose commit holds each of those changes, and by scene
  snapshots of Changes, a picked range, a hovered hunk, and a commit with two
  files. The hunk header's context text in secondary color is covered only by
  a unit test of where its ranges end: the fixture's hunks start at line 1,
  so no scene shows a function name after `@@`. Paths written with
  `diff.noprefix` or mnemonic prefixes are named from the `diff` line only in
  part, and none of this has been checked in the installed app.
- Hunk staging is covered against real repositories (staging and unstaging
  one of two hunks, refusing a hunk that moved, refusing filtered files, and
  byte-exact patches with tabs, CRLF, and missing final newlines).
- Discard is covered against real repositories (a file keeps its staged side,
  one hunk of two, an untracked executable deleted and restored with its mode,
  and a restore that refuses to write through a symlink). The native
  confirmation alert has not been exercised in the installed app.
- Push, publish, and fetch are covered against a local bare repository
  (publish, push, fetch revealing a remote commit, a fast-forward pull, a pull
  refused because it would overwrite an uncommitted edit, and refused diverged
  push and pull). Pushing to GitHub over SSH or HTTPS with `gh` sign-in has been checked
  only by asking Git for credentials, not by a push from the installed app.
- Creating and switching branches are covered by core tests against real
  temporary repositories (create and switch back; a switch that would
  overwrite an uncommitted edit refused with Git's message and nothing
  changed; a non-conflicting edit and an untracked file carried; names with
  spaces, `..`, a leading dash, `@{`, and an existing name refused; names with
  slashes; a detached HEAD; a repository with no commits; a held
  `index.lock`; the list's order, 500-branch cap, and current mark; a branch
  checked out in another worktree, which Git refuses), by state tests that
  only the newest list lands and that a switch reloads history, by desktop
  tests that drive the palette with real keys (listing, the Create row only
  for a new name, switching, creating, an invalid name) and refuse both writes
  while another Git write is queued and while a thread runs in the checkout,
  and by scene snapshots of the control and palette at both window sizes.
  The control has not been used with a native pointer in the installed app,
  its hover text has not been seen there, and no switch has run hooks or LFS
  checkout filters from it.

- Per-thread worktrees are covered against real repositories (refused before
  a first commit, created beside the repository on a new branch, and removal
  refused while untracked work remains, keeping the branch), by the Mac sandbox
  suite (`git status` and `git log` in a linked worktree succeed, creating a
  branch is denied), and by a desktop test that creates one and requests its
  thread there. A worktree thread has not been run from the installed app.

## v0.1.0-preview.8 acceptance

Preview.8 completes M2.5 in source and scripted tests. These checks need the
installed app, a native pointer, and a live provider, and are open until
recorded here:

- [ ] Streaming text flows in smoothly on a ProMotion display, and the window
      stops redrawing once a reply has caught up.
- [ ] Appearance follows a live macOS light/dark switch; the titlebar matches
      the theme in System, Light, and Dark; Stillwater is the default dark theme.
- [ ] A Base16 file placed in the themes folder appears after Reload, and a broken
      one is named with its reason.
- [ ] The changed-files card lists a running thread's writes with counts, and a
      row opens the file in Changes.
- [ ] Clicking diff line numbers and **Ask about this** quotes them into the
      thread's message.
- [ ] **New Thread in Worktree** runs a thread on its own branch, and removing it
      refuses while work is uncommitted.
- [ ] Pull fast-forwards from Changes and refuses a diverged branch.
- [ ] The sidebar's filter chips, bell, and refresh controls work with the pointer
      and show their names on hover.
- [ ] Cmd-P opens a file in the Files view; Cmd-F, Cmd-G, and Cmd-Shift-G find
      and step; a file an agent edits rereads when the window is focused again.

## Native acceptance check

The repeated `turn-undo-redo-shortcut` failure came from the desktop's shared
shortcut repeat guard: it latched Cmd-Option-U until a key-up or modifier change,
so a second fresh press without those events was discarded even with Redo ready.
The guard now uses the key-down event's platform repeat flag. A key rebound to
a shortcut without Command still waits for its release, because macOS reports
its repeats as new presses but does deliver its key-up. The same action,
record target, refusal messages and host operation serve both presses. A headless
regression reproduces the stuck Redo before the fix and checks consecutive presses,
held keys and mixed control/shortcut use. The unchanged item should pass on the
next CI native acceptance run; that real-window result remains unverified here.

`scripts/acceptance-macos.sh` drives a separate development copy of Plait.
**The first native run passed 4 of 42 items, failed 34, and skipped 4.** The
fixture, permissions, full-screen round trip and sidebar toggle passed. Thirty
failures came from the check mistaking a sidebar's static “Done” status for
the Settings Done button. Four pointer items were refused by a rectangle-based
ownership check. The window was present, permissions were granted, and its
984 × 639-point frame was too narrow for the Git inspector.

The first repair matched Done by button role and established window, view,
thread, draft and disposable checkout state before each item. It made commit,
handoff and approval prerequisites independent, stopped unfinished fixture
runs before resetting their checkout, and added three recorded-run Undo/Redo
items. Inspector checks now size their own window and skip if it remains
below 1150 points.

**The second run passed 20 items, failed 19, skipped 1, and was cut off before
the last 5 items.** Passes covered fixture isolation and native permissions;
the sidebar toggle; Changes and History tabs; opening Settings and its Providers,
Keyboard and Appearance pages; project settings; the command palette;
conversation find; switching and creating branches; committing a staged hunk;
editor and terminal handoffs to inert recorders; long-message typing; and both
approval shortcuts. These are real-window observations, not inferred coverage.

Fourteen failures said only “timed out; last observed: 0”. That message came
from tab or thread selection waits, including the shared reset. The log cannot
identify the particular selector for each item. Passing sibling tabs and
settings pages use the same app role and activation handler; they do not prove
which transition failed. Four pointer failures reported an unknown mouse
target and a screen-sized Dock-owned rectangle at layer 20 above the development
window. That looks like a system overlay, but a rectangle cannot establish
whether it receives clicks. The remaining failure observed full screen false
after entry had appeared successful. The entry setter was unchanged; the first
repair added window normalization before every item. Without a state trace,
the reason for the transient result cannot be established.

The workflow's eight-minute limit stopped the run after approval-deny. Streaming,
the three Undo/Redo items and the link skip were never reached. Only the log
was uploaded: captures were written to the disposable folder and copied into
the upload folder only on exit, which the external termination bypassed.
There are no second-run trees or screenshots to compare with passing states.
The first-run trees show real control names and roles; neither run establishes
an app accessibility defect that warrants an app change.

The second repair makes ordinary waits three seconds instead of fifteen,
reports the selector and named last observation, and selects tabs and threads
idempotently in one helper process. It reads tabs' AXValue and rows' AXSelected,
distinguishes unavailable attributes from false, and services the helper's run
loop while waiting. Full-screen completion requires a fresh window's attribute
to remain at the requested value for 1.5 seconds. Pointer checks initialize the
helper's AppKit connection without activation and resolve the actual mouse-down
window ID directly, without filtering that answer through a rectangle list.
AX top-left points are converted to AppKit bottom-left points without pixel
scaling. The development window ID and accessibility owner must both match;
Dock, another window, the menu bar and unknown targets are still refused.

**Runs with thread tabs.** One run on a real display against the release
build, and one on CI against the same code.

- **Real display, release build:** 40 passed, 4 failed and 2 skipped (40 of
  the 44 attempted items). Every workspace view, every Settings page, the tab
  row and its shortcuts, Find, branches, staging and committing, handoffs,
  long-message typing, text selection, approvals, streaming (41 distinct
  lengths) and closing-line Undo/Redo passed. The fourth failure is the known
  defect that Redo by shortcut straight after Undo by shortcut does nothing
  (`turn-undo-redo-shortcut`). The other three were the bar pointer items
  (`drag-sidebar-bar`, `drag-header-bar`, `double-click-system-setting`). In
  that run the window also ignored placement: it stayed at 1780 × 1129 points
  after a request for 1000 × 650. So these three failures do not show that the
  pointer was in the wrong place, and the pointer change below is not expected
  to fix them on that display. A debug build of the same code stopped at
  Settings → Editor on a framework assertion about a duplicate accessibility
  node, and the runner then failed each of the 24 remaining items with
  `development app is not running`.
- **CI:** 23 passed, 19 failed and 4 skipped (23 of 42 attempted). The skips
  were the two inspector items on the 984-point window, Show in Finder and
  following a conversation link. Sixteen failures were `reset` selection
  timeouts on `Files` (4) or `Conversation` (12), as in the two CI runs before
  tabs (16 each, plus one `Changes` timeout outside the reset). In every
  case the control was present, `AXPress` succeeded and `AXValue` stayed
  false for the full three seconds and in the dump taken just after. Passing
  selections finished within 0.45 seconds, almost all within 0.23.
  - **All three CI runs:** a reset timed out in 48 of the 73 items that
    followed a pass or a skip, and in none of the 54 items that followed a
    failure. After a failure the check takes an accessibility dump, a window
    list and a screenshot of the window. By the next item's first selection,
    the selection that had timed out had landed (16 of 16 in the run with
    tabs). Several failure screenshots still showed the previous view after
    the accessibility tree had changed. The evidence points to a frame that
    stalls on the CI display until something wakes the window. It does not
    yet show what wakes it.
  - **The other three CI failures:**
    - `drag-header-bar` reported that the sidebar button stopped responding.
      The check counted controls 14 ms after the press, and the dump 18 ms
      later showed the sidebar.
    - `streaming-reply` saw a single 30-character `Plait is writing:` label
      that disappeared when the turn finished about 8 seconds later. The
      message said "timed out after 15.0s", which was misleading.
    - The known Redo defect.

**Changed in the acceptance check (not the app):**

- **Pointer items:** There is no separate bar over the sidebar any more. One
  40-point tab row spans the window, and its empty right end moves the window
  and takes the title-bar double-click. `drag-sidebar-bar` and
  `drag-header-bar` become `drag-tab-bar` (sidebar shown) and
  `drag-tab-bar-sidebar-hidden` (sidebar hidden), next to
  `double-click-system-setting`. All three take their point from the
  accessibility tree's own frames instead of fixed offsets from the window
  edge (`tab-bar-end`: right of the rightmost control in the 40-point band,
  20 points down). A full row drags left so the drag stays inside the window.
  The row always keeps at least 48 points free at its right end, even when
  full, so a missing end fails, with its measurements, rather than skipping.
- **Post-drag sidebar check:** waits for `Resize sidebar` to appear or
  disappear, instead of counting once.
- **Reset:** closes Find on the active view before switching. It visits
  `Files` only when the previous check visited it. Both changes avoid view
  switches that test nothing. This lowers how often a reset meets the CI
  stall but does not explain it: the stall can now show up in the item's own
  first view switch, where it still fails. Ordinary waits stay at three
  seconds.
- **Selection and streaming diagnostics:**
  - Selection waits record each distinct state they observe (role, name,
    `AXSelected`, `AXValue`, frame), the sibling tabs and the window's
    focus.
  - After a `select` or `wait-selected` timeout, the check samples the tabs
    immediately, again after 1.5 seconds with no other action, then after
    its screenshot, then after activating the window. The next reset samples
    them around activation, placement and `Escape`.
  - `growth` records timed samples and reports a label that vanished
    separately from a real timeout.
- **App exit:** `axdrive` treats a process that has exited as gone even
  before the runner collects its status, and stops waiting. The runner prints
  the exit status and the end of `app.log` once and fails that item. It marks
  every remaining item `SKIPPED` (`not run: app exited`) and prints
  `incomplete=true`.

**Subsequent CI observations (without forced frames).** On the
v0.1.0-preview.19 tree, 37 passed, 5 failed and 4 skipped: five failures out
of 42 attempted items. Four selections still reported false after 1.5 seconds
of passive waiting and true at the first read after the failure screenshot.
On the v0.1.0-preview.20 tree, 34 passed, 8 failed and 4 skipped: eight
failures among 46 listed items (42 attempted). Six were selection timeouts:
`tab-changes`, `settings-open`, `find-conversation`, `switch-branch`,
`open-terminal-here` and `long-message-typing`. All six have the same
false-before/true-after screenshot evidence. `streaming-reply` saw no
`Plait is writing:` control during its 15-second wait, although the failure
dump already showed the thread Done. Both runs also failed
`turn-undo-redo-shortcut`, the known product defect; that item is unchanged.

The screenshot path reads the AX tree, lists windows, resolves the development
window's ID, then runs `/usr/sbin/screencapture -x -l <window-id> <file>`.
The selection is still false in the dump and true before the later activation
step. There is no AX sample between window lookup and capture, so the evidence
strongly implicates that path but cannot isolate a single WindowServer call or
prove how the app presents the frame. The check therefore uses the capture
itself rather than assuming that activation or a window-list read is enough.

**Changed in the check and CI, not the app:** `PLAIT_ACCEPTANCE_FORCE_FRAMES=1`
is now set only on the workflow's advisory native acceptance step. It is off
by default. An unsatisfied accessibility wait can capture the development
window and read again, with at least 0.5 seconds between capture attempts
across helper processes and waits. Captures use the same window-specific
command as failure evidence; temporary images are removed. Capture work uses
the original wait budget, stops at its deadline, and never substitutes for the
assertion. Capture failures are recorded; they do not turn missing state into
a pass or a skip. The original timeout messages and assertion conditions
remain in place.

This applies to control lookup (`find`, `frame`, `press`, `select`, `focus`,
`focused`, `selected`, `value`, `set-value`, `select-text`), selection completion
(`select`, `wait-selected`), window appearance, absence, stable full screen,
and Python's field-value, window-drag, double-click resize and panel-resize
waits. Growth captures from the first sample and throughout streaming, rather
than waiting until the transient label is gone. It still requires two distinct
label lengths; Done alone fails. One-shot reads and waits for Git, files, host
state, frontmost application or clipboard content do not initiate captures.

Each assisted pass says `after N forced frames`. Every capture attempt and
outcome is kept in the item's step log and `forced-frames.jsonl`, and
`results.json` records the item's count, including reset work. A separate
`FORCED FRAMES` summary counts affected items by outcome and capture attempts.
The count is conservative: an attempted capture may have woken the window even
if the capture failed or reached the deadline. An assisted pass establishes the
requested state after capture assistance; it does not claim the app presented
that state unaided or that streaming was smooth.

**Real Mac checks run without this variable.** They retain the original
read/sleep sequence, result format and failure behavior: a state that appears
only after the failure screenshot still fails. Windowless tests replay those
sequences through the actual Swift wait loops and Python runner, check the
capture rate and deadlines, reporting, and streaming dumps that grow, remain
unchanged or reach Done without growth.

**Unverified until the next CI run:** the new mode has not driven a real
window. Whether these captures unblock each of the six selections and expose
two streaming lengths before Done, capture duration on the virtual display,
and the number of assisted items remain unverified. If the diagnosis holds,
those seven items should pass (with assistance explicitly counted when used),
while the known shortcut Redo defect should still fail. On the narrow CI
display the two inspector items, Finder and following a link should still
skip. The mode adds no claim about real-display frame delivery, VoiceOver or
the installed app.

Failures remain failures, and a broken `AXPress` gets no pointer substitute.

Build the normal development executables and runtime dependencies with
`bash scripts/check.sh`, then run the check when the computer is free:

```sh
bash scripts/acceptance-macos.sh
# Skip bar and panel drags, double-click, and conversation text selection:
bash scripts/acceptance-macos.sh --no-pointer
# Keep machine-readable results and diagnostics in a new directory:
bash scripts/acceptance-macos.sh --artifacts /tmp/plait-acceptance-results
```

It requires the repository's normal macOS development tools (Python 3, Git,
Xcode's Swift compiler, and Node for the development host). It installs no
additional packages. `--app /path/to/plait` and `--host /path/to/plait-host`
select already-built executables, including extracted CI packages. The
terminal or runner needs **Accessibility** permission; macOS may also list
the compiled `axdrive` helper. **Screen Recording** permits failure captures.
The check never grants permissions or changes the system double-click setting.
It uses no Apple events, so it needs no Automation permission. A missing
Accessibility grant skips the native items with a reason. An inaccessible
window after permission was granted fails setup. Skips are not passes.

The normal run announces that it takes the pointer for a few minutes.
`--no-pointer` still brings its own window forward and sends native key events;
it is not a background check to run while typing elsewhere. Controls are
pressed with the accessibility API, keys go to the launched PID, and only the
six pointer items below post pointer events. Before a pointer gesture the
helper checks that the mouse target and accessibility hit belong to its own
window, then restores the
pointer's original position. The text-selection check temporarily uses the
clipboard and restores every representation it could read; clipboard contents
are never written to diagnostics.

Isolation is independent of the executable name. A private temporary folder
contains a copy of the desktop executable, `PLAIT_HOST_STORE`,
`PLAIT_DESKTOP_STATE_DIR`, `PLAIT_DESKTOP_PREFERENCES_FILE`, a disposable home
and Pi configuration, and a Git repository with two branches and two changed
hunks. The app and host receive a small environment without inherited provider
credentials or Git overrides; Git ignores system/global configuration, uses a
test identity, and has signing and hooks disabled. The current system
double-click choice is read once and mirrored into the disposable home's
global preferences, so isolating CF preferences does not change that choice.
Nothing is installed, and
no LaunchAgent is registered. Only the owned PIDs/process group are stopped.
The person's installed Plait may remain running.

Conversation data comes from the **production host and Pi runtime**, talking
to a scripted Anthropic-shaped SSE endpoint on loopback. Its event format
follows `runtime/test/support/replay.ts`; journal seeding follows
`runtime/test/support/installed-ui.ts`. A fixture model and a non-secret key
exist only in the private Pi directory. A real host thread holds the scripted
reply, pending edit approvals, and a deliberately slow stream. The desktop
reads it through the normal socket protocol. No sample view, hidden app mode,
real model, provider credential, or external service supplies the transcript.

Each row below corresponds to a reported item (the four workspace tabs and
seven Settings tabs each have separate results):

| Item | What must be observed | Pointer |
| --- | --- | --- |
| Fixture isolation | Private host and desktop state, test Git identity, scripted conversation | No |
| Native setup / permissions | PID-owned window and seeded assistant message; permission state recorded | No |
| Traffic lights, sidebar hidden | All three light frames centered within 3 points of the 40-point bar, first about 14 points in, sidebar toggle at least 4 points clear | No |
| Thread tab row and keys | Tab row stays visible; Cmd-T opens a draft, Cmd-1/Cmd-9 switch, and Cmd-W closes the empty draft tab | No |
| Drag tab bar | Tab row's empty right end (from AX frames) moves the window with sidebar shown; sidebar toggle still responds; fails if no empty end | Yes |
| Drag tab bar, sidebar hidden | Tab row's empty right end moves the window with sidebar hidden; sidebar toggle still responds; fails if no empty end | Yes |
| Double-click tab bar | Current system setting on the tab row's empty right end produces minimize, no movement, or resize; setting never changed; fails if no empty end | Yes |
| Full screen and back | Native full-screen state changes both ways; header control responds afterwards | No |
| Sidebar toggle and shortcut | AXPress and Cmd-B hide and restore it | No |
| Inspector toggle and shortcut | AXPress and Cmd-L hide and restore it; skipped on a display too narrow for it | No |
| Resize sidebar | Edge moves and new width is saved in private preferences | Yes |
| Resize inspector | Edge moves and new width is saved; skipped on a narrow display | Yes |
| Conversation view | Selected state and message field | No |
| Changes view | Selected state and commit field | No |
| History view | Selected state and fixture baseline commit | No |
| Files view | Selected state and Open File control | No |
| Settings | Cmd-comma opens Settings and Done is available | No |
| New threads settings | Selected tab and Models section | No |
| Providers settings | Selected tab and Find provider field | No |
| Agents settings | Selected tab and Agent harnesses section | No |
| Keyboard settings | Selected tab and Find shortcut field | No |
| Editor settings | Selected tab and custom-command fields | No |
| Appearance settings | Selected tab and Code font field | No |
| About settings | Selected tab and About and updates section | No |
| Project settings | Project's sidebar menu opens its Commands settings and returns | No |
| Command palette | Cmd-K, typing and Return dispatch Changes | No |
| Find in file | Cmd-F; three fixture matches; next/previous shortcuts and buttons; Escape | No |
| Find in conversation | Same sequence against the host's three text matches | No |
| Composer focus after tab switches | Clicked tabs and Cmd-Shift-1/2/3 preserve draft, return focus, and route later shortcuts and Find to the right view | No |
| Switch branch | Picker selection and actual Git HEAD agree | No |
| Create branch | New picker entry creates and switches the disposable branch | No |
| Stage one hunk | First hunk enters Git's index; second remains unstaged | No |
| Commit staged hunk | New commit and expected subject; empty index; second hunk retained | No |
| Custom handoff settings | Typed commands saved to private preferences | No |
| Open in Editor | Recorder receives exact file, current find-match line, and project arguments | No |
| Open Terminal Here | Cmd-Option-T recorder receives exact project argument | No |
| Show in Finder | Control is found; activation explicitly skipped (see below) | No |
| Long message | Native Unicode key events produce an exact long draft, grow the field, and save it without sending | No |
| Conversation text selection | Drag and Cmd-C copy a proper substring of the fixture reply | Yes |
| Allow approval shortcut | Cmd-Option-Y resolves host approval and changes the exact fixture file | No |
| Deny approval shortcut | Cmd-Option-period resolves host approval and leaves the file unchanged | No |
| Streaming reply | At least two lengths of the writing label, followed by final assistant text | No |
| Undo a run | Own scripted edit has a ready host record; closing-line Undo restores exact bytes and offers Redo, leaving HEAD, index and unrelated changes intact | No |
| Redo a run | Own recorded edit is undone through the fixture; closing-line Redo reapplies it and offers Undo | No |
| Undo/Redo shortcut | Cmd-Option-U undoes and redoes a fresh recorded edit with the same file and Git checks | No |
| Follow conversation link | Explicitly skipped because it would open the default browser | No |

The script cannot substitute a custom command for **Show in Finder**:
`plait_core::handoff::show_in_finder` always starts `/usr/bin/open -R`.
It finds that control but does not activate it. Conversation links similarly
open the system browser without a configurable override. Those two actions
remain manual acceptance; this check never opens Finder, an editor, a terminal,
or a browser. The editor and terminal custom commands only write their received
arguments to files. Testing actual editor line navigation, Finder selection,
Terminal/Rex startup and URL following still requires those applications.

Other limits: AX geometry cannot certify drawn pixels or macOS 15 appearance;
AXPress does not prove a mouse click's hit testing; the full-screen check does
not visually inspect the hidden lights; double-click tests only the current
system choice and checks resizing, not exact Fill margins. One long draft and
one short stream do not establish sustained performance, caret/IME correctness,
smoothness, scrolling, hover behavior, or VoiceOver usability. Settings checks
verify navigation and named contents, not provider sign-in or every control.
Live models, authentication, recovery, Stop, updates, signed commits, hooks,
Git LFS and older systems remain separate checks.

Output has a `START` line and one `PASS`, `FAIL` or `SKIPPED` result for each
attempted item, plus a summary covering all 46 items. The check has its own
390-second deadline, including compilation and setup, within the unchanged
eight-minute workflow limit. A deadline, termination signal, or mid-run app
exit records the affected item (printing the exit status and the last lines of
`app.log` once when the app dies), marks unattempted items as skipped with the
reason (`not run: app exited` after a crash), prints `SUMMARY ...
incomplete=true`, and exits nonzero. Reducing fourteen exhausted selection
waits from fifteen to three seconds saves at least 168 seconds; the remaining
margin allows evidence capture and cleanup. A hard kill cannot print a summary,
but evidence already written survives it.

Results, the failure digest, per-item step traces, AX trees, screenshots and
window inventories are written directly into `--artifacts` as items finish;
there is no end-of-run copy dependency. Results are saved before capture begins.
Each failed item attempts an AX dump and **only that PID's window screenshot**
with `screencapture -l`; bounded capture failures get their own reason file.
Trees include raw tab/thread selection flags and window state. Traces distinguish
reset from the item action and record successful transitions for comparison.
The log ends with a `FAILED ITEM` line per failure, naming its first missing
selector, expected state and last observation, or refused point. Pointer
refusals include the raw mouse target ID, its owner/frame, the first covering
rectangle and the frontmost application's owner/frame, without other apps'
window titles. Generated logs and preferences are also retained; host databases
and executables are not copied. Failures and skips retain the disposable folder.
A completely passing run removes it. An all-skipped native run exits zero with
explicit skips unless interrupted.

CI uploads these files and the console log. The check runs in a job of its
own after packaging, on each release build, on the package that run built,
without installing it. It is advisory (`continue-on-error`): publishing does
not wait for it, and its failures do not fail the run.

Verified without a window: Swift compilation and pure selector, flag decoding,
tab-row empty-right-end geometry (recorded one-tab, full-row and wide trees,
a crowded row, missing controls), an exited but unreaped process counted as
gone, wait-trace and growth-trace formatting,
full-screen stability, coordinate and ownership tests; Python tests of named
waits, deadlines and termination, mid-run app-exit reporting and `not run: app
exited` skipping, a missing tab-row end failing, post-drag sidebar waits,
conditional `Files` reset, selection probes taken before the failure
screenshot, immediate artifact
persistence, item ordering, reset after failure, skip reasons, disposable Git
reset, isolation and replay; and `bash scripts/check.sh` (format, Clippy,
workspace and runtime tests, release builds). The fixture's production-host
checks separately cover independent allow/deny effects, stopped-run reset,
recorded undo/redo, growing stream and argument recording without a desktop
process. The existing smoke AppleScript was compiled with `osacompile`, not
executed; this check adds no AppleScript or JXA. ShellCheck was not installed
during this verification. The check's own windowless entry points are:

```sh
bash scripts/acceptance-macos.sh --self-test
bash scripts/acceptance-macos.sh --fixture-test --host target/debug/plait-host
```

`scripts/check.sh` runs the Python tests and, on macOS, compiles the Swift
helper and runs its pure self-test. Neither windowless entry point launches
the desktop, reads an accessibility tree, or sends keyboard/pointer events.
They do not replace the next native run. On a display as narrow as the first
runner, the two inspector items plus Finder and URL activation are expected
to skip with reasons; the other 42 items can attempt their checks. This is an
expectation, not a passing result.

## Manual release acceptance

Once the native acceptance check above has passed its first real-window run,
run it before tagging and inspect every failure and skip, then complete the
remaining manual checks below. Until then it is an unproven advisory check.

Use a disposable checkout and private host/session directories. Do not reuse
another person's conversation, change OS security settings, or inject global
keystrokes into their desktop.

1. Install the downloaded package under normal macOS policy and authenticate
   with an account authorized for the test.
2. Check repository/model selection, native typing and IME, keyboard navigation,
   VoiceOver, and the 900 × 600 minimum window.
   - [ ] The window bar: the traffic lights sit centered on the 44-point bar
         about 14 points in, clear of the header with the sidebar hidden; an
         empty part of the rail strip or header drags the window while its
         controls still click; double-clicking there follows the system
         setting; full screen hides the lights and the header returns to the
         edge. Only where the lights sit with the rail shown has been checked
         in a real window.
3. Ask for a small real source change and test. Review exact approvals, actual
   output, the Git diff, and preservation of unrelated work.
4. Close/reopen during work, then test Stop, provider failure, and recovery.
   Confirm stable identities and no automatic replay.
5. Check independent client/host updates, including cleanup before activation.

Record the build, OS, model/provider, steps, and result. Do not publish credentials,
private prompts, or authentication seeds. Performance claims need separate,
repeatable measurements; see the [ADEMark plan](ADEMARK.md).

## Self-hosting tool and context coverage

Runtime integration tests exercise ranged reads beyond the former whole-file
bound, exact edits guarded by a full fingerprint, Unicode page boundaries,
unsafe paths, and stale edit rejection. Command-output tests cover scope,
retention, explicit size limits, incomplete capture, database reopen, and a
failed real sandboxed command whose earlier diagnostics remain readable after
a host restart without rerunning it.

Real-host tests trigger the pinned runtime's automatic compaction policy with
a scripted provider. They verify that the model keeps the task goal, original
history remains available, internal summaries do not become user messages,
restart during summarization makes no new requests until submission, and a
failed required summary can recover. They establish request sequencing and
storage behavior, not the quality of summaries produced by a live model.

An installed source build was also exercised with a live provider on Plait's
actual Rust and TypeScript code. The run made a cross-language change, added
regression tests and documentation, accepted review corrections in follow-up
messages, and completed a queued correction. Closing the desktop during work
left the host running; reopening restored the selected thread and an unsent
draft. The source installer’s active-run check refused to proceed while that run was active.

With Trusted commands explicitly enabled for that checkout, the agent ran the
ordinary full development gate: 332 Rust tests passed (one image-export test
ignored), 187 runtime tests passed including the macOS sandbox tests, and the
release-script suite passed 23 tests. Two permission fixtures were made explicit
so the gate also passes under the host's private `0077` umask. No production
permission checks or test assertions were weakened. This establishes a live
project-development loop; it does not certify every provider, native input
method, or long-running task. Automatic compaction remains covered by the
scripted tests above rather than this live run.

## Skills and user instructions

Runtime tests discover a user skill, add a project skill only for a trusted
project, follow Pi's precedence and size bound for the user-level context file,
map reads into a skill folder (refusing relative paths, prefix siblings, `..`,
and the agent directory), read a skill file through the host helper, and send
`/skill:name` in Pi's block. Regression tests refuse a skill file swapped for a
link to a dummy credential after discovery, grant a standalone skill in a
scratch `~/.ssh` only its own file, and skip skill folders inside a credential
folder or at the host store. A protocol test shows an expanded message in
history as typed. A real-sandbox test reads a skill folder only when it is
passed, and cannot write it. Test runtimes ignore the machine's own skills.
Template tests check Plait's argument parser and substitution against Pi's own
functions over a set of quoting and placeholder cases, load project templates
only for a trusted project, refuse a template swapped for a link after
discovery and links to non-Markdown or credential files, and keep the
`agent_resources` reply free of file contents and under the 1 MiB frame with
oversized lists. A host test sends `agent_resources` and sees trust follow the
command policy. Desktop tests cover agent command ranking and clashes, and
scenes render the project's settings page and the `/` list from sample state; choosing a
command inserts it. A live model receiving an expanded template in the
installed app has not been checked. Discovery on
a real Pi setup with 66 skills (29 turned off in settings) took 7–20 ms per run.
A live model choosing and following a skill in the installed app has not been
checked.

## MCP configuration and tool calls

`runtime/test/mcp.test.ts` compares accepted servers and rejected entries with
Pi 1.5.2's actual configuration loader: trusted project replacement, restricted
`enabled`/`exposure`/`toolExposure` overrides, namespace clashes, provider-auth
restrictions, and invalid transport and OAuth settings. Tests cover BOM input,
malformed and oversized files, bounded replies, Pi's built-in MCP disable
setting, and saved project choices under both command policies. Store reopen
and directory-identity tests cover persistence and scope. A host protocol test
saves a switch, reads it back, restarts the host, and verifies Pi's configuration
was not edited.

A guarded listing test supplies commands, environment placeholders, secret
arguments, headers, URL credentials and paths, and OAuth tokens while replacing
process and network entry points with failures. Listing calls none of them;
serialized reports contain none of those values. OAuth tests cover normalized
namespace/URL keys, legacy keys without migration, wrong-URL rejection, saved
tokens, and unknown requirements for an uncontacted HTTP server.

`mcp-tools.macos.test.ts` uses a scripted stdio fixture behind the real guardian
and Seatbelt profile, plus a loopback-only HTTP fixture. It checks trusted Ask
and Auto calls, allow/deny decisions, mandatory sandbox approvals even in Auto,
Stop during approval, argument and checkout identity guards after approval,
process-group cleanup, crashes and stderr diagnostics,
10-second startup and configured call timeouts, sampling/elicitation refusal,
collision-safe names, deferred exposure, and bounded output with stored overflow.
The sandbox test proves checkout reads, denied checkout/Git/outside writes and
outside reads, scratch/cache writes, and permitted loopback networking. Header
authentication resolves only during search; a 401 is reported as requiring Pi
sign-in without retrying or exposing its body.

`host-mcp.macos.test.ts` runs the actual daemon and Pi Durable against scripted
model output and the fixture server. It checks discovery, per-call approval,
`mcp:server/tool` in snapshots and history, actual counts after shutdown, and a
switched-off server with no guardian spawn or search tool. Killing the host
kills the server group; restarting and submitting new work does not repeat its
interrupted call. Seeded env/header/command secrets are searched for in tool
metadata, approvals, captured model contexts, snapshots, history, output storage,
and durable database bytes. These checks establish known-value redaction, not
resistance to an arbitrary server deliberately encoding credentials.
Redaction fixtures preserve ordinary login values (including SHLVL, USER, HOME,
LANG, TERM, and PATH), arguments, URL paths/query values, and short configured
values while still removing seeded configured credentials. The eight-character
minimum means shorter credentials are not scrubbed. Tool names with ESC,
U+202E, or U+200B are refused, as are credential-bearing names/titles/schema
keys and constraints; valid tools from the same server remain available. Schema
keys are preserved, and only schema descriptions are rewritten.

The first search makes zero display-label settings writes, including when the
server offers unused tools. Actual calls create one label per thread/tool;
repeat calls reuse it. There is currently no automatic cleanup for these label
settings, including on archive, so they accumulate with distinct tools actually
called across threads. The first-search deadline applies per batch, not to the
whole search: N silent enabled servers with immediate credentials take about
`10 × ceil(N / 4)` seconds. With the credential barrier, mixed slow preparation
and connections have a conservative `20 × ceil(N / 4)` bound, plus shutdown,
filesystem, and scheduling overhead.

Project-consent tests cover new and replacing repository definitions, enable
overrides, and attempts to expose user-hidden tools in trusted Ask and Auto
threads. Before opt-in they assert no MCP/credential guardian spawn, no fixture
server or credential-command marker, no loopback HTTP requests, and no observed
connection. After explicit enable the fixture runs; changing its entry prevents
another start. Missing and stale enable digests are rejected. Unit tests cover
all unresolved launch fields, exposure changes, legacy name-only switches,
directory identity mismatch, and exact/wildcard visibility intersections. Saved
switches contain only a digest and policy metadata, never configured values.

Security regression fixtures check that sandboxed servers and credential commands
cannot reach the actual host socket or another Unix socket outside scratch, while
scratch sockets, `localhost` resolution, and loopback requests by name and address
work. The resolver exception is the exact `/private/var/run/mDNSResponder` socket;
ordinary command sandbox tests remain unchanged. A compiled constructor fixture
proves that a configured dynamic library loads only in the explicitly trusted
server, never its guardian; sandboxed loader settings are refused before startup.
A second native probe deliberately bypasses that validation: its injected
constructor runs only after Seatbelt, can write scratch but not an outside file,
and the final program has no inherited descriptors numbered 3 through 15.
Expansion tests reject sidecar-only plumbing and keys in both policies. Two- and
five-server fixtures test credential preparation before any connection, including
a delayed credential command, and reject another server's credential in metadata.

Additional regressions cover control characters in programs and shared command/MCP
approval details; escaped tool rows in live and full history; 4,000-level JSON;
listing size before serialization and a 32-page cap; a normal call with a burst of
1,000 progress notifications; sustained, 20,000-notification and large byte floods
while another thread calls a healthy server; inherited-property tool
names under user-hidden/restricted exposure; and credential-bearing call arguments
refused before approval or dispatch. The pipe permits a burst of 2,048 stdout lines
and replenishes 512 lines per second. Its byte bucket remains 32 MiB, replenished
at 32 MiB per second across stdout and stderr. Exceeding either budget fails the
server; the sustained fixture emits 100 notifications every 50 ms, below the burst
limit per batch but above the sustained rate. Project config symlink tests
prove a linked final file and, on macOS, a linked `.pi` directory are not followed.

Process-group cleanup does not catch a descendant which deliberately calls
`setsid()` or `setpgid()` to leave the group. This is the same boundary as commands:
a sandboxed survivor remains confined, while a trusted survivor keeps user access.
These tests prove same-group cleanup, not containment of arbitrary descendants.

Core tests decode older hosts and unknown fields/transports and serialize the
version 3 switch request. Desktop tests cover state wording, source details,
and stale connection/checkout replies. `project-settings-mcp-*`,
`project-settings-mcp-observed-*`, `project-settings-mcp-review-*`, `project-settings-no-mcp-*`, and
`mcp-tool-approval-*` and `mcp-tool-controls-*` draw injected metadata and a separate tool/approval row at
1380×860 and 900×600. Headless Metal/Core Text rendering does not prove native
pointer interaction, focus, accessibility, or installed-app behavior.
Desktop tests check **Needs review**, **Changed**, **User limits**, redacted
argument-count hover wording, and that in-checkout configuration uses the
existing Files-tab navigation while external configuration has no open action.

No installed app was launched or inspected for this stage. No real configured
server, external-network MCP endpoint, browser sign-in, or saved OAuth token
was exercised. Those remain separate acceptance work; sign-in is not implemented.

### Release check for MCP tool calls

MCP tool calls shipped in v0.1.0-preview.17; the release check for that tag is
at the end of this document. On 2026-11-18, on the tree first tagged for them,
`bash scripts/check.sh` exited 0 with the Git that CI's fixtures use first on
the path: 632 Rust tests (171 desktop, 439 core, 14 host, 8 helper
integration), 423 runtime tests,
TypeScript checks, and release builds.

Before the release, a reviewer who had not written the code read the MCP
changes and confirmed eleven defects by experiment with fixture servers. Two
were ways out of the sandbox: the network exception given to a sandboxed stdio
server also let it connect to the host's own socket, where it could switch its
project to trusted, and a server's configured environment reached the
unsandboxed guardian. Two could expose a secret: variable expansion read the
runtime's whole environment, and one server's tools were registered before a
later server's credentials were known to the scrubber. The rest were
misleading characters in approvals, a crash on deeply nested arguments, a
flood from one server stalling other threads, a hidden tool offered under a
prototype name, and descendants that leave the process group. An earlier read
had found that a server defined by a repository's own `.pi/mcp.json` was on by
default, that scrubbing rewrote ordinary output, and that a display label was
stored for every offered tool. All were fixed with the regression tests
described above, except the process-group boundary, which is documented as for
commands. The same reviewer then checked each fix against its original
reproduction and found them closed, with nothing new.

None of this used a real MCP server, a real credential, an external network
endpoint, or the installed app. Servers that need a browser sign-in are not
started in this release.

## Agent harnesses and Codex

Protocol tests accept the version 4 operations and fields only with version 4,
keep version 3 snapshots and summaries free of the harness, and read threads
saved earlier as Pi. Runtime tests list harnesses against a scripted check
(ready, not installed, too old, signed out), reuse a recent check, check again
after a new path, and close the server started only to ask. They refuse a
model from the other harness at creation and in `configure_thread`, and map
thinking levels to Codex's efforts. The Codex adapter's own tests check
version parsing and grading, the JSON line transport (handshake, replies,
refused server requests, exit, oversized frames, bounded diagnostics),
translation of scripted turns through the same `update()` Pi threads use, and
approval wording and replies.

End-to-end runtime tests drive Codex threads through a scripted fake
app-server that echoes the settings it is sent: an Ask turn's command
approval, a declined edit that lists no changed file, a second run that
resumes the same Codex thread, the Full access default sending `never` and no
sandbox, Approve for me sending Codex's reviewer, a mode change during a run
that the next turn uses, the default mode setting, a Codex held to another
mode reporting it, a steering message joining the running turn, Stop sending
`turn/interrupt`, Codex exiting mid-turn ending the run with a fixed error,
history read live and again after a restart without starting Codex, and every
server closing once idle. The adapter's message types were checked against
`codex app-server` 0.169.0 with a handshake that starts no thread or turn
(initialize, model list, account state, and refusals).

Opt-in live tests (`PLAIT_LIVE=codex npm run test:live` in `runtime/`) run real
Codex turns through the runtime with the installed Codex and its own sign-in,
on Codex's smallest listed model. They passed against Codex 0.169.0 on 2026-11-17:
Full access running a command without asking and reporting `full_access`, a
second run recalling the first, Read only asking before a write and the
approved file landing, a steering message joining the turn and shaping its
answer, Stop ending a running command within 15 seconds, and history reading
after a restart without starting Codex. They assert structure, not wording,
and are not part of `npm test` or `scripts/check.sh`. Ask and Approve for me
have not been run live, nor has what Codex's reviewer decides.

Measured on a replayed stream of 7,081 Codex events (20 answers and 20
commands, after 100 earlier entries): translation cost 0.6 µs per event, and
rebuilding the snapshot cost 0.16 ms on average and 0.71 ms at worst, under the
5 ms per event target even if every event rebuilt it. Streaming updates are
also coalesced to one per 16 ms and are not written to disk. Frame cost in the
desktop while a Codex thread streams has not been measured.

## Claude Code

Protocol tests accept `set_up_harness`, the `claude` harness, and its modes
only with version 4, and refuse setting up any other harness. Setup tests
build archives in the test and serve them through a stand-in for `fetch`, so
nothing is downloaded: a hash mismatch, a registry error, a truncated archive,
a path that climbs out with `..`, a link, an archive missing the license, and
a compression bomb each install nothing; a good archive leaves exactly the
four kept files in place with no staging folder; a second setup downloads
nothing; and a changed `sdk.mjs` is refused at load and replaced by setting
up again. A test checks that the pinned URL and `integrity` are the ones
`package-lock.json` records, that the SDK is a development dependency only,
and that the dev copy's `sdk.mjs` has the pinned SHA-256. The tar reader was
also run on the real 0.3.326 tarball, whose SHA-512 matched the pin, and setup
with that tarball finished in about 20 ms.

Runtime tests drive Claude Code threads through the SDK's dev copy and a
scripted stand-in for `claude` that follows the SDK's control protocol and,
like Claude Code, asks nothing in Auto or Bypass and nothing for edits in
Accept edits. They cover: the entry listed as `setup_required` before setup
with nothing looked for or started, threads refused then, and a run refused
once the copy is gone; setup through the operation, then the normal checks;
an Ask turn's command approval, its transcript, usage, and reported mode, and
history read again after a restart without starting Claude Code; Auto, the
default, never asking; a second run resuming the same session; a mode change
during a run reaching Claude Code and `reported` following it; a steering
message placed once in the same process; Stop interrupting, cancelling what
was queued, and running nothing after; an edit listed as changed; Claude Code
exiting mid-turn and a lost session each failing with a fixed message; and a
signed-out Claude Code, a model it does not offer, and another harness's model
or mode refused. The adapter's own tests check version grading, sign-in
parsing, the model list, approvals and their replies, the transcript
translation through the same `update()` Pi threads use, and the session
against the stand-in.

Checked against a real Claude Code 2.1.326 without sending any message: the
version and sign-in checks, the harness entry (ready, 14 models, about 0.26
seconds including the SDK load), every flag a run starts it with, mode changes
in every mode, and interrupting with no turn running. Opt-in live tests
(`PLAIT_LIVE=claude npm run test:live` in `runtime/`) run real turns through
the runtime on the smallest model Claude Code lists, with its own sign-in:
Ask asking before a command and the approved command running, a second run
recalling the first, a steering message placed and answered, a mode change
during a run that Claude Code reports, Stop ending a turn within 15 seconds,
and history reading after a restart without starting Claude Code. They passed
against Claude Code 2.1.326, signed in with a Claude subscription, on
2026-11-17. On one earlier run the model answered the Stop case without
starting its command; the test now says so instead of timing out. Plan, the auto mode's own safety decisions, and MCP tool
approvals are not covered by any live test.

Measured on a replayed stream of 4,202 Claude Code events: translation cost
about 0.15 µs per event, and translating and rebuilding the snapshot on every
event about 52 µs per event, under the 5 ms target. Streaming updates are
coalesced to one per 16 ms and are not written to disk.

## Files

Core tests read text, binary, non-UTF-8, and oversized files; refuse symlinks,
paths outside the checkout, directories, missing files, and a FIFO; split CRLF
and final lines; notice a changed stamp; and find with letter case ignored
inside and outside ASCII, no overlaps, and the 1,000-match cap. Desktop tests
expand tabs and clip long lines on character boundaries and draw matches in the
conversation's neutral fill, the current one underlined, and a harness test
opens a real file, finds in it, finds the kept query again on reopening,
rereads it after an edit on disk, keeps the last version after deletion, finds
non-ASCII text in either case, describes a binary file, and lists files in
quick open. Scenes draw the tab, find with the current match apart from the
others, and quick open. In the frame bench a 20,000-line highlighted
file drew in 0.6 ms at rest and 1.2 ms per frame while wheel-scrolling. Native
pointer use, find shortcuts, and reloading after an agent's edit are open in the
[preview.8 acceptance](#v010-preview8-acceptance) list.

## Ways out

Core tests split custom command lines with single quotes, double quotes, and
backslashes, refuse unclosed quotes, a trailing backslash, line breaks, a
missing program, unknown placeholders, an editor command without `{file}`, and
a terminal command with anything but `{project}`, and fill placeholders inside
each word in one pass. The two escaping layers are tested on paths with spaces,
single and double quotes, backslashes, `$`, backticks, line breaks, tabs,
non-ASCII text, shell punctuation, a leading dash, and an empty path. On a Mac,
`/usr/bin/osascript` reads each AppleScript literal back unchanged (`return`
only, which reaches no application), and the full Rex command line, read back
the same way and run by `/bin/sh` and `/bin/zsh` with `printf` in place of
Neovim, gives every word back whole. An ignored test compiles both Rex scripts
for every such path with `osacompile` against Rex's dictionary; it compiles and
does not run them, and needs Rex installed. A preferences test round-trips each
choice, reads an older file as the system default and Terminal, refuses an
unusable custom command, and reads a hand-edited one as the default.

Desktop harness tests record the command each action would start; test builds
never start one. They check the file and line picked in Files (the first line
on screen, after scrolling, a line asked for but not yet drawn, and the current
find match), in Changes (the first change, a picked line, a staged diff that
begins with a removed line, and a deleted file that has nothing to open), the
Option-Command-E shortcut, Show in Finder and Open Terminal Here arguments,
the Rex scripts, the hover text, and a saved command that no longer reads. A
Settings test refuses an unusable command, saves a usable one to the test's
own preferences file, and shows it again on reopening. Scenes draw Settings →
Editor with ready choices and with custom commands, and the Files and Changes
headers with their Editor and Finder controls.

None of this has been run against a real editor, Finder, Terminal, or Rex:
whether Rex opens the tab and Neovim lands on the line, what each app does
when it is not running, and the two-second failure window are unchecked.

## Find in the conversation

Core tests match with letter case ignored, inside and outside ASCII, with
ranges on character boundaries; find several matches in one block, without
overlaps; keep matches in conversation order across blocks and messages with
each one's row; skip list markers, the streaming reply, reasoning, and tool
output; return nothing for an empty query; stop at 1,000 matches, keeping the
newest; and step next and previous with wrap-around. Desktop harness tests
check that Cmd-F opens the conversation's bar and not the Files bar, and the
reverse in Files; that Escape closes it and returns focus to the composer; that
a tab that hides the composer, chosen by shortcut or by click, takes the
keyboard from it so later shortcuts and Cmd-F in Files work, and that coming
back by shortcut gives the composer the keyboard again with its draft and
caret; that
a result for an older query or an older projection is dropped and the search
runs again; that next and previous scroll each match into the list, across
rows far apart and wrapping at both ends; and that a new reply is searched
while the current match and scroll position stay. Scenes draw the bar with
matches highlighted, with no matches, and with earlier messages not loaded, at
both window sizes. In the frame bench (release profile, November 2026), the
3,750-row conversation drew in 1.13 ms per frame at the median with 750
matches highlighted, against 1.02 ms without find, and a new query searched
every row in the background and drew its result in 4.6 ms at the median.
The scene harness gives "loaded messages only" its state
directly; reaching it through a version 3 host's paged history, and the hover
text, have not been checked in the installed app, nor have Cmd-G from the menu
bar and VoiceOver's reading of the bar.

## Themes

Core tests parse both published Base16 layouts, name what is wrong with a broken
file, read a themes folder, keep older preference files loading, and check every
shipped theme against the contrast floors (text 7:1, secondary text 4.5:1, hints
and accents 3:1). Snapshot scenes draw the conversation, Changes, and theme
settings in Plait Light, and the settings in the default Stillwater; every
other scene draws in Stillwater, and one draws Plait Dark. Following a
live macOS appearance switch, the native titlebar in each appearance, and
reading a user's themes folder have not been checked in the installed app.

The code font is covered by a core test (an older preferences file keeps the
system font, a named family round-trips, a blank name is no choice), by
desktop tests of the name matching (a family is found whatever its case;
offered names lead with those that start with what was typed, and leave out
system families), and by two desktop tests on a Mac that read the family code
is set to draw in and the preferences in memory. One types into the setting: a
family that is not installed changes neither, `menlo` selects Menlo, a
half-typed name keeps the last choice, Helvetica is refused because its
characters differ in width, reopening Settings shows the choice again, and an
empty field returns to the system's font. The other opens Settings before a
preferences file that names Menlo is read, as can happen at launch: Menlo is
then selected, shown in the field, and still in the file. Every scene draws
code in the system's monospaced font, and the Appearance scene shows the empty
field. The tests do not look at drawn pixels. The offered names as drawn, a
font installed or removed while Plait runs, and the setting in the installed
app have not been checked.

## Window bar

Desktop unit tests check that the traffic lights' offset centers them on the
44-point bar, that the header clears them only while the rail is hidden and the
window is not in full screen, and which presses may move the window: a press a
control took never does, a second press is a double-click. A desktop test sends
pointer presses through GPUI's dispatch: the rail's empty strip, the space the
header leaves under the lights, and the header's empty middle arm a move; the
rail's resize handle and the sidebar toggle do not, and the toggle's click
still works; in GPUI's test full screen the toggle returns to the edge. Every
release disarms. The test platform cannot move a window or perform the
system's double-click action, so the test stops at arming. Scenes show the
rail's top strip empty, where macOS draws the lights.

One capture of a development build's window, on macOS 26 with the rail shown,
shows the traffic lights where they are meant to be: about 14 points in, their
centers level with the header's title, and ending 10 points short of where
the header starts when the rail is hidden. Nothing else has been checked in
the native window: the lights on macOS 15, the header with the rail hidden,
dragging the window from the strip and the header, the double-click action,
and full screen are open in the
[manual acceptance](#manual-release-acceptance) list.

## Measuring frame cost

```sh
bash scripts/bench-frames.sh          # release profile
bash scripts/bench-frames.sh --debug  # quick, unoptimized
```

The bench draws generated heavy scenes (2,000 changed files, 20,000-line
patches, 5,000 commits, a 1,500-message thread) and times each frame and each
action-to-next-frame switch, reporting min, median, p95, and max. A frame is
`Window::draw` on GPUI's test platform with Core Text shaping: render, layout,
prepaint, and paint into a scene. These are headless CPU measurements. They
exclude GPU submission, presentation, real input latency, and the installed
app's process, so they do not establish the 60 FPS or 50 ms targets in the
[roadmap](ROADMAP.md) for the shipped app.

On an Apple Silicon development Mac (release profile, November 2026), every
heavy scene drew in under 3 ms per frame, tab and cached file or thread
switches took under 5 ms at the median (12 ms p95 for a 1,500-message thread),
and selecting a file whose diff is not cached took about 34 ms, almost all of
it the background Git read. Warm launch was not measured.

The streaming row draws frames 8 ms apart, as a 120 Hz display asks for them,
while a burst of about 10 KB in short paragraphs is revealed. Each frame took
1.6 ms on average and 3.4 ms at p95, inside the 8.3 ms a 120 Hz frame allows.
A scripted test checks that streamed text trails, catches up within a bounded
number of frames, and then stops requesting frames. How smooth it looks on a
real display, against a live provider, has not been checked in the installed
app.

## Checkout recording

Per-turn undo shipped in v0.1.0-preview.15. It was built in three stages, each
checked on its own: recording, the host's undo operations, and the desktop
controls. Stage 1 introduced internal run records without undo, record-listing
operations, protocol version 5, or desktop controls. The later
[stage 2 checks](#turn-undo-host-operations) cover the host operations, and the
[stage 3 checks](#turn-undo-desktop-controls) the desktop and the release. The
checks below are source-level evidence, not installed-app acceptance.

The real-repository core tests cover additions, deletions, mode-only changes,
created-then-deleted files, binaries, symlinks, exact CRLF bytes, an LFS-style
clean filter, and precedence of private conversion attributes over checkout
and repository-info attributes. That attribute precedence was an assumption in
the design; the byte comparisons and a filter-execution marker confirm it.
Tests also cover unborn HEAD, moved HEAD, linked worktrees, SHA-256 stores,
held index locks, split indexes, assume-unchanged entries, force-tracked ignored
files during full hashing, ignored directories, nested repositories,
uninitialized submodules, and directories that become repositories or gitlinks.
Configured excludes and repository-info excludes retain their precedence.
They exercise the 1 MiB, 2,000-file, 64 MiB, and 100 MiB limits, including growth
between records and clean oversized entries copied from an index. Coverage
lists exceeding 10,000 omitted files are retained in full. Parser tests cover
malformed status/diff records and unusual names; a concurrent writer exercises
change detection and subsequent recapture.

Read-only tests compare every path, size, mode, modification time, change time,
inode, content, and symlink target in disposable repositories before and after
recording and pruning. The host suite runs the real sandboxed helper and injects
a Git wrapper that attempts to touch the person's index: Seatbelt refuses the
write while the helper completes. Split-index tests exposed timestamp updates
in Git's ordinary index reader even with optional locks off; the recorder now
reads private copies of split-index files. Other tests prove only each run's
pair is linked, changed before-content is privately owned, expired refs and
packed content are reclaimed, privately owned before-content survives retention
and removal of the source object store, and a new record works after pruning the latest
checkout cache.

Runtime tests use the existing scripted Codex and Claude Code processes and Pi
replay through the real host. Each harness gets before/after pairs with the
expected changed paths, including stopped and failed runs. Steering shares the
active pair; queued follow-ups receive independent pairs. Tests kill the host's
runtime between records, recover the missing after record, mark it as taken
after restart, and defer recovery behind another checkout writer until the
next leased run. Failed-before tests prevent each harness from running; Codex
also exercises a held follow-up queue and failed-after state. Old protocol
error mappings, startup pruning, archive retention, active-run retention, and
pair retention across the cutoff are checked.

`bash scripts/check.sh` exercises formatting, workspace clippy with warnings
as errors, Rust tests, the host build, locked runtime installation, TypeScript
checking, runtime tests, and the workspace release build. The final gate result
and cost figures below apply to the stage-1 source in this change. On macOS the
final script exited 0: 525 Rust tests and 329 runtime tests passed, with five
opt-in Rust tests excluded from the ordinary gate (including the two timing
tests run separately below). The workspace release build also passed.

### Cost measurement

Run `bash scripts/bench-records.sh` to reproduce the timing fixtures. The target
is a warm unchanged 50,000-file record below 300 ms on Apple silicon. Each
fixture uses small source files in nested directories in a temporary checkout;
Git's index and filesystem caches are warm for seven unchanged records. Timings
are release-build wall times. A pair changes one file between records, outside
the measured helper time. The first pair also pays for newly written private
trees and copies of earlier content.

Measured on 2026-11-18 with Apple M5 Pro, 18 CPU cores, 48 GiB memory, macOS
26.6.2, Apple Git 2.54.0, Rust 1.98.1, and Node 26.12.0. The full check suite had
finished before this run; the machine was not otherwise isolated for benchmarking.

| Fixture | Cold first record | Warm median (min–max), 7 records | First one-file pair | Later one-file pair |
| --- | ---: | ---: | ---: | ---: |
| Core, 50,000 files, seeded index | 430 ms | 136 ms (134–151) | 597 ms | 305 ms |
| Sandboxed host helper, 50,000 files | 659 ms | 160 ms (158–168) | 655 ms | 379 ms |
| Sandboxed host helper, 200 files | 83 ms | 65 ms (60–67) | 140 ms | 131 ms |

The core's separate 50,000-file first record with full hashing took 5,343 ms.
The sandboxed warm median met the 300 ms target. Helper timings include process
startup, Seatbelt, Git, and reply parsing; they exclude runtime SQLite work,
harness startup, model latency, and desktop rendering. The pair costs should
not be read as the unchanged-record target.

This work has not been tested with a paid live harness, an installed desktop,
Linux containment, a power failure, or an undo operation. No native app was
launched for this verification. A concurrent external writer prevents a truly
atomic snapshot; detected changes are marked, and later undo must validate the
current files again. Unchanged borrowed objects do not constitute a backup of
a deleted repository. Stage 2 must validate availability and coverage before
restoring anything, page the stored path lists within protocol frame limits,
and expose the after-restart warning and unsupported metadata plainly.

## Turn undo: host operations

Stage 2 exposes protocol version 5 listing and undo in the host, with shared
Rust client types. It adds no desktop control. The checks below use disposable
repositories and scripted harnesses; no native app or paid model is launched.

On 2026-11-18, `bash scripts/check.sh` exited 0: workspace formatting and
clippy, 547 Rust tests, 344 runtime tests, TypeScript checking, and the workspace
release builds passed. Five Rust tests are ignored by the ordinary gate;
the two recording cost tests were separately run below. The new coverage
includes 19 core undo tests and 15 version 5 runtime tests. The release
metadata Python suite also passed all 12 tests.

The core undo suite checks exact bytes, executable bits, symlink targets,
binary content, individually ignored small files, spaces, newlines, non-ASCII
names and leading dashes. It exercises unborn repositories, linked worktrees,
mode-only changes, created-then-deleted files, removal of newly empty
directories, and undo of an undo. Preflight tests cover person/later-run edits,
case-only renames during and after runs on the case-insensitive test volume,
file/directory transitions, nonempty directories, and symlink ancestors.
Injected changes before the final recheck exercise parent swaps, edits before
rename and changes before deletion. Tests also cover temporary-name collisions,
wrong checkout identities, missing borrowed unchanged blobs, partial failure,
`leave_changed`, and exclusion of unrelated edits and refused restores from
the undo record's scope.

The ordinary core fixture compares unaffected working files and every Git
metadata path, byte, mode, inode and modification/change time around undo, and
checks each restored path against its recorded before bytes and Git mode.
The real host-helper tests extend the recorder's Git-directory comparison to
undo and inject a Git command that tries to change index timestamps under the
undo sandbox. Current hard links, including a working path linked to Git
metadata, are refused with the entire fixture unchanged. These hard-link
fixtures run in the host suite because the command sandbox intentionally denies
creating hard links even in scratch; they are not skipped by the full gate.

The version 5 runtime suite covers fake Codex and fake Claude Code through the
runtime request dispatcher, plus Pi replay through the host socket. It checks
busy checkout exclusion, another host's inode lease,
steering membership and separate follow-ups, a later record in another thread,
failed/wrong-thread records, restart warnings, command paths in `changed_files`,
old snapshot shapes, strict request validation, and multi-page paths and
coverage within the 1 MiB frame. Applied request IDs deduplicate after restart;
blocked IDs may be retried and conflicting reuse is refused. An injected U1
capture failure exercises recovery of completed writes without replay and
without claiming another file changed while the host was down. Rust client
checks cover version 5 request/reply shapes, status negotiation down to version
1, and an old host refusing undo without retrying the mutation at another
version. Packaging metadata advertises version 5.

A 3,500-file changed checkout with a private path manifest larger than 1 MiB
exercises the real 10-second undo deadline.
Preflight runs out of budget before applying; every file retains its after
bytes and the checkout lease is released. This checks the budget, not a claim
that a large undo completes in ten seconds. Capture, object reads and writes
share the helper deadline; subprocesses stay in its group for timeout cleanup.
The measured undo request in the final gate returned after 10,003 ms.

These tests do not prove an atomic transaction against external editors.
Existing-file replacement/deletion has a final check-to-mutation interval;
an unrelated writer can still race it and lose a change. Detected late races
stop with `undo_incomplete` and can leave earlier restores applied. A crash
between a file mutation and journal completion can omit that path from the
recovered undo record; no write is replayed. A writer changing an already
restored path before U1 capture can affect that recorded after image. Power
loss, arbitrary filesystem faults, installed UI acceptance, and live paid
harness turns are not verified here. Linux has no Seatbelt enforcement;
macOS helper evidence must not be read as Linux containment evidence.

After the case-spelling reconciliation was added, the existing 50,000-file
recording cost checks still passed (`bash scripts/bench-records.sh`, exit 0).
On the same platform and toolchain as the stage 1 measurements above, seven
warm samples measured a 195 ms core median (193–210 ms) and a 215 ms sandboxed
helper median (210–222 ms). The 200-file helper median was 69 ms (66–76 ms).
These release-build measurements include the added directory-name checks;
they measure recording, not undo or native rendering. They were taken during
development without otherwise isolating the machine.

## Turn undo: desktop controls

On 2026-11-18, `bash scripts/check.sh` exited 0: formatting, headless dependency
separation, Clippy with warnings denied, 610 Rust tests, 344 runtime tests,
TypeScript checks, and release builds passed. The Rust suite retains six
separately invoked or timing tests as ignored. `bash scripts/snapshots.sh`
also exited 0; all 18 new undo captures were inspected at both widths.

The desktop uses protocol version 5 records at each run's closing line. Core
projection tests cover steering in one run, queued follow-ups in separate runs,
no-answer runs ending on a tool or request, reasoning with and without text,
unloaded history, every recording and undo state, and undo-chain traversal.
Page-assembly tests append all three lists at their supplied offsets and reject
gaps or inconsistent metadata.
No host, runtime, or wire contract changed in this stage.

Desktop tests run in isolated homes and use scripted owner-only host sockets.
They exercise the actual asynchronous client through Undo, Redo, a blocked
reply, Cancel without a request, and Undo the rest with the explicit flag.
The partial result names only restored files. The changed-files card includes
recorded command paths. Tests reject replies across thread, watch, connection
epoch, revision and fetch-generation changes, keep raw records from enabling
an older row projection, check the shared button/shortcut gate, suppress held-key
repeats, repeat an identical request ID after a lost reply, and refresh an
interrupted operation without replaying it. Opening and closing an earlier
run's conflict panel remeasures its row. The action-table tests check shortcut
uniqueness and compare the default keyboard table with the Mac guide.

The snapshot scenes use the real GPUI views over sample state at 1380 and 900
points: finished Undo (dark and light), earlier-row hover, Undone with Redo,
the changed-since panel, failed and recording states, after-restart warning,
and an older host with no control. These are offscreen Metal/Core Text scenes,
not installed-app acceptance. Nothing in this stage has been checked in the
installed app; no app was launched, and no paid live harness was used. Native
pointer/keyboard behavior, accessibility-client interaction, and a live desktop
undo against an installed host remain unverified.

The stage 2 safety limits still apply. The checkout lease excludes host writers,
not arbitrary editors or shell commands. An external writer can modify or
replace an existing file after the last recheck and before rename or deletion;
that external change can be overwritten or deleted. No-follow traversal and
exclusive creation narrow this exposure but do not make checks and existing-file
mutations atomic. An ancestor moved after its last identity check is also
outside the verified guarantee.

An external writer can change an already restored path before the undo's final
record captures it. That same-path edit can become the undo record's after
image, so Redo can remove it while matching the recorded image. Excluding
unrelated paths does not eliminate this same-path capture race. Ordinary later
changes made after capture are protected by the fresh-record and immediate
content checks.

A detected late race or timeout can leave a subset restored; there is no rollback
transaction. Process death after a mutation but before durable journal completion
can omit that mutation from the recovered record. The write is never replayed,
but the recovered record may not undo every effect. Power loss and arbitrary
filesystem faults are not proven. Other permissions, extended attributes,
hard-link relationships, empty-directory history, Git history, outside-checkout
effects, and excluded coverage are not restored. Replacing a file can discard
metadata outside the content/executable/symlink promise. These limits prevent an
absolute guarantee against every concurrent writer.


### Undo safety regression coverage

The core suite exercises bare and separately named Git directories before,
after creation, and after deletion during a run. A later nested commit sharing
objects with the run's commit stays valid under `git fsck` after Undo the rest;
a repository appearing after capture blocks writes through older recorded paths.
The inventory tests cover unreadable folders in both directions, untouched
files becoming readable, FIFOs and Unix sockets. Unsupported device entries
use the same file-type exclusion, but an actual device node was not created.

Helper tests inject Git descendants and kill the helper directly, kill its
process group as the runtime does, and kill its worker. Both the checkout lease
and inherited store lock become available. The supervisor timeout kills its
whole worker group. Record and prune have a 240-second overall deadline,
individual Git commands are capped at 120 seconds within it, and the runtime
retains its 300-second outer limit. Undo retains its shared 10-second budget.
The 3,500-file undo fixture still verifies timeout before any file writes and
lease release; this is a limit on large undos, not evidence they complete.

A read-only checkout causes a real undo failure with zero file effects. Runtime
and Rust projection tests verify the `no_effect` state, original action target,
restart persistence, refusal to replay the same request, and a successful new
request after permissions are restored. An interrupted journal is not proof of
zero effects. Unicode tests on the case-insensitive macOS volume verify one
entry for a decomposed name, normal undo restoration, rejection of equivalent
aliases, and acceptance of distinct hard links. The equivalent-alias branch has
not been exercised on a case-sensitive volume.

Desktop model tests merge provenance and record paths in both reply orders,
preserve provenance after record expiry, and reject old requests after a thread
change or reset. Ten-thousand-entry coverage tests bound hover and panel previews
to 12 entries per list, with omitted counts and text prepared before render.
The wide and narrow `undo-many-exclusions` scenes cover the bounded panel.

Known retention limit: after 30 days, pruning can remove an original run's pair
while retaining a newer undo pair. The desktop projects controls from original
run records, so it can hide a Redo whose retained pair would still work. This
limit is not fixed here. The concurrent-writer, partial-restoration and crash
limits above continue to apply; no installed app or paid harness was exercised.

The regression benchmark (`scripts/bench-records.sh`, release build, 50,000
files, seven warm samples) passed the unchanged-record target after these
changes. On Apple M5 Pro with Git 2.54.0, the core median was 186 ms (184–199),
the sandboxed helper median 243 ms (230–248), and the 200-file helper median
73 ms (66–76). The same script on the preceding implementation measured
211 ms (199–247), 211 ms (210–214), and 59 ms (58–62), respectively. These are
local wall-clock samples, not isolated-system guarantees. The helper now starts
one additional supervisor worker; the inventory reuses directory-name reads
and does not run a Git process for each directory.

### Release check for v0.1.0-preview.15

On 2026-11-18, on the tree that was tagged, `bash scripts/check.sh` exited 0:
630 Rust tests (171 desktop, 437 core, 14 host, 8 helper integration), 353
runtime tests, TypeScript checks, and release builds.

Before the release, a reviewer who had not written the code read the undo
changes and reproduced eight defects with the release helper. Two lost data
with no concurrent writer: a bare repository inside the checkout was recorded
as ordinary files, so Undo the rest could delete its objects, and an untracked
folder that could not be read was taken as absent, so undo could delete files
that predated the run. Seven of the eight were fixed with the regression tests
described above; the retention limit above is the eighth.

Someone other than the fixer then ran separate reproductions of the bare
repository, the unreadable folder, and the decomposed file name against the
release helper built before the fixes, where each failed, and against the one
built from them, where each passed. An index holding two names that differ
only by case recorded and undid normally on both.

The release helper was also run under its production sandbox, outside the test
suite. On a linked worktree of a scratch repository: a record, a simulated run
(an edit, a deletion, files created in new folders, a mode-only change, a
retargeted symlink, an ignored file, a file with staged changes), a second
record naming exactly those paths, an undo that an outside edit blocked without
changing anything, and an undo that restored every file's bytes, mode, and link
target while leaving the index and every file in the repository's Git
directory byte-identical. On Plait's own main checkout and one of its linked
worktrees: two records each into an empty store, with the listing of Git
metadata (names, sizes, modes, times) and `git status` identical before and
after. The linked worktree took 118 ms for the first record and 62 ms for the
second.

An earlier tag for this release, v0.1.0-preview.14, was never published: its
CI check failed in two recording tests. Git 2.56, which that machine's test
fixtures use, starts detached maintenance after every commit and holds a lock
file under `.git/objects` for a few milliseconds. A test that compares a
repository before and after a record could see the lock disappear and blame
the recorder. Watching `.git/objects` for 40 ms after each of 100 commits
showed the lock in 99 with Git 2.56 and in none once the fixtures turn
maintenance off, as the recorder's own commands already do. With the unfixed
fixtures and Git 2.56 the core Git tests failed in one run of three; with the
fix they passed eight runs of eight, and the host's record tests four of four.
v0.1.0-preview.15 is the same application code with that fixture change.

None of this is installed-app acceptance. Undo and Redo have not been tried
against an installed host, and no paid harness was used.

## Images in messages

Protocol 6 shipped in v0.1.0-preview.17. Rust and runtime tests cover version 5/6 request
encoding, reply decoding, negotiation down to an older host, and removal of
image fields from older wire shapes. Scripted Pi, Codex, and Claude Code runs
check the exact original base64 blocks each adapter sends, store reopen,
runtime restart, history, and persisted references. Pi also crosses a complete
Rust-host process restart and checks the next model context. No live or paid
model was used.

Both sides test type-by-bytes, the inclusive 5 MiB image and message bounds,
count, total message size, and dimensions. Maximum images with complete
65,000-byte submitted JSON requests pass through scripted Pi, Codex, and
Claude Code; resulting harness frames leave more than 1 MiB below the 8 MiB
line bound. Four-block base64 padding is included in the arithmetic. Runtime
and socket-client tests check 512 KiB read pieces, ten reads for a maximum
image, the 1 MiB reply bound, and oversized-part refusal; uploads remain
32 KiB and oversized upload pieces are refused.
Desktop file tests reject links and special files. Host tests cover hash,
chunk offsets, incomplete uploads, and thread ownership. An isolated desktop
test keeps the text, images, and revision on refusal, saves and reopens existing-thread and new-thread image
drafts, and blocks known text-only models and older hosts. Cache completion
tests reject another thread, connection epoch, or watch generation and enforce
the 128-entry bound. The host refuses a known text-only Pi model before accepting
the message. A headless input test dispatches Cmd-V through the composer,
checks text precedence with mixed clipboard entries, and runs the file-drop
preparation path with a misleading extension, checking persisted originals.
Isolated desktop tests remove an original after host acceptance and after
attachment removal, keeping it while an uncertain submission still needs it. The acceptance test
also restores the removed original from a scripted host when returning it to
a draft for editing.
Core recovery tests sweep orphan files on startup, retain saved drafts and
pending submissions, and protect preparation and newer queued state during
collection. An upload guard test retains originals when a watch acknowledges
an earlier attempt while a retry is still reading files. Cleanup follows a successful journal save; a failed save retains
the durable draft's bytes.

Host retention remains a storage limit: accepted originals stay in
`host.sqlite` for the life of the store once they enter the conversation, even
after archive or stopping a run.
There is no age-based collection of accepted images. For example, 100 distinct
5 MiB screenshots each day for 30 days means 3,000 × 5 MiB = 15,000 MiB,
or about 14.65 GiB of image payload, plus SQLite page/index overhead and its
write-ahead log. Identical bytes deduplicate within a thread, not across
threads. Bytes occupy the separate `images` table's BLOB column;
`message_images` holds associations. Ordinary thread/snapshot/history queries
read references and metadata only, never those BLOBs. Bytes are selected for
explicit image reads and harness input. Removing the host store removes them;
unused uploads expire after 24 hours. Cancelling queued messages or stopping
a run removes their queued associations; an original with no remaining accepted
owner enters that same 24-hour grace period. Startup repairs associations left
by earlier hosts. SQLite reuses deleted pages; pruning does not shrink the
database file on disk.

Regression tests exercise edits while queued cancellation is awaiting a reply,
checkout-specific new-thread drafts and immediate Discard saves, cancelled-image
collection with shared accepted ownership, header-only paste validation,
LRU recency and 50-image scroll-back reuse, unavailable originals blocking Send,
and valid siblings surviving a missing or unreadable original in both local
draft restoration and queued Edit's host-fallback path. Removing the failed
attachment enables sending the valid remainder. Tests also cover
Uploading with image-only pending labels, disabled paste/drop, refusal clearing
on edit, prompt history for illustrated messages, removed images finishing
late, EXIF-rotated display dimensions, and Codex capability resolution before
its first watched snapshot. Scripted Pi, Codex and Claude Code inputs contain
only image blocks for image-only messages. Pinned source confirms their input
shapes; live provider acceptance and provider-specific refusals remain untested.
Image-caption regressions check format/control escaping in each harness's
snapshot and history while the scripted harness receives the unchanged prompt.
Long captions exercise full-text offsets before placement and through both
ordinary and typed-input history references.

Claude Code's pinned SDK has no model image-capability field;
unknown support is deliberately allowed, not inferred from model names.

`scripts/snapshots.sh` renders one and several composer images, Text only,
Update host, Uploading with an image-only pending message, an unavailable
original, a mixed valid/failed draft and its recovery after removal, a request
with several images, and the larger view at 1380 and 900 points. All twenty
image scenes were inspected offscreen with Metal and Core Text.
`scripts/bench-frames.sh`, release profile, measured 30 CPU frames of a
50-image conversation: median 0.85 ms at rest (p95 0.96 ms) and 1.68 ms scrolling
(p95 1.77 ms). Thumbnails were prepared in advance. The LRU cache retains up to
128 decoded 192-pixel thumbnails: at most 18 MiB of RGBA pixel payload, plus
metadata and renderer storage. The separate enlarged image is bounded to
1600 × 1600 pixels (9.77 MiB); temporary background decode allocations are
separate. This is render/layout/prepaint/paint cost, not GPU, network, decode,
or input-to-display latency.

`bash scripts/bench-images.sh` measures production Rust fetch, hash validation,
PNG decode, and 192-pixel thumbnail preparation through a real local Rust host
and SQLite-backed runtime. It uses generated 3840 × 2160 PNGs with valid
ancillary padding, release builds, two warmups, and ten measured thumbnails.
The former 2 MiB maximum with 32 KiB reads took 64 requests and median
57.450 ms (p95/max 63.318 ms). At the 5 MiB maximum, the old read scheme
required 160 requests and median 257.026 ms (p95/max 279.542 ms); 512 KiB reads
require ten requests and median 55.795 ms (p95/max 59.144 ms). The old scheme
is reproduced in the offline fixture, with the larger size limit enabled for
that comparison. Measurements include socket connections, host/runtime
framing, SQLite reads, base64 decoding, and thumbnail preparation; they exclude
upload, process startup, GPU drawing, and UI scheduling. No model runs.

The same script traverses 50 distinct 320 × 180 PNG originals through the
production client and thumbnail cache, then traverses them in reverse. The
first pass fetched and prepared 50 thumbnails in 23.984 ms with 50 socket
requests; the return pass took 0.008 ms with zero requests or decodes. This is
a cache traversal, not a native scroll gesture. Host request logs verify the
counts. Entries beyond the 128-image bound may be fetched again after eviction;
switching threads or connections clears this memory cache.

### Native clipboard exception and probe

GPUI reads the person's clipboard synchronously and allocates the complete
representation before Plait can check its size. This documented bounded-read
exception applies only to an explicit paste action; text paste already has
that behavior. Plait immediately validates limits and drops refused bytes,
without retaining them in a draft, cache, or log. Plait checks headers on the
UI thread and computes SHA-256 once in background preparation, alongside
decoding and thumbnailing. The pinned GPUI still computes its own image ID
when constructing the clipboard result; its hash and allocation costs are
included in the synchronous-read measurement.

Run `bash scripts/check-clipboard.sh` explicitly on macOS to pin native GPUI
representation behavior and measure the read. It creates no app window, saves
and restores readable pasteboard representations, and does not print their
contents. It asserts PNG+TIFF → PNG, JPEG+TIFF → JPEG, TIFF-only → refusal, and
PNG+text → text only. The text preference is a limit of the pinned GPUI, not
an image conversion. A later GPUI change that alters these results fails the
probe. It is opt-in so the ordinary test suite does not replace the clipboard.

For valid, ancillary-data-padded PNG fixtures of 5,000,000 and 20,000,000 bytes,
30 release samples after five warmups measured synchronous read + validation
+ drop: respectively median 0.271 ms / p95 0.284 ms / max 0.288 ms and
median 1.120 ms / p95 1.177 ms / max 1.424 ms. The 5,000,000-byte image passes
header checks; the 20,000,000-byte image is refused by size. Both measurements
include GPUI's image-ID hash; neither computes Plait's SHA-256 digest nor decodes
pixels. These timings cover a local pasteboard owner, not the
potential delay of another application's promised data provider.

A native screenshot clipboard probe offered PNG and TIFF and GPUI selected
PNG. Controlled mixed fixtures also offered plain text, which GPUI selected.
Browser and Preview pasteboard types were not observed, so this check cannot
claim which copy commands in those apps attach images. Any source offering
only TIFF is refused; any source offering text beside its image pastes text.
Drop the PNG/JPEG file, or copy the image alone, to attach it.

Nothing about this feature was checked in the installed app, and Plait was
not launched. Native window paste/drop interactions, accessibility-client
activation, live provider acceptance and vision quality, and provider-specific
limits remain unverified. The original bytes are passed to the harness; a
harness or provider may apply its own transformations. The public SDK schemas
and scripted transport do not establish live-model acceptance.

## Recorded turn review

### Recorded-diff backend

Core tests compare owned changed blobs after removing the source repository's
object directory, covering creation, deletion, executable mode, symlink target,
binary bytes, CRLF and missing final newlines. They reject unsafe paths and
missing objects. Runtime tests exercise the real sandboxed helper, compare
checkout and Git metadata before and after reads, retain the same original
patch after a commit and undo, and check same-checkout cross-thread reads.
Pagination tests check complete assembly, repeat offsets, frame bounds, invalid
offsets and five-minute expiry. The existing version 1–5 suites remain required.
No installed-app or native-window acceptance is claimed for these operations.

### Recorded Changes navigation

The current source adds the desktop recorded view. Core tests keep original
run identity separate from the displayed undo leaf, preserve deleted paths,
and reject every changed read-stamp member.
Socket-backed desktop tests cover original selection after a later run and
undo, zero net paths, the all-files target used by step count and N more,
partial-undo Original/Restored lists, clean/deleted files, picker Enter/Escape
and focus restoration, old-host fallback, and incomplete-page refusal. The
reply tests queue reads, invalidate thread, checkout, connection, watch,
repository epoch, generation, comparison or path, and reject the completed
metadata/diff/page sequence rather than install stale data. Preference-writing
UI tests use the isolated home and host store. The final `scripts/check.sh`
run, with Homebrew Git 2.56 first on PATH, exited 0: formatting, workspace
Clippy with warnings denied, 649 Rust tests, 358 runtime tests (including the real macOS sandbox), and the release
build passed.

`bash scripts/snapshots.sh <output-directory>` renders the new `turn-recorded`,
`turn-recorded-clean`, `turn-recorded-empty`, `turn-recorded-loading`,
`turn-recorded-unavailable`, `turn-review-old-host`, `turn-recorded-undo`,
`turn-recorded-long-path`, and `turn-recorded-light` scenes. All were inspected
at 1380 and 900 points. The final scenes keep comparison controls visible when
a path truncates, omit bulk staging in Recorded, and give the clean fixture
consistent current-status counts. These are injected test scenes, not real
agent runs.

On Apple M5 Pro, macOS 26.6.2, Apple Git 2.54.0 and Node 26.12.0, a release
`turn_diff` handler read a 5,000-changed-line file (2,500 removals and 2,500
additions) in a 50,000-file checkout. After a 452.66 ms initial request, seven
warm requests had a **67.47 ms median**, **64.36–73.51 ms** range. The measured
boundary includes root validation, SQLite lookup, the real sandboxed helper,
helper decoding and immutable page construction; it excludes socket framing,
transport and desktop patch preparation. The patch was 275,105 bytes, and the
JSON result was 368,277 bytes. The helper's counter measured **two Git
processes** (one batch blob read and one blob comparison); its supervisor,
worker and sandbox launcher add three, for **five child processes**. An
absent side can require an additional private empty-blob process. This is a
warm-cache fixture measurement, not a cold-disk or whole-application latency
claim; checks and snapshot rendering were running concurrently.

`scripts/bench-frames.sh` now includes the recorded view with a prepared
5,000-line patch. At 1380 × 860 points in a release build, 30 frames per
scenario measured the following CPU costs in `Window::draw` (render, layout,
prepaint and paint):

| Recorded view | Median | p95 | Maximum |
| --- | ---: | ---: | ---: |
| Top | 1.03 ms | 1.43 ms | 1.48 ms |
| Middle | 1.07 ms | 1.33 ms | 1.38 ms |
| Wheel scrolling | 2.31 ms | 2.80 ms | 2.99 ms |

The render method itself measured 0.03 ms. These measurements exclude GPU
submission, native event delivery and host I/O. No installed app was checked
or launched for this feature; native-window acceptance, VoiceOver and live
model runs remain unverified. The broader attribution and staging benchmark
matrix belongs to later stages; this measurement covers one changed path,
not 50,000 changed paths or large retained histories.

### Recorded review edge cases

Regression tests cover a long-path patch whose byte limit crosses a hunk header,
an unfinished hunk body, or a long line. The preview stops at a complete line and
omits its incomplete trailing hunk. Empty file creation and deletion remain
Ready rather than Mode, and Latin-1 edits retain Git's text classification and
raw bytes while the desktop decodes them for display.

Desktop tests cover Current while status loads, records with no selected file,
and a staged-only file previously selected on its empty unstaged side. An older
version 5 host falls back to the clicked current file on the first unsupported
probe; stale replies cannot redirect a different checkout. Known older-host
links describe their actual destination. State wording tests cover the single
limit notice, pending or unavailable record scopes, absent line counts, and
distinct undo picker descriptions. Multi-line coverage explanations are
flattened before entering the picker so fixed-height rows cannot overlap.

Runtime tests reject inherited operation names with `invalid_request` and send
a full-size patch alongside large hunk metadata through a shared reply budget.
The combined initial reply fits the 1 MiB transport frame, and continuation
pages preserve their boundaries on retries without losing items.

The additional scenes cover Current loading and staged-only selection; a
loading, unavailable or older-host record with no selected file; older-host
closing lines with and without files; limited patches; empty additions and
deletions; Latin-1 text; and multiple undo choices. All 21 recorded-review scenes
were inspected at 1380 and 900 points. Inspection corrected a repeated Older
host label, stale fixture ages, and overlapping multi-line picker details.
These are headless, injected scenes; nothing was checked in the installed app.

### Recorded review with MCP and images

A regression covers format controls, line separators and C1 controls in
recorded paths, run and thread names, omitted paths and patch text. Display
escapes share the MCP helper; record IDs, selected paths, patch bytes and line
numbers retain their meaning. Recorded review stays unversioned alongside
protocol 6 image operations, with its dispatch ahead of legacy validation.

## Release check for v0.1.0-preview.17

On 2026-11-18, on the tree that was tagged, `bash scripts/check.sh` exited 0
with the Git that CI's fixtures use first on the path: 680 Rust tests (195
desktop, 463 core, 14 host, 8 helper integration), 447 runtime tests,
TypeScript checks, and release builds.

Images in messages and the recorded turn view were each read before release by
a reviewer who had not written them, and each finding was confirmed by
experiment.

- **Images in messages:** fifteen defects. Two lost images: Edit on a queued
  message whose message box had changed kept only its text, and switching
  checkout with a New thread draft open moved that draft to the other
  checkout. One left a cancelled message's images marked as used for ever.
  Two did avoidable work: hashing on the UI thread at paste, and a small
  first-in-first-out thumbnail cache that fetched originals again. The rest
  were wrong or missing states. All were fixed with regression tests and
  checked again; that second pass found one more, a draft whose valid images
  stayed failed after a missing one was removed, which was also fixed and
  checked. Accepted images staying for the life of the host's store is a
  documented limit, not a fix.
- **Recorded turn view:** eight defects. Three showed a wrong diff: a large
  diff cut inside a hunk header was reported as unavailable, a created or
  deleted empty file showed as a mode change, and text that is not UTF-8
  showed as binary. Two showed a wrong Current view, and one sent the first
  click on an older host to a notice in place of the file. All were fixed
  with regression tests and checked again, with nothing new found. A turn is
  named by its request text; an earlier build named it by a record id.

Text shown in approvals, tool rows and recorded diffs escapes control and
format characters. A carriage return that is part of a CRLF line ending is
dropped from what is shown; one that stands alone is escaped.

This release also carries MCP tool calls, whose own release check is in the
MCP section above. The tag first made for them, v0.1.0-preview.16, was never
published, for two faults in tests and none found in the product. A core undo
test compared a bare repository before and after an undo, and Git's detached
maintenance, started by the push into that repository, could change it in
between; the fixtures' environment settings do not reach the receiving side
of a push, so that repository's own config now turns maintenance off. And in
the installed-package suite the MCP byte-flood fixture wrote 1 MiB lines, a
full pipe refused a write, and the fixture exited before the output budget was
spent; it now writes lines the pipe can take, and the test accepts a server
that dies of its own flood.

Not exercised for either feature: a live model (an image-only message in
particular), browser and Preview clipboards, a real pointer for paste and
drop, and the installed app.

## Thread-workspace refactor baseline

The generated fixtures at `f0fc5f5` include the conversation redesign in
`f49309e`. Fresh measurements on that revision, before further refactoring:

- `bash scripts/snapshots.sh` rendered 467 PNGs at wide (1380 × 860) and
  narrow (900 × 600) sizes, using offscreen Metal and Core Text.
- Two consecutive `PLAIT_BENCH_FRAMES=90 bash scripts/bench-frames.sh`
  release runs measured CPU `Window::draw`, without GPU submission:

| Case | Median, run 1 / run 2 (ms) | p95, run 1 / run 2 (ms) |
| --- | --- | --- |
| Idle home (`e1`) | 0.32 / 0.32 | 0.39 / 0.36 |
| Small conversation (`e2`) | 0.55 / 0.55 | 0.62 / 0.59 |
| Streaming reveal (`e3`) | 2.97 / 3.09 | 4.25 / 4.24 |
| Heavy conversation, bottom (`c1`) | 1.04 / 1.05 | 1.08 / 1.10 |
| Heavy conversation, scrolling (`c3`) | 2.20 / 2.19 | 2.56 / 2.59 |
| Conversation find (`c5`) | 4.38 / 4.37 | 4.48 / 4.43 |
| Changes, scrolling diff (`a3`) | 2.40 / 2.39 | 2.50 / 2.48 |
| History, scrolling patch (`b4`) | 2.01 / 2.01 | 2.07 / 2.08 |
| Files, scrolling (`f2`) | 1.26 / 1.27 | 1.29 / 1.30 |
| Heavy view switch (`d1`) | 3.90 / 3.94 | 4.10 / 4.10 |
| Uncached diff (`d2`) | 20.15 / 19.70 | 28.78 / 29.61 |
| Cached diff (`d3`) | 1.56 / 1.52 | 2.84 / 2.78 |
| Step file (`d4`) | 3.31 / 3.29 | 3.47 / 3.52 |
| Thread switch including projection (`d5`) | 3.31 / 3.36 | 11.00 / 10.92 |
| First frame after thread install (`d6`) | 0.62 / 0.61 | 0.64 / 0.64 |

A separate five-frame run of the release test binary, measured directly with
`/usr/bin/time -l` and isolated desktop/host storage, reported 74,645,504 bytes
(71.2 MiB) maximum resident set and 49,709,704 bytes (47.4 MiB) peak footprint.
This is the headless fixture process across all cases, not memory per thread;
it excludes a native window, GPU textures, and host/harness processes.

Future workspace-retention stages will count active/hidden fetch, projection,
decode, and layout work. Generated 1 MiB prose, 16 MiB prose, tool-heavy, and
image-heavy fixtures need allocation and resident/footprint deltas through
activation, parking, switching, and closure, alongside wire-frame sizes.
Those measurements and native input, accessibility, and GPU presentation are
not established by these headless baselines.

### Thread-workspace ownership checks

The ownership refactor retains one selected workspace and the existing window
controls. With test-only fixture paths, Git dates, read durations, diagnostics,
and stream-reveal prefixes pinned, two baseline renders are byte-identical.
All 467 existing scenes also match that baseline after each ownership stage.
Four added wide/narrow scenes cover stale-thread replies and checkout identity
errors. No README images were regenerated.

The final full development check passed: 706 Rust tests and 447 runtime tests,
workspace formatting and Clippy, the headless dependency boundary, runtime
TypeScript checks, pure acceptance self-tests, and the release build. Linux
Clippy for `plait-core` and `plait-host`, including all targets with warnings
denied, also passed. Eight existing Rust tests remain ignored by the normal run.

New coverage exercises delayed reads across A→B→A, inactive Git write outcomes,
checkout aliases and replaced directory identities, stable path selection after
status reorder/rename/deletion, commit draft revisions, original undo request
owners, and file/policy replies after checkout changes. Native input,
accessibility, GPU presentation, real provider runs, restart workspace recovery,
and per-thread retained-memory budgets were not verified by this refactor.

Two final 90-frame runs measured idle home at 0.33 / 0.33 ms, small
conversation at 0.56 / 0.55 ms, heavy conversation scrolling at 2.31 / 2.26 ms,
and uncached Git reads including their wait at 22.47 / 21.81 ms. The original
baseline pairs were 0.32 / 0.32, 0.55 / 0.55, 2.20 / 2.19, and 20.15 / 19.70 ms
respectively. Repeats on the shared machine showed broad load-dependent spikes;
these numbers do not establish the final performance gate. A quiet comparison
against the baseline, including uncached Git reads, remains required before
release. GPU submission is excluded from every frame measurement here.

## Release check for v0.1.0-preview.18

On 2026-11-19, on the tree that was tagged, `bash scripts/check.sh` exited 0
with the Git that CI's fixtures use first on the path: 789 Rust tests (229
desktop, 538 core, 14 host, 8 helper integration), 505 runtime tests,
TypeScript checks, and release builds.

This release carries thread tabs, Settings and Project settings as modals, the
redesigned conversation, and the first part of Linear in the host, which
nothing in the desktop uses yet. Each piece was read before release by a
reviewer who had not written it, and each finding was confirmed by a failing
test or an experiment before it was fixed.

- **The refactor under tabs** (a thread's workspace state moved out of the
  window, and every background result routed to the thread or checkout that
  owns it): three defects. Send did nothing until the checkout's Git status
  had been read, and never if that read failed. A file read during a host
  reconnect left Files on Reading. And scene renders that ran at the same time
  shared one fixture directory. All were fixed with regression tests. Every
  existing scene rendered byte for byte the same before and after the
  refactor; to make that comparison possible the scene harness now pins what
  varied between two renders of the same code (measured read times, fixture
  paths, how much streamed text is revealed, a diagnostics render time).
- **The tab model** (headless): five defects. Saving dropped tabs whose thread
  was missing from the thread list at that moment; a tab row that fitted
  exactly hid one tab; row data scanned the thread list once per tab; and two
  edges of reopening a closed tab. All were fixed with regression tests.
- **The tab row, hidden tabs and restore:** eight defects. A hidden tab's
  changed-file counts could land on the tab in front. A New thread tab showed
  nothing after its thread was created if it was not on Conversation. At the
  draft journal's size limit the tab set stopped being saved at all. A large
  view bookmark wrote a new record on every save. Restoring saved tabs closed
  an open Settings modal. The rest were a misaligned overflow summary, close
  controls that shared one accessible name, and design rules dropped from
  `DESIGN.md`. All were fixed with regression tests.
- **Settings and Project settings as modals:** four defects. A thread created
  while the modal was open closed it and took the keyboard; a new-thread draft
  counted as off screen behind the modal; Escape skipped the trusted-commands
  confirmation; and accessibility actions reached the workspace behind the
  modal. All were fixed with regression tests. An accessibility action in a
  live window was not exercised; the test platform cannot deliver one.
- **The conversation redesign** (You and Plait labels, a neutral panel for
  requests, quoted context beside a rule): seven defects before it merged,
  among them empty quote lines dropped from the screen and text below the
  contrast floors against the new panel in twelve bundled themes. All were
  fixed with regression tests.
- **Linear in the host** (key sign-in, cached reads, thread-to-issue links,
  and two writes made only on a person's action): fifteen defects in the first
  review, among them the key leaking when a server echoed the request's
  authorization header into issue data, and a sign-in in flight overwriting a
  later sign-out. A later design pass found that a write was sent before
  anything was recorded, so a lost reply could post it twice; writes now
  record their intent first, and an unknown outcome is never replayed. A
  review of that correction found two smaller defects. All were fixed with
  regression tests. Nothing has been tried against Linear itself.

Frame cost was measured with nothing else running, 90 frames a case, twice
each, against the tree before tabs. No existing case is slower. A long
conversation at rest costs 1.03 ms a frame against 1.03 to 1.04 ms, and 2.10 to
2.11 ms against 2.17 to 2.18 ms while scrolling; Changes at rest 1.05 ms
against 1.11 to 1.12 ms; History at rest 0.90 ms against 0.93 ms; switching
views 3.7 to 3.8 ms against 3.9 ms; a real Git read to its frame 8.2 to 8.5 ms
against 8.4 to 8.5 ms. Twenty open tabs with five running threads cost 1.39 to
1.40 ms a frame, about 0.37 ms more than one thread, which is more than the
0.1 ms the design allowed. Drawing the tab strip and the sidebar's thread rows
as cached views had held that case to 1.08 ms, but left them out of the
accessibility tree (see below), so they are drawn each frame; getting that
time back without losing accessibility is open. The streaming case measured
2.7 to 3.3 ms against 3.0 to 3.5 ms; it follows the machine's power state and
is reported, not used as a gate.

Saved tabs live in a file of their own, `tabs.json`. The draft and submission
journal is unchanged, including its 4 MiB limit, and its redesign remains
planned. Over that limit a tab switch is refused and the tab stays in front,
because the work in it could not be saved; before tabs, switching threads over
the limit was allowed and the draft stayed in memory. Tabs themselves are
still saved over the limit.

The native acceptance check was run against a real window on the release
build of the tagged code: 40 of 44 items passed, among them the tab row and
its keys, all four views, every Settings page, the command palette, find,
branches, staging and committing, the editor and terminal hand-offs, typing,
text selection, both approval shortcuts, a streaming reply, and Undo and Redo
from the closing line. Three failures are the check's own pointer steps, which
still drag and double-click the bar where the tab row now is; the window is
moved by the row's empty end. The fourth is known: Redo by shortcut pressed
straight after Undo by shortcut does nothing.

That check found one defect before release. The sidebar's thread rows and the
tab strip were drawn as cached views, and a view reused from the last frame
contributes no accessibility nodes: assistive technology could not reach a
thread row at all, and saw the tabs only on frames that redrew them. Both are
ordinary views again. Every scene rendered byte for byte the same before and
after that change.

One defect it found in debug builds was identified and fixed afterward. On a
debug build the check stopped at Settings → Editor: the framework's debug
assertion reported a duplicate accessibility node id and the app exited, so the
items after it did not run. The two colliding elements were the custom-command
notes under **Open files with** and **Open terminals with** (`Runs <program>
directly, not through a shell.`). The check's fixture starts with both settings
set to Custom commands that run the same recorder script, so both notes show on
the first visit, and `tag()` derived each text element's accessibility id from
its own text under the shared `settings-page` ancestor. The recorder lives in a
fresh folder for each run, which is why the reported id differed from run to
run. The reported id was matched afterward: it is the hash GPUI makes of the
second note's element path, recomputed headlessly with that run's folder. A
release build drops the duplicate node and carries on, which is how both the
local release run above and the packaged release build that CI drives passed
that page.

Not exercised: a live model in more than one tab at once; VoiceOver itself;
dragging a tab and middle-click on a tab; a host that reconnects under several
tabs; and the installed app.

## Headless accessibility guard

Accessible text helpers (`a11y_text`, `tag`, and `section_label`) require an
explicit element ID that names the text's place instead of deriving it from the
text. The duplicates this exposed across the scene suite (the Editor
custom-command notes, repeated harness states on Settings → Agents, queued
follow-up labels, repeated skill badges, and the colliding title and button IDs
on Project settings agent rows) are fixed.

Because the pinned GPUI test window never activates `Window::a11y`, debug test
builds use a prepaint collector (`crates/plait/src/ui/a11y.rs`) instead. It
records each audited control's and text element's `GlobalElementId` hash (the
value GPUI uses as the `accesskit::NodeId`), role, label and value during
`Element::prepaint`. Outside debug test builds the wrapper is a plain type
alias and records nothing.

What the guard covers:

- **Duplicate IDs in every scene:** each scene in `every_view_draws` is drawn
  with the collector active and fails if two elements in a frame produce the
  same node ID. Scenes drawn for images (`scripts/snapshots.sh`) are not.
- **Reachability on a steady frame:** a guard test draws two consecutive frames
  of a workspace with the sidebar, tabs and a streaming reply; every Settings
  page and Project settings as modals over it (Editor both at its defaults and
  with both settings Custom on one program); Changes, History and Files; an
  approval; and the open tab overflow list. On the second frame it fails on any
  duplicate, and unless each sidebar thread row, each visible tab and its close
  control, the four views and each message are present and named. A view drawn
  with `.cached()` drops out on that frame, so the test fails if the sidebar
  rows or the tab strip are cached again.
- **Stable IDs:** sidebar thread rows and their Actions buttons keep their node
  IDs when the thread list is reordered, and Project settings agent rows keep
  theirs when a skill is added in front of them.
- **Nothing outside the collector:** source checks fail on raw `text!()`, on
  `Text::new` or `InteractiveText` outside `ui/a11y.rs`, and on any `.role(`
  chain that does not reach the collector. The last is a text scan of the
  builder chain, not a type check: it accepts `.audited()` in the chain, a
  helper that returns an audited element, a closure inside an audited chain,
  and a `let` binding audited later.

What the guard cannot cover:

- It is not VoiceOver and does not run against a real window.
- It does not exercise GPUI's `Window::a11y` tree builder, AccessKit's tree
  updates, or the macOS accessibility bridge, focus announcements, or
  assistive actions.
- It sees only the states the scenes draw. An element that repeats only with
  real data (a list longer than any scene draws, two rows that share a key) is
  caught only if a scene draws it.
- Synthetic accessibility children are not recorded. Plait's only use of them
  marks the parent disabled or read-only and adds no nodes.

## Release check for v0.1.0-preview.19

On 2026-11-19, on the commit before this record, `bash scripts/check.sh` exited
0 with the Git that CI's fixtures use first on the path: 794 Rust tests (234
desktop, 538 core, 14 host, 8 helper integration), 505 runtime tests,
TypeScript checks, and release builds. This record and the README's release
line are the only changes after it.

This release adds no feature. It fixes how accessible text is identified and
brings the native acceptance check up to date with the tab row.

- **Accessibility ids.** Text that assistive technology reads took its id from
  its own words, so two identical lines at one level collided: a release build
  dropped the second from the accessibility tree, and a debug build stopped.
  Accessible text now takes an id from its place. The headless guard described
  above draws every scene and fails on a duplicate id, and on a sidebar thread
  row, tab, close control, view or message that is missing. It found and fixed
  duplicates on Settings → Editor, Settings → Agents, queued follow-up labels,
  skill badges and Project settings agent rows. The work was read before
  release by a reviewer who had not written it, which found five defects: the
  guard could not see an element that bypassed its wrapper; sidebar thread
  rows and Project settings agent rows took their ids from their position, so
  an id moved when the list did; and two smaller ones. All were fixed with
  tests. Every scene rendered byte for byte the same before and after.
- **The native acceptance check.** Its three pointer steps use the tab row's
  empty end, found from the accessibility tree; a missing end fails. An app
  that exits during a run is reported once with the end of its log. A reviewer
  who had not written the change found six defects in it, among them a failure
  softened into a skip, and all were fixed. On CI the check then passed 37 of
  42 items, against 23 of 42 on the previous release's code. Four of the five
  failures are selections that time out on CI: the diagnostics added here show
  the control's value still false after 1.5 seconds and true straight after a
  screenshot is taken, so on CI's display the frame that carries the new state
  is not drawn until something forces one. A real Mac passed the same steps.
  The fifth is known: Redo by shortcut pressed straight after Undo by shortcut
  does nothing.

Not exercised: a debug build driven through Settings → Editor in a real window
since the fix; VoiceOver itself; and the installed app.

## Release check for v0.1.0-preview.20

On 2026-11-19, on the commit before this record, `bash scripts/check.sh` exited
0 with the Git that CI's fixtures use first on the path: 895 Rust tests (247
desktop, 614 core, 18 host, 16 helper integration), 560 runtime tests,
TypeScript checks, and release builds. This record and the README's release
line are the only changes after it.

This release lets a second thread start in a project that already has one
running, in a worktree of its own, and removes the limit of one New thread
tab. It was built in six pieces. Each was read before release by a reviewer
who had not written it; each finding was confirmed by a failing test or an
experiment before it was fixed, and each fix has a regression test.

- **Write exclusion (Stage 6a):** nine findings. Every desktop Git write
  started one extra `git` process and every runtime lease one extra
  `plait-host` process; both are gone. `plait-host undo` upgraded an inherited
  lock in place. A Git process could outlive the lease of the process that
  started it; a lease is now lent to `git worktree add` and `git worktree
  remove` only. Tests used the person's real lease directory. The docs said
  nothing was visible when desktop Git writes during a run had become
  refusals.
- **Cleanup confirmation (Stage 6b):** seven findings. A normal Git exit
  released the checkout while a hook's background job still ran in its process
  group. Stop gave up when its signal ended Git and other members of the group
  survived. Recording a Git write forked the whole desktop, 8 to 9 ms for each
  staged hunk or commit; recording now starts no process. A record from before
  a reboot was never recognized. The host checked Git records before it held
  the checkout's locks.
- **Host-owned worktrees (Stage 7):** ten findings, two of which would have
  destroyed work. Removal deleted edits hidden by assume-unchanged or
  skip-worktree index bits, and lost commits kept only by the worktree's own
  HEAD reflog or refs. Also: the host's umask made every file in a new
  checkout private to its owner; a forged journal entry could remove a
  person's own linked worktree; a failed checkout hook hid a finished
  worktree; and Cmd-Option-N resent a stale request in another project.
- **The busy-project choice (Stage 8):** eight findings. The busy probe
  reported an idle project as busy when two probes ran together, or while the
  desktop was making a Git write such as a push; with the project's automatic
  setting on, either made a worktree nobody needed. Own worktree sent the
  draft's text from before the person's last edits. Closing the window while a
  worktree was being prepared led to a second thread and worktree on the next
  send. The new controls were missing from the accessibility guard.
- **A lock-file race,** found by the Stage 8 review in the lease code beneath
  it. On macOS, two callers that create the same file at the same moment
  through `openat` with `O_CREAT` leave one of them with "no such file". In a
  fresh lease directory that surfaced as an unconfirmed cleanup, which holds a
  thread. Lock files are now opened first and created exclusively when absent,
  and a lock file that cannot be opened is reported as itself.
- **Independent New thread tabs:** four findings. The draft journal gained a
  field that older builds ignore. A v0.1.0-preview.19 build that opened such a
  journal deleted the images of every draft but one per project as it opened,
  and dropped their text at its first save. Each save now also writes those
  drafts, and an unanswered first send, where older builds keep them without
  showing them. This was tested against a copy of the released build's journal
  types and read against its code; the released app itself was not run. Empty
  drafts whose tab did not come back were never removed, and the guide did not
  say that a full journal also refuses opening a tab.

Of the 520 scene images the previous release rendered, 504 are byte for byte
the same on this tree. Sixteen differ, all intended: twelve show Project
settings with its new **New threads** section; two show a new-thread default
that names a harness which is not ready, which now stays chosen instead of
silently becoming Pi; and two show a second New thread tab where the earlier
build returned to the first. Twenty-two images are new: a desktop Git write
refused during a run, a checkout held because its cleanup is unconfirmed, a
busy project's choice, a worktree being created, created in the background
and failing, a run waiting its turn to be recorded, twenty threads running at
once, several drafts in one project, and three hundred open tabs with their
overflow list.

Frame cost was measured with no other build or test running, 90 frames a
case, twice on each tree, interleaved, against v0.1.0-preview.19. No case is
slower. A long conversation at rest costs 0.98 to 0.99 ms a frame on both
trees, and 2.14 to 2.15 ms while scrolling; Changes at rest 1.06 to 1.07 ms
and History at rest 0.91 to 0.92 ms on both; switching views 3.8 to 3.9 ms on
both. Twenty open tabs with five running threads cost 1.42 to 1.43 ms against
1.42 ms. Three hundred open tabs cost 1.53 ms a frame, and their overflow
list, which draws only its visible rows, 1.75 ms. A real Git read to its
frame took about 20 ms on both trees; the Mac was in use, and the same case
measured 8 ms on an idle machine for v0.1.0-preview.18. The streaming case
measured 3.1 to 3.4 ms against 2.6 to 3.1 ms; it follows the machine's power
state and is reported, not used as a gate.

Known limits. Wait is shown and does nothing: the queue is not built. A
worktree starts cold and is never removed for you. A worktree whose checkout
hook failed has no Remove in the app. A hook or credential helper that keeps
running after Git exits keeps the checkout held until it ends; with `git
credential-cache` that is the cache's lifetime, fifteen minutes by default,
after which the hold clears by itself. Programs outside Plait, such as an
editor or Git in a terminal, do not take Plait's locks. An older build shows
one draft per project, and that draft does not carry its unanswered first
send through an older build's save. Every New thread tab is titled "New
thread", whatever it holds.

Not exercised: a real window on this tree before the tag (the native
acceptance check runs on CI after the push); live models writing in two
worktrees of one project at once; a provider's own rate limits and real
resource exhaustion; the released v0.1.0-preview.19 app opening a journal
written by this one; VoiceOver; Linux execution; and the installed app.

## Release check for v0.1.0-preview.21

On 2026-11-19, on the commit before this record, `bash scripts/check.sh` exited
0 with the Git that CI's fixtures use first on the path: 951 Rust tests (275
desktop, 642 core, 18 host, 16 helper integration), 562 runtime tests,
TypeScript checks, and release builds. This record and the README's release
line are the only changes after it.

This release adds a Worktrees tab that draws a project's worktrees as a branch
graph, names each New thread tab by the first line of its draft, and makes
Discard close the draft's tab. Each piece was read before release by a
reviewer who had not written it; each finding was confirmed by a failing test
or an experiment before it was fixed, and each fix has a regression test.

- **The Worktrees view:** twelve findings. Listing worktrees took an exclusive
  lease on each one, so a run that started at that moment was refused; the
  listing now takes no lease, and removal keeps its own. Remove acted on a row
  number from the last drawn frame, so a re-sort before the next frame made it
  ask about a different worktree; it is now bound to the drawn row's folder
  and identity. A worktree that had merged a side branch drew edges to commits
  that were not parents. The size walk followed a directory that was swapped
  for a symbolic link out of the checkout, and counted a nested worktree
  twice; it now walks by directory descriptor, stays on the checkout's device
  and skips other checkouts. After Remove in the view, the sidebar still
  treated the folder as present. A removal whose reply was lost in one project
  blocked Remove in another project's tab. And in the confirmation that lets a
  person remove a worktree whose commits are not pushed, **Keep local commits
  only** was offered for a clean, published worktree too; it is now offered
  only when publication is what blocks removal. The view's facts and the
  removal itself were fed the same eighteen cases for both answers and agree
  in each.
- **Git processes.** One refresh started about sixteen Git processes for each
  checkout, re-ran on every change to a thread's summary, and could overlap
  itself. Now a summary change starts none, a run that ends re-reads its own
  checkout with thirteen, and opening the view on a project with twenty
  worktrees starts 172 where it started 300. Each count is a test.
- **The saved tab.** A tab file holding a Worktrees tab among thread and draft
  tabs was given to v0.1.0-preview.20's own decoder: it restores every other
  tab in order with its bookmark and drops the Worktrees tab, and writes that
  back.
- **Draft tabs and Discard:** four findings. Discard pressed while another
  thread was being created erased the draft and left its tab open and empty;
  it is now refused first. Tabs and runs of spaces reached the title as typed.
  A draft that began with a long run of blank space was scanned whole on each
  keystroke; a title now looks at most 4,096 characters into the text. The
  guide still described the old Discard. Separately from the review, Escape
  was changed: it closes a draft that holds nothing and does nothing to one
  that holds text or images, where before it cleared the text.
- **The native acceptance check** gained a mode, set only for CI's run, that
  takes a window capture when a wait has not yet seen its state, because CI's
  display does not present a frame until something forces one. An item that
  needed it says so, and a summary line counts them; a real Mac runs without
  the mode. On v0.1.0-preview.20's tree CI passed 34 of 46 items; seven of
  the eight failures were of this kind. The change was read against the
  runner's behaviour with the mode off, which is unchanged, and has not been
  run on CI yet.
- **A test on Linux.** After the previous release a lease test failed on
  Linux only: lock files are named by a folder's device and inode, Linux
  reuses a removed folder's inode, and the test met an earlier test's file.
  The test now starts from that case.

Of the 542 scene images the previous release rendered, 484 are byte for byte
the same on this tree. Fifty-eight differ, all intended: forty-six show a
draft tab named by its first line, or the neighbouring tab in front after
Discard closed a draft's; twelve show the new Worktrees entry in the project
menu, a worktree thread's menu, Project settings and the keyboard list.
Forty-six images are new: eleven Worktrees scenes, each in dark and light at
two sizes (the graph, twenty worktrees, twenty forks, merged and detached
branches, missing and foreign folders, a refusal on hover, a lane under the
pointer, measuring, offline, an older host, and no worktrees), and the
overflow list with named drafts.

Frame cost was measured with no other build or test running, 90 frames a
case, twice on each tree, interleaved, against v0.1.0-preview.20. No case is
slower. A long conversation at rest costs 0.98 to 0.99 ms a frame on both
trees, and 2.15 to 2.17 ms while scrolling; Changes at rest 1.07 ms on both;
twenty open tabs 1.42 ms on both; three hundred 1.53 to 1.56 ms on both. The
Worktrees view with fifty lanes, its graph and sizes already read, costs 0.75
to 0.76 ms a frame. A real Git read to its frame took about 21 ms on both
trees, with the Mac in use. The streaming case measured 2.4 to 2.7 ms against
2.4 to 2.6 ms; it follows the machine's power state and is reported, not used
as a gate.

The release-mode test build, which only the frame benchmark uses, failed to
compile on this tree at first: three new tests read the debug-only
accessibility collector. They are gated now; `scripts/check.sh` does not
build tests in release mode, so nothing but the benchmark would have caught
it.

Known limits. Opening the Worktrees view reads each checkout in turn, about
eight Git processes each. Sizes are logical bytes, and a walk that reaches
its bound reports a lower bound. An ignored file that another program writes
between the host's check and the removal goes with the folder. Discard stays
available while a draft's first send is unanswered; the host may then hold a
thread nobody opened. The sidebar shows one draft row for a project, the
draft in front. Wait is still shown and does nothing.

Not exercised: a real window on this tree before the tag (the native
acceptance check runs on CI after the push, with its new mode for the first
time); Remove against a live host; the Worktrees view on a second volume;
live models; VoiceOver; Linux execution; and the installed app.

## Release check for v0.1.0-preview.22

v0.1.0-preview.21 was tagged and not published. On CI, the test in which the
production runtime edits Plait's own source and runs Cargo inside the command
sandbox reached its 240-second limit, in both the check and the installed
suite, and nothing else failed. That test built the core crate and ran its
whole suite, twice. It had taken 208 and 223 seconds on CI for the two
releases before, and the core crate grew. It now runs the tests of the module
the turn edits, and asserts that the regression test the first turn adds ran
and that the second turn, with the source restored, no longer has it. Here
the test takes 20 seconds where it took 56.

This release is v0.1.0-preview.21's tree with that one change to a test. The
record above describes what it contains, what its reviews found, the known
limits, and what was not exercised.

On 2026-11-19, on the commit before this record, `bash scripts/check.sh` exited
0 with the Git that CI's fixtures use first on the path: 951 Rust tests (275
desktop, 642 core, 18 host, 16 helper integration), 562 runtime tests,
TypeScript checks, and release builds. This record, the README's release
line, and the release number in two roadmap lines are the only changes after
it. Scenes and frame cost were not measured again: the change touches one
runtime test.

Not verified: that the test now fits CI's limit, until this tag's own run.

## Release check for v0.1.0-preview.23

On 2026-11-19, on the commit before this record, `bash scripts/check.sh` exited
0 with the Git that CI's fixtures use first on the path: 961 Rust tests (280
desktop, 647 core, 18 host, 16 helper integration), 584 runtime tests,
TypeScript checks, and release builds. The release-mode test build, which the
frame benchmark uses, compiled too. This record and the README's release line
are the only changes after it.

This release makes **Wait** work. A message sent to a busy checkout, from a
new thread or an existing one, can wait in a line the host keeps, and starts
when its turn comes. The stage was read before release by a reviewer who had
not written it, against the real runtime; each finding was confirmed by an
experiment before it was fixed, and each fix has a regression test.

- **Messages that were lost:** three findings. A waiting message's text was
  cleared from the message box when it was accepted and shown nowhere while it
  waited; if it then failed its check at the front of the line, or was
  cancelled, nothing gave the text back. It now returns to the draft. Stopping
  the host while a waiter was being admitted failed it for good; it now waits,
  held, for Resume. And a checkout held for an unconfirmed cleanup failed
  every waiter in its line in one pass; it now holds the line and says why.
- **An older build:** choosing Wait on a new thread saved a placement value
  that v0.1.0-preview.22 does not know, and that build then refused the whole
  saved workspace and turned sending off. Wait is now saved as the placement
  that build knows plus one field it ignores. This was run through
  v0.1.0-preview.22's own decoder and validation.
- **Work nobody released:** Resume in one thread also started another
  thread's follow-ups that had been held after a failed run. Resume now
  releases only messages waiting for the checkout.
- **Cost:** a page of thread summaries read each checkout's line once for
  every thread on it, 13 ms for 200 threads where a plain listing takes
  0.3 ms, on every summary change. It reads each line once: 0.5 ms.
- **Smaller ones:** a host that reported a queue capability this build did
  not recognise broke the thread list; cancelling a follow-up left other
  threads showing a stale place in line; and Waiting was drawn as a button
  that did nothing.

The reviewer found no path that starts a cancelled message, starts one twice,
or starts one in another folder, with other settings or with other text:
sixty timings of Cancel against the checkout coming free each ended either
cancelled and never run, or run once and stopped. A waiting message holds no
lock, and the order is the documented one.

Of the 588 scene images the previous release rendered, 586 are byte for byte
the same on this tree. Two differ, as intended: the busy project's choice,
where Wait can now be chosen. Six images are new: a thread waiting for its
checkout, a line held after a restart, and a cancelled wait, each at two
sizes.

Frame cost was measured against v0.1.0-preview.22, 90 frames a case, in five
interleaved pairs: the first two with nothing else building, the last three
while another test build was starting. No case is consistently slower. A long
conversation at rest costs 1.01 to 1.03 ms a frame on both trees, and about
2.3 ms while scrolling; Changes at rest 1.11 to 1.12 ms on both; twenty open
tabs 1.48 to 1.56 ms on both; the Worktrees view with fifty lanes 0.77 ms on
both. Switching between a long thread and a short one took 3.55 to 3.78 ms
against 3.57 to 3.81 ms; the first two pairs had it slower by 0.2 ms and the
next three did not. The streaming case ranged from 1.6 to 3.6 ms on both; it
follows the machine's power state and is reported, not used as a gate. Every
figure is a few hundredths higher than in the last release's record, on both
trees: the Mac was in use.

Known limits. While a message waits, the thread says Waiting and does not show
the message's text; Cancel gives it back to the draft. When a waiting message
fails and its thread's draft already holds other text, the message's text goes
to the clipboard, with a notice, and its images are not restored. The desktop
remembers sixteen accepted messages for each thread, and a checkout's line
holds thirty-two. A held message in an archived thread holds its line while
Resume and Cancel may be out of sight. After a restart nothing waiting starts
until Resume is pressed.

Not exercised: Wait, Cancel and Resume in a real window before the tag (the
native acceptance check has no item for them yet); live models; a provider
sign-out while a message waits, other than simulated; an outside program
holding the checkout for a long time; VoiceOver; Linux execution; and the
installed app.

## Releases after v0.1.0-preview.23

From the release after v0.1.0-preview.23, a preview is published by merging
to `main`, not by a hand-made tag (see [Releasing](RELEASING.md)). The tests
run once, on the pull request, by GitHub: the macOS check and the Linux
check. The build runs once, on `main`. The per-release records above end
here. What was reviewed, measured and left unverified for a change is written
in its pull request, and what stays true is kept in the sections of this
document that describe each check.

The first run of the real-window check as its own job, on a commit that held
the fix for Redo by shortcut, passed 42 of 46 items and skipped 4, with no
failure: the first run with none. Twenty-six of the passes needed a forced
frame and say so.
