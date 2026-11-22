# Architecture

## Scope

The local preview (M2.5) uses a native desktop and an independent host. The workspace
contains `crates/plait` (native desktop), `crates/plait-core` (headless models,
client state, local host client, and Git), and `crates/plait-host`
(local daemon and filesystem/process helpers). `runtime/` contains the pinned
Pi Durable TypeScript sidecar. There is no remote execution,
embedded terminal, or editor. Git writes are limited to
staging, unstaging, committing, pushing (or publishing a branch), fetching,
fast-forward pulls, discarding unstaged changes after saving a copy for undo,
creating a branch at the current commit or switching to a local branch, and
adding or removing an optional per-thread worktree.

This remains an early local preview. Linux command execution and remote
transport are not implemented. [Verification](VERIFICATION.md) records versioned
Mac installation, sandbox, and recovery evidence, with the remaining gaps.

```text
crates/plait/src/
main.rs                 native app lifecycle, window, startup
ui/                     GPUI layout, actions, focus, lists, theme
  mod.rs                UI state and background-task dispatch
  a11y.rs               explicit-ID accessible text helper and debug-test prepaint collector
  actions.rs            every application action: ID, title, menu, default key
  commands.rs           command palette sources and the overlay above the workspace
  branches.rs           the branch control and its palette: switch to or create a branch
  palette.rs            search field over a ranked list; ranking off the UI thread
  model_picker.rs       model catalog as a palette; default, favorites, new thread
  new_thread.rs         the mounted new-thread draft and its controls
  draft_tabs.rs         independent draft configuration, creation owners and late replies
  draft_titles.rs       bounded draft titles and updates to cached presentation rows
  new_thread_view.rs    the draft's project, model, harness, permissions row, and menus
  harnesses.rs          agent harnesses: the host's list, setup, Settings → Agents, mode lines
  permissions.rs        the Permissions chip and menu of a harness's own modes
  text_field.rs         single-line input: IME, selection, clipboard, undo
  navigation.rs         virtualized projects and threads, filtering and management
  navigation_host.rs    cross-checkout listing, live summaries, and notifications
  tabs.rs               measured native rows, overflow and keyboard dispatch
  tab_identity.rs       background host/directory verification and safe promotion
  tab_lifecycle.rs      park, hydrate and retire the single mounted workspace
  tab_bookmark.rs       semantic locations, selection, find and editor bookmarks
  tab_persistence.rs    debounced tab saves and verified launch restore
  host.rs               host connection, thread watch, creation, and request guards
  conversation.rs       no-thread state and the Git inspector
  live_conversation.rs  real host messages, ordered tools, and approvals
  history.rs            paged history reads off the UI thread, and their parts
  markdown.rs           drawing precomputed conversation blocks and code
  selection.rs          text selection, copy, and link clicks in the conversation
  conversation_find.rs  find in the conversation: background search and highlights
  tool_card.rs          one line per tool call, its details, and grouped runs
  thread_controls.rs    model, thinking, context, run settings, and queued inputs
  thread_files.rs       files the thread changed, above its composer
  thread_workspace.rs   GPUI presentation container for the single selected thread workspace
  turn_records.rs       guarded record pages, closing-line undo, conflict panel
  changes.rs            changed-file list and diff view
  commit_panel.rs       staging buttons, commit message, and write results
  commits.rs            the History view: commit list and a commit's patch
  file_view.rs          the Files view: quick open, virtualized lines, find, reload
  panels.rs             sidebar and inspector widths and their drag handles
  window_bar.rs         the traffic lights' place, header inset, and window dragging
  jump.rs               Cmd-1 to Cmd-9 and the desktop preferences file
  session.rs            draft and submission recovery, and save scheduling
  settings.rs           the Settings modal contents (the app and host) and sections
  settings_project.rs   a project's modal target, independent of the active checkout
  settings_modal.rs     centered modal, dismissal, shortcut shield and focus containment
  settings_keyboard.rs  the shortcut editor
  settings_editor.rs    Settings → Editor: what opens files and terminals
  ways_out.rs           Open in Editor, Show in Finder, Open Terminal Here; starts them
  settings_policy.rs    the checkout's command policy, read from the host
  settings_providers.rs provider sign-in, API keys, and custom endpoints
  update.rs             release checks and in-app update installation
  worktree.rs           starting a thread in its own worktree, and removing it
  theme.rs              the current palette and the small original controls
  appearance.rs         light and dark themes, the themes folder, and their settings
  snapshot.rs           headless scene snapshots of the root view (tests only)
  snapshot/a11y.rs      headless accessibility duplicate and steady-frame guards (tests only)
  snapshot/showcase.rs  README screenshots over a sample project (tests only)
  snapshot/ways_out.rs  which file and line the ways out pick; Settings → Editor (tests only)
  snapshot/find.rs      find in the conversation: scenes and tests (tests only)
  images.rs             bounded thumbnail cache and larger view; background decoding
  composer/             native input, IME, clipboard, undo, and its events
    images.rs           paste/drop attachments, validation, private draft originals
    draft.rs            text, selection, undo, acceptance guard, prompt recall
    history.rs          prompts already sent in the thread
    input.rs            wrapped layout, auto-growing height, painting
    trigger.rs          `@` and `/` detection and the inserted mention text
    completion.rs       reusable suggestion list: state and rendering
    mentions.rs         background file ranking and its stale-result guard
    commands.rs         the local command table
    parent.rs           what the window supplies and performs for a composer

crates/plait-core/src/
host.rs                 bounded Unix-socket framing and requests; background only
host/watch.rs           cancellable reader and bounded async snapshot delivery
host_state.rs           scoped host connection, selected-thread state, and submission ledger
checkout_state.rs       legacy creation selection and shared repository data/caches
thread_view.rs          presentation-independent thread view enum and Git view selection
ownership.rs            host/thread/checkout identities and read lifetimes
operations.rs           thread-owned undo acknowledgements and uncertain retries
state/registry.rs       checkout routing, root verification, and inactive write outcomes
tabs.rs                 headless tab order, lifetimes, transitions, and cold residency
tabs/bookmark.rs        bounded semantic workspace bookmarks; no loaded content
tabs/row.rs             catalog-derived tab labels and measured-width overflow grouping
tabs/persistence.rs     pure versioned tab-preferences codecs; no journal access
tabs/storage.rs         bounded no-follow reads and private atomic tab writes
session/blobs.rs        bounded private bookmark and parked-editor records
transcript.rs           stable row identity and incremental list-change plans
history.rs              stored/live merging, text parts, memory limits, run wording
history/records.rs      complete record pages, run membership and closing-line projection
lease.rs                authoritative checkout lifecycle, writer, and common-dir OS leases
local_socket.rs         owner-only paths and OS peer-identity checks
state.rs                application state, repository job coordinator, and accessors
model.rs                plain Rust data, no GPUI
machine.rs              this machine's display name
preferences.rs          desktop-only preferences and Cmd-number order
handoff.rs              editor and terminal commands: parsing, escaping, starting
fuzzy.rs                subsequence matching and scoring for pickers
navigation.rs           project grouping, activity tracking, filtering, and jump order
session.rs              private desktop journal, identity checks, writer lock
markdown.rs             plain text blocks and style ranges, parsed off render
reveal.rs               frame-paced reveal of streaming text and cut blocks
theme.rs                palettes, Base16 scheme parsing, contrast floors
file_view.rs            bounded no-symlink file reads, line index, find, change stamps
highlight.rs            lexical syntax classes for fenced and tool code
diff.rs                 line diff of an edit's old and new text
patch.rs                which rows a Git patch draws, each mapped to its patch line
review.rs               quoting picked diff lines into a message
tool_view.rs            what each tool call shows, and grouping of long runs
selection.rs            selection order, ranges, word bounds, and copied text
conversation_find.rs    case-insensitive matches in drawn conversation blocks
git.rs                  synchronous Git adapter: hardened reads; background only
  git/write.rs          stage, unstage, commit, push, pull, fetch, discard
  git/branch.rs         list local branches; create and switch (via write)
  git/log.rs            commit history and one commit's patch (read-only)
  git/counts.rs         uncommitted line counts for named paths (read-only)
  git/worktree.rs       add and remove optional per-thread worktrees
  git/record.rs         private checkout records, coverage, changed paths, pruning (M9)
  git/undo.rs           checked working-file restoration and durable undo journal (M9)
```

The desktop imports `plait_core`; the core has no GPUI dependencies. Core and
host Rust tests can run on Linux without compiling the desktop. The default
Cargo member remains the macOS desktop. The macOS workspace checks still cover
all crates, including the native composer. See CONTRIBUTING.md for both commands.
The core uses Unix filesystem and socket APIs; it is not a Windows port.
Filesystem, socket, and Git operations run on the background executor.

A Linux build does not enable agent command execution. The runtime still rejects
non-macOS commands under either command policy, and the Rust command guardian
rejects both its sandboxed and trusted entries there. Linux
containment and cancellation need a separate safety review. Local peer identity
is checked with `getpeereid` on macOS and `SO_PEERCRED` on Linux.
Both platforms use the same local protocol (version 1, with opt-in versions 2
and 3) and the same ownership boundaries.

The Linux sandbox mechanism is an open decision. An unprivileged namespace
probe succeeded, but a read-only bind mount still permitted access to an outside
pathname Unix socket. See [the evidence, alternatives, and enablement gates](LINUX-SANDBOX.md).
Landlock ABI 9 is one option, not a fixed product requirement.

## Client/server direction

[Host-first Plait](HOST-FIRST.md) is the agreed next step: every host
operation is available to any client, the desktop runs no Git and keeps no
project, tab, or unread state of its own, and clients follow a snapshot and
numbered events. The sections below describe the current, mixed ownership.

