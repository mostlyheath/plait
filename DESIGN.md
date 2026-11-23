# Plait desktop design

Plait is a native tool for working with coding agents. Design for clear
execution state, informed approvals, keyboard access, and review of real
changes. Use original application design and the [contribution rules](CONTRIBUTING.md#originality-and-licenses).
Show only data the host or client actually has; test scenes may inject sample
state. Do not imply that an action ran or succeeded without evidence.

Product rules live in [PRODUCT.md](PRODUCT.md) and agreed scope in the
[roadmap](docs/ROADMAP.md). This page says how the app looks and behaves, and
in what order the design is built. Each part carries one of four marks:

- **Built** — in the app today. Keep it working.
- **Planned** — presentation only. It needs no change to the protocol, to
  state the host stores, to permissions, or to Git writes. See
  [Build order](#build-order). Nothing is marked Planned at present: the
  parts that were are built.
- **Needs agreement** — needs a product or contract decision first. Do not
  build it, and do not show a placeholder or disabled control for it.
- **Direction** — where Plait is heading. Not specified enough to build.

Sizes are in points. Example names (`forecast`, `studio`, `main`) are samples.

## What makes it Plait

Three things hold in every view. A change that breaks one is the wrong change.

1. **One line.** A thread is a line with marks on it: a ring where a request
   starts, a filled node where a turn closes or shows its live state. The same
   line and marks join a project's threads in the sidebar, a thread's turns in
   the conversation, and a request's attempts. Draw no other kind of connector.
2. **Proof you can take back.** Every finished turn ends with what the host
   recorded: its steps, the files it changed, check results where checks
   exist, and Undo. The same record appears on the turn's closing line, in
   This turn, and in [Needs me](#needs-me--needs-agreement). Never describe a
   turn in words the host did not report.
3. **Yours.** Code font and size, theme, keys, and panel sizes belong to the
   person and stay on the device. Names on screen are the person's real
   machines, projects, and branches.

## Window

```
┌────────────┬─────────────────────────────────────────────────┐
│ ● ● ●   ▯  │ tabs                                            │ 40
├────────────┼─────────────────────────────────────────────────┤
│ New thread │ project / machine · branch    views   Chat│Work │ 36
│ Needs me 2 ├──────────────────────────────────┬──────────────┤
│ Find       │                                  │ This turn    │
│            │ conversation, capped at 760      │ changed      │
│ project    │                                  │ Undo         │
│  ● thread  │                                  │              │
│  ● thread  │ message box                      │ branch · N   │
├────────────┴──────────────────────────────────┴──────────────┤
│ machine · counts                                    Next: …  │ 26
└──────────────────────────────────────────────────────────────┘
```

| Region | Size |
| --- | --- |
| Tab bar | 40 high |
| Workspace bar | 36 high |
| Status line | 26 high |
| Sidebar | 240 wide, 272 when the window is wider than 1150; drags 200–480 |
| This turn | 280 wide; drags 220–560; shown only when the window is wider than 1150 |
| Conversation, message box, settings content | capped at 760 wide |
| List rows | 28 high |
| Pane header (Work view) | 30 high |
| Thread pane (Work view) | 420 wide at first; no pane under 320 wide or 160 high |

A thread tab has two views. **Chat view** is the default: the conversation
with its record beside it. **Work view** puts the same thread beside its
changes and files in panes. Keep project, machine, checkout, and thread
context visible in both. Side panels resize from their inner edges and can be
hidden. Preserve a readable workspace at narrow widths.

## Chat view

### Sidebar — Built

Top to bottom, on the navigation fill:

1. Three plain rows with no heading:
   - `New thread` — mint plus glyph, key hint at the right. Opens a draft tab.
     The row names no model; the draft's message box chooses it. New Thread
     with Model… stays in the menu and palette.
   - `Needs me` — sand dot and, at the right, the count in sand: threads on
     this machine for which `needs_attention` holds, leaving out archived
     threads. At zero, show a hint-gray dot and no number. Until
     [Needs me](#needs-me--needs-agreement) is built, the row turns the
     existing needs-me filter on and off and shows selected while it is on.
   - `Find anything` — search glyph, key hint. Opens the palette, which lists
     threads and commands. It replaces the filter field.
2. One group per project. The project row is as built: fold control, name,
   menu, and thread count when folded.
3. A project's threads hang on one vertical line, 1 wide, in the control
   outline color. Each thread is a 7-point node on that line with 3 points of
   the row's own fill around it, so the line never touches the node.

   | Thread | Node | Title | At the right |
   | --- | --- | --- | --- |
   | Working | blue | secondary | `Working`, blue |
   | Waiting for approval | sand | main | `Approve`, sand |
   | Failed | coral | main | `Failed`, coral |
   | Interrupted | sand | main | `Interrupted`, sand |
   | Waiting in the checkout queue | hint | secondary | the queue's label |
   | Finished, unread | mint | main, medium weight | age |
   | Anything else | hint | secondary | age |

   Title names a text role. A selected row adds the selection fill and main
   text. Say `Failed` for a thread whose run ended in an error, as the closing
   line does. Keep, as built, the harness mark, the worktree glyph, the
   actions control, rename in place, the `Archived` tag, hover text, and the
   accessible name, which states the thread's state and unread activity in
   words.
4. A last row, `Open project…`, in hint text with its key hint.

Nothing else lives in the sidebar. The machine, its connection, Update, and
Settings move to the [status line](#status-line--built). Archived threads
show through Show or Hide Archived Threads in the menu and palette.

### Tab bar — Built

A 40-point tab bar serves as the window bar, with native traffic lights,
sidebar toggle, tabs, overflow, and New tab. Tabs show titles and meaningful
state in words and color. Empty bar space moves the window.

Tabs retain their own view, scroll position, find state, and draft. A new
thread action creates a separate draft. Closing a tab does not stop its run.
Virtualize long lists and hidden tab content.

### Workspace bar — Built

Left: `project / machine · branch` in 12-point secondary text, with the full
path in hover text. Omit machine at narrow widths. Right, in order: the view
switcher (`Conversation`, `Changes N`, `History`, `Files`); a
`Chat │ Work` switch; and the toggle for This turn. Content with no subviews
omits this bar.

### Conversation — Built

The conversation is a log of turns on the line. Requests and replies have
visible author labels; requests use a raised neutral panel. Reasoning and tool
calls use compact disclosures. Preserve selection, copy, links, and scroll
position when history loads. Do not invent durations or outcomes.

A recorded turn closes with one line: the state word, the host-reported steps,
the changed files as links to the recorded diff, and Undo or Redo. The
controls reflect availability; explain conflicts and excluded paths. Recorded
diffs remain distinct from current Git changes and do not offer file or hunk
writes.

**Earlier turns fold.** A turn before the latest draws as two rows
on the line: its ring with the first line of the request and the age at the
right, then its node with the closing line unchanged. An earlier turn uses the
host’s recorded outcome, falls back to what the tab observed, and otherwise
folds when eligible.

- Never fold the latest turn; a turn that is running, failed, stopped, interrupted, or
  waiting for approval; a turn holding the current Find match; or a turn the
  person opened, for as long as the tab is open.
- The folded request is a disclosure: click, Enter, or Space opens it, and its
  accessible name says whether the turn is shown.
- Opening a turn keeps the activated row where it is on screen.
- Folding changes only what is drawn. History paging, selection, copy, and
  Find's "loaded messages only" note behave as before.
- Settings → Appearance has `Fold earlier turns`, on by default.

### Message box — Built

One raised outline in the control outline color, 12-point corners, capped at
760. Inside, two rows:

1. The draft, at the conversation's line height. Placeholder:
   `Write a message · @ files · / commands`. Attachments sit above the draft
   as built.
2. A settings row in 12-point secondary text: the harness mark, model, effort,
   and approval mode (or the harness's own permission mode), each opening the
   picker it opens today; then, at the right, context use when the host
   reports it, and Send. Stop is separate and appears during a run.

The strip under the box goes away. Everything it showed moves into the
settings row with the same pickers, lock reasons, hover text, and notices,
including the note that a run is using a different approval mode. Nothing is
dropped. When the row does not fit it wraps; the draft keeps its full width.

Draft tabs use the same box: model, effort, and permissions move into it,
while project and harness stay in the row above. Pending messages and their
actions stay above the box. Suggestions keep focus in the draft. Keep IME
composition from submitting and preserve unsent revisions.

### This turn — Built

This turn takes the place of the Git inspector's working-tree panel beside the
conversation. It is the same panel: same width, drag handle, key, and
narrow-window note. Its menu title becomes Toggle This Turn; the action ID
stays `view.toggle_inspector` so saved keys keep working.

Top to bottom:

1. `This turn`, with the turn's age at the right. Under it, one line of
   secondary text: the state word and steps as the host reports them
   (`Finished · 5 steps`, `Working`, `Failed`, `Stopped`, `Undone`).
2. `Changed` — one row per recorded path: a change letter in the code font
   (`A` mint, `M` blue, `D` coral, with the word in the accessible name), the
   file name, its folder in hint text, and `+N −N` when the record has them.
   Leave the counts out when it does not; never show zero. Activating a row
   opens that file's recorded diff. When the record lists more paths than are
   loaded, say how many more.
3. `Checks · N of M` — only once
   [checks](#checks-after-a-run--needs-agreement) exist.
4. `Undo` or `Redo`, with the availability, refusal text, and conflict steps
   of the closing line; and `Open in Work view` with its key hint.
5. A footer, 40 high, above a hairline: the branch name, which opens Switch
   Branch, and `N uncommitted` at the right, which opens Changes.

It shows the latest recorded turn. Activating the steps control on an earlier
turn's closing line shows that turn under the heading `Earlier turn`, with a
`Latest` control. While a run is under way it shows `Working` and adds the
changed files only when the record is ready. With no recorded turn to show (a
new thread, a plain folder, an older host) the panel shows the working tree as
built.

### Status line — Built

One line across the bottom of the window, in both views: navigation fill,
hairline above, 12-point secondary text. Left to right:

1. The machine: a dot and its name — mint when connected, `Connecting…`, or
   `Offline` with a sand dot. It opens the machine menu the sidebar used to
   hold. The window title still names a remote machine.
2. Counts for this machine's threads, each a dot and words: `N working`
   (blue), `N finished` (mint; finished and unread), `N failed` (coral),
   `N waiting for approval` (sand). Leave out a count of zero.
3. At the right, `Next: <thread> · <state>` with its key hint. It selects the
   first thread that needs the person: waiting for approval, then failed or
   interrupted, then finished and unread, oldest first within each. Show
   nothing here when no thread needs the person.
4. `Update` when an update is available, then the Settings control.

When the window is narrow, drop the counts first, then shorten the thread
title. The optional diagnostics line sits above the status line.

## Work view — Built

Work view shows the thread beside the things it changed. It is for thread
tabs. Enter and leave it with the `Chat │ Work` switch, the Toggle Work View
key, or `Open in Work view`.

```
┌───────────────────┬──────────────────────────────────────────┐
│ Thread      Ready │ Changes ⌄   This turn · 3 files   +69 −5 │ 30
├───────────────────┼──────────────────────────────────────────┤
│ conversation      │ a.rs +66   b.rs +2 −5                    │
│                   │                   This turn│Uncommitted  │
│                   │ diff                                     │
│ message box       │                                          │
└───────────────────┴──────────────────────────────────────────┘
        420                    the rest of the window
```

- **Layout.** The sidebar hides on entering and returns as it was on leaving;
  Toggle Sidebar still shows it. The workspace bar keeps the context and the
  switch and drops the view switcher, since panes carry the views. This turn
  is not shown. The status line stays.
- **Panes.** The first pane is the tab's thread, 420 wide. A tab's first visit
  adds one pane beside it: Changes on `This turn`, or on `Uncommitted` when
  there is no recorded turn. A tab holds at most four panes. Dividers are a
  hairline with a 6-point drag area; double-click evens the two sides.
- **Pane header.** 30 high, 12-point text: the kind menu (name and a small
  chevron), a secondary label that says what the pane shows and truncates, a
  short state at the right, then `Split right`, `Split down`, and `Close
  pane` as 22-point glyph buttons with those accessible names. The focused
  pane's header uses the selection fill; the others use the navigation fill.
- **Thread pane.** The conversation and its message box at pane width, with
  16-point side padding. Its header reads `Thread` and the thread's state
  word. It has no kind menu and no close control.
- **Kinds.** `Changes`, `History`, and `Files` follow the rules of the views
  of the same name, drawn at pane size. A tab shows each kind at most once;
  the kind menu lists the kinds not already open. List no kind that is not
  built.
- **Splitting.** `Split right` gives the new pane half this pane's width;
  `Split down`, half its height. The new pane takes the first of Changes,
  Files, History that is not open. With four panes, or no kind left, the split
  controls are disabled and their hover text says why.
- **Closing.** Closing the last pane beside the thread returns to Chat view.
- **Changes pane.** A row of file chips (name and counts; the selected chip
  uses the selection fill) with a `This turn │ Uncommitted` switch at the
  right, then the diff. `This turn` is the recorded diff of the latest turn,
  or of the earlier turn chosen in the thread, and offers no writes.
  `Uncommitted` is Changes as built: staged and unstaged diffs, their
  actions, and the commit bar. Show the commit bar only on `Uncommitted`.
- **Keys and focus.** Find acts on the focused pane. The Conversation,
  Changes, History, and Files actions focus the pane of that kind; when it is
  not open, the last focused pane beside the thread changes to it. Focus Next
  Pane and Focus Previous Pane move between panes. Tab order runs pane by
  pane.
- **Width.** Work view needs a window at least 800 wide. Below that the switch
  is disabled with hover text that says so, and a tab in Work view draws as
  Chat view until the window is wide enough again.
- **Memory.** A tab keeps its panes, sizes, and each pane's own state while
  the window is open. Tabs restored at launch open in Chat view until the host
  stores pane layouts, which [needs agreement](#more-in-work-view--needs-agreement).

## Needs me — Needs agreement

A tab that lists every thread on this machine that needs the person, so
finished and failed runs can be reviewed one after another.

- Left, 320 wide: rows grouped under `Waiting for approval`, `Failed`, and
  `Finished`, each with title, project, state word, and age.
- Right: the selected thread's last request, closing reply, closing line, and
  recorded diff. A thread waiting for approval shows its approval card with
  the same choices and availability checks as the conversation.
- Keys act only while the list has focus, and a hint row names them: `J` and
  `K` move, Enter opens the thread, `Y` keeps the turn, `U` undoes it, `R`
  tries again.
- Empty state: `Nothing needs you.`

To agree first: a new kind of saved tab (older builds read saved tabs); what
`Y` does beyond marking read (staging a turn's files waits for staging by
turn); and `R`, which waits for [attempts](#attempts--needs-agreement).

## Checks after a run — Needs agreement

Commands the person selects for a project; the host runs them after a turn and
stores the results with the turn's record.

- Closing line: `N of M checks` after the steps.
- This turn: `Checks · N of M`, one row per check — a mark, the command in the
  code font, and at the right what the host reports (exit state, duration).
- Changes pane, on `This turn`: the same rows under `Checks after this run`.
- States, each in a word and a mark: passed (mint), failed (coral), running
  (blue), not run (hint). Never show a check as passed without a stored
  result, and do not summarize output the host did not parse.

To agree first: where checks are defined, the permissions they run under, how
results are stored and bounded, and the protocol.

## Attempts — Needs agreement

This is the proposed answer to the roadmap's
[prompt tree](docs/HOST-FIRST.md#the-prompt-tree) exploration: a retry or an
edit of a request is another **attempt** of that request. Attempts are
siblings, not separate kinds of branch.

- The request's author row shows `Attempt 2 of 2` at the right, with previous
  and next controls.
- The other attempts hang under the request as one-line stubs on a short
  branch of the line: a small ring, the attempt's number and first line, its
  state (`Undone`), `N steps · N files`, and `Compare`.
- Choosing an attempt moves the thread to it; files change only by the host's
  undo and redo rules.

To agree first: the tree's model, how it spans worktrees, and files changed
since an attempt was recorded.

## More in Work view — Needs agreement

- **Saved layouts.** Pane layouts stored per tab with the host's per-device
  state, and whether a tab reopens in Work view. Saved state and protocol.
- **Preview pane.** A rendered document or page. Waits for the previews
  milestone.
- **Output pane.** The output of a command the agent ran, read-only, with no
  input. Agree that this is not an embedded terminal.
- **Another thread** in a pane. Today only the active workspace keeps loaded
  content and a transcript watch.
- **Needs me** as a pane kind, after the tab exists.

## Other views — Built

- Worktrees use a branch graph with neutral lanes and nodes, state colors,
  readable branch and thread labels, and a detail pane. Place detail beside the
  graph when wide and below it when narrow. Name unknown or bounded measurements
  rather than displaying a false zero.
- Settings and Project settings are centered window modals over the workspace.
  Preserve the underlying selection and draft, keep focus in the modal, and
  return focus on dismissal. Opening another project's settings does not select
  that checkout.

## Interaction — Built

These rules hold for built and planned parts alike.

- Use native menus, traffic lights, and folder picker. Show keyboard focus and
  accessible names, including full paths and state where visible text truncates.
  Do not rely on color alone. Keep long lists virtualized.
- Use neutral selection and hover fills. Reserve pills for meaningful metadata.
  Name loading, offline, failure, and refusal states and give a useful recovery
  action where one exists. Explain disabled controls in hover text.
- Image attachments show thumbnails and removal before sending. Show upload,
  failure, and refusal states without silently losing a draft. Follow the
  [host contract](docs/HOST.md#protocol-version-6) for limits.
- Approvals show the exact effect, risk, protected paths, and explicit choices.
  Keyboard shortcuts use the same availability checks as the buttons.
- Changes separates staged and unstaged diffs. File rows name status and path;
  diff actions appear where applicable. Keep commit results and unavailable
  branch actions legible. History shows commit identity, message, and patch.
- Files is read-only. Editor and terminal handoffs use the configured choice or
  a custom command with placeholders; report failures in plain language. Remote
  file handoffs are unavailable as described in the [remote guide](docs/REMOTE.md).
- Settings place controls at their scope: thread choices in the message box,
  project choices in Project settings, and app or host choices in Settings.
  Mask credentials and make reveal explicit. Confirm trusted command access
  with its consequences.
- Find shows match count, navigation, and current match without relying on color.
  State when conversation results cover only loaded messages. The palette and
  pickers use a centered search field, compact rows, and keyboard hints.

## Keys

New actions, added to `ACTIONS` and to the [shortcut list](docs/SHORTCUTS.md)
in the same change. None of the defaults is taken today.

| Default | Action | ID |
| --- | --- | --- |
| Cmd-Backslash | Toggle Work View | `view.toggle_work` |
| Cmd-Option-J | Next Thread Needing Me | `thread.next_needing_me` |
| Cmd-Option-Right | Focus Next Pane | `pane.next` |
| Cmd-Option-Left | Focus Previous Pane | `pane.previous` |
| — | Split Pane Right | `pane.split_right` |
| — | Split Pane Down | `pane.split_down` |
| — | Close Pane | `pane.close` |

Cmd-L keeps its ID and toggles This turn. Every other shortcut is unchanged.

## Tokens

Colors are semantic roles in `plait_client::theme::Palette`. The default dark
palette is Stillwater:

| Role | Color |
| --- | --- |
| Background / navigation / panel | `#1f222e` / `#242835` / `#2b303f` |
| Border / control outline / selection | `#313547` / `#464d66` / `#2e3243` |
| Text / secondary / hint | `#dfe3ed` / `#b0b7cb` / `#9da5bf` |
| Addition / selected option / passed / finished and unread | mint `#70d689` |
| Running / modification / link / focus | blue `#74cde2` |
| Thread identity / keyword | violet `#ec8ac7` |
| Attention / error, failed, deletion | sand `#eec96e` / coral `#ec7368` |

Themes map to the same roles. Maintain contrast floors of 7:1 for body text,
4.5:1 for secondary text, and 3:1 for hints and accents. Accent colors carry
state rather than decoration, and a state always has a word or a mark beside
its color. Diff lines use a gutter mark and a faint tint. Code uses semantic
syntax colors; plain identifiers remain main text.

The line is 1 wide in the control outline color, so it reads beside dense
text. A request's ring is 9 across; a node is 7. Leave clear space between the
line and a mark.

Use system sans-serif at 11–15 points, with 13-point body and navigation text
and 12-point text in the workspace bar, pane headers, and status line.
Conversation text uses 21-point line height. Code, paths, and commands use the
device's code font, the system monospaced font unless the person names one, at
12 points. Settings → Appearance has `Code size`, 12 to 14,
because some faces run small. Conversation, message box, and settings content
are capped at 760 points. Spacing follows a 4-point base. Preserve gradients
in supplied brand artwork only; avoid decorative gradients, shadows, and cards.
Render timings are an optional diagnostic, not permanent chrome.

## Direction

Where the design is heading. Each idea reuses the line and the record, and
each is held back by a product rule named here. Until that rule changes by
agreement, do not build, mock in the shipped app, or hint at any of them.

- **Benches.** A named pane layout with its checks, kept with the project, so
  a kind of work (code, a demo, a guide) opens the way it is done. Waits for
  checks and saved layouts.
- **Kept while away.** A standing promise about a project: one sentence, the
  check that proves it, and what an agent may do alone to keep it. A
  `While you were away` view lists what was kept, with each fix's record.
  Held back by: no hidden execution; unattended automations are not agreed.
- **Why is this line here.** Files marks the lines a turn wrote. A story pane
  follows one line back: what was asked, the attempt that was dropped, the
  attempt that wrote it, the checks that passed, the commit. Held back by:
  tracing lines to turns is not agreed.
- **Several attempts at once.** More than one agent attempts a request; the
  line shows them side by side, chosen by checks first and smallest change
  second. A time marker shows the code as it was at any turn. Held back by:
  no agent orchestration.
- **Receipts.** Each commit carries the request that led to it and the proof
  that it worked, stored in the repository, with `Run the proof again`. Held
  back by: the Git write allowlist.

## Build order

**Steps 0 to 9 are built** and were released in `v0.1.0-preview.80`. They were
checked by tests and by scenes rendered on Linux; the native app was not
inspected at a wide and a narrow window before release, which the list below
asks for. That inspection, and what the scenes already show needs fixing, is
tracked in [#130](https://github.com/mostlyheath/plait/issues/130). The order
is kept here as the record of how the design was built and as the pattern for
the parts that still need agreement.

Each step is one small pull request with one owner, as
[Contributing](CONTRIBUTING.md) requires, and can be built on its own unless
it names a step it follows. After step 0, steps 1, 3, 4, 5, 6, and 7 can be
built at the same time; then 2, then 8, then 9. Paths are under
`crates/plait/src/ui/` unless they name a crate. `mod.rs`, `actions.rs`, and
`theme.rs` are shared by several steps: keep edits there small.

Every step is done when:

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, and `cargo test --workspace` pass.
- State and its transitions live in `plait-client` with unit tests and no
  GPUI; the desktop crate only presents them.
- Test scenes for each new state are added under `snapshot/` at both window
  sizes, and images from `bash scripts/snapshots.sh` are in the pull request.
- The native app was built and inspected in a wide and a narrow window.
- Every new control has a visible focus state, an accessible name, and a
  keyboard path, and no shipped view shows sample data.
- The [guide](docs/GUIDE.md) and [shortcut list](docs/SHORTCUTS.md) match.
- No change was made to the protocol, to state the host stores, to
  permissions, or to Git writes. A new device preference is optional, and a
  preferences file saved before it existed still loads.

**0. Record the decisions.** Documentation only; steps 1–9 follow it. In the
roadmap, add the layout work to Now, and move "Show conversation, files, and
changes side by side" out of Proposed as Work view with Thread, Changes,
History, and Files panes. Everything else in Proposed stays there.

Chat view:

1. **Sidebar rows and thread marks.** The three top rows, nodes on the line,
   the row states, and `Open project…` as the last row; remove the filter
   field and chips. In `navigation.rs` (`actions`, `navigation_controls`,
   `host_thread_row`, `tree`) and `theme.rs`. Done when each row state in the
   table has a scene and the count matches `needs_attention`.
2. **Status line.** Follows 1. Add the line and Next Thread Needing Me; move
   the machine row, connection state, Update, and Settings out of the
   sidebar. A new `status_line.rs`, the root layout in `mod.rs`,
   `navigation.rs` (`machine_row`, `host_footer`), `actions.rs`; the ordering
   rule in `plait-client/src/navigation.rs` with tests. Done when connected,
   connecting, offline, update available, and nothing-needs-me have scenes.
3. **Message box.** Move the strip's controls into the box for threads and
   drafts. In `thread_controls.rs` (`thread_environment`), `composer/`, and
   `new_thread_view.rs`. Done when every control, lock reason, and notice
   the strip had is reachable in the box, at both window sizes.
4. **This turn.** Replace the working-tree panel beside the conversation,
   keeping it as the fallback. In `conversation.rs` (`inspector`), a new
   `this_turn.rs`, `turn_records.rs`, and the toggle's titles in `mod.rs` and
   `actions.rs`. Done when a finished, working, failed, undone, earlier, and
   no-record turn each have a scene, and Undo refuses exactly where the
   closing line does.
5. **Folded turns.** In `live_conversation.rs`, with which turns are open
   held in `plait-client` and the setting in
   `plait-client/src/preferences.rs` and `appearance.rs`. Done when Find
   opens a folded match, opening a turn does not move the page, and a long
   thread scrolls as fast as before.
6. **Code size.** The setting, read wherever the code font is. In
   `plait-client/src/preferences.rs`, `appearance.rs`, `theme.rs`. Done when
   an older preferences file still loads at 12.

Work view, in order:

7. **Pane model.** A plain tree of splits in a new
   `plait-client/src/panes.rs`: split right and down, close, resize, change
   kind, focus next and previous, with the limits above. No GPUI. Done when
   tests cover each operation and each limit.
8. **Work view shell.** The switch and Toggle Work View, hiding and restoring
   the sidebar, the Thread and Changes panes with headers, divider drag, the
   `This turn │ Uncommitted` switch, and the width rule. Follows 7; follows 4
   for `Open in Work view`. In `mod.rs`, `thread_workspace.rs`, `changes.rs`,
   `turn_review.rs`, and a new `work_view.rs`. Done when a thread tab enters
   and leaves by the switch, the key, and the button; the sidebar returns as
   it was; the commit bar shows only on `Uncommitted`; and under 800 wide the
   switch is disabled and says why.
9. **Pane kinds and controls.** The kind menu with History and Files, split
   and close, pane focus keys, Find and the view keys acting on the focused
   pane, and per-tab memory while the window is open. Follows 8. Done when
   each kind opens at most once, a fifth pane is refused with its reason,
   closing the last pane returns to Chat view, and two tabs keep different
   layouts.

Not started until agreed, and then specified here first: Needs me, checks
after a run, attempts, and the further pane kinds and saved layouts.
