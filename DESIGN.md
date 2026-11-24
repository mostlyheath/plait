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
  state the host stores, to permissions, or to Git writes. Each planned part
  names the step of the [build order](#build-order) that builds it. Until
  that step merges, the app keeps the part listed under
  [Built today, to be replaced](#built-today-to-be-replaced).
- **Needs agreement** — needs a product or contract decision first. Do not
  build it, and do not show a placeholder or disabled control for it.
- **Direction** — where Plait is heading. Not specified enough to build.

Sizes are in points. Example names (`forecast`, `studio`, `main`) are samples.

## What makes it Plait

Three things hold in every view. A change that breaks one is the wrong change.

1. **One set of marks.** A thread's state is a mark and a word, and they are
   the same mark and word on its sidebar card, its tab, its card in Every
   thread, and the header: a turning ring while it works, sand when it waits
   for the person, coral when it failed, mint when it finished and has not
   been read. A turn is a ring where a request starts and a filled node where
   the turn closes; where turns are drawn [on the line](#turns-on-the-line--planned-step-19),
   that line and those marks are the only connector.
2. **Proof you can take back.** Every finished turn ends with what the host
   recorded: its steps, the files it changed, check results where checks
   exist, and Undo. The same record appears on the turn's closing line and in
   the panel's Changes tab. Never describe a turn in words the host did not
   report.
3. **Yours.** Fonts and sizes, theme, how turns are drawn, keys, and panel
   sizes belong to the person and stay on the device. Names on screen are the
   person's real machines, projects, and branches.

## Rules for drawing

These replace the earlier rules "no decorative animation" and "avoid
decorative gradients, shadows, and cards". They hold for built and planned
parts alike once [step 11](#11-tokens-and-controls) merges.

- **Two grounds.** The window is the navigation fill. The content is one
  sheet on it, in the background fill, with its own corners. The sidebar sits
  directly on the navigation fill. Do not rule the window into boxes: use the
  step between the two grounds, then a hairline, and only then an outline.
- **Four tones of text.** Emphasis, main, secondary, and hint, each clearly
  dimmer than the one before ([contrast bands](#text-and-surfaces)). A fifth,
  faint, is for decoration and disabled controls only and never carries
  information by itself. A screen in which every label has the same tone is
  wrong: decide what the person reads first and dim the rest.
- **One emphasised action.** A view has at most one primary button: the
  action the person most likely came for (Send, Allow once, Commit, the
  header's Git step, New thread in Every thread). Everything else is a
  standard or quiet button. A button looks like a button: it has a fill or an
  outline at rest, or gains one on hover. Text that only looks like a label
  is not a control.
- **Controls answer.** Hover changes the fill at once, with no fade in.
  Pressing dims the control until release. Keyboard focus draws the focus
  outline. A disabled control is dimmed and says why in hover text.
- **Elevation means floating.** A shadow marks something that floats above
  the content and will go away: a menu, a picker, the palette, a modal, a
  toast, the focused card in Every thread. The message box is the one
  resting element with a shadow, because the conversation scrolls under it.
  Nothing else has a shadow. No gradients except in supplied brand artwork.
- **Motion is feedback.** Nothing moves for decoration. The only motion that
  repeats is the running mark. Other motion, where a step adds it, answers
  an action of the person, lasts 200 ms at most, never delays the action,
  and is skipped under Reduce Motion. Layout does not animate until
  [step 26](#26-motion-measured) has measured its cost.
- **Colour is state.** Mint, blue, violet, sand, and coral carry state and
  code. Chrome is neutral. A state always has a word or a mark beside its
  colour.
- **Say it once.** A thread's state appears once in each region. Do not
  repeat in the header what the sidebar card already says about another
  thread, and do not add a second place that counts the same thing.

## Window

```
┌───────────────┬─────────────────────────────────────────────────┐
│ ● ● ●      ▯  │ title  project  branch       state  ±  Push     │ 46
│ New thread    ├──────────────────────────────┬──────────────────┤
│ Needs you   2 │                              │ Changes Files …  │ 36
│ Find anything │ conversation, capped at 760  │                  │
│               │                              │ panel            │
│ forecast      │                              │                  │
│ ┌───────────┐ │ ┌ 2 files changed +69 −5 ──┐ │                  │
│ │ thread    │ │ │ message box              │ │                  │
│ └───────────┘ │ └──────────────────────────┘ │                  │
│ studio  ⚙     │                              │                  │
└───────────────┴──────────────────────────────┴──────────────────┘
     288                    the sheet: 8 from the top, right, bottom
```

The window has two regions and no bar that spans it: the **sidebar** on the
navigation fill, and the **sheet**, which holds one **header**, the
conversation with its message box, and the **panel** when it is open. There
is no tab row above the header, no workspace bar, and no status line.

| Region | Size |
| --- | --- |
| Sidebar | 288 wide; drags 200–480; 236 when the window is under 760 wide |
| Sidebar top strip | 44 high; holds the traffic lights and the sidebar toggle |
| Sheet | 8 from the window's top, right, and bottom edges; 12 corners; 1 outline in the border role |
| Header | 46 high, inside the sheet, hairline below |
| Conversation, message box, settings content | capped at 760 wide |
| Panel | half the sheet's width at first; no narrower than 360; leaves the conversation at least 420; drags from its inner edge |
| Panel tab row | 36 high |
| Sidebar action rows and foot rows | 30 high |
| List rows in the panel and settings | 28 high |

- **Sidebar hidden.** The sheet fills the window, with no margin and square
  corners. The header starts 84 from the left to clear the traffic lights (8
  in full screen) and gains the sidebar toggle.
- **Narrow.** When the sheet is under 1020 wide, an open panel covers the
  conversation and its close control returns to it. When the window is under
  760 wide, the header drops the project and branch.
- **Dragging the window.** The sidebar's top strip and any empty part of the
  header move the window. Double-click follows the system setting.
- **Corners.** Children do not clip to the sheet's corners. A child that
  reaches a corner of the sheet (the panel at the right, the header at the
  top) either has no fill of its own or takes the same radius on that corner.

Keep project, checkout, and thread context visible at every width. The
window uses one machine at a time; its name is in the sidebar's foot.

## Built today, to be replaced

The app draws these until the named step merges. The [guide](docs/GUIDE.md)
describes their controls.

| Part | Today | Replaced by |
| --- | --- | --- |
| Tab row | 40 high across the window: traffic lights, sidebar toggle, thread tabs with a state word, overflow list, New tab | [Header](#header--planned-step-16), step 16 |
| Workspace bar | 36 high: `project / machine · branch`, the view switcher (`Conversation`, `Changes N`, `History`, `Files`), the `Chat │ Work` switch, the This turn toggle | Header, step 16; its views move into the [panel](#panel--planned-step-15), step 15 |
| Status line | 26 high across the window: machine, counts, `Next:`, Update, Settings | [Sidebar foot](#foot--planned-step-14), step 14 |
| Sidebar rows | 28-high rows on one vertical line, a 7-point node per thread, state word or age at the right; a `Needs me` row that filters the list | [Thread cards](#thread-cards), step 17; [Every thread](#every-thread--planned-step-18), step 18 |
| This turn | 280-wide panel beside the conversation (220–560), only when the window is wider than 1150 | Panel, step 15 |
| Work view | The thread at 420 beside up to four panes (`Changes`, `History`, `Files`), entered by the `Chat │ Work` switch; needs 800 of width | Panel, step 15; removed in step 24 |
| Requests and replies | `You` and `Plait` labels on the line; requests in a raised panel | [Bubbles](#bubbles--planned-step-19) by default, step 19; the line stays as a setting |
| Tool calls | One row each; five or more in a row fold to `N steps · 3 read, 2 edit` | [Folded work](#folded-work--planned-step-20), step 20 |
| Approval | A card above the message box | [In the message box's place](#approval--planned-step-21), step 21 |
| Changed files | A collapsible card above the message box | [A strip joined to it](#changed-files-strip--planned-step-21), step 21 |

## Sidebar

On the navigation fill, with 8 of padding and no border at its right edge.
Top to bottom: the top strip, three action rows, the project groups, `Open
project…`, and the foot.

### Top strip and action rows — Planned (step 14)

The top strip is 44 high. The traffic lights sit where they sit today; the
sidebar toggle is at the strip's right end. The rest of the strip moves the
window.

Three rows, 30 high, 7 corners, 13.5-point secondary text, a 16-point glyph
at the left, and the key hint at the right shown only while the row is
hovered or focused:

- `New thread` — mint plus glyph. Opens a draft, as built.
- `Needs you` — the count at the right in sand and medium weight: threads on
  this machine for which `needs_attention` holds, leaving out archived
  threads. At zero, a hint-tone `0`. Until step 18 it turns the needs-me
  filter on and off as today and shows selected while on. From step 18 it
  opens and closes [Every thread](#every-thread--planned-step-18).
- `Find anything` — search glyph. Opens the palette, as built.

### Project groups — Planned (step 17)

One group per project, 12 apart.

- **Label row**, 26 high: fold chevron in the faint tone, the project's name
  in 11-point medium hint text, the thread count when folded, and at the
  right, shown on hover or focus, `New thread in <project>` and the project
  menu (as built: Open project, Worktrees, Linear issues, Project settings…,
  Remove from sidebar). A missing folder keeps its `Missing` tag in sand.
- **Threads**, as cards, 2 apart. Within a project, threads that need the
  person come first (waiting for approval, then failed or interrupted, then
  finished and unread), then working threads, then the rest by last update,
  newest first.

### Thread cards

A card is one thread. 7 corners; 7 top, 9 sides, 8 bottom of padding; lines 3
apart. The second and third lines are indented 15, under the title.

```
● Retry rate-limited forecast requests       Approve
   ⑂ retry-429
   ◇ Model name                                   6m
```

| Line | Left | Right |
| --- | --- | --- |
| 1 | The state mark, then the title in 12-point medium emphasis text, truncated | The state word in 11-point medium, in the state's colour, when the thread needs the person or waits in the queue |
| 2 | A branch glyph and the branch in the code font, 11-point hint text. Drawn only when the client knows the thread's own branch: a thread in a worktree Plait created | — |
| 3 | The harness mark at 11 and the model's name as the model picker shows it, 11-point hint text | The age of the last update, in the code font and the hint tone |

State mark and word, by thread state (`ThreadRowState`):

| State | Mark | Title | Word |
| --- | --- | --- | --- |
| Working | running mark, blue | emphasis | — |
| Waiting for approval | 7-point dot, sand | emphasis | `Approve`, sand |
| Failed | dot, coral | emphasis | `Failed`, coral |
| Interrupted | dot, sand | emphasis | `Interrupted`, sand |
| Waiting in the checkout queue | hollow dot, hint | secondary, regular weight | the queue's label, hint |
| Finished, unread | dot, mint | emphasis | `Done`, mint |
| Anything else | hollow dot, faint | secondary, regular weight | — |

- Selected: the selection fill and a 1-point inner outline in the control
  outline role. Hover: the hover fill, at once.
- A line with no data is not drawn. Never draw a branch the client inferred,
  a count of zero, or an elapsed time the host did not report.
- Keep, as built: rename in place, the `Archived` tag, the thread menu
  (`thread-actions` and right-click: Rename thread…, Archive or Unarchive,
  the worktree items), draft rows, hover text, and the accessible name,
  which states the thread's state and unread activity in words.
- Cards have different heights, so the list is a measured, virtualized list,
  not a list of uniform rows.

More on a card (the branch of every thread, change totals, elapsed time,
what the agent is doing, helper agents) [needs agreement](#thread-summary-additions).

### Foot — Planned (step 14)

Below the groups: `Open project…`, 30 high, in hint text with its key hint;
then a hairline; then one row, 30 high:

- The machine: a dot and its name, mint when connected, `Connecting…`, or
  `Offline` with a sand dot. It opens the machine menu the status line holds
  today. The window title still names a remote machine.
- `Update`, a standard button, only when an update is available.
- The Settings control, an icon button, at the right.

The optional diagnostics line stays at the bottom of the sheet.

## Header — Planned (step 16)

One row, 46 high, at the top of the sheet: 16 of padding at the left, 10 at
the right, 8 between items. Left to right:

1. With the sidebar hidden: the traffic-light inset, then the sidebar toggle.
2. **The thread.** One of two forms, chosen in Settings → Appearance →
   `Thread tabs` (`Hide`, the default, or `Show`):
   - *Title.* The thread's title in 14-point semibold emphasis text,
     truncated, with the full title in hover text. Then two chips, 22 high,
     6 corners, hover fill, 11-point secondary text with a 12-point glyph:
     the project, and the branch (with the worktree glyph when the thread has
     its own worktree; it opens Switch Branch, as the branch control does
     today). Hover text on the project chip gives the full checkout path.
   - *Tabs.* The tab strip as built (order, drag to reorder, overflow list,
     New tab, close control, bookmarks), drawn as pills 30 high with 7
     corners in place of the title and chips. A tab shows its state mark and
     title and no state word; the active tab has the selection fill and a
     1-point outline. The project and branch chips move to a row under the
     message box.
3. Flexible space, which moves the window.
4. **State**, only when the open thread is working, waits for the person, or
   failed: a pill 20 high, fully rounded, 11-point medium, in the state's
   colour on a 14% tint of it, with the state mark and word (`Working`,
   `Needs approval`, `Failed`, `Interrupted`).
5. **Panel toggle.** A quiet button with the changes glyph and the thread's
   uncommitted totals (`+69 −5`) when the client has them, the glyph alone
   when it does not. Selected while the panel is open. Hover text: `Toggle
   Panel` and its key.
6. **Git step** — [step 22](#22-git-step).

Hiding the tabs changes only what is drawn. The tab model, its saved order,
its keys (New Tab, Close Tab, Reopen, Next and Previous Tab, Cmd-1 to 9), and
the tab list all work as built; closing the open tab shows the one before it.

Content that is not a thread (Worktrees, Linear issues, a draft) shows its
own title in the title form and has no state, panel toggle, or Git step.

While [Every thread](#every-thread--planned-step-18) is open the header is
its bar instead.

## Conversation

A column capped at 760, centred in the space beside the panel, with 28 of
padding above, 24 at the sides, and 12 below. Reading text is 14.5 on a
23-point line. Preserve selection, copy, links, Find, and scroll position
when history loads. Do not invent durations or outcomes.

How a request and a reply are drawn is the person's choice, in Settings →
Appearance → `Turns`: `Bubbles` (the default) or `On the line`.

### Bubbles — Planned (step 19)

- **A request** is a bubble at the right: at most 84% of the column wide,
  the raised fill, a 1-point outline in the control outline role, 12 corners
  with 4 at the bottom right, 10 by 14 of padding, emphasis text. No author
  label. Attached images sit inside it, above the text.
- **A reply** is plain text at the full width of the column, with no fill,
  no label, and no mark. `Copy message` appears at its top right on hover or
  focus, as built.
- Turns are 22 apart; the parts of a turn (request, work, reply, closing
  line) are 14 apart.
- The accessible name of each still says who wrote it.

### Turns on the line — Planned (step 19)

The built drawing, kept as the other setting: `You` and `Plait` labels, the
request in a raised panel at full width, a 9-point ring on the line at each
request, and a 7-point filled node where the turn closes. The line is 1 wide
in the control outline role, 12 from the column's left edge. Restyle it with
the tokens; change nothing else.

### Work — Built

Reasoning and tool calls are single rows, 26 high, 12-point: a 13-point glyph
or the running mark, the verb, the target in the code font, and facts
(`+66`, `40 lines`, `Failed`). A row opens in place to its detail. While a
turn runs, its rows show as built: the latest three stay visible and earlier
ones fold to `N earlier steps`. The state line under a running turn says what
the host reports (`Thinking…`, `Writing…`, `Running tools…`, `Waiting for the
model…`) beside the running mark.

### Folded work — Planned (step 20)

When a turn finished, everything between its request and its final reply
(reasoning, tool calls, and any replies in between) folds into one row:

```
›  9 steps · 3 read, 2 edit, 1 command
```

- 28 high, 12-point hint text, a chevron that turns when open. It reads the
  host's steps as the group label does today; it gives a duration only once
  the host [reports one](#turn-timing).
- Opening it shows the rows in order, indented 18 behind a 1-point guide in
  the control outline role. It opens in place and keeps the activated row
  where it is on screen.
- Fold only a turn whose outcome is `Finished`. A turn that failed, was
  stopped, or was interrupted keeps its work open, so the person sees where
  it stopped. A running turn is never folded.
- A turn with no rows before its final reply has no such row.
- The current Find match opens the fold that holds it. A fold the person
  opened stays open while the tab is open.
- This replaces folding five or more tool calls of a finished turn. It does
  not replace [folded earlier turns](#earlier-turns-fold--built), which fold
  a whole turn.
- Settings → Appearance has `Fold finished work`, on by default.

### Closing line — Planned (step 20)

A recorded turn closes with one line, 12-point, at least 30 high, which
wraps when the column is narrow:

```
✓ Finished · 9 steps    3 files +146 −12                Undo   Review
```

1. A check glyph in mint and `Finished` in medium emphasis text; or `Failed`
   (alert glyph, coral), `Stopped` or `Interrupted` (sand), `Undone` (hint).
   The word comes from the record's outcome and state, on every closed turn.
2. The host-reported steps in hint text.
3. `N files` with the record's totals, a quiet control that opens the list
   of paths under the line: one row per path, 28 high, with the change
   letter (`A` mint, `M` blue, `D` coral, the word in the accessible name),
   the file name in the code font, its folder in hint text, and `+N −N` when
   the record has them. Activating a row opens that file's recorded diff in
   the panel. When the record lists more paths than are loaded, say how many
   more. With no paths: `no files changed`.
4. At the right, `Undo` or `Redo` as a quiet button, with the availability,
   refusal words, and conflict steps built today; then `Review`, a standard
   button that opens the panel on Changes for this turn.

In Bubbles the line has a hairline above it. On the line it starts at the
closing node, as built. Recorded diffs remain distinct from current Git
changes and do not offer file or hunk writes. Older hosts keep their line.

### Earlier turns fold — Built

A turn before the latest folds to its request's first line and its closing
line, by the rules built today: never the latest turn, a turn that is
running, failed, stopped, interrupted, or waiting for approval, a turn
holding the current Find match, or a turn the person opened. In Bubbles the
folded turn is one row, 32 high, 7 corners, 12-point hint text: a chevron,
the first line of the request, and at the right `N files +a −r`. On the line
it is drawn as built. `Fold earlier turns` stays in Settings → Appearance.

## Message box

Capped at 760 and centred under the conversation, with 6 above, 24 at the
sides, and 18 below. Suggestions keep focus in the draft. Keep IME
composition from submitting and preserve unsent revisions.

### Box — Built, restyled in step 21

One raised box: the panel fill, a 1-point outline in the control outline
role, 12 corners, and the box shadow. With focus inside, the outline takes
the hint tone and a 3-point ring in the focus colour at 13% surrounds it.
Inside, two rows:

1. The draft, at 14.5 on a 21-point line; at least 42 high and at most 200,
   then it scrolls. Placeholder, in hint text: `Write a message · @ files ·
   / commands`. Attachments sit above the draft as built.
2. The settings row, 12-point secondary text, each control 24 high with 6
   corners and the hover fill on hover: the harness mark and model, effort,
   and approval mode (or the harness's own permission mode), each opening
   the picker it opens today; then, at the right, context use when the host
   reports it, and Send. `Full access` is drawn in sand. Lock reasons,
   notices, and the note that a run is using a different approval mode stay
   as built. When the row does not fit it wraps; the draft keeps its width.

Send is the primary button: 28 square, 8 corners. With nothing to send it is
disabled. While a run is active and the draft is empty it is Stop, in coral.
Keys are as built: Enter sends or queues, Option-Enter steers, Escape twice
stops.

Queued and pending messages stay above the draft as built, as rows 28 high
with the hover fill: the mode tag, the text, Edit, and Cancel.

### Changed files strip — Planned (step 21)

The changed-files card becomes a strip joined to the top of the message box:
inset 10 from the box's sides, 12 corners at the top only, the navigation
fill, a 1-point outline in the border role. Closed, it is one row 28 high in
11-point hint text: a chevron, `N files changed`, the totals, `Uncommitted`,
and at the right the file names in the code font, truncated. Open, it lists
up to six rows before scrolling, each like a row of the closing line's list;
activating one opens that file in the panel's Changes tab on `Uncommitted`.
Its data, its refresh, and `Changed files above the message box` in
Settings → Appearance stay as built.

### Approval — Planned (step 21)

A thread waiting for approval shows the approval where the message box is,
and the message box is not drawn. It is the one thing on screen that waits
for the person, so it is the heaviest thing on screen:

- 12 corners, the panel fill, a 1-point outline in sand at 55%, a 4-point
  ring in sand at 12%, and the floating shadow; 14 of padding, parts 10
  apart.
- A sand dot and the question in 13.5-point semibold: `Run this command?`
  or `Make this edit?`, by the approval's kind. At the right, in hint text,
  which harness is waiting.
- The exact call in the code font, in a box with the navigation fill, a
  border, and 7 corners; then the details, the risk in sand, and the dirty
  or protected paths, in 12-point secondary text, with the words built
  today.
- `Allow once`, the primary button, with its key hint; at the far right,
  `Deny once`, a quiet button, with its key hint. The same choices, keys
  (`approval.allow_once`, `approval.deny_once`), availability checks, and
  repeat guard as built. No other choice is added.
- Under the card, `Reply instead`, a quiet button, draws the message box
  below the card for this approval. The draft is never lost by hiding the
  box: the box keeps its state while it is not drawn.

Queued messages and notices stay above the card.

## Panel — Planned (step 15)

The panel holds what the thread changed and where it works. It opens at the
right of the sheet, beside the conversation, on the navigation fill, with a
hairline at its inner edge. It replaces This turn, the workspace bar's view
switcher, and Work view.

```
┌──────────────────────────────────────────────┐
│ Changes 3   Files   History              ✕   │ 36
├──────────────────────────────────────────────┤
│ This turn│Uncommitted   3 files +69 −5   ↗   │ 40
│ a.rs +66   b.rs +2 −5   c.rs +1              │
│ diff                                         │
└──────────────────────────────────────────────┘
```

- **Opening and closing.** The header's panel toggle; Toggle Panel
  (`view.toggle_inspector`, Cmd-L); `Review` and a file on a closing line; a
  row of the changed-files strip; and the Changes, History, and Files
  actions, which open the panel on that tab. The Conversation action closes
  it. The panel is closed when a tab is first shown.
- **Width.** Half the sheet's width at first; drag from the inner edge (a
  hairline with a 6-point drag area; double-click returns to half). Never
  narrower than 360, and never leaving the conversation under 420. The
  width is a device preference.
- **Fill.** Fill Panel (`view.toggle_work`, Cmd-Backslash) makes the panel
  cover the conversation and back. When the sheet is under 1020 wide the
  panel always fills.
- **Tab row.** 36 high: `Changes` with the count of uncommitted files,
  `Files`, `History`, each 26 high with 6 corners; the open tab has the
  selection fill and a 1-point outline. At the right, the close control.
  List no tab that is not built.
- **Changes.**
  - A bar, 40 high: the `This turn │ Uncommitted` switch; `N files` and the
    totals; at the right, the editor handoff. On `This turn`, the bar also
    says which turn (`This turn` or `Earlier turn`, its age, its state word)
    and holds `Undo` or `Redo` and, for an earlier turn, `Latest`: the
    record This turn shows today.
  - A row of file chips (name and counts; the selected chip uses the
    selection fill); it wraps to two rows, then scrolls.
  - The diff of the selected file, as built: gutter marks, faint tints, and
    line numbers. On `This turn` it is the recorded diff of the latest turn,
    or of the turn chosen in the conversation, and offers no writes. Its
    loading, unavailable, limited, binary, and older-host states stay as
    built.
  - On `Uncommitted` only: a row 32 high with the `Unstaged │ Staged`
    switch at the left and, at the right, `Stage file` or `Unstage file` as
    a standard button and `Discard…` as a quiet button in coral; hunk
    actions on the diff as built; and the commit bar fixed at the bottom
    (the sync row, the message, and `Commit`, which is the primary button
    when a commit is possible).
  - With no recorded turn (a new thread, a plain folder, an older host) the
    tab opens on `Uncommitted`.
  - Timings (`9 patch lines · 19 ms`) show only with diagnostics on.
- **Files** and **History** follow the rules of the views of the same name,
  drawn at panel width: Files is the read-only viewer with Open File…, Find,
  and the handoffs; History is the commit list above the selected commit's
  patch.
- **Keys and focus.** Find acts on the panel when focus is in it, otherwise
  on the conversation. Focus Next Pane and Focus Previous Pane
  (`pane.next`, `pane.previous`) move between the conversation and the
  panel. Tab order runs conversation, message box, panel.
- **Memory.** A tab keeps whether its panel is open, the open panel tab, the
  scope, and the selected file while the window is open. Tabs restored at
  launch open with the panel closed until the host stores panel state,
  which [needs agreement](#more-in-the-panel).

## Every thread — Planned (step 18)

Every thread on this machine at once, so the person can see what is running
and what needs them without asking. It opens from `Needs you` in the sidebar
and from Every Thread (`view.overview`, Cmd-Shift-E), and takes the whole
sheet. Escape, `Back`, or the same row or key returns to the thread that was
open.

```
┌───────────────────────────────────────────────────────────────┐
│ Every thread  2 need you  3 working  1 to review    New thread │ 46
├───────────────────────────────────────────────────────────────┤
│ forecast  ~/code/forecast                                      │
│ ┌ ● Needs approval   6m ┐ ┌ ◌ Working           ┐ ┌ ○ 3h     ┐ │
│ │ Retry rate-limited…   │ │ Add a search page   │ │ Explain… │ │
│ │ ◇ Model  ⑂ retry-429  │ │ ◇ Model             │ │ ◇ Model  │ │
│ └───────────────────────┘ └─────────────────────┘ └──────────┘ │
└───────────────────────────────────────────────────────────────┘
```

- **Bar**, in the header's place: `Every thread` in 14-point semibold; then
  one pill per count that is not zero, in the state's colour (`N need you`
  in sand: waiting for approval, failed, and interrupted; `N working` in
  blue; `N to review` in mint: finished and unread); flexible space; `Find`
  with its key hint; `New thread`, the primary button; `Back` with its key
  hint.
- **Body.** One section per project, 14 apart: a label row 26 high with the
  project's name in 12-point semibold secondary text, its path in 11-point
  hint text in the code font, and `New thread in <project>` at the right;
  then a grid of cards, as many columns as fit at 290 or wider, 10 apart.
  Threads are in the sidebar's order. Archived threads are left out.
- **Card.** 12 corners, the panel fill, a 1-point outline in the border
  role. A thread waiting for approval has its outline in sand at 45%. A
  thread that is neither working nor needing the person has no fill.
  - Top row, 11-point: the state mark and the state word in medium weight
    and the state's colour (`Working`, `Needs approval`, `Failed`,
    `Interrupted`, `Finished`, or `Idle` in hint text); at the right, the
    age of the last update in the code font.
  - The title, 13.5-point semibold emphasis text on a 19-point line, at most
    two lines.
  - A foot, behind a hairline, in 11-point hint text: the harness mark, the
    model's name, and the thread's own branch when the client knows it.
- **Focus.** One card has focus: the outline takes the hint tone, the fill
  becomes the raised fill, and it gains the floating shadow. Focus starts on
  the thread that was open. Arrow keys move by row and column; Enter or a
  click opens the thread and closes Every thread; Home and End go to the
  first and last card.
- **Empty.** With no threads: `No threads yet` and `New thread`.
- Cards update as thread state changes. Nothing here needs a request per
  thread: it reads the thread list the sidebar reads.

Answering an approval from a card, what an agent is doing now, change
totals, and helper agents on a card [need agreement](#thread-summary-additions).

## Other views — Built

- Worktrees uses a branch graph with neutral lanes and nodes, state colours,
  readable branch and thread labels, and a detail pane. Place detail beside
  the graph when wide and below it when narrow. Name unknown or bounded
  measurements rather than displaying a false zero.
- Linear issues, as built.
- Settings and Project settings are centred window modals over the sheet.
  Preserve the underlying selection and draft, keep focus in the modal, and
  return focus on dismissal. Opening another project's settings does not
  select that checkout.
- The palette, pickers, menus, and toasts keep their content and behaviour.
  Step 11 gives them the raised fill, a 1-point outline, 12 corners (14 for
  the palette), and the floating shadow; the palette keeps its scrim.

## Interaction — Built

These rules hold for built and planned parts alike.

- Use native menus, traffic lights, and folder picker. Show keyboard focus and
  accessible names, including full paths and state where visible text truncates.
  Do not rely on colour alone. Keep long lists virtualized.
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
- Find shows match count, navigation, and current match without relying on colour.
  State when conversation results cover only loaded messages. The palette and
  pickers use a centred search field, compact rows, and keyboard hints.
- Toasts and system notifications fire as built: when a thread that is not in
  view changes to waiting for approval, failed, interrupted, stopped, or
  finished, unless notifications are muted.

## Keys

Action IDs do not change, so saved keys keep working. The build order gives
some actions a new title and target, adds one, and retires three. Change
`ACTIONS` and the [shortcut list](docs/SHORTCUTS.md) in the step named.

| Default | Action | ID | Step |
| --- | --- | --- | --- |
| Cmd-L | Toggle Panel (was Toggle This Turn) | `view.toggle_inspector` | 15 |
| Cmd-Backslash | Fill Panel (was Toggle Work View) | `view.toggle_work` | 15 |
| Cmd-Shift-1 | Conversation: closes the panel and focuses the conversation | `view.conversation` | 15 |
| Cmd-Shift-2 | Changes: opens the panel on Changes | `view.changes` | 15 |
| Cmd-Shift-3 | History: opens the panel on History | `view.history` | 15 |
| — | Files: opens the panel on Files | `view.files` | 15 |
| Cmd-Option-Right | Focus Next Pane: conversation, then panel | `pane.next` | 15 |
| Cmd-Option-Left | Focus Previous Pane | `pane.previous` | 15 |
| Cmd-Shift-E | Every Thread | `view.overview` | 18 |
| Cmd-Option-J | Next Thread Needing Me, unchanged | `thread.next_needing_me` | — |
| — | Quote Selection | `conversation.quote_selection` | 23 |
| — | Split Pane Right, Split Pane Down, Close Pane: removed from menus and the palette; a saved key for one does nothing | `pane.split_right`, `pane.split_down`, `pane.close` | 24 |

Every other shortcut is unchanged, including the tab keys while tabs are
hidden.

## Tokens

Colours are semantic roles in `plait_client::theme::Palette`. Step 11 adds
three roles and changes the default values. The names in code are in
parentheses.

### Text and surfaces

| Role | Used for | Dark default | Light default |
| --- | --- | --- | --- |
| Navigation (`rail`) | The window and sidebar; the panel; wells inside a raised surface | `#11131b` | `#efefec` |
| Background (`bg`) | The sheet; diffs and code | `#171a24` | `#f7f7f5` |
| Panel (`panel`) | The message box, cards, the header of a list | `#1f2330` | `#ffffff` |
| Raised (`raised`, new) | Menus, pickers, the palette, toasts, bubbles, standard buttons | `#272c3b` | `#ffffff` |
| Border (`border`) | Hairlines | `#252a38` | `#e3e3df` |
| Control outline (`edge`) | Outlines of controls, boxes, and floating surfaces; the line | `#363d52` | `#d3d4cf` |
| Selection (`selected`) | The selected row, tab, chip, or segment | `#262b3a` | `#e7e7e3` |
| Hover (`hover`) | A hovered control or row | white at 4% | black at 4% |
| Emphasis (`strong`, new) | Titles, the selected item, bubbles, the primary button's fill | `#e9ecf4` | `#16181c` |
| Text (`text`) | Reading text and values | `#cdd2df` | `#2b2e33` |
| Secondary (`muted`) | Labels, controls at rest, metadata that matters | `#9aa2b8` | `#5d636e` |
| Hint (`subtle`) | Ages, counts, paths, placeholders, key hints | `#767f97` | `#737985` |
| Faint (`faint`, new) | Chevrons and other decoration, disabled controls | `#545c73` | `#a4a8b0` |

The dark default is a new built-in palette, Stillwater Deep. The light
default is the built-in light palette as it is, with the three new roles.

Contrast, measured against the background, as bands every shipped theme
keeps:

| Tone | Band | Dark default | Light default |
| --- | --- | --- | --- |
| Emphasis | above Text | 14.7 | 16.6 |
| Text | 9.5 or more, and in a dark theme no more than 13.5 | 11.5 | 12.7 |
| Secondary | 5 to 7.5 | 6.8 | 5.6 |
| Hint | 3.5 to 4.7 on the background, and 3 or more on Panel, Raised, and Navigation | 4.3 | 4.1 |
| Faint | under Hint; no floor, because it never carries information alone | 2.6 | 2.2 |

- Text is at least 1.35 times Secondary in contrast ratio, and Secondary at
  least 1.35 times Hint. That step is what Stillwater's earlier mapping
  lacked (12.3, 7.9, 6.5).
- Navigation is always the darker of the two grounds, in dark and light
  themes alike, so the sheet reads as lying on it.
- Text that carries information is never Faint: an age, a count, a path, or
  a placeholder is Hint.

State colours keep their values in every theme:

| Role | Dark default | Light default |
| --- | --- | --- |
| Addition / selected option / passed / finished and unread (`mint`) | `#70d689` | `#1d7a5a` |
| Running / modification / link / focus (`blue`) | `#74cde2` | `#2f62b8` |
| Thread identity / keyword (`violet`) | `#ec8ac7` | `#6f4fc2` |
| Attention (`amber`), called sand on this page | `#eec96e` | `#8a5f0a` |
| Error, failed, deletion (`red`), called coral | `#ec7368` | `#b53a4c` |

Accents keep 3:1 or more on Background, Panel, and Raised. Diff lines use a
gutter mark and a tint of mint or coral at 11%. Code uses semantic syntax
colours; plain identifiers remain main text. A state always has a word or a
mark beside its colour.

**Themes.** Every shipped theme is held to the bands, so hierarchy does not
depend on the theme a person picks.

| Built-in | What changes |
| --- | --- |
| Stillwater Deep (new, the dark default) | The values above, with Stillwater's accents |
| Stillwater | Keeps its ground `#1f222e`, panel, and text. Navigation becomes `#191c26`; Secondary becomes its comment gray `#9da5bf` (6.5); Hint becomes `#7f879f` (4.4); it gains Emphasis `#eef0f6`, Raised `#313647`, and Faint `#5f667a` |
| The built-in dark | Already in the bands (12.3, 6.6, 4.5). Navigation becomes `#151618`; it gains Emphasis `#eceef2`, Raised `#323439`, and Faint `#595e69` |
| The built-in light (the light default) | Already in the bands. It gains the three roles in the table above |

A person's saved choice of theme is kept. Bundled and imported Base16
themes derive the roles: Navigation is the background mixed toward black
(in a dark theme) or toward the text (in a light one) until it is the darker
ground; Panel and Raised are the background mixed toward the text; Emphasis
is the text mixed toward the scheme's brightest foreground; Secondary, Hint,
and Faint are the background mixed toward the text until each sits in its
band. A theme that cannot meet a band keeps the nearest readable value, as
the floors do today.

### Shape, elevation, and size

| Token | Value |
| --- | --- |
| Corners: controls, rows, cards in the sidebar | 7 |
| Corners: chips, panel tabs, menu items | 6 |
| Corners: the sheet, the message box, cards, menus, bubbles | 12 |
| Corners: the palette | 14 |
| Corners: pills and dots | full |
| Floating shadow, dark | `0 18 44 −12` black at 70%, and `0 2 8` black at 40% |
| Floating shadow, light | `0 14 36 −12` `#242a3e` at 22%, and `0 2 6` at 8% |
| Box shadow (the message box) | `0 8 24 −10` black at 45% (dark), `#242a3e` at 14% (light) |
| Focus | a 1.5-point outline in blue, 1 outside the control |
| Buttons | 28 high, or 24 where a row is 28 to 30 high; 11 of padding at the sides; 12-point medium |
| Icon buttons | 28 square, or 22 inside a row; glyphs 16, or 13 to 14 in small controls |
| Segmented control | a track in the navigation fill with a border; segments 22 high |
| Key hints | 17 high, 4 corners, 10.5-point code font, on the hover fill |
| Dots | 7; 9 where a card or bar names the state |
| Spacing | a 4-point base |

Shadows are written as x, y, blur, spread, as GPUI's box shadow takes them.

**Buttons.** Three kinds, all from `theme::control`:

| Kind | At rest | Hover | Pressed |
| --- | --- | --- | --- |
| Primary (`Tone::Primary`) | Emphasis fill, background-coloured text | 88% opacity | 70% opacity |
| Standard (`Tone::Filled`) | Raised fill, control outline, main text | Selection fill, outline in the faint tone | 70% opacity |
| Quiet (`Tone::Ghost`) | No fill, secondary text | Hover fill, main text | 60% opacity |

### Type

The interface font is IBM Plex Sans, bundled in Regular (400), Medium (500),
and SemiBold (600) under the SIL Open Font License; Settings → Appearance →
`Interface font` offers `Plex Sans` (the default) and `System`. Code, paths,
commands, branches, and ticking numbers use the device's code font, the
system monospaced font unless the person names one, at the code size (12 to
14). Numbers that change in place use tabular figures.

| Size | On a line of | Used for |
| --- | --- | --- |
| 20 semibold | 26 | Not used yet; reserved for a page title |
| 14.5 | 23 | Reading text: requests, replies, the draft (21) |
| 14 semibold | 20 | The header's title; the bar of Every thread |
| 13.5 | 19 | Rows, menus at rest, card titles in Every thread (semibold) |
| 12 | 17 | Buttons, tabs, sidebar card titles (medium), work rows, the closing line, menu items |
| 11 | 15 | Metadata: ages, counts, chips, state words, group labels |

No size under 11. No letter-spacing: the pinned GPUI has none. Conversation,
message box, and settings content are capped at 760. Render timings are an
optional diagnostic, not permanent chrome.

### The running mark

A thread, tab, tool call, or card that is working shows a ring three
quarters closed, in blue, turning once in 0.9 seconds. It is one SVG drawn
once and turned by the renderer, so it re-renders no view. Under Reduce
Motion it is the same ring, still. It is 13 across, or 11 on a sidebar
card's second and third lines. It is the only thing in the app that moves
on its own.

## Needs agreement

Each of these changes the protocol, stored state, permissions, or scope the
[roadmap](docs/ROADMAP.md) has not agreed. None is built by the steps below.
Agree it, then specify it here before building.

### Thread summary additions

The thread list the host sends has an identity, a checkout, a model, a
status, a title, two timestamps, an archived flag, whether an approval is
pending, the harness, and the checkout queue. A card can draw nothing else
without asking the host again for each thread, which the sidebar must not
do. To agree: adding to the summary, at a new protocol version,

- the thread's branch, for every thread;
- when the current run started, so a working card can show elapsed time;
- totals of what the thread changed (files, added, removed), from its turn
  records, with no Git read;
- a short line saying what the run is doing now;
- the pending approval's summary and risk, so Every thread can answer it,
  with the same checks as the conversation.

Each costs the host work on every thread on every change; agree the cost
and the cadence with the field.

### Turn timing

Turn records carry one timestamp, taken when the record was made. The
runtime stores when each run started and finished and does not send it. To
agree: sending both on the record, so folded work can read `Worked for 2m
28s · 9 steps` and the closing line can give a duration. Until then no
duration is drawn anywhere.

### Helper agents

A harness can start agents of its own. Today the adapters drop them: only
the parent's tool call reaches the desktop. To agree: a read-only report of
the agents a thread's harness started (a name, the model, a state, what it
is doing), shown as a `Runs` tab in the panel and as extra lines on the
thread's card. Plait starts no agents itself and adds no orchestration.

### Limits

Provider limits are not parsed, and a run that ends on a usage limit shows
a generic failure. To agree: what the host may read from each harness
(limit events, reset times), where it is stored, and showing it in the
sidebar's foot as one meter per provider; and the failure text for a run
stopped by a limit. Usage totals are on the roadmap as proposed.

### Pull requests and remote checks

Nothing reads pull-request or CI state. To agree: which service, with whose
credentials, how often, and a pill beside the Git step.

### More in the panel

- **Preview.** A rendered document from the checkout, with selection quoted
  into the message box. Rendered documents are proposed on the roadmap. The
  Markdown drawing is tied to the conversation's selection and Find today
  and needs an owner of its own.
- **Checks.** A tab for [checks after a run](#checks-after-a-run).
- **Saved panel state.** Whether a tab's panel is open, and on what, stored
  per tab with the host's per-device state. Saved state and protocol.
- **Output.** The output of a command the agent ran, read-only, with no
  input. Agree that this is not an embedded terminal.
- **Several files at once.** A stack of every changed file with its diff
  costs one read per file. Agree a bound before drawing it.
- **Viewed marks** on files of a diff: stored state.

### More in Every thread and the message box

- Acting from a card: allowing or denying an approval, undoing the latest
  turn, trying a failed run again.
- A choice beyond `Allow once` and `Deny once`: permission rules.
- Commit and push as one action from the header: it stays inside the Git
  write allowlist but chains two writes with one confirmation.

### Checks after a run

Commands the person selects for a project; the host runs them after a turn and
stores the results with the turn's record.

- Closing line: `N of M checks` after the steps.
- Panel, Changes on `This turn`: one row per check — a mark, the command in
  the code font, and at the right what the host reports (exit state,
  duration).
- States, each in a word and a mark: passed (mint), failed (coral), running
  (blue), not run (hint). Never show a check as passed without a stored
  result, and do not summarize output the host did not parse.

To agree first: where checks are defined, the permissions they run under, how
results are stored and bounded, and the protocol.

### Attempts

This is the proposed answer to the roadmap's
[prompt tree](docs/HOST-FIRST.md#the-prompt-tree) exploration: a retry or an
edit of a request is another **attempt** of that request. Attempts are
siblings, not separate kinds of branch.

- The request shows `Attempt 2 of 2`, with previous and next controls.
- The other attempts hang under the request as one-line stubs: the attempt's
  number and first line, its state (`Undone`), `N steps · N files`, and
  `Compare`.
- Choosing an attempt moves the thread to it; files change only by the host's
  undo and redo rules.

To agree first: the tree's model, how it spans worktrees, and files changed
since an attempt was recorded.

## Direction

Where the design is heading. Each idea reuses the marks and the record, and
each is held back by a product rule named here. Until that rule changes by
agreement, do not build, mock in the shipped app, or hint at any of them.

- **Benches.** A named panel arrangement with its checks, kept with the
  project, so a kind of work (code, a demo, a guide) opens the way it is
  done. Waits for checks and saved panel state.
- **Kept while away.** A standing promise about a project: one sentence, the
  check that proves it, and what an agent may do alone to keep it. A
  `While you were away` view lists what was kept, with each fix's record.
  Held back by: no hidden execution; unattended automations are not agreed.
- **Why is this line here.** Files marks the lines a turn wrote. A story pane
  follows one line back: what was asked, the attempt that was dropped, the
  attempt that wrote it, the checks that passed, the commit. Held back by:
  tracing lines to turns is not agreed.
- **Several attempts at once.** More than one agent attempts a request; they
  show side by side, chosen by checks first and smallest change second. A
  time marker shows the code as it was at any turn. Held back by: no agent
  orchestration.
- **Receipts.** Each commit carries the request that led to it and the proof
  that it worked, stored in the repository, with `Run the proof again`. Held
  back by: the Git write allowlist.

## Build order

Steps 0 to 9 built the layout listed under
[Built today](#built-today-to-be-replaced) and were released in
`v0.1.0-preview.80`. They were checked by tests and by scenes rendered on
Linux; the native app was not inspected before release
([#130](https://github.com/mostlyheath/plait/issues/130)). Steps 10 to 26
replace that layout with the one on this page. Every one of them is
presentation only.

### Decisions these steps assume

Each is recorded here so a step can be built without asking again. Changing
one changes only the step named.

| # | Decision | Assumed | Changes |
| --- | --- | --- | --- |
| D1 | How turns are drawn by default | `Bubbles`; `On the line` is a setting | The default in step 19 |
| D2 | Whether thread tabs show by default | Hidden; the header shows the title. `Show` is a setting | The default in step 16 |
| D3 | The dark default | Stillwater Deep, a new built-in; Stillwater stays selectable, with its secondary and hint tones moved into the bands | A constant and one palette in step 11 |
| D4 | The interface font | IBM Plex Sans, bundled; `System` is a setting | Step 12, which can be skipped whole |
| D5 | Work view's splits and This turn | Removed; the panel replaces both | Step 24, which can be skipped whole, leaving the code unreachable but present |
| D6 | What `Needs you` does | Opens Every thread; the list is no longer filtered | Step 18 |
| D7 | The status line | Removed; its parts move to the sidebar's foot | Step 14 |

### How the steps are worked

- **One step, one branch, one small pull request, one owner**, as
  [Contributing](CONTRIBUTING.md) requires. The pull request names the
  authoring model and harness and the reviewing model. `main` changes only
  through a pull request with passing checks.
- **Order.** 10, then 11. After 11, four lanes can run at the same time;
  inside a lane the steps run in order.

  | Lane | Steps | Owns |
  | --- | --- | --- |
  | Shell | 14 → 15 → 16 → 22 → 24 | `ui/mod.rs` layout, `tabs.rs`, `status_line.rs`, `panels.rs`, `work_view*`, `this_turn.rs`, `conversation.rs`, `changes.rs`, `commit_panel.rs`, `turn_review.rs`, `commits.rs`, `file_view.rs` |
  | Sidebar | 17 → 18, both after 14 | `ui/navigation.rs`, `plait-client/src/navigation.rs`, a new `ui/overview.rs` |
  | Conversation | 19 → 20 → 23 | `ui/live_conversation.rs`, `tool_card.rs`, `turn_records.rs`, `markdown.rs`, `selection.rs`, `plait-client/src/folding.rs`, `tool_view.rs`, `transcript.rs` |
  | Message box | 21 | `ui/thread_controls.rs`, `thread_files.rs`, `composer/`, the `approval` function in `live_conversation.rs` |

  Steps 12 and 13 follow 11 and touch only what they name. Step 25 follows
  every other step; step 26 follows 25.
- **Shared files.** `ui/mod.rs`, `ui/theme.rs`, `ui/actions.rs`,
  `ui/snapshot.rs`, `ui/settings.rs`, `ui/appearance.rs`, and
  `plait-client/src/preferences.rs` are edited by several lanes. Keep each
  edit there to the lines the step needs, add new scenes in a new file under
  `ui/snapshot/`, and rebase before opening the pull request. A lane does
  not restyle another lane's files; step 11 has already moved every file to
  the new tokens.
- **Paths** below are under `crates/plait/src/ui/` unless they name a crate.
- **APIs.** Check GPUI's source at the pinned revision before using an API
  named here (box shadows, the pressed style, timed SVG transformation,
  bundled fonts, font features, measured lists).

Every step is done when:

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, and `cargo test --workspace` pass.
- State and its transitions live in `plait-client` with unit tests and no
  GPUI; the desktop crate only presents them.
- Test scenes for each new state are added under `snapshot/` at both window
  sizes (1380 by 860 and 900 by 600), with at least one in the light
  default, and images from `bash scripts/snapshots.sh` are in the pull
  request. `every_view_draws` passes, so no accessibility node is doubled.
- **The native app was built and looked at**, in a wide and a narrow window,
  in the dark and light defaults, with a pointer and with the keyboard, and
  the pull request says what was seen. Scenes do not replace this.
- Every new control is built on `theme::control`, has a visible focus
  state, an accessible name, a keyboard path, and hover text when disabled.
  No shipped view shows sample data.
- `bash scripts/bench-frames.sh` shows the cases the step names no slower
  than `main` beyond run-to-run noise, measured on a quiet machine, and the
  pull request gives both numbers.
- The [guide](docs/GUIDE.md) and [shortcut list](docs/SHORTCUTS.md) match,
  and [verification](docs/VERIFICATION.md) lists anything not checked.
- No change was made to the protocol, to state the host stores, to
  permissions, or to Git writes. A new device preference is optional, is
  read leniently, and a preferences file saved before it existed still
  loads, with a test that proves it.

### 10. Record the decisions

Documentation only. This page; the motion rule in `PRODUCT.md`; and in the
roadmap, the rebuild under Now and the [items that need
agreement](#needs-agreement) under Proposed. Steps 11 to 26 follow it.

### 11. Tokens and controls

The change every later step stands on. It moves no layout.

- **Palette.** In `plait-client/src/theme.rs`: add `strong`, `raised`, and
  `faint` to `Palette`; add `STILLWATER_DEEP` with the dark values in
  [Tokens](#text-and-surfaces) and Stillwater's accents, and make it
  `DEFAULT_DARK`; change `STILLWATER`, `PLAIT_DARK`, and `PLAIT_LIGHT` as
  the Themes table says; extend `derive()` for Base16 themes, including the
  darker navigation ground. Keep every existing role and its meaning.
  `raised_hover` stays the hover of a raised surface. `primary_hover` is not
  needed once Primary hovers by opacity; remove it.
- **Tests.** Replace `every_shipped_theme_keeps_text_readable` with the
  bands: for each shipped theme, on Background, Text 9.5 or more, Secondary
  5 to 7.5, Hint 3.5 to 4.7; Hint 3 or more on Panel, Raised, and
  Navigation; Text at least 1.35 times Secondary and Secondary at least
  1.35 times Hint in ratio; every accent 3
  or more on Background, Panel, and Raised; Navigation darker than
  Background. Keep `the_dark_text_is_softer_than_near_white`.
- **Controls.** In `theme.rs`: the corner constants; the three button kinds
  with the pressed style; `Tone::Selected` unchanged; the segmented control,
  chips, tags, and key hints at the sizes in [Tokens](#shape-elevation-and-size);
  two helpers, one for the floating shadow and one for the box shadow, each
  choosing its values from the palette's `light` flag.
- **Floating surfaces.** The palette, pickers (`model_picker.rs`, the
  permission and thinking pickers, the branch picker), menus
  (`navigation_menu`, the machine menu, the tab list), the settings modal,
  toasts, and the image overlay take the raised fill, a 1-point control
  outline, their corners, and the floating shadow.
- **Tones.** Go through every view once and set each text to the tone its
  role has in the table, including ages, counts, and paths to Hint,
  decoration to Faint, and titles and selected items to Emphasis. Replace
  each text-only button with a Standard or Quiet button. Make the one
  primary action of each built view Primary: Send, `Allow once`, `Commit`
  when a commit is possible, `Update`. Make `Discard…` Quiet in coral.
- **Files:** `plait-client/src/theme.rs`; `theme.rs`, `appearance.rs` (the
  theme list gains Stillwater Deep), and small edits wherever a tone or a
  button kind is set.
- **Scenes:** a new `tokens` scene that draws every role as a swatch with
  its name, the four text tones on each surface, and each button kind at
  rest, hovered, pressed, focused, and disabled, in the dark and light
  defaults and in two Base16 themes. Every existing scene changes; say so
  in the pull request and attach the ones that show a floating surface.
- **Bench:** all cases; this step must not add cost.
- **Done when** the bands hold for every shipped theme; no view has a
  text-only button; a menu, the palette, a toast, and the settings modal
  visibly float in the native app in both defaults; an older preferences
  file that names Stillwater still opens in Stillwater.

### 12. Interface font

Skip this step if D4 is declined; nothing later depends on it.

- Bundle IBM Plex Sans Regular, Medium, and SemiBold under `assets/fonts/`
  with its license and a provenance note, and load them at startup. Check
  each face loaded before drawing with it, as the code font does.
- A device preference, `interface_font` (`plex` or `system`), read
  leniently, with `Interface font` in Settings → Appearance beside `Code
  font`. Use it where `.SystemUIFont` is set (`mod.rs`, `tabs.rs`).
- Set the sizes and weights of [Type](#type) through named constants in
  `theme.rs`, and turn on tabular figures where numbers change in place.
- **Scenes:** `interface-font-plex` and `interface-font-system` of a
  conversation, the sidebar, and Settings → Appearance.
- **Bench:** e1 to e3 and c1 to c5.
- **Done when** both choices draw at both window sizes with no clipped or
  truncated label that fit before, and a preferences file without the key
  loads as `plex`.

### 13. Running mark

- A new original SVG, the open ring, in `assets/`, and `theme::running_mark`
  that draws it at a size and turns it with a timed transformation; under
  Reduce Motion it draws still.
- Use it wherever a working state is drawn: thread rows, tabs, the state
  line under a running turn, and a running tool row.
- **Scenes:** `running-mark` (still, as scenes are), in the sidebar, a tab,
  the state line, and a tool row.
- **Bench:** e1 to e3 with a thread working must match `main`: the mark may
  not cause a view to re-render. Say how that was checked.
- **Done when** it turns in the native app at a steady rate while idle CPU
  stays where it was, and stands still under Reduce Motion.

### 14. Sheet, sidebar foot, no status line

- **Root layout** (`mod.rs`, `Plait::render`): the window's fill is the
  navigation role. The content column (the workspace bar, notices, and
  content body, for now) is drawn as the sheet: 8 from the top, right, and
  bottom, 12 corners, the background fill, a 1-point border. With the
  sidebar hidden the sheet fills the window. The tab row stays above for
  now, on the navigation fill, and loses its bottom border.
- **Sidebar** (`navigation.rs`): no right border; the three action rows at
  30 high as [specified](#top-strip-and-action-rows--planned-step-14); the
  foot row with the machine control, `Update`, and Settings, moved from
  `status_line.rs` with their menu, states, and accessible names.
- **Status line:** removed from the root layout. `next_needing_me` moves to
  `navigation.rs` with its action; `activity()` and `machine_activity` stay.
  Delete `status_line.rs` when nothing in it is used. The counts it showed
  are not redrawn: the sidebar's cards and `Needs you` already say them.
- **Resize** (`panels.rs`): the sidebar's default becomes 288, and 236 when
  the window is under 760 wide; its limits stay 200 to 480.
- **Scenes:** `sheet` (sidebar shown and hidden), `sidebar-foot-connected`,
  `-connecting`, `-offline`, `-update`; retire the `status-*` scenes.
- **Bench:** d1 to d7.
- **Done when** the sheet's corners are clean at all four corners with the
  panel of step 15 not yet present; the machine menu, Update, and Settings
  work from the foot by pointer and by keyboard; Next Thread Needing Me
  still works; and dragging the sidebar's top strip moves the window.

### 15. Panel

The largest step. It moves three built views into one container and changes
none of their content.

- **Model** (new `plait-client/src/panel.rs`, no GPUI): per tab, `Panel {
  open, kind: PanelKind { Changes, Files, History }, fill, scope: {
  ThisTurn, Uncommitted }, recorded }` with `toggle`, `open_on(kind)`,
  `close`, `set_fill`, `select_scope`, and the rule that a sheet under 1020
  wide forces fill; plus the width rule (half; at least 360; the
  conversation at least 420) as a pure function with tests for each limit.
  Kept in the tab's session while the window is open, where Work view's
  state is kept today.
- **Drawing** (new `panel.rs`): the tab row, the divider and its drag
  (`panels.rs`: the `Inspector` entry becomes the panel's, with the new
  limits, saved as the device preference `panel_width`; `inspector_width`
  is no longer read), and the
  three tabs, which draw `work_changes` (`changes.rs`), `file_view`, and
  `history_view` (`commits.rs`) at the panel's width. The Changes bar is as
  [specified](#panel--planned-step-15): it gains the turn's heading, `Undo`
  or `Redo`, and `Latest` from `this_turn.rs`, and loses a toolbar by
  putting the scope switch, totals, and handoff in one row and the stage
  and discard actions in the next.
- **Attached** where the inspector is attached today
  (`live_conversation.rs`, `conversation.rs`, `new_thread_view.rs`): change
  only those lines in the conversation lane's files.
- **Retired here:** This turn and the working-tree fallback (`this_turn.rs`,
  `inspector()`), `shows_inspector`, the `WIDE_WINDOW` rule, and
  `hide_inspector`, which is no longer read. `show_earlier_turn` now opens
  the panel on Changes for that turn.
- **The view switcher**, until step 16 removes the bar: `Changes`,
  `History`, and `Files` open the panel on that tab, and `Conversation`
  closes it. The `Chat │ Work` switch and the This turn toggle are removed
  from the bar; Work view is no longer reachable from chrome.
- **Actions** (`actions.rs`): the titles and targets in [Keys](#keys).
- **Scenes:** `panel-closed`, `panel-changes-turn`, `-changes-turn-earlier`,
  `-changes-uncommitted`, `-changes-staged`, `-no-record`, `-files`,
  `-history`, `-fill`, `-narrow` (forced fill), `-dragged`, and
  `-older-host`; retire the `this-turn-*`, `inspector-hidden`, and
  `panels-resized` scenes.
- **Bench:** a1 to a4, b1 to b4, f1, f2, and d1 to d7.
- **Done when** each tab opens by its control and its key; `Review` and a
  file on a closing line open the right recorded diff; Undo refuses exactly
  where the closing line does; the commit bar shows only on `Uncommitted`;
  two tabs keep different panel states; the divider drags and resets and the
  width survives a restart; under 1020 the panel fills and its close control
  returns to the conversation; and Find acts on whichever side has focus.

### 16. Header

- One row in the sheet (`mod.rs`, `Plait::header`, rewritten) replaces the
  workspace bar and the tab row: the title form and the tabs form as
  [specified](#header--planned-step-16), the state pill, and the panel
  toggle. `workspace_switch` and `work_switch` are no longer drawn.
- **Tabs** (`tabs.rs`): `tab_row` is no longer a window bar. The sidebar
  toggle moves to the sidebar's top strip, or to the header when the sidebar
  is hidden, with the traffic-light inset logic (`LIGHTS_INSET`,
  `FULLSCREEN_INSET`). The tab strip, overflow control, and New tab draw
  inside the header in the tabs form, as pills; `tab_control` drops the
  state word and the top accent bar and keeps the mark, title, and close
  control. `measure_tabs` measures against the header's free width.
- **Preference:** `thread_tabs` (`show` or `hide`, default `hide`), lenient,
  with `Thread tabs` in Settings → Appearance. In the tabs form the project
  and branch chips draw under the message box through `environment()`.
- **Window drag:** the header's empty space, through `window_drag`.
- **Scenes:** `header-title`, `-title-narrow`, `-tabs`, `-tabs-overflow`,
  `-working`, `-approval`, `-failed`, `-no-sidebar`, `-full-screen`,
  `-worktrees`, `-draft`; move the `tabs-*` scenes to the tabs form.
- **Bench:** d1 to d7, and the twenty- and three-hundred-tab scenes' cases.
- **Done when** the window has no bar above the header; with tabs hidden,
  every tab key and the tab list still work and closing the open tab shows
  the one before it; with tabs shown, reorder, overflow, close, and New tab
  work in the header; the title and chips truncate, never wrap; the state
  pill agrees with the sidebar card at every state; and the window drags,
  zooms, and enters full screen from the header in the native app.

### 17. Thread cards

- **Rows** (`plait-client/src/navigation.rs`): a `Card` view of a thread
  with its lines already decided (state, word, title, the thread's own
  branch when `lists().worktree` knows it, harness, model name, age) and the
  order within a project, with tests for each state and for the order.
- **Drawing** (`navigation.rs`): the group label row and the card as
  [specified](#project-groups--planned-step-17), replacing `host_thread_row`
  and `thread_mark`. Cards have two or three lines, so `tree()` moves from
  `uniform_list` to a measured list; keep one entity per thread, not cached,
  as today, so accessibility nodes are kept. Keep rename, menus, the
  `Archived` tag, drafts, and the accessible name.
- **Scenes:** `cards-states` (every row of the state table), `-worktree`,
  `-selected-hover`, `-rename`, `-archived`, `-folded`, `-missing-project`,
  `-many` (300 threads in six projects), `-narrow`; retire `sidebar-states`
  and the `navigation-*` scenes they replace.
- **Bench:** d1 to d7, and a new case that scrolls the 300-thread sidebar.
- **Done when** no card draws a line it has no data for; the list scrolls
  300 threads at the frame cost of today's list or better; selection, hover,
  rename, both menus, and the keyboard path through the tree work; and
  VoiceOver reads a card as its title, state, and unread activity.

### 18. Every thread

- **Model** (`plait-client/src/navigation.rs`): `Overview { focus }` with
  the grid's order (projects, then the sidebar's order), `move_focus` by
  direction for a given column count, and the three counts, all tested. It
  reads the thread list and `needs_attention`; it makes no request.
- **Drawing** (new `overview.rs`): the bar and the grid as
  [specified](#every-thread--planned-step-18), drawn in the sheet in place
  of the header and content while open. Columns come from the sheet's width.
  Rows of cards are virtualized.
- **Wiring:** `needs_me_row` opens and closes it and shows selected while it
  is open; the `navigation.attention` filter and its branch in
  `ensure_navigation_rows` are removed. A new action, `view.overview`,
  Cmd-Shift-E. Opening a card calls `open_thread_tab`. Selecting a thread in
  the sidebar closes it.
- **Scenes:** `overview`, `-focus-moved`, `-approval`, `-all-idle`,
  `-empty`, `-one-project`, `-many` (300 threads), `-narrow`, `-no-sidebar`.
- **Bench:** a new case opening and closing it with 300 threads, and d1 to
  d7.
- **Done when** it opens from the row, the key, and the palette; arrow keys,
  Enter, Escape, Home, and End work with no pointer; the counts match the
  cards on screen; a thread changing state changes its card without the
  view being reopened; and no host request is made by opening it.

### 19. Bubbles

- **Preference:** `turns` (`bubbles` or `line`, default `bubbles`), lenient,
  with `Turns` in Settings → Appearance.
- **Drawing** (`live_conversation.rs`): `live_message`, `message_author`,
  `live_stream`, and the strand functions take the style. Bubbles as
  [specified](#bubbles--planned-step-19): no labels, no strand, no ring or
  node; the padding of `live_event` becomes the spacing given there. The
  line as built. Folded earlier turns (`folded_turn_row`) in each style.
- Selection, copy, Find marks, links, and images work the same in both: a
  request's text is still one selectable owner.
- **Scenes:** `bubbles`, `-long-request`, `-images`, `-streaming`,
  `-folded-turns`, `-find`, `-selection`, `-narrow`, and `turns-line` with
  the same content; keep `message-roles-*` under the line.
- **Bench:** c1 to c5 and e1 to e3 in both styles.
- **Done when** a request is told from a reply at a glance in both defaults;
  selecting across a request and a reply copies the same text as today; the
  accessible names still say who wrote each; switching the setting redraws
  the open conversation without losing its scroll position.

### 20. Folded work and the closing line

- **Model** (`plait-client/src/folding.rs`, `tool_view.rs`): a finished
  turn's rows between its request and its final reply form one fold, with
  the rules [above](#folded-work--planned-step-20): outcome `Finished` only,
  never a running turn, opened by Find and by the person, the setting off
  shows everything. `Runs::plan` no longer groups a finished turn's tools;
  it still folds a running turn's earlier steps. The fold's label is the
  summary `summary()` writes today. Tests for each rule, for a turn with no
  work, for a turn whose final reply is not loaded, and for steered turns.
- **Preference:** `fold_finished_work`, optional, default on, with `Fold
  finished work` in Settings → Appearance.
- **Drawing** (`live_conversation.rs`, `tool_card.rs`): the fold row and
  its opened body. Folded rows keep their list entries at zero height, as
  folded tools do, and opening keeps the activated row in place, as opening
  a folded turn does.
- **Closing line** (`turn_records.rs`, `record_line`): as
  [specified](#closing-line--planned-step-20). The state word on every
  closed turn; `N files` with totals opening the list; `Undo` or `Redo` and
  `Review`. `Review` and a file call into the panel of step 15. The
  older-host line (`turn_record`) is restyled and otherwise unchanged.
- **Scenes:** `work-folded`, `-opened`, `-failed-open`, `-running`,
  `-no-work`, `-find-inside`, `-setting-off`; `closing-line-finished`,
  `-files-open`, `-many-files`, `-failed`, `-stopped`, `-undone`,
  `-undo-refused`, `-no-files`, `-narrow`, `-older-host`.
- **Bench:** c1 to c5, with a 3,750-row conversation.
- **Done when** a finished turn reads as request, one row, answer, closing
  line; a failed turn shows all its work; Find reaches a match inside a fold
  and opens it; opening a fold does not move the page; a long thread scrolls
  as fast as before; and Undo from the closing line behaves as today.

### 21. Message box, changed files, approval

- **Box** (`thread_controls.rs`, `message_box`): the fill, outline,
  corners, box shadow, and focus ring; the settings row's controls at 24
  with the hover fill; Send as the primary button and Stop in coral; queued
  rows restyled. No control, lock reason, or notice is dropped.
- **Strip** (`thread_files.rs`, `thread_files_card`): drawn joined to the
  box as [specified](#changed-files-strip--planned-step-21), closed at
  first; its rows open the panel on `Uncommitted`.
- **Approval** (`live_conversation.rs`, `approval`, and where the composer
  stack is laid out): the card as [specified](#approval--planned-step-21),
  in the box's place, with `Reply instead`. Whether the box is drawn is
  state on the thread's workspace (`thread_workspace.rs`), reset when the
  approval is answered. The composer entity is kept alive while not drawn.
- **Scenes:** `box-idle`, `-focused`, `-running`, `-queued`, `-wrap`,
  `-locked`, `-mode-differs`; `strip-closed`, `-open`, `-many`, `-hidden`;
  `approval-command`, `-edit`, `-protected-paths`, `-mcp`, `-reply-instead`,
  `-narrow`; keep the `composer-v3-*` scenes passing.
- **Bench:** e1 to e3 while typing.
- **Done when** a draft typed before an approval arrives is still there
  after it is answered, and after `Reply instead`; both approval keys work
  with focus anywhere in the window and with the repeat guard; IME
  composition does not submit; the strip's counts match Changes; and the
  box's shadow does not cost a frame while the conversation scrolls.

### 22. Git step

- **Model** (`plait-client`; `sync` moves there from `commit_panel.rs`
  unchanged): `next_git_step(changes, staged, sync) →
  None | Commit | Publish | Push | Pull` with a blocker text, tested for
  each row of `sync` and for a dirty tree.
- **Drawing** (header): a primary button, 24 high, labelled with the step.
  `Commit…` opens the panel on Changes, `Uncommitted`, with focus in the
  commit message; it writes nothing. `Publish`, `Push`, and `Pull` run the
  operation the commit bar runs, with the same blockers; a blocked step is
  disabled with the blocker in hover text. With nothing to do it is not
  drawn. It is not drawn for a plain folder.
- **Scenes:** `git-step-commit`, `-publish`, `-push`, `-pull`, `-diverged`,
  `-clean`, `-plain-folder`.
- **Done when** the label always agrees with the commit bar's sync row, no
  Git status is read that was not already read, and each operation reports
  its result as it does from the commit bar.

### 23. Quote selection

- With a selection inside one reply, `Quote selection` shows beside `Copy
  message`, and the action `conversation.quote_selection` is in the palette.
  It appends the selected text to the draft as a quote, one `> ` per line,
  then a blank line, and focuses the draft. It never sends.
- **Files:** `selection.rs`, `live_conversation.rs`, `composer/draft.rs`,
  `actions.rs`; the quoting as a pure function in `plait-client` with tests
  (several lines, a selection across blocks, a selection inside code).
- **Scenes:** `quote-selection-button`, `-in-draft`.
- **Done when** it works by pointer and by key, keeps an existing draft and
  its caret, and the draft's unsent revision is preserved.

### 24. Remove Work view

Skip this step if D5 is declined.

- Remove `work_view.rs`, `work_view/header.rs`, the split tree and its
  operations in `plait-client/src/panes.rs`, their tests, and the `work-*`
  scenes. Keep `pane.next` and `pane.previous` on the panel. Remove
  `pane.split_right`, `pane.split_down`, and `pane.close` from menus and
  the palette; a saved key for one of them is kept in the file and does
  nothing.
- **Done when** nothing refers to Work view, the guide and shortcut list no
  longer describe it, and a keymap that names a removed action still loads.

### 25. Inspect and close out

- Build the release app and go through every view on this page in the dark
  and light defaults, at a wide window, at half a display, and at the
  narrowest size, by pointer and by keyboard, with VoiceOver on for the
  sidebar, the header, the approval, and Every thread. Fix what it shows,
  each fix as its own small pull request.
- Run `bash scripts/bench-frames.sh` for all cases against
  `v0.1.0-preview.80` and record both in [verification](docs/VERIFICATION.md).
- Update the README pictures, the [guide](docs/GUIDE.md), and the
  [shortcut list](docs/SHORTCUTS.md); mark the parts of this page Built;
  empty [Built today, to be replaced](#built-today-to-be-replaced); close
  [#130](https://github.com/mostlyheath/plait/issues/130) or say what is
  left.

### 26. Motion, measured

Not started until step 25 is done and a number says it is affordable.

- An animation in GPUI re-renders the view that owns it, and every view
  above it, each frame; most of the window is one view today. Measure first:
  a spike that animates the panel's width over 160 ms and reports the frame
  cost with `bench-frames.sh`, at a 120 Hz budget of 8.3 ms.
- If it fits: the panel opening and closing (160 ms, ease out), a menu or
  picker appearing (120 ms, opacity only), a fold opening by the person's
  action (140 ms). Nothing that happens without the person's action
  animates. All are skipped under Reduce Motion and while a drag is under
  way.
- If it does not fit, the fix is smaller views, and cached views lose their
  accessibility nodes at the pinned revision. Stop and write up the choice
  instead of shipping motion that drops frames.