The [roadmap](ROADMAP.md#clientserver-platform-and-installation) agrees a
client/server platform with independent updates. Plait Server is the product
boundary represented today by the host and runtime; `plait-core` remains an
internal library, not a separately installed component. Other apps will use
the versioned public protocol, not GPUI or Rust implementation details.

This is direction, not shipped installation behavior. The app currently carries
its host archive, and the in-app updater replaces both components. The first
change separates initial local-server setup from explicit replacement. A bundled
server is a bootstrap source, not a requirement to keep server and client
versions equal. Remote transport and additional clients remain later work.

## Checkout recording and undo

The TypeScript runtime owns run boundaries for every harness. `records.ts`
records before entering a harness and after releasing it, keeping the checkout
lease until the latter completes. Steering joins the active pair; queued
follow-ups get new pairs. The `records` and `record_paths` tables in
`host.sqlite` hold ordering, states, exact changed entries, line counts, and
coverage exclusions. Before-record failure prevents harness startup and holds
the queue; after-record failure disables future undo for that run. Startup
recovery marks a late after record as possibly including changes while the host
was down, and uses the checkout lease. A durable cleanup marker defers that
recovery until process cleanup is confirmed.

The Rust host exposes internal `record` and `prune` helpers, running the headless
core's Git recorder in a macOS Seatbelt process. Private Git stores are separated
by object format and have one private index/HEAD per checkout identity. The
person's repository is read-only, including its object timestamps. Only a
run's before and after commits are linked, so expiring old pairs can release
objects. Startup and daily pruning pause recording and reclaim expired refs,
private caches, loose objects, and packs after 30 days, including for archived
threads. The [host contract](HOST.md#checkout-records)
describes the layout, limits, exact-byte policy, recovery, and platform boundary.
A directory inventory excludes unreadable folders, unsupported entries, and
embedded repositories, including bare Git directories. Private records keep the
filesystem's name spelling and reject equivalent aliases in changed trees.
A supervisor holds a lifetime pipe to a worker whose process group contains
Git and sandbox descendants. Helper death closes the pipe; worker death or a
240-second record/prune deadline kills the group and releases inherited locks.
Git commands have at most 120 seconds within that budget; the runtime's outer
record/prune timeout is 300 seconds.

Protocol version 5 adds record listing and undo.
`record-views.ts` pages complete paths, coverage and request membership within
the client frame. Snapshot `records_revision` changes when records change;
older snapshot shapes are unchanged. The desktop draws the closing-line
control from these. `history/records.rs` assembles page fragments at their offsets and maps
run request IDs, including steering, to the last loaded message or tool. It
follows undo links to the usable record and prepares file names, counts and
hover text off the UI thread. Detached history does not claim its final run
has ended; missing requests do not produce an invented step count.

`ui/turn_records.rs` fetches on selection and `records_revision` changes. Thread,
connection epoch, watch epoch, revision and fetch generation reject stale pages.
The conversation projection prepares a map; render only looks up its visible
rows. Changed records and conflict panels remeasure affected list rows, including
rows whose message-based summary disappeared because steering joined a run.
The changed-files card merges complete record paths with existing provenance
paths, including when retained records are empty. Separate request generations
keep record refreshes from invalidating provenance replies; thread and connection
guards still reject stale replies. Git supplies current uncommitted counts.
Hover and conflict-panel previews are prepared before rendering, show at most
12 entries per list, and report omitted counts. Undo requests use version 5 only; older hosts keep their existing
summaries without controls. An explicit retry of an unconfirmed undo keeps its
request ID and flags; an interrupted undo is refreshed, never replayed.
Cmd-Option-U is the Thread action Undo Last Turn and uses the same gate as the
visible control, with settings and palette visibility checks and key-repeat
suppression. Failed undo records marked `no_effect` are skipped by action-chain
projection, so the original target stays available without replaying a request.

`plait-host undo` holds the checkout lease and runs the headless restoration
code under a sandbox that allows working files but denies Git-directory
writes. It takes U0, checks each target against the run's after entries and
current no-follow reads, then restores eligible files using descriptor-relative
temporary files and renames. Default conflicts apply nothing; `leave_changed`
selects only safe paths. A final U1 and its private commit note claim only
completed restores, excluding unrelated edits observed during undo. The pair
can itself be undone. Content, executable bits and symlinks are restored;
other permissions, extended attributes and hard links are outside that promise.

The runtime budgets undo at 10 seconds within the serialized sidecar's
20-second request limit. `undo_operations` stores applied-result deduplication
and pending/interrupted identities; `record_garbage` defers removal of abandoned
captures to pruning. A private durable journal precedes writes and records
completed paths. Startup recovers a missing after capture without replaying
writes and marks it after restart. Failures after applying starts can be
partial; callers refresh records and inspect the checkout. The lease excludes
host writers, not editors or other external processes. Immediate revalidation
and exclusive creation protect detected changes, but replacing/deleting an
existing path still has a check-to-mutation race. See the
[version 5 contract](HOST.md#protocol-version-5)
for paging, recovery, deduplication and these verification limits.

## Local agent boundary

The Rust daemon owns the sidecar, the owner-only Unix socket, and the store
writer lock. The desktop is a client, not the owner of an agent run. Window
closure drops subscriptions; it does not stop execution. See [HOST.md](HOST.md)
for the exact protocol, storage location, launch commands, and safety policy.

The host owns model discovery, Pi authentication, thread persistence, and the
default model. The desktop receives availability, not credentials. The host's
[provider sign-in operations](HOST.md#provider-sign-in) take a pasted key or
sign-in code from a client once and store it in Pi's files. Settings → Providers
calls these operations and refreshes the model catalog after changes.
Each thread keeps its exact provider/model pair and thinking level until a
client changes them between runs ([protocol version 3](HOST.md#model-and-thinking-level)).
Saving a default does not change an existing thread.

The host also owns each thread's title, timestamps, archive flag, and the queue
of inputs waiting behind a run. A snapshot is the bounded live tail of a thread.
The complete conversation is read on demand, in pages, from the thread's saved
conversation store; an idle thread's store is opened read-only for that, so a
history read cannot recover or run anything. The desktop loads stored history,
shows reasoning, and uses live summaries, host-owned titles, rename, and archive.
Composer controls configure models and thinking and submit follow-up or steering
messages through the same negotiated version 3 contract.

The desktop uses bounded snapshot subscriptions. Runtime state changes signal
the host through a private inherited Unix socket, separate from request/reply
stdio. The host wakes matching watchers and reads the latest snapshot; idle
watches do not poll the sidecar. IDs coalesce under backpressure, and each client
has at most one pending wakeup. Watches register before the initial read and
wait on both the wakeup and client disconnect. Channel EOF replaces the host's
periodic child-process check. Client protocol version 1 and durable schemas are
unchanged; the internal channel ships with the host/runtime update unit.

A thread-list subscription uses the same wakeup path with one reserved
notification name, sent only when a thread's summary changes. The host repeats
the bounded listing and sends the summaries that differ, so streamed text costs
list watchers nothing.

The desktop owns a cancellation lease for its selected subscription. Selection,
reconnect, and view release drop that lease; socket shutdown runs on the background
executor and does not send Stop. A dedicated reader keeps idle socket waits off
GPUI's shared worker pool. A one-slot async inbox delivers the newest snapshot
without periodic UI wakeups or an unbounded event queue. Thread ID and
checkout are checked before coalescing, so an invalid frame cannot be hidden by
a later valid one. The model is not pinned: it can change between runs. The
thread-list reader shares this machinery and delivers the merged list. Initial replies and
partial frames retain absolute deadlines; an idle established watch has no timer.

A repository epoch, watch epoch, and thread ID guard each async result. An uncertain submission keeps its request
ID for an explicit retry. Reconnect loads state; it does not submit a new task.
The composer keeps its draft until acceptance and clears it only if its edit
revision still matches the submitted draft. Each pending request retains its own
revision, queue mode, and approval mode, so out-of-order replies cannot erase a
newer draft or change a retry’s authorization. Each thread has its own composer.

Each run is submitted with an approval mode. In `ask` mode every file change and
command needs a one-call approval with full arguments; in `auto` mode, the
desktop default with a version 2 host, they run without a prompt. File helpers own checkout writes. By default a project uses the trusted
[command policy](HOST.md#command-policy), in which commands run as the user and
may write the checkout. A project can instead be sandboxed, in which commands
read the checkout and use private scratch space for build output. The
project's settings modal (its sidebar menu → Project settings…) offers the
policy, with a deliberate confirmation of Trusted access. Stop revokes pending approvals and
cancels work. Interrupted work must not replay automatically after host failure.
These boundaries need the tests listed in HOST.md before valuable-repository use.

### Pi MCP tools

`runtime/src/mcp/config.ts` validates bounded Pi configuration with
`@earendil-works/pi-mcp` **1.5.2** and follows that revision's project replacement,
restricted override, namespace, and exposure rules. Run discovery reads each
applicable configuration file once; settings additionally reads saved sign-in
metadata without using or refreshing credentials. `mcp/policy.ts` applies
checkout-identity and unresolved-definition-bound switches after merge.
Project definitions and replacements require explicit consent; unreviewed
overrides intersect user tool visibility and cannot enable a disabled user
server. Settings echoes the listing digest when enabling, rejecting a changed
definition. Other harnesses bypass this path.

`mcp/tools.ts` installs search only for eligible enabled servers. First search
connects them, then installs one deferred, sequential, unsafe-to-replay tool per
remote tool. Pi Durable freezes a tool task's `api.agent()` before execution;
the search wrapper supplies the tools just discovered to the pinned search
implementation. Code mode is never installed. Stable collision-checked names
cannot shadow local tools; safe display labels are saved on first call in a thread so
live and historical rows retain `mcp:server/tool` across restarts.

`mcp/transport.ts` owns execution-time value resolution and transports.
`PipeTransport` connects to `plait-host mcp-server`; the Rust guardian owns the
stdio process group, writer leases, and a separate lifetime pipe. The stdio
server cannot inherit private descriptors. Sandboxed servers use the command
filesystem profile with IP networking and the exact system DNS socket added; trusted servers use the
shared login environment. Streamable HTTP uses bounded fetches with no redirects
or OAuth provider. Sampling and elicitation are declined by the pinned client.
`mcp/secrets.ts` scrubs resolved configured credentials of at least eight
characters in free text. It preserves identifiers, rejects credential-bearing
tool names/titles/schema keys and constraints, and leaves inherited environment,
arguments, and URL paths/queries alone. Large results use `CommandOutputs` and
`read_output`. Closing a run revokes permission and closes its transports before
releasing the checkout. Recovery never restarts an old MCP effect.

The [host contract](HOST.md#mcp-server-listing-and-project-switches) records
permissions, bounds, deadlines, and sign-in limitations. `agent_resources` adds
optional observations only after a real connection; configuration fingerprints
and checkout identity reject stale observations. No protocol version changes. The core decodes these
fields without GPUI; `settings_agent.rs` prepares text off render. Switch replies
retain the existing checkout, host epoch, and refresh guards. `snapshot/mcp.rs`
injects settings, observed counts, tool rows, and approvals for headless scenes.
OAuth reuse/refresh and browser sign-in remain planned.

### Run authorization

`runtime/src/authorization.ts` owns a run's activation and revocation. Each new
admission gets a fresh authorization object; a saved thread does not carry
permission into another turn. Recovery keeps it inactive until old tasks are
stopped. Stop, completion, and host shutdown revoke it permanently.

`runtime/src/approval.ts` implements both policies. The interactive policy owns
the exact-call prompt, one-use answers, cancellation, and snapshot updates. The
automatic policy grants an active run's calls without a prompt. The tool
registry asks the run for authorization without owning the approval lifecycle.
Argument limits, dirty-path protection, checked file helpers, and command
containment remain mandatory in `tools.ts` and its helpers. Authorization is
checked again before an effect starts, so a late Allow cannot override Stop.
Command containment is a property of the checkout's command policy, not of the
approval mode: a run reads the policy once at admission, and only a trusted
project takes the separate uncontained path. File-tool checks apply to both.

The submitting desktop chooses the mode for one run (see
[protocol version 2](HOST.md#protocol-version-2)). There is no persisted grant.
A version 3 input [queued behind a run](HOST.md#messages-during-a-run) keeps the
mode it was submitted with and, when it starts, is admitted like any submission
with a fresh authorization object. A steering input joins the active run and
uses that run's authorization. Stop clears the queue; a failed run or a restart
holds it. A model request that fails transiently is retried with backoff; tool
calls are marked unsafe to replay and are never repeated. Tests include a clearly
labeled permissive policy fixture to check that authorization cannot bypass file
protection or command containment. On Linux, the command check proves denial,
not a working sandbox. The automation requirement remains tracked in
[issue #13](https://github.com/heath0xFF/plait/issues/13).

## Portable runtime and development

The daemon uses the core's low-level local-socket checks, not its client state.
Both accepted connections and client connections verify the peer's effective
user ID. Socket and parent-directory ownership/modes are also checked before
clients connect. The wire protocol is unchanged.

`runtime/src/safety.ts` owns common checkout, file, and lease checks.
`command/platform.ts` selects only an implemented platform policy;
`command/macos.ts` owns Seatbelt and Apple build-environment setup.
`command.ts` owns scratch lifetime and root validation, while
`command/supervisor.ts` owns bounded output, cancellation, timeout, and guardian
cleanup. Spawn failures clear their timer/listener. Final output is drained, with
a one-second drain bound after guardian exit so an unsupported detached process
cannot keep a subscription open. Each output pipe decodes UTF-8 across read
boundaries. `runtime/src/text.ts` keeps text cuts on whole Unicode code points
within the existing UTF-16 budgets. Accepted-input previews, transcript fields,
and tool output use these helpers; the original input journal is not clipped.

`command/policy.ts` owns the per-checkout command policy, its two host
operations, and the trusted wording for instructions, the tool description, and
approvals. `command/login-environment.ts` reads the account's login-shell
environment for trusted commands. `command/trusted.ts` runs a trusted command
through the same supervisor and guardian. Before the command it drops
ownership records that had already drifted; afterwards it refreshes the records
of this thread's files that the command changed and reports changes to
pre-existing dirty paths.
`tools.ts` chooses the path from the policy the run was admitted with.

`bash scripts/check.sh` is the common development entry point. Runtime test
selection is explicit: common daemon/file tests on both systems, real sandbox
tests on macOS, and command-denial tests on Linux. The full sandbox gate fails
on Linux instead of returning a partial acceptance result. Scripted guardian
tests check process lifecycle only; they do not establish containment.

## Git data flow

1. A UI action changes application state.
2. `RepositoryState::next_job` returns at most one pending job.
3. The UI dispatcher runs `Job::run` on GPUI's background executor.
4. The result returns through a weak entity handle.
5. State checks the repository epoch and selected diff key before displaying it.
6. `cx.notify()` schedules an update. There is no redraw loop or repository poll.

Only one Git job runs at a time. Rapid selections replace the pending job, not
the running job. A repository change or refresh increments the epoch. An old
result cannot overwrite the new repository. A stale file result can enter the
current snapshot's cache, but cannot replace another selected file.

Diffs are loaded on demand. Up to 16 diffs are cached per status snapshot.
Refresh clears that cache. Threads within a checkout share its Git state.

### Core state scopes

Plain core state separates ownership while preserving one selected thread slot:

- **`HostState`** (`host_state.rs`) groups host connection, catalog, and
  default-model settings (`HostConnection`), legacy direct-thread
  creation (`CheckoutCreationState` in `checkout_state.rs`), selected-thread
  read/control state (`SelectedThreadState` in `thread_view.rs`), and the
  per-thread submission ledger (`Submissions`) and undo ledger (`UndoLedger`).
- **`RepositoryState`** (`state.rs`) separates shared checkout repository data,
  caches, writes, and background job queues (`CheckoutRepositoryData` in
  `checkout_state.rs`) from the selected workspace's Git view selection
  (`ThreadRepositoryView` in `thread_view.rs`, alongside the
  presentation-independent `ThreadView` enum).
- **`ThreadWorkspace`** (`ui/thread_workspace.rs`) owns the GPUI presentation
  state for the single selected thread (active view, transcript list/scroll
  and formatted blocks, history view, images, line pick, changed files, turn
  records/review, reveal cursors, reading selection, conversation/file find,
  file viewer, Changes/History scroll handles, commit message state, and the
  existing per-thread composer map). The
  root `Plait` view retains window-owned state (focus, approval key debounce,
  sidebar navigation, overlays, settings modals, panels, new-thread drafts,
  and shared desktop services) and delegates selection, reconnect, and checkout
  resets through `ThreadWorkspace`.

The checkout's file list is a third job, started only when a composer asks for
it. It has its own queue slot, so it cannot displace a diff, and it runs after
status because it needs the root that status reports. The list is kept until
the epoch changes; a failed read is not cached, so the next request retries.
Git reads do not start SSH or network connections. Remote transport is not
implemented.

### Headless tab model

`plait_core::tabs` provides the window tab model. The desktop draws its
40-point top row and state-grouped overflow from cached core row metadata;
only width or metadata changes rerun measured layout. The active thread
adds a 36-point view row. Keyboard and sidebar commands dispatch to the
model, while presentation remains in the single workspace container.
The desktop hydrates only the active workspace; durable launch integration uses the separate tab-preference codec. `WindowTabs` owns an ordered collection of `TabRecord`s without an interactive count ceiling,
one active ID, and a session-only stack of up to 20 closed bookmarks.
`TabContent::Thread` and `TabContent::Worktrees` share tab identity, lifetime,
residency and bounded bookmarks through `TabSession`. `TabTarget` identifies
either a thread, a draft or one project Worktrees view. A thread identity includes
its host and store; its expected checkout includes the host-supplied root verbatim
and, when verified, directory device/inode. The additive `host_identity` read
verifies the host before a first thread exists. Older hosts fall back to
`turn_review_info` once a thread exists; until verification, tabs work silently
for the session and are neither saved nor restored. A replaced checkout is
reported on sidebar open, with instructions to close its old tab and reopen. Opening a thread activates
its existing tab. New-thread drafts have distinct IDs, even in one checkout; a creation
acknowledgement replaces only its originating draft without changing its tab
position or taking focus. Closed draft entries can also bind to their created
thread. Empty drafts close without entering history. Nonempty drafts outlive
the twenty-entry bookmark history in the private journal and can be reopened
at the end with Cmd-Shift-T after that history is exhausted.

The pure transitions cover activation, parking, close/reopen, reorder, cycling,
numbered selection, confirmed deletion, project removal, and reconnect.
Positions 1–8 address those tabs; position 9 addresses the last tab. Closing
an active tab selects its right neighbor, then its left; closing the last
leaves an empty model. Close never sends Stop, archives a thread, or removes a
worktree. Its caller must finish saving drafts and bookmarks before closing.
Rename and archive remain catalog facts and leave open tabs intact. Confirmed
thread deletion and explicit project removal purge matching open and closed
records; hiding a sidebar group is separate. Disconnect parks the host's tabs;
a validated reconnect reloads only the active one.

A tab retains a bookmark, not a loaded view, while hidden. Bookmarks describe
the selected view, stable conversation row/offset or follow-latest, bounded
history window, file position, diff path/layer/record and content fingerprint,
selected commit, find queries/current matches, draft cursor/revision, semantic
focus, panel/image/turn descriptors, logical selection/folds, and commit draft.
Sidebar and inspector visibility remain window preferences. Draft text, image
originals, and pending submissions remain in their existing owner stores.
The cold record is checked against 16 KiB of owned storage, including string
and path capacities. Inline bookmark JSON is at most 8 KiB, stored without
spare capacity and decoded only when requested for activation or validation.
A larger bookmark returns an explicit storage requirement; an opaque blob
reference and bounded 1 MiB extended-bookmark codecs provide the handoff for
private UI storage. No state is silently truncated on parking.

`Residency::Cold` explicitly means reload is required. The desktop mounts one
thread presentation. Parking settles composition and captures semantic view
state, then flushes the existing draft journal and any extended bookmark before
switching. A save failure keeps the tab open. The old workspace releases its
transcript watch, history, Markdown, file/diff views, decoded images, find scans,
focus handles and pointer geometry. Read replies retain the owner guards;
submission and Git acknowledgements still complete into their owner ledgers.

Inactive editor undo/recall is stored in bounded private cache records, without
retaining GPUI composers. An unavailable editor cache leaves the saved draft
intact and reports that editor history could not be restored. Projections check
cancellation between messages and tools; already executing bounded reads may
retire, but cannot install into a different workspace. Cold activation reloads
content, restores anchors where they survive, and explains a missing position.

The active conversation accounts retained text, projection and container
capacities against 96 MiB and evicts farthest history entries while retaining the
visible anchor. A single part that cannot fit reports the reading limit without
changing stored history. Repository diff/patch caches share a 16 MiB byte budget. The
editor cache is limited to 8 MiB; prompt recall also has byte limits. These are
payload budgets, not total process RSS or GPU-memory guarantees.

`TabLifetime` changes on close/reopen and draft replacement. `ReadStamp` adds
visibility and connection generations, rejecting old activation replies even
after A → B → A. Each new window lifetime, including restore, receives a fresh
`WindowId` from its caller. Per-request generations, checkout revisions, and
file/record fingerprints still need their existing checks. Mutation and submit
acknowledgements must reach their owning ledgers regardless of these read guards.

`tabs::row` derives title, state word, hover text, and shortcut hints from real
thread summaries and navigation unread flags. It does not load a conversation
or infer a current tool, exact approval call, or error body from a summary.
Activating Files does not mark a conversation read. Its layout function takes
measured widths and returns a contiguous visible range, hidden counts, and all
tabs grouped as Needs me, Failed, Working, Finished, and Idle. The UI supplies
measurements and draws the returned data. The strip indexes its cached visible
slice without scanning hidden tabs. Overflow lists above 32 entries use a
virtual list with cached row indices; hidden drafts mount no views and add no
per-frame work. The frame suite includes 300 open tabs and their virtual
overflow, measuring CPU draw phases rather than GPU presentation.

Draft titles are a desktop presentation cache, derived from the first nonblank
message line with whitespace runs folded to one space. They keep at most 120
characters before display escaping and look at only the first 4,096, so pasted
blank space is not walked on every edit. Composer notifications update just the mounted draft's cached row when that title changes;
equal-width tab layout and overflow grouping remain intact. Parking refreshes
that title from the composer, while restored cold tabs derive it once from
already-loaded draft text. Rendering reads cached titles only. Titles do not add
fields to the journal or tab preferences. Only `TabTarget::Draft` enters this
cache; thread and Worktrees rows retain their own metadata. Parking a draft
clears its mounted identity, so a front Worktrees tab has no draft title to
refresh. Discard clears the draft's journal entry and closes its tab as an empty
draft, keeping it out of reopen history.

### Worktrees graph

`ui/worktrees_view.rs` mounts one project graph only when its tab is active.
Host inventory, the single hardened Git history read and storage walks run on
background executors. Read stamps include tab lifetime, activation and host
connection; a separate generation and cancellation flag retire in-flight work
on refresh, hiding or closing. Completed old reads never install or start a
measurement. Activation and explicit Refresh read the whole inventory; there is
no polling. A thread summary for a listed checkout starts no Git or host read:
titles and state words come from the summary already held. When a run ends (the
status leaves working or awaiting approval), only that checkout's row is read
again, through `project_worktrees` with `only`, and the graph is drawn again
from every row's head. A row reply that is not exactly that checkout is refused,
and the view reads the whole inventory instead. One read runs at a time: a
retired read's host helper still runs to the end, so a read asked for during
another waits and runs once after it; two checkouts waiting become one full read.
**Remove** waits while a run works in that checkout, in the host's words,
because the facts shown were read before the run; the host refuses it then too. Hidden tabs retain only bookmarks and
bounded size cache entries.

`git/worktree/inventory.rs` supplies live host preservation facts using the
same function as removal: changed/untracked/ignored paths, hidden index edits,
remote-tracking publication and commits kept only by worktree refs/reflogs.
The runtime adds creation ownership, recorded last-use time and busy refusals.
The desktop never derives its own removal permission from Git status.
`worktree_removal` owns ask, confirmation, exact request retry and result state;
mutation replies settle their original operation even after navigation.
The host's `can_keep_local` fact distinguishes a publication-only refusal from
every other refusal. The desktop offers only permitted choices and freezes the
choices, path and identity before asking. Both the sidebar and graph use
`ui/worktree.rs` for the prompt, request and receipt; the sidebar's older-host
path retains its two explicit choices without inventing live counts.

`build_output` in core owns descriptor-relative CoW cloning, private approval
policy, immutable seed publication and recoverable installation journals.
`runtime/build-output.ts` schedules helper processes, preempts seed capture for
runs, persists outcomes and publishes progress through host notifications.
Creation awaits preparation before admitting the thread. Sandboxed Pi receives
approved Cargo output in its existing per-thread cache; other harnesses use the
checkout. Approval is stored outside tracked project configuration, with a
revision and the main checkout identity. A verified FIFO in the shared lease
directory lets admissions in other hosts interrupt capture without signaling a
PID; only the seed helper listens. Current policy is read under removal's leases
and common metadata lock; inventory uses the same preservation function.
Only approved ignored output is exempt from preservation.

`ui/build_output.rs` owns settings request guards and a pushed progress
subscription, guarded by request generation. Its progress label is prepared
only when a pushed state changes, not on frames. Project settings offers
suggestions without approving them; creation exposes Skip, and Worktrees
presents Warm/Cold and known build output.
The new inventory operation carries a capability and falls back to the prior
shape with older hosts. Policy reads, Git validation, clone work and storage
walks stay off the render path. Render consumes prepared state; clone progress
adds no timers or filesystem work per frame. Logical accounting classifies
approved custom paths and never equates shared blocks with reclaimable space.

`git::log::graph` reads at most 300 commits and parents from all checkout heads
in one command. Pure `worktree_graph::layout` follows the trunk's first parents,
gives each worktree its head's first-parent chain down to its base, puts merged-in
commits on lanes of their own, preserves merge-parent edges and leaves
unavailable ancestry detached. An edge whose column holds another commit between
its ends is routed through a column with no commits, so no line reads as a parent
link it is not. Layout and labels are prepared on changes, never during
render. A uniform list draws only visible commit/head rows, with stable semantic
IDs and keyboard selection. More lanes widen the scrollable graph. The frame
suite's fifty-worktree case measures cached graph rendering, layout, prepaint
and paint with no host, Git or storage work; it does not measure GPU presentation.

The desktop prepares rounded connector strokes and label style ranges when
the graph changes. The core retains which edges cross each row boundary;
head stubs and the first and last edge rows stop at their node centres.
Selection and hover only tint those strokes; the virtualized
list builds text and geometry for visible rows. Accessible row names retain the
title, state and branch order independently of their visual styles.

`worktree_size` bounds each walk, including the private build cache, to 50,000
entries and a 250 ms deadline checked between entries. It reaches every entry
from its parent's open directory descriptor, opens folders without following a
link and requires the folder it listed, so a folder swapped for a symlink mid-walk
cannot lead it outside. It stays on each root's device, leaves the project's other
checkouts to be measured as themselves, excludes `.git`, deduplicates hard links
within a walk and classifies conventional top-level output directories.
Values are logical bytes, not exclusive physical allocation. A 256-entry cache
keys directory identity, head, changed count, last use and removed state.
Refresh clears it; removed worktrees retain cache-only accounting. Partial
results remain lower bounds. Cancellation is checked between entries; a blocking
filesystem call is not preempted. Selected path and sorting use a Worktrees
bookmark, without loading a thread workspace.

### Tab preferences codec

`tabs::persistence` supplies pure encode/decode functions for a separate
`tabs.json` beside desktop preferences. Schema 1 holds ordered tab IDs, the
active ID, content identities, and bookmark records. Keeping it separate avoids
rewriting unrelated preferences or risking draft/submission recovery when only
a tab order or scroll position changes. The file is bounded to 16 MiB; restore
examines at most 10,000 entries. These limits bound untrusted disk input. Corrupt input, unsupported versions, missing
threads, mismatched host/store/checkout identities, invalid bookmarks, and
duplicates produce a valid window with unrestorable tabs omitted. A missing
active entry selects the first surviving tab. Every restored tab starts cold.
Encoding saves every open thread or Worktrees tab whatever the current listing holds, so a
save during loading or reconnection cannot erase tabs; only drafts not known to
be recoverable are left out.

Restoration requires a validated complete catalog and known checkout identities,
not a newest-200 watch subset. Archived threads may restore. Known disconnected
identities can remain offline; missing identities drop silently. Draft tabs,
including empty ones, restore when their caller knows the draft is recoverable;
these codecs neither read nor require the recovery journal. Closed history,
resident payloads, and submission data are never serialized. The desktop uses
bounded no-follow reads and private (0600) atomic writes on its background
executor. Input and tab transitions debounce saves for 400 ms; closing flushes
the current bookmark and journal. A serialized writer rejects superseded work,
keeps the previous file on failure, and reports a retryable save notice. Extended
bookmarks use bounded private records. A future version disables tab writes for
that session so it remains untouched. Opening tabs before restore completes
keeps that action in front while the other recoverable tabs are added.
A reconnect must verify identity again before restoring or writing tab state.
Unvisited views retain their saved locations until their content loads; opening
a file or commit explicitly replaces that pending location. Leaving Conversation
detaches its snapshot watch and cancels projection work; returning reloads its
saved history window before restoring the anchor.

**The draft journal remains schema 1 with additive fields.** Its proposed v2
index/blob redesign remains planned work with a separate compatibility review.
`draft_tabs` maps stable draft IDs to checkout, text/image draft, choices, and an
optional captured first-send request. One entry per checkout owns a `legacy`
mirror in the unchanged `new_draft`/`new_drafts` fields. Opening another empty
draft cannot overwrite that mirror. Removing its owner promotes an intact
sibling. Legacy mirrors also retain the draft's unconfirmed `creation` request,
so compatible older readers reuse the same thread and worktree identity. An older
journal imports its single draft and any captured creation; changed legacy text
from an older build imports as another draft, preserving both versions. Older
builds ignore `draft_tabs` and collect images only from the legacy fields, so
each save also copies every other nonempty draft into `new_drafts` under the key
`<checkout>` NUL `<draft id>`, which no folder can have. A draft with an
unanswered first send also gets `<checkout>` NUL `<draft id>` NUL `creation`,
whose text is the captured request and message as JSON and whose images are the
message's, so the request ID and images survive an older build's save. Older builds keep,
validate and retain images for those entries without showing them; this build
moves them back into `draft_tabs` on open. Copies are omitted when they would
exceed 4 MiB or 384 legacy records (128 below the older 512-record bound). No first send
is replayed at launch; an explicit retry reuses its captured ID and payload.
Atomic synced replacement and the 4 MiB serialized limit are unchanged. Legacy
thread/project fields retain their 512-record bound, independent drafts have a
10,000-record bound, and each draft allows 256 KiB of text. Exceeding a limit keeps the previous good file;
submission and required close/park saves fail explicitly while the current work
stays in memory. Twenty maximal drafts and a separate 4 MiB resident submission
cache are not supported capacity promises of this release.
The codecs are tested without filesystem access. Separate private-file and
isolated GPUI window tests cover saving, valid restore, damaged input and future
versions. Launch restoration waits for a complete catalog and verified identity;
an offline or older host can be used with session tabs until verification succeeds.

## Git boundary

Read operations are `status(path)`, `diff(root, change, mode)`, `files(root)`,
`log(root, skip, limit)`, `show(root, id)`, and `branches(root)`. The only
write operation is `write(root, op)`: stage, unstage, stage all, unstage all,
commit, push, publish, fetch, pull, a hunk, discard, restore, create a branch,
or switch branches. Each returns data or a user-facing error.

`write`, `add_worktree` and `remove_worktree` take a `plait_core::lease`
checkout lease before any Git runs, always on the background executor: the
lifecycle lock (exclusive for a write or removal, shared on the parent for
adding a worktree), the checkout directory's writer lock, and, for writes to
shared refs, configuration or the worktree list, the common-directory lock,
in that order. The root and common directory come from the root's `.git`
entry, so a lease starts no process. The two worktree commands lend the
lease to their Git process (`CheckoutLease::lend_to`), so it outlives a
crash of the desktop until Git exits; other writes do not lend it, since Git
can pass it on to long-lived helpers. `RepositoryJob` passes the checkout's
verified identity, which the lease checks under the lock. Turn undo takes the
same exclusive lease in the runtime, and `plait-host undo` accepts it only if it
is already exclusive; a run takes the writer class. The hardened Git functions
underneath are unchanged.

`lease/cleanup.rs` stores private, inode-keyed cleanup markers beside the lease
files. A run/undo saves its marker before starting effects, and lease admission
checks it for both runtime and desktop mutations. Unconfirmed cleanup retains
logical exclusion after OS handles close, including across host stores and
restarts. Only a matching token under exclusive recovery ownership can clear a
marker; a damaged save fails closed. `runtime/src/cleanup.ts` collects positive
cleanup receipts independently from turn/tool outcomes. Guardians acknowledge
normal process-group absence over a private descriptor; Claude's pinned SDK
spawn hook exposes actual process exit; Codex retains its terminal route after
an interrupt timeout without killing a shared server. Late proof clears the
marker; unavailable proof requires explicit confirmation. External detached
writers remain outside the normal-group guarantee.

`lease/git_cleanup.rs` records desktop Git children separately from run markers,
including worktree children that hold lent leases. A fixed-size pre-exec write
records owner/group birth identities without another process or an fsync. Git's
normal exit clears it; an orphan or unconfirmed exit is checked at admission and
by cleanup reads. Kernel identity checks gate the explicit group-stop action,
which can run while that group still owns the lent lease. Read-only Git queries
remain outside this recording path.

The additive cleanup read/resolve/stop contract is in [HOST](HOST.md#cleanup-confirmation-and-quarantine).
`ui/cleanup.rs` fetches it on the background executor with checkout identity,
connection and generation guards. Only an unconfirmed checkout gets a header
control, with its explanation in hover text; it refreshes while uncertainty is
visible. Render performs no filesystem, process or socket work.

Branches are read with `for-each-ref` over `refs/heads/`, newest committer
date first, at most 500, when the branch palette opens, never on a status
refresh; a newer request supersedes an older reply. Creating runs
`git switch -q -c <name>` and switching runs `git switch -q --no-guess
--end-of-options <name>`, both with the repository's configuration, so
checkout filters and the post-checkout hook run, within 120 seconds. Before
creating, Plait refuses a leading dash, `@{`, and `@`, then asks
`git check-ref-format --branch` and requires it to print the name unchanged,
then refuses an existing branch. A switch must name an existing
`refs/heads/` entry, so `-`, `@{-1}`, commits, and remote branches are refused.
Both refuse while the checkout's `index.lock` exists, since creating a branch
does not touch the index and Git would not notice. Git's own refusal (changes
that would be overwritten, a branch checked out in another worktree) is the
result. A switch reloads history; a new branch does not, as its commits are
the same.

Reads and writes differ on purpose. A read must not run anything the
repository configures, so it neutralizes filters and never runs hooks. A write
is something the person asked for and must produce what their own `git add` or
`git commit` would, so it runs with the repository's configuration: clean
filters (such as LFS) run while staging, and hooks run while committing. Writes
pass paths on standard input (`--pathspec-from-file`), never open an editor,
and allow 60 seconds for staging and 120 for a commit. A held `index.lock` is
reported as another Git process at work. On an unborn branch, unstaging uses
`git rm --cached` because there is no HEAD to restore from. `state.rs` runs one
write at a time, ahead of reads, then refreshes status and keeps the same file
selected. `show` validates a full object ID and passes it after
`--end-of-options`.

- Git CLI, not libgit2. Use porcelain v1 with NUL-delimited paths.
- Keep paths as `PathBuf`/OS bytes. Lossy text is for display only.
- Use literal pathspecs and an argument array, never shell interpolation.
- Separate index and worktree diffs. No combined “all changes” view that hides
  partially staged files.
- Disable optional index locks, fsmonitor, untracked cache, external diff, and
  textconv. Read configuration **key names only** and disable all configured
  clean/process filter drivers for each command. Git can otherwise execute a
  filter even during a read-only diff. A worktree inventory reads each
  checkout's key names once and reuses them for that pass; removal reads them
  afresh under its leases. No pager, prompt, hook invocation, or
  network command. Ignore dirty submodule contents; show gitlink changes only.
- Ignore inherited repository-location environment variables.
- Reap subprocesses. Bound each subprocess to 10 seconds, status/config keys to
  8 MiB, diff output to 512 KiB, and stderr to 8 KiB. Pipe readers drain
  concurrently. A job includes multiple subprocesses, so its total deadline can
  be longer than 10 seconds.
- Reject oversized status rather than showing an incomplete file list.
- `files` lists tracked files plus untracked files that are not ignored, with
  one `ls-files` call under the same hardening and limits. It reads names, not
  contents. Files deleted from the worktree, sparse-checkout entries, and
  nested repositories are left out. A list over 8 MiB or 200,000 paths is
  clipped and marked, because a picker can still use part of it. Names that are
  not UTF-8 or contain control characters are counted and skipped: they cannot
  be typed into a message unchanged.
- Limit displayed patches to 10,000 lines and 2,000 characters per line.
  A clipped preview is labeled. Unified source line numbers are parsed in the
  background, as is the widest-row index used for horizontal scrolling.
- A diff keeps every patch line but draws rows (`plait-core::patch`), worked
  out with the parse. A file's diff in Changes leaves out the header lines
  that only repeat its path (`diff`, `index`, `---`, `+++`), since the bar
  above names it; a commit's patch replaces them with one heading row per
  file, named from `+++`, `rename to`, `---`, or the `diff` line in that
  order. Lines before the first `diff` line, and header lines about modes,
  renames, similarity, or binary content, are always drawn. Each row holds
  the index of the patch line it stands for, so picking lines, hunk buttons,
  and the patch-line count use the patch, and staging still rebuilds its
  patch from Git's own output.
- Untracked regular files get a bounded text preview. Binary/non-UTF-8 content,
  symlinks, directories, and special files are not followed as text.
- Untracked file opens walk directory descriptors with `openat`, `O_NOFOLLOW`,
  and `O_NONBLOCK`. This prevents a parent symlink swap from escaping the
  checkout or a special file from blocking a preview. The small `libc` dependency
  supplies platform constants and calls; it is already used by GPUI.

A status snapshot and its later diff are not a filesystem transaction. Another
program can change the checkout between reads. Refresh is explicit. No data is
staged, discarded, committed, or repaired in response to a read error.

## macOS installation boundary

The client and host are independent update units. A combined installer carries
both component packages; either package can also be installed alone. The host
unit includes Rust, pinned Node, and locked production runtime dependencies.
It runs as a per-user LaunchAgent, never as a root agent executor. The agent uses
standard service scheduling because it serves desktop requests, not discretionary
background work.

Host releases use content-derived directory names and distinct package receipts.
A shared receipt would let PackageKit remove the old code during an update.
The LaunchAgent label remains stable. The loaded job and sidecar
keep their original paths while a new package stages the next release. An
identical host reinstall is rejected before payload writes. Activation requires
an explicit session restart after work finishes. Client updates do not control
the host process. User state and credentials are not package payloads.

Per-user installs update themselves. `plait_core::update` reads GitHub
Releases, verifies an Ed25519-signed `update.json`, and checks each download's
signed digest. The app bundle carries its host as a data archive; an update
unpacks it as a new immutable release under the user's Library, rewrites the
per-user LaunchAgent, and restarts the host only when the store has no active or
waiting run. Releases other than the active one are then removed.
`plait-host update` applies the same steps on machines without the app.

This changes launch configuration, not the frozen protocol or durable thread
schema. The client accepts protocol versions 1 through 3. Future protocol
and storage changes need a separate compatibility review. Developer ID signing
and notarization are not implemented. See [Mac installation](MACOS.md) for
paths and operational limits.

## UI and performance

Use GPUI's native macOS platform, Metal renderer, system sans-serif font, and
the system's monospaced font for code. That font is SF Mono, which macOS does
not install under a public name, so it is asked for by its Core Text family
name, `.AppleSystemUIFontMonospaced`. A family chosen in Settings → Appearance
replaces it only after the installed families have been read and the family
has been loaded and measured, both in the background. GPUI draws a missing
family in a proportional fallback, so a name that is not installed is not
drawn; and file and diff views size their sideways scrolling from the line
with the most characters, so a family whose characters differ in width is
refused. The installed families are read again each time Settings opens. SVG assets are embedded strings, not runtime file reads. The
app shows no in-window logo; category glyphs use GPUI's tinted SVG path. Brand
files live in `assets/branding`. The development bundle and icon build are
described in [MACOS.md](MACOS.md).

### Desktop preferences

Model defaults and credentials belong to the host. Choices that belong to one
desktop installation live in `plait_core::preferences`, saved as JSON at
`~/Library/Application Support/Plait/desktop.json` on macOS
(`$XDG_CONFIG_HOME/plait/desktop.json` or `~/.config/plait/desktop.json`
elsewhere). It holds what Cmd-1 to Cmd-9 select, the approval default, whether
the Git inspector is hidden, favorite and recently used models, shortcut
overrides by stable action ID, what opens files and terminals (see "Ways out"
below), and the harness new threads start in (Pi unless
chosen; a harness this client does not know reads as Pi). A draft starts in
that harness while the host lists it as ready, and in Pi otherwise. `PLAIT_DESKTOP_PREFERENCES_FILE` selects an
absolute preferences path for an isolated development instance. The model
lists are provider and model identifiers only, bounded in length and count; the
file never holds a credential. The navigation section also stores opened and
hidden checkout paths, up to 1,000 observed thread activity stamps and unread
markers, and the notification preference. Older files load with empty navigation
state; thread content stays in the host and draft journal.

The file is read and written on the background executor. A missing file means
defaults. A malformed or oversized file is reported in Settings and left in
place; defaults apply. Saves replace the file atomically, and Settings says
"Saved" only after the write succeeds. Writes are serialized, with intervening
changes coalesced to the newest snapshot; only the newest save reports a result.
An unsafe or failed load disables writes for that window, preserving the file.
Unknown fields are ignored so a newer client's file still loads, and a file
written before a field existed loads with that field's default.

Cmd-Option-1 to Cmd-Option-9 count matching real host threads in project order on the local
machine. A machine or project selects its first matching thread; an empty row
does nothing. Jumping expands collapsed parents.

### Ways out

Open in Editor, Show in Finder, and Open Terminal Here are the only places the
desktop starts a program of the person's choosing. `ui/ways_out.rs` picks the
file and line on screen (the Files view's current find match or first visible
line; in Changes the picked line or the diff's first change, mapped to the
file's own line numbers) and the open checkout. `plait_core::handoff` turns the
choice saved in Settings → Editor into a program and arguments:

- The system default is `/usr/bin/open <file>`; Finder is `/usr/bin/open -R`;
  Terminal is `/usr/bin/open -b com.apple.Terminal <checkout>`.
- Neovim in Rex and a Rex tab are AppleScript run by `/usr/bin/osascript`,
  addressed to Rex by bundle ID (`com.superlogical.rex`). The editor's script
  runs `nvim +<line> -- <file>` with `run command … at <checkout>`; the
  terminal's opens the checkout with `open`. Each path is escaped twice, as a
  POSIX shell word in single quotes and then as an AppleScript string, by pure
  functions with tests.
- A custom command is one line split into words with shell quoting rules
  (single and double quotes, backslash) and `{file}`, `{line}`, and `{project}`
  filled inside each word. It never goes through a shell, so nothing in it is
  expanded. A command that does not parse, or lacks `{file}` for files, or uses
  anything but `{project}` for terminals, is refused before it is saved.

The program starts on the background executor with no input, its output
discarded, and its error output kept only until it exits. The desktop checks it
every 50 ms for two seconds without blocking a thread; an exit with an error
in that time becomes one sentence in the notice line. After that the process is
left to run, and a thread empties its error output and reaps it when it exits.
Plait does not depend on Rex: without it, `osascript` fails at once, and the
sentence says Rex is not installed. Test builds record the command instead of
starting it.

### Projects and thread activity

With protocol version 3, the desktop subscribes before paging the complete thread
list across checkouts. Live summaries replace their matching rows; old listings
are replaced on reconnect. The host watches its newest 200 threads. Older loaded
threads remain readable and explicitly refreshable, with this limit shown above
the list. A failed watch is reported as paused updates rather than current data.
No subscription is polled while idle.

Projects come from host checkout paths and opened folders. Hidden projects stay
in desktop preferences and can be reopened through the palette. Folder availability checks
run in the background. Successful Git status results normalize a selected
subdirectory or symlink to its checkout root. Switching projects preserves
per-thread composers and pending request IDs, changes the repository epoch, and
cancels only the selected snapshot subscription. It never sends Stop.


### Checkout owners and asynchronous results

The desktop still presents one thread workspace. `CheckoutRegistry` routes Git
jobs by a captured `CheckoutId`, independently of the selected workspace. A
verified `CheckoutKey` contains the local host endpoint, canonical root, device,
and inode. Filesystem verification runs on the background executor before and
after repository work. An alias of a known root rejoins its existing writer;
a replaced directory is an identity error. This is desktop routing, not a
cross-process writer lease or a claim that external programs cannot edit files.

Leaving a checkout drops its snapshots, file lists, diff caches, history pages,
and read demand. Its identity, queued/running mutation, and last write outcome
remain available to the initiating owner. No inactive repository read or
prefetch is scheduled. The selected Git view is separate from that data; status
refresh restores a selected path and layer, rather than a status-list position.
A removed or renamed selected path is cleared. Line selections also require the
same checkout and repository epoch.

| Result | Required owner and freshness checks |
| --- | --- |
| Snapshot, history, projected transcript | Host connection, selected thread, watch lifetime, and source/projection revision |
| Record and recorded-review reads | Thread and checkout, connection/watch lifetime, record ID, repository epoch, request generation and source revision as applicable |
| File contents and file stamp | Thread and checkout lifetime (not the host connection: the read is local), viewer generation, verified root, relative path, device/inode/size/modification time |
| Git status, files, branches, history and diffs | Captured checkout owner and directory identity, repository/history/request epoch; diff and commit-patch presentation also requires its view lifetime |
| Policy and agent settings | Captured checkout and connection, checkout lifetime, form/request generation; a reply for another checkout is rejected |
| Submit/create acknowledgements | Original host, request ID and payload owner; creation also validates checkout, model and harness; selection does not discard the acknowledgement |
| Git write acknowledgements | Original checkout writer; commit field clearing additionally requires the initiating thread and unchanged text revision |
| Undo acknowledgements | Original host/thread and request ID; outcome is stored before consulting selected presentation, and refresh targets the original checkout |

For the cross-checkout host protocol, changing checkout advances a checkout
lifetime without replacing the connection generation or interrupting a pending
host settings save. Returning A→B→A does not revive A's earlier read. File view
contents are cleared when changing checkout. Existing same-checkout selection
resets remain in place; this is not retained workspace or tab behavior.

The protocol does not expose a persistent host UUID, so the desktop currently
uses its local host-store endpoint as `HostKey`. Directory bindings live for the
desktop session; restart identity recovery and durable undo-operation recovery
are not added here. Submission recovery continues using its existing journal.
Tests inject delayed and misowned replies, directory replacement, aliases, and
same-name files in separate checkouts. Headless scenes and frame measurements
do not verify native GPU presentation, accessibility, or real provider runs.

A completion, failure, interruption, stop, or approval change marks an unseen
thread until it is opened. Observations and unread markers persist in desktop
preferences. The initial listing updates markers but does not post notifications.
While another thread is visible or the window is inactive, later attention-worthy
changes request window attention and post a GPUI system notification. Opening a
notification selects its thread. Alerts off suppresses both external signals,
while in-app markers remain. GPUI checks for a bundle identifier before using
macOS UserNotifications and requests authorization on first delivery. Unbundled
builds and denied authorization retain in-app markers and the attention request.
No test posts a real system notification.

### Desktop recovery

`plait-core::session` owns a private, versioned desktop journal. Its directory is
next to `desktop.json` under `session/`; `PLAIT_DESKTOP_STATE_DIR` can select an
absolute isolated directory for development. A lifetime writer lock prevents two
clients from overwriting each other's drafts. Malformed, oversized, future-schema,
or wrong-host state is left intact and blocks submission, not silently reset.

The final journal owner explicitly releases its writer lock, so a descriptor
inherited by a concurrent fork does not keep a released journal locked. Host and
command leases have different lifetimes and are not released this way.

The UI owns selection and composer changes. It queues immutable memory snapshots
and coalesces disk saves after 250 ms. Background writes are serialized and atomic,
with file and directory synchronization. Closing a window waits for its save;
normal app shutdown also flushes the journal. Force quit and power loss cannot
guarantee the last unsaved keystroke. No filesystem work runs in render.

The journal keeps the last checkout/thread, readable titles, draft text/edit
revisions, and unconfirmed submission IDs. It does not read or copy provider
credential stores. Draft text is private local JSON, not application-encrypted.
A submission's exact ID and text must be durable before the socket request starts.
Reopening never submits automatically. Explicit retry uses that same ID, and a
matching accepted-input event can resolve it. The composer revision guard survives
restart, so an acknowledgement cannot erase a subsequently edited draft. The host
protocol and durable conversation schema are unchanged.

### Composer

The composer owns editable text and reports intent as events: Submit (text, edit
revision, and whether to steer), Stop, a
chosen local command, and a request for the checkout's files. The window owns
host requests and Git reads and performs the commands. `Draft` holds every
editing rule without GPUI, so the rules are unit-tested.

The input is one line when empty and grows with the wrapped draft to about a
third of the window height, with a floor of four lines, then scrolls. Its
height comes from a measured layout at the final width. The conversation list
is bottom-aligned, so it stays pinned to its newest row as the input grows.
Wrapped rows are kept between frames. Moving the cursor shapes nothing, and an
edit reshapes only the lines whose text changed, so a large paste stays
responsive. Undo keeps at most 100 steps and 4 MiB of draft text.

Up on the first visual line recalls prompts sent in the thread, newest first.
Down walks forward and finally returns the draft, its selection, and its undo
history unchanged. Recall adds no undo step; editing a recalled prompt makes
the previous draft one undo step. Every recall step advances the edit
revision, so an acknowledgement for an earlier submission cannot clear recalled
text. The saved draft is the one being written, not the prompt on screen.
Sending a recalled prompt unchanged returns to that draft. History comes from
the host snapshot's user messages. The host keeps only the tail of a long
prompt, so prompts sent from this composer are also remembered exactly, for
the life of the window.

Typing `@` at the start of a word lists checkout files; `/` as the first
character of an otherwise empty draft lists local commands. Both use one
suggestion list, `composer::Completion`: plain state and a render function
that never takes focus. The file list comes from `git::files` through the
repository state. Ranking runs on the background executor with
`plait_core::fuzzy`; a result is applied only if it belongs to the newest
query and the list is still open. Accepting a file replaces `@query` with the
checkout-relative path and a space, such as `src/main.rs `. A path with spaces
or other word-ending punctuation is wrapped in backticks. Nothing is attached
and no file is read: the agent receives the text a person would have typed.
Commands are a table in `composer/commands.rs`; each maps to an action the app
already has.

Escape closes the suggestion list, then leaves recall. In a new-thread draft a
further Escape closes the tab only when the draft holds no text, images (loaded
or loading), Project busy choice or unanswered first send. While the agent works,
two Escape presses within 1.5 seconds stop the run; one press only arms the
gesture, and any edit or cursor movement cancels it.

The scene walk draws the composer empty, with several lines, past its height
limit, and with each suggestion list open, at both window sizes. One scene
types a mention, lists the fixture repository through Git, and accepts the
result. The scenes do **not** exercise key events, IME, pointer selection,
wheel scrolling, or the accessibility tree; those have had no check in a
native window. The editing rules behind them are unit-tested.

### Images in messages

Protocol 6 carries image references in snapshots and history, with separate
32 KiB upload and 512 KiB read operations. Originals belong to the thread in the host's
private `host.sqlite`, outside the checkout and its Undo records. See the
[host contract](HOST.md#protocol-version-6) for limits,
capabilities, transport, and retention. Pi and Codex capability information
comes from their catalogs; Claude Code's pinned model catalog cannot tell.
Unknown support permits sending, with any harness refusal shown as a run error.

The desktop saves originals in `session/images/<sha256>` under its private
state directory, mode 0700 with mode 0600 files. On macOS this defaults to
`~/Library/Application Support/Plait/session/images`; `PLAIT_DESKTOP_STATE_DIR`
can move the enclosing session directory. The journal holds only references
in existing-thread drafts, independently identified new-thread drafts, and pending
submissions. Changing tabs saves and sets aside the current new-thread draft;
Discard saves the removal immediately. Editing a queued message reserves the
empty composer until cancellation returns, so text and attachments restore together.
Original files are synced before references enter a draft. Sending flushes the
journal, uploads originals, then submits their IDs. A refusal preserves the
draft; only an acknowledgement for its unchanged revision clears it. Removing
an attachment, or accepting its submission, schedules an immediate background
journal save and collection. Once the saved draft and pending submissions no
longer refer to an original, the desktop deletes it. Accepted queued messages
keep host references only; editing one fetches its original back from the host.
An existing-thread draft can also recover a missing local original from its host.

Opening the journal sweeps unreferenced originals on the background executor,
after validating recovery state and acquiring its exclusive lease. In-memory
pins protect active preparation, uploads, visible drafts, and the latest queued journal
while saves overlap. A separate set retains the last durable journal references,
even after its window closes; collection waits until recovery has loaded it. Collection and persistence
share a background storage lock; UI pin updates use only a short in-memory
lock. A failed save keeps the previous journal's originals. Abandoned image
preparation is collected after its result is discarded. No deletion, directory
scan, read, or decode runs during rendering.

File drops use bounded regular-file reads with no-follow and nonblocking open.
Type, byte, count, and dimension checks precede decoding and storage. The
existing pinned `image` crate decodes PNG/JPEG on the background executor with
an allocation limit, applies orientation, and prepares a 192-pixel thumbnail.
The original bytes are sent unchanged. No decoder process or shell is used.

Clipboard reading is a deliberate exception to bounded reads: GPUI returns
an already allocated image, just as text paste already reads the person's
clipboard whole. It is read only on an explicit paste action, never during
render, focus, or control enablement. Immediately on return, Plait checks the
byte/type/dimension/count limits and drops refused bytes without caching,
logging, or saving them. These immediate checks read headers only; Plait's
SHA-256 hashing, pixel decoding, and thumbnailing run in background work.
GPUI itself computes a non-cryptographic image ID while constructing its
clipboard result; that cost is part of the synchronous read exception. An
immutable prepared original carries its digest through persistence without
hashing again. The pinned native GPUI reader prefers text over images, then
PNG over JPEG and other image representations. A text-plus-image paste therefore keeps
ordinary text behavior. TIFF-only images are refused, without conversion.

Conversation rendering draws prepared `RenderImage` pixels only. Visible rows
schedule missing thumbnails after rendering returns; fetching, disk reads,
and decoding happen on the background executor. At most four reads/decodes
run at once and the least-recently-used cache holds at most 128 results,
including errors. It retains decoded thumbnails, not original bytes: at most
128 × 192 × 192 × 4 = 18 MiB of pixel payload. Hits refresh recency without
fetching or decoding. A larger view has one separately prepared image, up to
1,600 pixels per side (9.77 MiB). These bounds exclude renderer copies, small
metadata, and temporary decode allocations. Decoding records oriented
dimensions for the tooltip and larger view. Results
must match connection epoch, watch generation, thread ID, and cache selection;
a larger-view result also matches its own generation. Changing thread or
connection drops the cache. Draft thumbnail replies match composer generation
and discard pixels and errors for references removed while loading. Restoring
a draft, including a queued message returned by Edit, prepares each attachment
independently. A missing original alone shows Failed with its reason on hover;
valid siblings keep their thumbnails. Removing failed attachments leaves those
valid originals sendable. Uploading marks a
pending image submission and blocks sending that draft twice; pending rows
show the image count even without text. Disabled composers reject paste/drop,
and a refused attachment's notice clears on the next text edit.
The journal and caches contain no image data in snapshot frames.
Displayed image captions use the same format/control-character escaping as
tool rows and approvals, including paged history. Stored prompts and harness
input retain the original text; history offsets count the displayed text.

The release frame benchmark includes 50 distinct image references with a
bounded cache of prepared thumbnails. It measures CPU render/layout/prepaint/
paint, excluding decode, transport, GPU submission, and native input latency.
See [Verification](VERIFICATION.md#images-in-messages) for results and the
explicit clipboard allocation measurements.

### Durable checkout waiting

`runtime/src/checkout-queue.ts` owns durable queue metadata in `checkout_waiters`;
message payloads and image associations use the existing private request store.
`runtime.ts` admits the lowest eligible acceptance order for each checkout,
sharing that order with same-thread follow-ups. Startup holds every not-started
input. Explicit Resume releases the checkout line, while interrupted runs keep
the existing recovery path. Identity, settings, policy, availability and
cancellation are checked again at admission, before the run's ordinary record.
See [the host contract](HOST.md#durable-checkout-queue) for states, limits and
fairness, including steering and held follow-ups.

The core's `host/checkout_queue.rs` validates the additive operations and compact
summaries. A capability probe chooses `watch_threads_queue` and its paged listing;
older hosts keep the old operations and unavailable Wait. The daemon uses the
existing coalesced summary invalidation and change-event path. Queue descriptions
are prepared when summaries decode, tab metadata is rebuilt when the catalog
changes, and visible rows draw cached facts. No per-waiter desktop task, polling,
filesystem read, image decode or transcript projection is added. Host drains wake
on completion and queue actions; one shared contention timer covers external
lease release, which has no runtime completion event.

`ui/checkout_queue.rs` presents Waiting/Held above the message box with Cancel
and Resume. The same summary supplies tab/sidebar state and position on hover.
`ui/draft_tabs.rs` journals explicit Wait placement with the originating draft;
its first message binds to that draft's created thread even after navigation.
`PendingSubmit.checkout_wait` preserves the additive operation across lost
replies and restart, with the original request ID, revision and image references.
It is distinct from the host's checkout line. The desktop's `queued_submissions`
cache keeps each accepted Wait message's exact text and images: when Cancel's
receipt, or the one `checkout_queue` read made after a thread's queue summary
ends, says it was cancelled or failed without starting, they return to the
thread's draft. Acceptance clears
only the sent draft revision, and closing or restoring tabs schedules no run.

### Queue and per-thread controls

A version 3 composer can send during a run: Enter follows up, Option-Enter
steers, and Stop remains a separate control. The core `Submissions` collection
tracks each outstanding request independently. Request ID, full text, queue
mode, approval mode, and draft revision are written before socket I/O. A queued
snapshot item acknowledges a request just as its accepted message does. Sending
flags are transient; reopening and reconnecting never submit on their own.
The legacy journal `pending` field holds the first outstanding item;
`pending_submissions` holds the rest. Additive `queued_submissions` preserves
exact local text for cancelling a queued message back into the draft. Queues
owned by the host remain authoritative; this local cache starts no work.

`ui/thread_controls.rs` presents model, thinking, approval, context usage, and
the input queue. Model changes are refused during a run. Thinking choices come
from the catalog, and request replies are guarded by host epoch, selected
thread, and request sequence. The subscription owns snapshot order; control
replies do not replace a newer event. Saved thread identity is ID and checkout;
a model change preserves draft recovery. Pure usage formatting and thresholds
live in `plait-core::compose`, with no GPUI dependency.

### Native conversation text

`plait-core::markdown` projects CommonMark, with tables, strikethrough, and task
lists, into plain blocks and style ranges on the background executor.
`pulldown-cmark` is pinned to 0.13.0 with default features disabled; its
MIT-licensed source and event API were checked at that version. No HTML renderer
or web view is used. HTML stays text and an image is never fetched: its
description and address are shown as text. Unchanged rows retain their
projection while the live answer changes. Excessive block counts fall back to
the original text rather than dropping content, and an oversized table becomes
tab-separated text.

A link is data, not an action. Only an `http` or `https` destination without
whitespace or control characters becomes one, whether written as a Markdown link
or as a bare URL in prose; every other destination stays inert text beside its
label. The view underlines a link, shows its address at the edge of the
conversation while the pointer rests on it, and opens it in the default browser
only when a click starts and ends on it. Nothing opens on its own.

Fenced code and tool content are tokenized by `plait-core::highlight`, a small
original lexer with no dependency. It assigns lexical classes (comment, keyword,
string, number, type, function, key, punctuation) for Rust,
TypeScript/JavaScript, Python, shell, JSON, TOML, YAML, Markdown, diff, C/C++,
and Go; any other label stays plain. It has no grammar and cannot fail, so
partial code from a streaming answer is colored as far as it goes. A parser
with per-language grammars was not adopted: it would add a C build and several
megabytes for a view that only needs code to be scannable. The classes map onto
the existing palette; see [DESIGN.md](../DESIGN.md).

`plait-core::tool_view` prepares what each tool call shows from its bounded
arguments and result: `edit` as a line diff of old against new text
(`plait-core::diff`), `write` as the new file highlighted by its extension,
`command` as a prompt line and its output, and `read` as the first lines of the
file. Arguments that the host clipped or that are not the expected JSON are
shown as received, with a note. The same pass plans groups: a finished run of
five or more consecutive calls folds under one line with its count and kinds,
and the run a busy thread is still adding to keeps its latest three calls in
view. Version 3 stored tool fields replace bounded live previews; opened read results
show all loaded lines. Version 1 and 2 hosts keep the bounded-preview behavior.
Git refreshes after an observed busy-to-complete transition.

`plait-core::history` keeps a reading window of at most 600 stored items and
16 MiB of field text. The projected snapshot, Markdown blocks, and syntax spans
are additional bounded representations of that window. One page hydrates up to
2 MiB beyond its inline text. An oversized field is read in parts of roughly
262,144 UTF-16 units, with explicit Next part and Start over controls; copying
copies the displayed part. Every continuation must advance on the declared
UTF-16 boundary and finish at the declared length.

The host snapshot stays the authoritative live state for submissions and status.
A separate display snapshot combines stored items with its live tail, preserving
full stored fields and the newest tool status. Reading older pages evicts the
newer end when needed and labels the detached window; Latest reopens the live
end. Forward paging is not available. Ordinary forward scrolling works within
the loaded window. Evicted older pages can be loaded again, including when an
eviction cuts through a page. A missing history seam resets to the newest page
instead of implying that two separated ranges are contiguous.

History socket reads and Markdown/tool projection run on the background executor.
There is one history read and one projection in flight per view. New snapshots
coalesce while projection runs; host epoch, watch epoch, thread ID, and projection
revision reject stale delivery. Pages arriving after newer live state are merged
with that state. Prepending uses the existing stable-ID list splices, so the
current item and pixel offset survive. The scene harness verifies that invariant.

Reasoning has its own stable row identity and parsed blocks. Completed reasoning
starts collapsed; live reasoning has a compact, scrollable body. Disclosure and
selection survive ordinary snapshot updates. Replacing an oversized text part
clears selection because its offsets now refer to another part. Run-state copy
uses only host facts: no invented stop actor or per-turn duration. A finished
turn's record counts observed tool steps and names successfully edited or
written paths (`plait-core::history::TurnSummary`). A turn with no tool calls
has a record with nothing to count. A turn whose request is outside the loaded
rows has none, because the steps before the first loaded row cannot be counted.
A response still streaming can only show the host's bounded live tail; its full
stored text loads after the host saves it.

Conversation text can be selected with the pointer: drag, double-click for a
word, triple-click for a block or a line of code, and shift-click to extend,
within a message or across messages and opened tool details. Every piece of
text paints through one element that records its laid-out position for the
frame; pointer events map those records to a row, block, and byte offset using
GPUI's text layout, and `plait-core::selection` decides order, ranges, and the
copied text. The selection is drawn as a background on the text runs and is
kept by row identity, so it survives streaming and list changes. Cmd-C copies
it once a press in the conversation has moved focus there; the composer keeps
its own Cmd-C while it has focus. Copy controls for a message's source, a code
block, and a tool's details appear when the pointer is over their row or when
they take keyboard focus. Dragging past the top or bottom edge scrolls the list
one step for each pointer movement; it does not keep scrolling while the
pointer rests. Rows between the two ends are copied whether or not they are on
screen; a tool's details are copied only while they are open.

Find in the conversation (`conversation_find.rs`, with the matching in
`plait-core::conversation_find`) searches the parsed blocks the view draws,
not the Markdown source, so each match is a byte range of one block. The
search runs on the background executor over the display snapshot and its
projection, both shared and replaced rather than changed, so starting one
copies nothing on the UI thread. One search runs at a time. A new query and
new rows (a reply, a history page, another thread) bump a generation; a result
is shown only if its generation and its projection are still current, and
otherwise the search runs again. Results come grouped by message ID, so drawing
a row looks up only its own matches and restyles them with the same run
splitting as the selection. Moving to a match scrolls the list to its row, then
places the match in view on the next frame from where its text was drawn,
since only drawn text knows where a match sits inside a tall row. A new result
for the same query keeps the current match by its message, block, and range and
leaves the scroll position alone. Requests and replies are searched; the
streaming reply, reasoning, tool rows, and turn records are not, and pages that
are not loaded are never read for a search.

Conversation events use GPUI's variable-height `list`. The conversation is
drawn as a log of turns: a request heads its turn at a ring on the turn line.
Requests use a neutral panel and a visible You label; replies use a Plait
label and stay open beside the line, as do tool calls. The request projection
in `plait_core::markdown::Message::request` (`markdown/request.rs`) separates
quoted context on the background executor without interpreting the rest of the
request as Markdown. The source
is retained for Copy message; selection and find use the projected blocks.
Stored messages and the host protocol do not change. Each row draws its own
piece of that line across its full height, padding included, so the pieces
meet; a row needs only its own kind and position, so no pass over the
transcript runs per frame. An earlier turn's record is part of its last
answer's row. The latest
turn's record is drawn with the run state below the list, where the line ends.
When a new request arrives, the answer before it takes its record into its own
row; that row is measured again, since its text has not changed. While the
list is scrolled back from its newest row, or older history is being read, the
rows on screen stop short of the latest turn, so the line is broken above the
state. Reading older history, no record is drawn there either.
Tool details start collapsed. Disclosure state belongs to the UI and clears on thread selection.
A call folded into a closed group keeps its list row and draws nothing, so row
identity and the scroll anchor do not depend on grouping; rows whose folding
changes are remeasured. A control to jump to the newest row appears while the
reader is elsewhere in the history, and says when content has arrived since.
Live transcript changes are planned in `plait-core::transcript`, without GPUI.
Ordered stable IDs retain existing rows, including after history-tail clipping.
Structural changes use `splice`; changed content uses `remeasure_items` so the
scroll anchor and measured unchanged rows survive streaming. Duplicate IDs or
a changed thread identity reset the list rather than reuse the wrong row.
Live tool disclosure and element IDs follow tool/message IDs, not row positions.
Expanding a live tool remeasures only that row. Changed files and diff
lines use `uniform_list`; only visible rows are built. Cached data uses `Arc`
rather than cloning large strings on every render. Long diff lines scroll
horizontally.

One 44-point header carries the thread title, its state, and the Conversation /
Changes switch; there is no separate tab strip. The header is also the window's
title bar (`ui/window_bar.rs`). The window opens with a transparent, untitled
title bar, so macOS draws only the traffic lights, placed by
`TitlebarOptions::traffic_light_position` to center on the 44-point row 14
points in. The window keeps its title, Plait, for Mission Control and the
Window menu. The rail starts with an empty 44-point strip under the lights;
with the rail hidden, the header's left padding clears them, except in full
screen. The window is opened with `app_owns_titlebar_drag`, so AppKit neither
moves it from the title bar area nor delays clicks there. Instead the strip
and the header move it themselves: a left press that no control took (a
control prevents default when it takes focus) arms a move, the first drag
calls `Window::start_window_move`, and any release disarms it. A second press
calls `Window::titlebar_double_click`, which follows the system's
double-click setting. The strip stops short of the rail's resize handle. Navigation, changed-file, and
model rows are 28 to 36 points and one line. The composer is one raised box; the
Project, Machine, branch, and saved model sit on a context line beneath it.
`ui/theme.rs` owns the small original controls and the current palette, which
views read with `colors()` while drawing; `ui/appearance.rs` picks it from the
preferences and the macOS appearance and redraws when either changes. The
palette is per thread, so parallel UI tests with different themes stay apart. A control's tone
(ghost, selected, filled, primary) is chosen when it is built, because GPUI
accepts one hover style per element and reports a second one only at draw time.

The sidebar defaults to 272 points on wide windows and 240 on narrow windows.
Below 1150 points, the conversation's Git inspector (280 points by default) is
removed; Git remains available in the Changes view. The minimum window is
900 × 600. Cmd-B hides the sidebar.

Both panels resize by dragging their inner edge (`ui/panels.rs`): the sidebar
between 200 and 480 points, the inspector between 220 and 560, and neither so
wide that the conversation has less than 420. A drag changes only the width on
screen and saves it to desktop preferences on release; double-clicking the
edge restores the default.

Cmd-L hides the inspector on wide windows and the choice is saved; on a
narrow window the same control says that Git is in Changes.

Controls have focus rings, tab stops, accessible names, and semantic roles.
Thread state has both a color and a text label.
No decorative animation is used.

### Actions and shortcuts

`ui/actions.rs` lists every application action once: a stable string ID, a
title, the menu it belongs to, and a default keystroke. `actions::bind` is the
only place application shortcuts are bound. It takes a map of action ID to
keystroke; an entry replaces the default, an empty entry unbinds, and an entry
that does not parse leaves the default in place. `actions::install` clears and
rebinds everything and rebuilds the menus, so a keymap can be applied again at
run time. Settings → Keyboard records shortcuts through GPUI's pre-dispatch
keystroke interceptor, detects conflicts, and rejects text-entry and common
system keys. Overrides load from desktop preferences; unknown IDs survive.
The menu bar, the command
palette's entries and shortcut hints, and the table in [MACOS.md](MACOS.md) come
from the same table, and a test compares the document with it. Editing keys that
belong to one control (the composer, the text field, the palette list) are bound
by that control.

### Command palette and model picker

`ui/palette.rs` is one view: a single-line search field over a list, drawn as a
centered overlay that blocks the workspace behind it. It knows nothing about
what its items do. A caller passes items (title, detail, section, state, and a
command) and receives the chosen command. The command palette lists threads,
the action table, and "New thread with" each signed-in model; the model picker
lists the host's catalog under Favorites, Recent, and each provider. Adding a
source is a function that returns items and a `Command` variant.

Ranking is a pure function from the search text and plain item data to rows. It
runs on the background executor, guarded by a query counter so only the newest
result is shown. Enter ranks synchronously when a result is still pending, so
the choice always matches the text in the field. Rows use `uniform_list`; only
visible rows are built, which matters for a catalog of well over a thousand
models. Focus stays in the search field and the selected row is reported to
assistive technology as the active descendant. Escape, or a click outside,
closes the overlay and returns focus to where it was.

Models that are not signed in are listed after every usable one, marked in
words, and cannot be chosen. The default model is marked wherever models are
listed. Choosing a default sends the host's `set_default_model` request at
once; the picker footer and Settings report Saving, the host's confirmation, or
the failure. Nothing is shown as saved before the host answers.

### Settings modals

Application and Project settings are window-owned overlays. Opening one leaves
`ThreadWorkspace` mounted at its existing geometry and preserves its conversation
projection, scroll handles, selection, find and draft. The same thread watch
continues applying replies. A full-window dimming layer blocks pointer input;
the root action shield suppresses workspace commands, including menu dispatch.
Tab traversal wraps within the modal and dismissal restores the previously
focused handle. Opening Settings closes tab overflow and cancels tab dragging;
the two tab rows retain their layout beneath the scrim. Their keyboard and
assistive activation/focus actions remain inert until the modal closes. Escape gives nested pickers, shortcut recording, provider
forms and the trust confirmation precedence. GPUI runs an accessibility
action on its element without a hit test, so each frame records what covers
the window (`theme::Covered`): buttons built outside the modal or a picker
over it ignore an assistive Click, the composer and text fields outside them
ignore SetValue, and focus moved beneath the modal returns to it. Covered conversation scrolling cannot demand an older page.

Project settings stores its own target and, for another checkout, separate
policy and agent-resource state. It never selects that checkout on opening.
Requests capture checkout and host epoch; agent reads also carry a revision.
Replies route to that target even after closing, without updating another
project's controls. A pending request prevents retargeting that modal. Choosing
an MCP configuration file explicitly closes settings and opens its checkout's
Files view. Filesystem canonicalization and host requests remain in background
jobs; section rendering only draws prepared state.

### Provider settings and command policy

`ui/settings_providers.rs` renders the host's catalog, subscription flow state,
API-key form, and custom endpoint editor. plait-core's `host/providers.rs` defines typed
requests and tolerant replies; its `host/sign_in.rs` projects each flow into its
available controls. Status polling runs only while its form is open and pending.
Host epochs and form generations reject stale replies. Closing an in-flight
start cancels the returned flow instead of reopening the form.

Secret fields omit accessibility values and clipboard copy, keep no undo history,
and move their value into a redacted request on send. Request buffers and owned
secret strings are overwritten on disposal. They never enter desktop preferences
or the recovery journal; this is not a claim to erase every allocator or OS copy.
The host remains the credential owner. No sign-in URL opens without a click.

`ui/settings_policy.rs` reads the canonical checkout's policy on connection or
checkout change. A response must match that checkout, its lifetime, and its host epoch. Until
it arrives the UI says the policy is unknown, never promises a sandbox. A trusted
change needs explicit confirmation; the host's response determines the shown value.

### New threads

New Thread (`Cmd-N`) opens an unsent draft in the active checkout (`ui/new_thread.rs`)
and focuses its composer. Each invocation opens a fresh draft, even if another
empty draft is in front. Draft identity owns text, image references, configuration
and its pending admission in the desktop recovery journal (`core/session.rs`).
Parking drops the composer and retains only plain draft state; the mounted draft
is the only one presented. `ui/draft_tabs.rs` routes acknowledgements by draft
and creation IDs independently of the selected checkout. Nothing is sent to the host until the first message
is submitted; the desktop then sends `create_thread` and submits that message to
the created thread. An unconfirmed creation keeps its request ID: the workspace
shows a retry that reuses that ID, and that draft sends no new creation request while it is
pending. Other drafts can create independently. A model is recorded as recently used only after the host confirms the
thread. `New Thread in Worktree` (`ui/worktree.rs`) sends the additive
`create_worktree_thread` operation with a captured base and parent identity.
The host reserves one UUID and durably binds the selected harness/model before
creating files. `runtime/src/worktrees.ts` owns operation receipts, startup
reconciliation and thread admission; `plait-host worktree` writes private synced
journals and invokes the core's lease-protected Git worktree implementation.
A nested supervisor reclaims checkout hooks and filters before returning to the
outer worker that holds the leases. No cache or dirty-file copying occurs.

The window polls only its explicitly pending operation off the UI thread.
Completion checks the originating tab, selection and connection; a background
thread appears in the sidebar without taking focus. Reconnection lists durable
operations to recover grouping after a lost reply or closed window. Legacy
navigation entries are imported grouping records, with no deletion authority.
Neither tab-row nor sidebar rendering performs filesystem, Git or host work.
Removal is explicit and host-authorized by the saved inode, common-directory
identity and journal, with fresh checks under exclusive checkout leases.
See [the worktree contract](HOST.md#host-owned-worktrees) for refusals and recovery.

The app records root-view construction time, peak construction time,
selection-handler-to-render time, and startup-to-first-render time. View →
Toggle Diagnostics (Cmd-Shift-D) shows them in a footer; it is hidden by
default. Git panels show background read durations. These are diagnostics,
**not** measurements of GPU frame time, input-to-display latency, or complete
launch time. Instrumentation does not write a log from the UI thread.

The [ADEMark plan](ADEMARK.md) tracks external performance goals and the local
transport probe. These probes do not measure native input-to-pixel latency.

### Scene snapshots without a window

`ui/snapshot.rs` is a test module for macOS and Linux. It opens the real root
view in GPUI's test platform, which has no native window, walks every view at
1380 × 860 and 900 × 600, and draws each one. `cargo test -p plait` runs that
walk without a renderer, so layout and paint errors fail the test.
`bash scripts/snapshots.sh <directory>` runs the same walk with a headless
renderer and writes one PNG per scene, at twice the logical size.

| | Renderer | Text |
|-|-|-|
| macOS | GPUI's Metal renderer, drawing into an offscreen texture | Core Text, with the system font and the system's monospaced font |
| Linux | GPUI's wgpu renderer on a software Vulkan driver | cosmic-text, with the fonts installed there |

On macOS the scene is drawn by the same renderer and shaped by the same text
system and fonts as the app, so glyphs, wrapping, and truncation come from the
code the app runs. On Linux they do not; those images check layout, density,
and color only.

Neither is the native window. The images do **not** show or check:

- the traffic lights, which macOS draws over the rail's top strip; scenes
  show that strip empty;
- presentation through the window's layer, display color management, or a
  display that is not 2×;
- hover, pressed, and focus states that follow native pointer and key events,
  the text caret, IME, or scrolling by input. Most scenes set view state
  directly. The reading scenes deliver pointer and key events through GPUI's
  own dispatch to check text selection, copy, and link hover; that exercises
  hit-testing and handlers, not the operating system's event path;
- the native accessibility tree: GPUI's `TestWindow` does not activate
  `Window::a11y`, so headless tests check node ID uniqueness and steady-frame
  presence through Plait's debug-test prepaint collector rather than AccessKit
  or macOS `NSAccessibility`;
- animation, frame timing, or latency. Each image is one frame.

Host data in these scenes is injected sample state held in small functions in
the test module. The repository is a disposable fixture. Building the root view
probes the host socket and reads desktop preferences, so each scene test
re-runs itself in a child process whose home directory and host store are empty
scratch directories. It cannot reach a running host or a user's saved state.
The images do show the machine's name and a temporary path, so they are
inspected locally, not committed.

Two tests in the same module send key events to that window instead of setting
state: Cmd-K, typed letters, the arrows, Return, Cmd-Return, Tab, and Escape
reach the palette and model picker through GPUI's key dispatch and text input
path, and New Thread keeps the request ID of an unconfirmed creation. They
check routing, focus return, and ranking. They do **not** check a native key
event, an input method, the caret, or pointer input.

The harness needs GPUI's `test-support` feature. It is a dev-dependency feature
for macOS and Linux test builds; `cargo build` and release builds do not
compile it.

### Accessibility identities

GPUI assigns an accessibility node to every `Text` element that carries an
element ID, hashing its `GlobalElementId` path to an `accesskit::NodeId`. Two
elements with the same ID under the same id-bearing ancestors panic in debug
builds when an accessibility client is attached and silently discard the second
node in release builds.

To prevent text collisions by construction, Plait's accessible text helpers
(`a11y_text`, `tag`, and `section_label` in `ui/a11y.rs` and `ui/theme.rs`)
require an explicit place-based `ElementId` (scoped to the setting, row, or
region) instead of deriving IDs from rendered text. Sidebar thread rows and
Project settings agent rows name each row by what it is (the thread's own row
view; the skill's list, title and path), not its position, so their IDs hold
as rows come and go; some other lists (Changes file rows, MCP servers, palette
rows) still use the position. Raw `text!()`, `Text::new` and `.role(` chains
that bypass the collector are rejected by source checks in
`ui/snapshot/a11y.rs`. Under `cfg(all(test, debug_assertions))`, `Audited<E>`
wraps audited controls and text elements transparently to record each node's
`GlobalElementId` hash, role, label, and value during `Element::prepaint`. In
non-test and release builds `Audited<E>` compiles to a zero-cost type alias.

## Dependency decision

GPUI and `gpui_platform` are pinned together at:

`596dfdcf32d85d4d8d17a8887b4d0979ec7c5e14`

The toolchain is pinned to Rust 1.98.1. `Cargo.lock` pins the full
resolved graph. Only macOS features are enabled; `font-kit` is required for text.

Use the APIs at the pinned revision, not examples from a different release.
The app entry point uses `gpui_platform::application()`.

Upstream references:
- [GPUI site](https://gpui.rs/)
- [README](https://github.com/zed-industries/zed/blob/596dfdcf32d85d4d8d17a8887b4d0979ec7c5e14/crates/gpui/README.md)
- [Contexts](https://github.com/zed-industries/zed/blob/596dfdcf32d85d4d8d17a8887b4d0979ec7c5e14/crates/gpui/docs/contexts.md)
- [Key dispatch](https://github.com/zed-industries/zed/blob/596dfdcf32d85d4d8d17a8887b4d0979ec7c5e14/crates/gpui/docs/key_dispatch.md)
- [Examples](https://github.com/zed-industries/zed/tree/596dfdcf32d85d4d8d17a8887b4d0979ec7c5e14/crates/gpui/examples):
  hello_world, list_example, uniform_list, set_menus, a11y, tab_stop,
  focus_visible, and svg.

Plait uses the Apache-2.0 GPUI libraries, not the GPL-licensed Zed application UI
crate. Its controls use GPUI's focus and action APIs. Dependency notices are
included in release packages; see the [originality policy](ORIGINALITY.md).

### Self-hosting tool output

`tool-output.ts` stores bounded command logs in the host SQLite database. Tools
return thread-scoped IDs rather than host paths; `read_output` pages the stored
bytes without another command execution. Process capture is separate from the
bounded live preview. Ranged source reads use a descriptor-checked Rust helper;
edit admission inspects a complete fingerprint without transferring the whole
file across the helper pipe. Context summaries remain in Pi's durable store but
are omitted from the presentation projection, which retains original messages.

### Immutable record diffs

`runtime/src/turn-review.ts` handles the additive `turn_review_info`,
`turn_diff`, and `turn_review_page` operations before versioned dispatch.
A small `review_checkouts` table names saved directory identities; record
mutation triggers advance its catalog revision in the mutation transaction.
The host ID is persisted in settings. Existing snapshots are unchanged.

`git/review.rs` reads the selected path's owned blobs in one batch and compares
those blobs under canonical Git flags. It never opens the recorder's index,
walks complete record trees, or reads current file bytes. `plait-host review`
uses the existing lifetime supervisor and a separate read sandbox. The runtime
pins against pruning, then caches immutable bounded page documents. The desktop
uses only the socket API; `host/review.rs` validates complete documents and
prepares the existing virtualized patch model on the background executor.


The desktop's `review/recorded.rs` holds plain record selection and reply
identity, independent of GPUI. `ui/turn_review.rs` coordinates socket reads
and installs only a complete file list and prepared patch. Original run ID,
displayed record ID, and undo target are separate; watch updates never retarget
an open recorded view. Current Git status does not determine its file list.
File rows, scope descriptions and patches are prepared in background work;
render builds only visible rows of the existing uniform lists.

Reads are coalesced to one in flight. Completion checks thread, checkout,
connection epoch, watch epoch, repository epoch, selection generation, record,
path and comparison. Changing any member drops the whole reply. Entering
Changes, selecting a file/run, regaining focus, a same-checkout run-state
change, local Git write completion, and Refresh request fresh metadata; there
is no review timer. An invalid-request probe identifies an older host, while
other failures remain unavailable or offline. The desktop never opens the
host store. The retained-record picker is limited to the current thread's v5
list; current Git reads and writes continue through the existing repository
queue. Recorded diffs expose no write controls.

Recorded review prepares visible paths, run descriptions and patch text with
the shared display-text escaping used for MCP data. Format and control
characters remain visible without changing the path or record identity sent
to the host. This preparation runs off the UI thread.


### Busy-project admission and finite resources

`runtime/src/admission.ts` owns additive placement negotiation, identity-bound
project authorization, durable creation receipts and linked-thread provenance.
It composes the host-owned worktree operations; no Git process moves into the
window. `run-capacity.ts` has no default execution ceiling, offers an explicit
host limit and serializes before-record preparation per filesystem device.
Cancellation removes waiting preparation and cleanup releases the existing
writer lease. The immutable run still has the chosen harness and policy.

`host/admission.rs` validates additive capabilities and receipts. The desktop's
`ui/admission.rs` captures request ownership, handles the draft choice and guards
late results by connection/request identity; `settings_isolation.rs` presents the
main project's stored authorization in its modal. Unsupported hosts retain the
old in-place flow without promising a queue. The selected thread alone observes
preparation and linked provenance; tab/sidebar drawing consumes cached summaries
and never calls the filesystem, Git or host. Headless scenes exercise twenty
running summaries and the busy/resource choices; the frame suite includes a
burst of twenty summaries with five runs and one transcript watch.
