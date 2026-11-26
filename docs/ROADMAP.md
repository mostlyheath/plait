# Product roadmap

This page records agreed scope and priorities. See the [guide](GUIDE.md) for
shipped controls, [Host](HOST.md) for contracts, and [Verification](VERIFICATION.md)
for evidence and limits.

## Now / Next / Later

### Now

- Finish host-first version one: the workspace stream, host Git and file reads,
  checkout watching, per-device workspace state, and operation IDs are built.
  Complete their verification and compatibility work.
- Explore the prompt tree with mockups before agreeing its model and view.
  See [Host-first Plait](HOST-FIRST.md#the-prompt-tree).
- Redraw the desktop as the [design](../DESIGN.md) sets out, in the order of
  its [build order](../DESIGN.md#build-order), steps 10 to 26: new tokens
  and controls; one sheet with one header in place of the tab row, workspace
  bar, and status line; thread cards in the sidebar; a panel for Changes,
  Files, and History; Every thread;
  requests as bubbles, with the line kept as a setting; folded work; and the
  approval in the message box's place. Every step is presentation only: no
  change to the protocol, to state the host stores, to permissions, or to Git
  writes. Each step is inspected in the native app before it merges. The
  layout it replaces was released without that inspection; see
  [#130](https://github.com/mostlyheath/plait/issues/130).
- Make the test suites reliable and faster. Several tests depend on timing and
  fail now and then on changes that do not touch them, and the macOS check
  takes about 20 minutes. See
  [#128](https://github.com/mostlyheath/plait/issues/128) and
  [#129](https://github.com/mostlyheath/plait/issues/129).
- Write an end-to-end walkthrough of how the system works. See
  [#134](https://github.com/mostlyheath/plait/issues/134).

### Next

- Show several independent machines in one sidebar. Hosts report a
  fingerprint per project and the client can group copies across machines;
  no view uses it yet. See [#133](https://github.com/mostlyheath/plait/issues/133).
- Update Linux hosts in place, from the machine and from the Mac. Prebuilt
  Linux hosts are published with each release and install from the Mac.
- Complete independent client and host releases and update flows.
- Verify detached Pi runners that survive a host restart; they are built behind
  `PLAIT_DETACHED_RUNNERS=1` and remain off by default.
- Build automations: threads that run on a schedule in a project or
  directory, run by the host. The design and host contract are agreed; see
  [Automations](AUTOMATIONS.md).
- Build the thread graph: split work into linked threads, with a Graph tab
  per project. The design and host contract are agreed; see
  [Thread graph](THREAD-GRAPH.md).

### Later

- Add the next ACP harness under the [agreed contract](ACP.md).
- Complete turn review: safe staging by turn and changes since last reviewed;
  agree authorship, overlap, review semantics, and storage first.
- Add MCP browser OAuth, token reuse and refresh; named sandboxed CLIs wait
  for a project that needs them.
- Add host-enforced Pi read-in-place, further worktree storage management, and
  the larger draft recovery format after their safety and compatibility gates.
- Explore Linux command containment; trusted execution is built. See
  [Linux sandbox](LINUX-SANDBOX.md).
- Complete installed first-launch and live remote and Linear verification;
  signing and notarization remain deferred.

## Milestones

Milestone IDs identify scope, not build order.

| Milestone | Built | Remaining |
| --- | --- | --- |
| M1 — Native foundation | GPUI desktop, navigation, conversation, Git reads | Maintenance |
| M2 / M2.5 — Daily use | Durable host runs, approvals, history, settings, tabs, optional worktrees | Independent updates and installed first-launch acceptance |
| M3 — Remote execution | SSH and Tailscale, prebuilt Linux hosts installed from the Mac, Linux trusted commands and user service, remote Git and files | Linux host updates, live verification, containment |
| M4 — Development tools | Git review and agreed writes, file viewer, editor and terminal handoffs | Further review features |
| M5 — Previews | — | Planned previews; scope requires agreement |
| M6 — Multiple machines | One machine per window, host-owned workspace and machine switching | Several machines in one sidebar |
| M7 — Harnesses | Pi, Codex, Claude Code | Next ACP harness |
| M8 — Skills and tools | Pi skills, instructions, templates and MCP without browser OAuth | MCP OAuth; conditional named CLIs |
| M9 — Turn records | Records, Undo/Redo, immutable recorded diffs | Staging by turn and review tracking |

Each change needs its own checks. Agree changes to shared models, protocol,
storage, and permissions before implementation; preserve existing stores and
older reply shapes.

## Host and release boundaries

The host owns execution, durable threads, approvals, checkout safety, and
recovery. The native Mac client presents host state; closing or updating it
must not stop accepted work. Hosts remain independent, with authenticated
remote transport and no public unauthenticated socket. A window uses one
machine at a time. See [Remote hosts](REMOTE.md).

Protocol version 7 is an agreed change to the protocol and to stored state:
the host stores how each run ended and reports it on retained turn records,
so the desktop does not fold an earlier turn that failed or was stopped.
Older clients and hosts keep the previous reply shape. It is built; its
limits are in [Host](HOST.md#protocol-version-7).

**Input consumption (agreed).** The owner agreed this change to stored state on
2026-11-25, for [#131](https://github.com/mostlyheath/plait/issues/131). Each
accepted request row records how its input was used: it started its own run,
or it steered into a run. Rows consumed before the change read as unknown, and
recovery keeps its earlier inference for them. The change adds a column and two
triggers that fill it. The triggers are stored in the schema, so writes made by
an older build after a rollback are marked too, and those writes still succeed.
Another model reviews it independently before merge.

**Per-machine saved work (agreed).** The owner agreed this change to the
desktop's saved workspace on 2026-11-25, for
[#132](https://github.com/mostlyheath/plait/issues/132). The same day the owner
amended it: saved work is keyed by the machine entry the person selected in
Plait, not by the verified host identity, and kept in one file per machine
([design](https://github.com/mostlyheath/plait/issues/132#issuecomment-7153471382),
amended below it). What a person types while a machine is selected belongs to
that machine from the start.

Each machine's unsent drafts and thread work are kept in their own file under
`desktop-machines/`, beside `desktop.json`. Thread work is keyed by thread ID
and folder, and includes waiting messages and the thread's approval choice.
Older builds never read or write these files. The window shows only the
selected machine's work. Writes go to memory first, then a debounced
background write of only the changed machine's file, one writer per file.

Sending, and settling saved waiting messages from host receipts, require the
selected machine's host identity to be verified. Receipts settle a message
only if the host that accepted it is the one verified now. On the first
launch, `desktop.json`'s older drafts are copied once, in the background load,
into this computer's file. They stay in place for older builds, and each
copied record is noted so it is not copied twice. A machine's file may hold up
to 8 MiB. Past that, its work is kept in memory with a notice. A close or a
quit that cannot save stays open. A quit that cannot be cancelled, such as
a system shutdown, writes the unsaved work to a recovery file. Another model
reviews the change independently before merge.

Client and host releases must be independently selectable and publishable.
Compatibility is negotiated through protocol versions and capabilities, not
matching release numbers. Preserve supported old/new pairs and stores; show a
clear update requirement for incompatible pairs. A client update must not
replace the host. Host activation excludes new admissions and waits for a safe
point without interrupting accepted work. Separate selection, publication and
update flows remain planned; agreed contract steps already built are described
in [Releasing](RELEASING.md#independent-client-and-server-releases).

## Git, files, and worktrees

Agreed Git writes are staging by file or hunk, unstaging, committing, push,
publish, fetch, fast-forward pull, confirmed and undoable discard, creating and
switching branches, restoring files a turn changed when it is undone, and
adding or removing an optional per-thread worktree. General history rewriting,
force push, amend, merge, rebase, cherry-pick, and branch deletion are outside
this scope.

Git reads disable optional locks, external diff, textconv, and filters. Writes
use the person's Git configuration, hooks, filters, and host credentials.
One writer owns a checkout at a time; linked worktrees also coordinate shared
Git metadata. The file viewer is read-only, bounded, and does not follow
symlinks out of the checkout. Filesystem and Git work stays off the UI thread.
Editor and terminal handoffs use configured commands.

**Plain folders (agreed).** A project can be a folder that is not a Git
repository. Threads, approvals, and the file viewer work there; Changes,
History, branches, worktrees, turn records, and undo need Git and say so.
A folder is plain only when no repository marker (a `.git` file, or a `.git`
folder with `HEAD`) exists at it or above it, so a broken repository is
refused rather than treated as plain. A thread's kind is fixed when it is
created. Nothing in a plain folder is protected as already changed, and
nothing an agent changes there can be undone. Plait does not initialize
repositories.

Worktrees are optional and begin at a captured commit, without copying dirty
work. Removal verifies identity and safety, preserves branches and history,
and refuses uncommitted or unknown ignored files. Publication uncertainty
requires an explicit preservation exception. Approved build-output reuse is
macOS-only. The durable checkout queue holds interrupted pending inputs for
Resume; interrupted effects are not replayed. See [Host](HOST.md).

## Harnesses and tools

Pi, Codex, and Claude Code share the host's thread model. Adapters use pinned,
documented programmatic interfaces and translate events into that model;
threads retain their harness identity and history. Each harness keeps its own
authentication, configuration, and session files. Plait adds no instructions
or tools to Codex or Claude Code, does not read their credentials, and does not
make unrequested model calls or send telemetry. Optional thread naming is a
separate, explicitly enabled model call through the host's provider catalog;
it does not use either harness's conversation or credentials.

**Thread naming (agreed).** Settings → New threads has host-owned automatic
naming and a separate provider/model choice, including custom local endpoints.
Choosing a model enables naming; an unconfigured host makes no naming calls.
Only the first newly accepted message is summarized, in the background, without
tools, files, or images. A failed or interrupted call keeps the prompt-based
name, with no retry or fallback provider. Existing conversations and names the
person chose are not replaced. Settings changes cancel pending naming work.
The existing title field and older reply shapes stay unchanged; separate
settings operations let an older host refuse this feature cleanly.

Pi uses Plait's approval and command policy; other harnesses use their own
permission and sandbox modes, which the UI must identify accurately. The host
stores translated events and never replays interrupted work. Agree capability,
protocol, and version checks before adding an adapter.

Pi discovers skills, instructions, templates, and configured MCP tools.
Project resources require trust; changed MCP configurations require renewed
enablement. Sandboxed projects require approval for every MCP call. Approval
gates calls, not server startup or network access. Interrupted calls are not
replayed; sampling and elicitation are refused. Other harnesses load their own
resources. See [Host](HOST.md#mcp-server-listing-and-project-switches).

## Turn records and review

The host records the checkout before and after each run for every harness and
permission mode. A submission and its steering messages share a pair; queued
follow-ups get their own. Records cover bounded working-tree bytes, executable
bits, and symlink targets, independent of what the harness reports. Active
pairs are protected; records otherwise expire after 30 days. Failure to record
before prevents execution; failure after prevents Undo.

Undo restores eligible paths only when they still match the recorded after
state; conflicts offer Undo the rest or Cancel. Undo can itself be undone. It
does not reverse Git history, external effects, other permissions, extended
attributes, or hard-link relationships. The checkout lease cannot exclude every
external race. Exact limits and recovery are in [Host](HOST.md#protocol-version-5).
Saved diffs are built; matching current changes to a turn and staging safe
changes, and tracking review per file, remain planned.

## Thread tabs and Linear

Thread and draft tabs have persisted order, bookmarks, keyboard navigation,
overflow, and per-thread view state. The host stores tabs, layout, and selection
per device; closing a tab does not stop a run or remove a worktree. Only the
active workspace retains loaded content and a transcript watch. Settings does
not change the selected tab. See [Guide](GUIDE.md).

Linear settings and project issue views are built: search and read issues,
insert references into drafts, and start a thread from an issue while retaining
its source link. The host stores the key, project binding, cache, and thread
links. Explicit person-initiated comments and workflow changes have durable
write receipts; nothing writes automatically. No harness or agent receives a Linear tool; writes come only from explicit requests on the host socket, which any process running as you can reach. Issue-triggered runs and browser OAuth require separate agreement.
Live API verification remains open. See [Linear](LINEAR.md).

**Issue writes and images (agreed).** The owner asked on 2026-11-25 for the
Issues tab to create and edit issues and to show their images. Agreed: three
additive host operations, which the desktop sends only on a person's explicit
action. No harness or agent receives a Linear tool; writes come only from explicit requests on the host socket, which any process running as you can reach. `linear_create_issue` creates an issue in the linked team with a
title, an optional description and state. `linear_update_issue` changes an
issue's title or description and refuses when the issue changed in Linear since
it was read. Both use the existing durable write receipts and request IDs.
`linear_read_image` reads a PNG or JPEG uploaded to Linear's file storage
(`uploads.linear.app`) through the host, so the key never reaches the desktop,
in bounded chunks with a bounded cache. `linear_info` reports each capability;
an older host shows no such controls.

## Proposed, not yet agreed

- Run person-selected checks after a turn and show their results.
- Trace lines to turns, expand turn records, shelve results, or bisect turns.
- Provide an interactive proof of sandbox and undo behavior.
- Offer hosted machines.
- Read and edit documents natively, as Sidelit does: rendered Markdown,
  math and Mermaid, select a passage to ask the agent, and explicit saves.
  The direction is agreed; the host contract is not. See
  [Documents](DOCUMENTS.md).
- Add a browser or a web view.
- Extend editor and terminal handoffs.
- Show local usage totals with agreed accounting and storage.
- Add to the thread list what a sidebar card cannot show today: each
  thread's branch, when its run started, totals of what it changed, what it
  is doing, and its pending approval. Send when a turn started and finished
  with its record. See the [design](../DESIGN.md#needs-agreement).
- Report, read-only, the agents a thread's harness starts itself. Plait
  starts none and adds no orchestration.
- Read provider limits and reset times, and say so when a limit ends a run.
- Read pull-request and remote check state for a pushed branch.

## Performance goals

Targets, not installed-app measurements:

- 60 FPS or better for normal local interaction; 120 Hz streaming reveal.
- Cached thread switching and Git views below 50 ms; warm launch below 250 ms.
- Under 5 ms added per harness event, incremental events, and no idle process
  per thread; remote events must not block input.
- A warm unchanged 50,000-file record below 300 ms on Apple silicon.

Use background work, cached state, and virtualized views. Publish installed-app
results with their measurement conditions; see [Verification](VERIFICATION.md)
and the [ADEMark plan](ADEMARK.md).
