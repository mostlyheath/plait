# Product roadmap

This page sets direction and records scope decisions. The [README](../README.md)
describes the current app; the [Mac guide](MACOS.md) explains its controls.
Implementation contracts belong in [HOST.md](HOST.md) and
[Architecture](ARCHITECTURE.md), not in a second specification here.

## Now / Next / Later

This is the planning order, not a claim that every item is in progress.
**Now** follows the existing agreement. **Next** contains follow-on choices
whose relative priority still needs a decision; design proposals are marked.
**Later** separates conditional work from proposals that need scope agreement.

### Now — installation, then alerts, Antigravity, and Linear

Deliver each as a separate change. Installation comes first, with independent
client/server lifecycles. These priorities precede the remaining M9 review
features; their agreed scope remains.

- **Drag-and-drop installation.** Put `Plait.app` and an Applications shortcut
  in the DMG. On first launch, set up a missing local server without a required
  installer script. Preserve existing servers and active work. Separate first
  setup from server replacement before wiring it into startup. See
  [Client/server platform and installation](#clientserver-platform-and-installation).
- **Thread alerts — built.** In-app toasts for other threads while Plait is
  active; native macOS notifications while it runs in the background or
  Settings covers the window. Quit stops new alerts, not host work. Reopening
  shows unread state without replaying old alerts. See
  [alerts in the Mac guide](MACOS.md#projects-and-threads); native delivery
  remains a manual check.
- **Antigravity (M7 step 3).** Add a first-class harness through the official
  `agy acp` interface, using the person's installed CLI and sign-in. Prefer
  the official ACP SDK over an unofficial bridge. Follow the [agreed contract](ACP.md)
  for capabilities, permissions, sessions, and the prerequisite refactor;
  show only the models Antigravity offers to Plait.
- **Linear view.** A persistent project-level issue view with search, ticket
  details, references in existing threads, and starting a thread from an issue.
  Keep a source-issue link and Open in Linear. Reuse the host foundation and
  verify real read-only API access. This view does not depend on automation.

### Next — independent releases and the review loop

- **Independent client/server releases.** Finish separate update selection,
  publication, and compatibility checks. Installing or updating a client must
  not force a server replacement. Keep the bootstrap work compatible with
  this direction from the first change.
- **Commit by run (M9 step 2).** Saved before/after diffs are built. Matching
  current changes to a run and staging independently safe changes remain.
  Agree authorship wording and overlap handling before implementation.
- **Since last reviewed (M9 step 3).** Remember the reviewed record for each
  file and show later changes. Agree what counts as reviewed and how that
  state is stored. A later edit must make the file appear changed again.
- **MCP browser authentication (M8 step 4).** Add OAuth token reuse and refresh
  for configured servers. Header-key and anonymous connections already work.
- **Workspace decisions.** Decide whether side-by-side conversation, files,
  and diffs should come before more integrations. This is a design proposal,
  not an agreed implementation.

### Later — dependencies and explicit gates

- **More harnesses:** other ACP adapters follow Antigravity, with separate
  capability and compatibility checks.
- **Concurrency and storage:** Pi read-in-place, further worktree cleanup,
  and the larger draft recovery format have separate safety and compatibility
  gates. Tabs, the checkout queue, manual worktree removal, and approved
  build-output reuse are already built.
- **Named CLIs (M8 step 3):** wait for a sandboxed project that needs one.
  Trusted projects already run the person's tools.
- **Other clients and remote servers (M3/M6):** support other apps through a
  versioned public protocol, not GPUI or Rust internals. Authenticated remote
  transport and recovery come first for network access; Linux execution also
  needs tested containment. No additional client or public listener is built yet.
- **Developer ID signing and notarization:** deferred. Script-free installation
  does not remove unsigned-download warnings. Do not remove quarantine or
  bypass Gatekeeper to simulate a signed release.
- **Previews, editing, automation, and other ideas:** see
  [Proposed, not yet agreed](#proposed-not-yet-agreed). These are not an active
  implementation queue.

Checks belong to each change, not to a general verification phase. Name the
behavior being checked and the evidence that is missing: for example, draft
recovery after restart or Undo preserving later edits. Use
[Verification](VERIFICATION.md) and the relevant pull request to distinguish
source tests, native-window checks, and live-service checks. Do not treat an
old unchecked item as proof that a current feature is broken or untested.

## Milestones

Milestone IDs remain stable for links and contracts; their numbers are not the
build order. The priority list above controls planning.

| Milestone | Built | Remaining |
| --- | --- | --- |
| M1 — Native foundation | GPUI desktop, navigation, conversation, Git reads | Ongoing maintenance |
| M2 / M2.5 — Local daily use | Durable local agents, history, approvals, provider settings, thread management, search, tabs, and optional worktrees | Drag-and-drop installation and first-launch local-server setup; independent update flows |
| M3 — Remote execution | Local host boundary | Authenticated connections, remote project discovery, network recovery; Linux containment |
| M4 — Development tools | Git staging, commits, history, push, fetch, fast-forward pull, branches, file viewer, and editor/terminal handoffs | Editor, review feedback, process and log views |
| M5 — Previews | — | HTML, Markdown, and MDX with refresh and selected-content feedback |
| M6 — Multiple machines | — | Unified navigation across independent hosts, including offline states |
| M7 — Agent harnesses | Pi, Codex, and Claude Code with their own permission modes | ACP client, then other agreed adapters |
| M8 — Skills and tools | Skills, instructions, prompt templates, and MCP tool calls without browser sign-in | MCP OAuth; named CLIs only when needed |
| M9 — Turn records | Per-run records, Undo/Redo, and immutable recorded diffs | Staging by run and review tracking; later proposals below |

Each change must compile, run, and have its own checks. Protocol, storage,
shared models, and permission changes need agreement before implementation.
Existing stores must keep loading and older protocol replies must stay compatible.

Releases must install without development tools, preserve user data, and support
normal in-app updates. Package migration needs one administrator confirmation;
active work must finish before migration or host replacement. See
[Releasing](RELEASING.md) for delivery and review requirements.

## Client/server platform and installation

**Agreed direction, not all implemented.** Plait is a client/server platform:

- **Plait Server** owns execution, durable threads, approvals, checkout safety,
  process cleanup, and recovery. The existing host and TypeScript runtime are
  its local implementation. Closing or updating a client does not stop it.
- **Plait Client** is the native Mac application. Other apps should eventually
  use the same server through a documented, versioned public protocol.
- **`plait-core`** is an internal Rust library compiled into programs. It is not
  an independently installed server or the public integration boundary.

This direction does not require a crate or executable rename, a generic SDK,
remote transport, or another client in the installation change. The native app
remains Rust + GPUI. The current owner-authenticated local socket must not be
exposed as an unauthenticated network service. See [HOST.md](HOST.md) for the
implemented protocol, not a claim of a complete external-app API.

### First implementation: remove the mandatory installer script

1. Separate **first setup**, **client replacement**, and **server replacement**
   in the install layer. They have different permissions and safety conditions;
   app startup must never reuse a function that stops an existing server.
2. Put the prebuilt app and an Applications shortcut in the DMG. Normal new
   installations use download → drag into Applications → open. Build tools and
   Terminal commands are not prerequisites.
3. Treat the bundled server as a bootstrap copy only. Install a missing server
   into the existing per-user, versioned location outside the app bundle, then
   register its service. Never modify the signed app bundle at first launch.
4. Preserve existing managed or manually started servers, including those with
   running work or pending approvals. Do not downgrade a server to match the
   app's bundle. Source builds and explicitly selected development stores must
   not provision a default server as a side effect.
5. Run filesystem and service work off the UI thread. Serialize setup attempts,
   validate the bundle and release identity, and reject unsafe paths. Surface
   progress, failures, and Retry in the native app. Handle launch from the DMG
   explicitly rather than registering a service that depends on a mounted disk.
6. Keep legacy system-wide migration explicit. Do not silently remove root-owned
   files, ask for administrator access, or stop an unmanaged server on startup.
   Preserve threads, drafts, credentials, and the independent server lifetime.

Use the existing Rust install layer as the shared foundation, not a shell
installer invoked from the UI. A client-only replacement must not install,
activate, or prune server releases. A server replacement must preserve the
client and use a separate, explicit safe activation path.

Verify new installation, repeated launch, concurrent setup, an existing older
or newer server, active work, a manual host, development overrides, legacy
installs, malformed bundles, setup failure/retry, and reopening without replay.
A test that mocks service startup is not installed-app evidence. Validate a
real downloaded DMG separately, with quarantine intact. Signing and notarization
remain deferred; normal macOS security and background-service prompts can remain.

### Independent release contract

The repository already produces separate component artifacts, but the current
in-app update flow replaces both client and host. That is built behavior, not
the target contract.

Client and server versions must be independently selectable and publishable.
Compatibility comes from negotiated protocol versions and capabilities, not
equal application versions or a shared release tag. Preserve older replies and
stores, and test supported old/new pairs. An incompatible pair must give a clear
update requirement, not silently replace either component or replay a request.

Server updates wait for a safe activation point, with new admissions excluded
before shutdown. They must not interrupt accepted work merely because a client
was installed or updated. Keep immutable running releases until cleanup is
proved. The publication format, supported compatibility range, and any new
protocol fields need a separate concrete contract before implementation.

## Product structure

**Project → Machine → Thread.** A project can have a checkout on more than one
machine. Each machine owns its repositories, runtime, processes, and durable
state. A thread is a persistent conversation in one of those checkouts.

The client is an interface, not the owner of execution. Hosts must remain
independent, with no required central coordinator. Remote work needs authenticated
private-network transport; the planned Tailscale integration does not exist yet.
Do not expose the current local socket protocol over a public TCP listener.

Multiple threads do not imply isolated files. Worktrees stay optional, and
writers must coordinate around existing dirty work. Each thread is one agent
the person directs; Plait does not orchestrate agents or run swarms. There is
no embedded terminal or built-in cloud compute service.
Existing checkouts can be opened as projects.

**Built around the person's checkout.** An agent works in the checkout the
person already uses, beside their uncommitted work, their build output, and
their tools. Worktrees stay optional: safety must not depend on working in a
copy. What makes the checkout safe is the host, not the harness. The host keeps
its own record of the checkout around every turn (M9), so it can show what a
turn changed, and take it back, without relying on what the agent reports.

Plait also leaves the person's agent as they set it up. In Codex and Claude
Code threads it adds no instructions and no tools, and writes nothing into the
harness's configuration. It calls no model on its own to write titles, branch
names, or commit messages, and sends no telemetry. Running many agents at once
on copies of one repository is a different product, and Plait does not aim at
it: a feature that needs a fleet, a copy of the checkout, or a model call the
person did not ask for is out of scope.

### Git and file boundaries

The allowed Git writes are staging and unstaging by file or hunk, committing,
push and publish, explicit fetch, fast-forward pull, confirmed undoable discard,
creating and switching local branches, restoring files through run Undo/Redo,
and adding or removing optional worktrees. No force push, amend, merge, rebase,
cherry-pick, or branch deletion is added.

Reads remain hardened: no optional locks, external diff, textconv, or filters.
Explicit writes use the person's Git configuration, hooks, filters, and
credentials. One writer owns a checkout at a time; linked worktrees also
coordinate writes to shared Git metadata. Work returns by commit, push, and
pull request, not a new integration command.

The file viewer is read-only. Reads are bounded, stay inside the checkout
without following symlinks, and run off the UI thread. Editing and interactive
shells use the person's tools through Open in Editor and Open Terminal Here.
The reading, walking, and find work has provenance in Sidelit (same maintainer,
MIT); Plait's views remain native GPUI. See the [Mac guide](MACOS.md) and
[Git boundary](ARCHITECTURE.md#git-boundary) for shipped behavior.

## Harnesses and tools

**Direction for M7, any agent harness:** a thread may be run by another agent
harness instead of Pi: Claude Code, Codex, omp, Antigravity, and others with a
machine-readable interface. Plait's value is the workspace around the agent
(projects across machines, Git review, durable threads, approvals in one
place), so the agent itself should be the person's choice.

- **Adapters live in the host.** The client keeps one thread model: messages,
  reasoning, tool calls, approvals, status, usage. Each adapter runs one
  harness through its documented programmatic interface, at a pinned version
  checked against upstream before use, and translates its events into that
  model. Screen-scraping a terminal UI is out.
- **Each harness keeps its own identity.** Its authentication, configuration,
  and session files stay its own; Plait never reads its credentials. A thread
  records which harness and version ran it, and continues in that harness.
- **Say what protects the checkout.** Plait's command sandbox and approval
  policy govern Pi threads. Another harness runs under its own permission and
  sandbox model, which the thread names plainly; where the harness offers a
  permission callback, its requests appear as Plait approvals. Plait never
  implies a protection it does not enforce.
- **Durable on Plait's side.** The host stores the translated events, so
  history, search, and changed files work the same for every harness, and a
  restarted host shows the thread as it was without replaying work.
- **Faster than running it elsewhere.** Measure against the harness run
  directly: added latency to first token and per event, memory per idle
  thread, and frame cost while streaming. Targets: under 5 ms added per event,
  no full-snapshot resend per event (adapters send increments), idle threads
  holding no process, and the same 120 Hz reveal for every harness.

Before adding an adapter, agree its protocol additions, capabilities, and
install/version checks. Compare other agent workspaces by outcomes only,
per the [originality policy](ORIGINALITY.md).

**Built for M7 steps 1 and 2:** Pi, Codex, and Claude Code threads share the
conversation and Git workspace. Codex uses the person's installed app-server;
Claude Code uses the person's install through a pinned SDK downloaded only
with their setup action. Their credentials and configuration remain their own.
Each harness exposes its own permission modes, not an implied Plait sandbox.
Pi commands are trusted by default; a project can opt into the command sandbox.
See the [host contract](HOST.md) for supported versions, setup, and modes.

**Current work:** an ACP client, Antigravity first. The
[agreed ACP contract](ACP.md) covers capabilities, permissions, session
recovery, version candidates, and the prerequisite routing and transcript-store
refactor. Implementation starts with that refactor; ACP support is not built yet.
The official
[Antigravity ACP guide](https://antigravity.google/docs/ide/extensions/acp/)
and [ACP TypeScript SDK](https://github.com/agentclientprotocol/typescript-sdk)
are the upstream sources; live compatibility still needs verification.

**Built for M8 steps 1, 2, and part of 4:** Pi threads discover the person's
skills, user instructions, and prompt templates. Settings shows the resources;
`/` completion inserts skills and templates. Configured MCP tools are found by
search, with one tool row and one approval decision per call. Discovery is
bounded, cached, and off the request path. Only names and descriptions enter
context until needed.

Project skills and templates require trust. Project-defined or replaced MCP
servers require explicit enablement bound to the reviewed configuration;
a change disables them again. Trusted projects follow the thread's approval
mode. Sandboxed projects require server enablement and approval for every call.
Approval gates tool calls, not server startup behavior or network access.
Secrets never enter the desktop or model context; interrupted calls are never
replayed. MCP sampling and elicitation are refused.

**Remaining for step 4:** browser OAuth, reuse and refresh of Pi's saved tokens,
and provider-backed `auth`. A displayed saved sign-in is not a working OAuth
connection. The built transports cover stdio and anonymous/header-key HTTP;
real-server acceptance is separate from scripted transport tests.

**Step 3 waits for a need:** allow a person to name a CLI that a sandboxed
project may call with network access or its own credentials. Every call must
show the exact command for approval, and the checkout stays read-only to it.
Trusted projects already have the person's login-shell environment and tools.

Other harnesses load their own skills and MCP servers; Plait does not inject
Pi's resources into them. Pi TypeScript extensions come after MCP, if at all:
they execute inside the runtime and need a separate replay and permission design.
See [MCP tools](ARCHITECTURE.md#pi-mcp-tools) and [HOST.md](HOST.md) for the
implemented discovery, configuration, and approval contracts.

## Turn records and review

**Built for M9 step 1:** the host records the checkout before and after each
run, for every harness and permission mode. One submitted message, its steering
messages, and the harness turns inside it share a pair; queued follow-ups get
new pairs. Recording does not depend on the harness reporting its edits.

The private Git store saves working-tree bytes, executable bits, and symlink
targets without writing the checkout or its Git metadata. Coverage is bounded;
excluded paths are listed. Records are retained for 30 days, with active pairs
protected from pruning. A failed before record prevents execution; a failed
after record prevents Undo. A recovered after record says “after restart”
because it may include changes made while the host was down.

Undo restores only paths that still match the saved after record. Later edits
are not overwritten; conflicts offer Undo the rest or Cancel. Undo can itself
be undone. It does not restore Git history, outside-checkout effects, other
permissions, extended attributes, or hard-link relationships. The checkout
lease excludes host writers, not every external-editor race.

The exact coverage limits, failure states, retention rules, and compatibility
contract are in [HOST.md](HOST.md#protocol-version-5). Implementation boundaries
are in [Architecture](ARCHITECTURE.md#checkout-recording-and-undo); evidence is
in [Verification](VERIFICATION.md#checkout-recording). A warm unchanged
50,000-file record targets under 300 ms on Apple silicon; this is a target,
not an installed-app measurement.

### Remaining daily-use work

M9 steps 2 and 3 remain agreed, after the current installation, alerts,
Antigravity, and Linear work. The high-level goals are agreed; unresolved review semantics
and new shared contracts still need agreement.

**M9 step 2, commit by turn — the first part shipped in v0.1.0-preview.17.** A
finished run's closing line now opens that exact record's saved before/after
change. Recorded Changes includes every retained path, even committed or clean
files, and remains bound after later runs and undo. Recorded/Current and
Original/Restored distinguish the comparisons. Additive host operations supply
immutable diffs without changing protocol versions 1–5.

Origins and staging by turn are not yet built. Their planned work is to show
which current changes match a run and stage independently safe changes while
preserving other work. Authorship wording and review semantics remain
unsettled; the recorded view displays no origin or review control. A recorded
interval by itself does not establish who wrote its bytes.

**M9 step 3, since you last looked — not built.** Plait will remember the record
the person last reviewed for each file, so its diff can show only later changes.
A file that a later turn changes again must be marked as changed, not hidden.

Ways out, branch controls, images in messages, and conversation search are
already built. Their controls and limits belong in the [Mac guide](MACOS.md),
not in the remaining-work list. MCP authentication and conditional named-CLI
work are described under [Harnesses and tools](#harnesses-and-tools).

## Thread tabs

**Built:** independent thread and draft tabs, persisted order and bookmarks,
keyboard navigation, overflow, and separate per-thread view state. Closing a
tab never stops a run or removes a worktree. Only the active workspace retains
loaded content and a transcript watch; cold tabs retain bounded bookmarks.
Settings opens over the workspace without changing the selected tab.
See [tabs in the Mac guide](MACOS.md#tabs) and the
[headless tab model](ARCHITECTURE.md#headless-tab-model).

### Several threads in one project

**Built:** one writer per checkout, cleanup confirmation, host-owned worktrees,
a busy-project placement choice, a durable checkout queue, approved
copy-on-write build output, and the Worktrees graph. The stage numbers below
remain references for the remaining concurrency work, not a second priority list.

| Stage | Status and boundary |
| --- | --- |
| 6a / 6b — Exclusion and cleanup | Runs and Git writes take ordered checkout and shared-Git locks. Unconfirmed cleanup blocks new writes across restart; detached trusted processes remain a stated limit. |
| 7 / 8 — Worktrees and admission | Durable host-owned creation and removal; busy projects offer an own worktree or Wait. Automatic worktree placement requires per-project authorization. |
| 9 — Checkout queue | Explicit acceptance into a bounded durable line; restart holds pending inputs for Resume. Interrupted effects are never re-queued. |
| 10 — Read in place | Planned, only as a host-enforced Pi mode. All shipped runs remain writers. |
| 11 — Build output | Implemented: clone only approved ignored directories on macOS, using immutable idle seeds when the source is busy. No byte-copy fallback; Skip or interrupted preparation can recover cold. |
| 12 — Storage | Graph, storage visibility, and manual safe checkout removal are built. Further storage management, cache deletion, and opt-in cleanup rules remain. |

Worktrees start from an explicit captured HEAD commit, never from a copy of
uncommitted files. Host-owned branches use `plait/<UUID-prefix>`, extended on
collision, in `<main-repository>.worktrees/`. Build output is copied only from
the project's approved list. Removal preserves branches and history, verifies
identity and leases, and refuses uncommitted or unknown ignored files. Unpushed
or unknown publication state requires an explicit preservation exception.
Imported desktop associations never grant removal authority.

There is no default active-checkout ceiling; a host can set
`PLAIT_MAX_ACTIVE_CHECKOUTS`. Preparation is serialized per filesystem and
resource exhaustion fails explicitly. This is not a promise of unlimited
machine or provider capacity.

**Remaining recovery work:** the agreed v2 index/blob layout needs its own
compatibility review and a retained v1 backup. The shipped v1 journal is limited
to 4 MiB serialized and 256 KiB per draft. Its additive `draft_tabs` field and
tab preferences each decode at most 10,000 entries; preferences also have a
16 MiB limit. Legacy journal snapshots retain their 512-record bound. Twenty
maximal drafts and a separate 4 MiB resident submission cache are not supported
capacity promises. At a limit, required saves refuse submission or closing and
keep the last good journal instead of discarding work.

New host capabilities remain additive, with explicit support checks and old
reply shapes preserved. The remaining proposed flags include
`execution_class_v1`, per-harness `shared_reader`, and `worktree_storage_v1`.
Read-in-place and automatic cleanup need their own complete safety checks before
enablement; neither is implied by existing lifecycle locks or manual removal.
See [HOST.md](HOST.md), [Worktrees architecture](ARCHITECTURE.md#worktrees-graph),
and the [Mac guide](MACOS.md#worktrees) for implemented contracts and controls.

## Agreed next: Linear

Native Linear work has three steps. The first is built in the host, not exposed
in the desktop. Nothing has been verified against Linear's live API.

1. **Host foundation — built.** Private API-key storage, project/team binding,
   bounded issue reads and offline caching, local thread-to-issue links, and
   durable receipts for two explicit person-initiated writes: post a comment
   and change workflow state. No automatic writes or background polling; no
   agent or MCP server can call these native host operations.
2. **Desktop view — agreed, not built.** API-key Settings and a persistent
   project-level issue view: search, read details, insert references and chosen
   context into existing threads, or fill a new draft for review before sending.
   Keep the source-issue label and Open in Linear. No automatic run or issue
   write in this first UI step. Thread tabs, its prerequisite, are built.
3. **Issue-triggered runs — planned, rules not agreed.** Separate from the
   issue view. Decide eligible issues, unattended permissions, stopping
   conditions, and where results wait for review. This also requires an
   explicit change to the rule against agent orchestration; that change is
   not agreed.

Native features use the host's GraphQL client. Agents may separately use
Linear's MCP server. Browser OAuth comes later; it needs callback registration,
PKCE, and token-refresh decisions. See [Linear integration](LINEAR.md) for
write retry rules, identity handling, live checks, and remaining UI decisions.

## Proposed, not yet agreed

The later steps of M9 need agreement before implementation. They build on
[turn records](#turn-records-and-review), but any extension to restoration or
Git writes needs its own explicit scope and safety contract.

**Step 4, checks the host runs.** After a turn, the host can run a check the
person named for the project, such as a build or a test run, and show its
result on the turn beside whatever the agent said. It runs in the command
sandbox, so it can read the checkout and cannot write it or reach the network.
It is off until the person turns it on for a project, runs at low priority and
never in a frame, and is cancelled when a new turn starts. Nothing is sent back
to the agent unless the person sends it.

**Later, from the same records:**

- **Line to turn.** From a line in the file viewer or a diff, to the turn that
  wrote it and the message that asked for it.
- **A fuller account of each turn.** The branches and `HEAD` it moved, with a
  way to put them back; each command's exit status; processes it left running;
  and files that changed outside the checkout while it ran.
- **A shelf.** An undone turn can be put back later, and a message can be tried
  again with another model before the person keeps one result.
- **Turn bisect.** Which turn broke a check, found by running it on recorded
  trees in the sandbox while the checkout stays as it is.
- **Run the proof.** A command that shows, on the person's own Mac, the sandbox
  refusing what it says it refuses and an undo restoring identical files.

**Ideas, not yet shaped.** Wanted, but none is designed or agreed, and each
names what has to be settled first.

- **Threads, files, and Git side by side.** One window that shows a
  conversation, a file, and the working tree's changes together, instead of
  the tabs that replace the conversation today. To settle: what it shows at
  900 points, and whether it replaces the Git inspector or grows from it.
- **Hosts that Plait runs for you.** A host in the cloud, reached like any
  other machine, for people who do not want to run their own. It could be how
  Plait pays for itself. It needs remote hosts first (M3), and a position on
  what a hosted checkout may hold and who can read it.
- **Automations.** A thread that starts by itself: first on a schedule, later
  also when something happens in Git or when an issue arrives. An automation
  is created with everything its runs need (the project, the harness and
  model, the message, and its permissions), because nobody is there to approve
  a step. The host would start it, since the host already outlives the window.
  Today nothing starts by itself, and the
  [host contract](HOST.md#internal-authorization-boundary) requires a
  permission design before that changes. To settle: where a run's results wait
  for the person and how they are told; what a run does when a step needs more
  than the permissions it was created with; what happens to a run whose time
  came while the Mac was asleep or the host was down; and how a run that
  starts in a checkout the person is working in shares it with them.
- **A browser.** See a page the work produced, or one the agent needs, without
  leaving the window. Previews (M5) are the planned first step. To settle:
  whether the agent can drive it, which changes what a thread can reach.
- **Rendered Markdown.** Read a Markdown file as a document, not as source.
  Part of previews (M5).
- **Closer work with Rex.** Opening a file or a terminal in Rex is built. What
  more would help is not known yet. Candidates: open a file in a Neovim that
  is already running instead of a new tab; send a thread's command to a named
  pane; a link from a Rex pane back to its thread.
- **Usage.** A page that says how much the agents have used: at first only
  the number of tokens over the last so many days, for all threads together.
  Later, perhaps by harness, model, project, or thread. It is counted on the
  host from what each harness reports, kept on the person's machine, and sent
  nowhere. To settle: what Pi, Codex, and Claude Code each report and whether
  the counts can be compared; where the host keeps a daily total, which is a
  change to what it stores and reports; and whether to show a cost, which
  needs prices that change.
- **An editor.** Edit a file in Plait, not only read it. The planned file
  editor (M4) is not designed. To settle: how much editing is wanted beyond
  the hand-off to the person's own editor; and whether it is native, built on
  the file viewer, or a web editor component, which would bring a web view
  into an app whose interface has none.

## Performance goals

Targets from the product brief. Headless frame-cost measurements, which
exclude GPU and the installed app, are in
[Verification](VERIFICATION.md#measuring-frame-cost); installed-app results
are not yet measured:

- 60 FPS or better for normal local interaction.
- Cached thread switching and Git views below 50 ms.
- Warm launch below 250 ms.
- Incremental remote events without blocking input.

Keep filesystem, Git, and network work off the UI thread. Use cached state,
background work, and virtualized views rather than unnecessary polling or full
view rebuilds. The [ADEMark plan](ADEMARK.md) records the measurement boundary;
Plait does not have an ADEMark score.

Results are published only from the installed app, with the conditions they
were measured under; goals and headless figures are not presented as results.
ADEMark measures Mac agent apps with Claude Code at full access, which a
release with Claude Code threads makes comparable, so entering it is planned.
It also records what an app adds to the agent's request, the model calls it
makes on its own, what it writes into the agent's configuration, and what
keeps running after quit. Plait means to keep the first three at nothing. The
host is meant to keep running, so its memory after quit is reported, not
hidden.
