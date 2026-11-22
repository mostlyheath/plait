# Host-first Plait

**Agreed direction, not built.** The maintainer chose this direction after
remote hosts exposed a split in ownership. The host owns all of Plait's
functionality and state. The desktop is one client of it: a fast, careful
reader of what the host streams, with only personal preferences and unsent
drafts of its own. [HOST.md](HOST.md) remains the contract for what is
implemented; this document is the plan and its agreed decisions.

## Why

The host already owns threads, runs, approvals, the queue, turn records and
turn undo, worktree creation and removal, checkout leases, providers,
harnesses, MCP, the Linear cache, and turn review. The desktop still owns
the rest:

| Desktop-owned today | Effect |
| --- | --- |
| Opened and hidden projects, worktree ownership | A remote window lists this Mac's projects |
| Unread and observed activity | Alerts are per window, not per person |
| Tabs, bookmarks, drafts, the selected checkout and thread | One journal for all machines; a remote path reopens as a local one |
| Git reads and writes for this Mac's checkouts | Two code paths; only the desktop knows when status changed |
| The file viewer, Worktrees graph and sizes | The same |
| The machine label | Read from a preference, not from the connection |

A window that talks to another machine needs all of this to come from that
machine. Fixing each item on the desktop would keep two sources of truth.
Long-running work also needs one owner: worktrees, Git, undo and redo, and
the planned prompt tree all span many turns and outlive any window.

Comparable tools that run agents or terminals on a server keep their state
there and treat every app, command line, and script as an equal client. The
user outcomes Plait adopts from that comparison are listed under
[Principles](#principles). No code or design is taken from them.

## Principles

1. **One way in.** Everything the app can do is a host operation that any
   client may call. This Mac's host is not special: the desktop reaches it
   over the local socket and runs no Git and reads no checkout files itself.
2. **Snapshot, then numbered events.** A client asks for a workspace view
   and gets one consistent snapshot with a sequence number, then every later
   event in order. A client that falls behind is told to start again from a
   fresh snapshot, never to guess.
3. **Structure and content travel separately.** Events are small: what
   changed and its new revision. Large content (transcripts, diffs, patches,
   file bytes) is read in bounded pages when a view needs it, as Git reads
   already are.
4. **Every value names its host.** Each snapshot and event carries the
   host's stable ID and name, so the client always knows, and can show,
   where something lives.
5. **Writes are intents with an ID.** The client sends what the person
   asked for with an operation ID. The host records it, so a retried request
   returns the first outcome instead of acting twice, and the reply says
   whether it was accepted, applied, or refused.
6. **The client is smart about presentation.** It caches by revision,
   renders and highlights locally, keeps the last settled view on screen
   while reconnecting, and never invents state the host did not send.

## Who owns what

| State | Owner |
| --- | --- |
| Projects, hidden projects, worktree ownership, project settings | Host, shared |
| Threads, turns, the prompt tree, records, undo and redo | Host, shared |
| Git status, diffs, history, branches, files, worktree sizes | Host, read on request; status changes pushed |
| Unread and observed activity | Host, shared by the person's clients |
| Tabs, layout, selection, bookmarks | Host, per device |
| Unsent drafts and the offline send queue | Device; a draft is sent to the host when submitted |
| Theme, keymap, fonts, editor and terminal choice, panel widths | Device |
| The list of machines a device connects to | Device |

**Per device** means stored on the host under a device ID the client creates
once and keeps. Two Macs on one host each keep their own tabs; reopening a
device restores its own. Unread state is shared, so reading a thread on one
device marks it read on the others.

## The workspace stream

- `workspace_subscribe {device}` replies with a snapshot: schema, host
  identity (ID, name, platform), `seq`, and the current projects, worktree
  ownership, thread summaries, unread state, Git status summaries for
  watched checkouts, and this device's tabs and selection, each with a
  revision.
- Events follow on the same connection, each with the next `seq`, the host
  ID, an entity, and its new revision; small entities carry their new value.
- The host keeps a bounded queue per subscriber. A subscriber that exceeds
  it receives `resync` and is closed; the client subscribes again and
  replaces its state with the new snapshot.
- The existing thread watches and listings stay for older clients. A host
  without `workspace_subscribe` answers `invalid_request`, and a newer
  client uses today's behavior with that host until the transition ends.

## Git and files through the host

- Every Git and file read for every machine uses the host's `git_read`
  family, and every write uses `git_write`. The desktop's local Git backend
  is removed once the speed check passes.
- The host watches the checkouts of open projects and worktrees. A change,
  debounced, becomes a `git_status_changed` event with a new revision; a
  visible Changes view rereads, and a hidden one marks itself stale. No
  client polls.
- **Speed check:** on this Mac, through the local socket, status and diff
  reads of a large checkout stay within the existing frame and latency
  budgets ([performance goals](ROADMAP.md#performance-goals)), measured
  with the ADEMark harness against the direct path. The local backend is
  removed only when this holds; until then it remains the fallback.

## Intents

- Every write carries `op_id`, a client-made unique ID. The host records
  each operation and its outcome; a repeat with the same ID returns that
  outcome. Records are kept for a bounded time.
- Replies distinguish `accepted` (recorded, will run), `applied` (done, with
  its result), and `refused` (with the reason). Long operations report
  progress as events on the workspace stream.

## The prompt tree

The person's work is a tree, not a line. The shape is open; it is explored
with mockups before it is built. The working model:

- A node is one prompt and the turn that answered it. Its parent is the
  node it continued from.
- Editing an earlier prompt, rewinding, forking into a worktree, retrying,
  or undoing makes a branch instead of overwriting history.
- Each node links to its checkout before and after (the turn records), its
  worktree, and any commits it made.
- Undo and redo move the thread's position in the tree; the host restores
  the files the turn changed, as turn undo does today.
- The client draws the tree and lets the person compare and return to
  branches.

Open questions: whether retries and edits are siblings or a separate kind
of branch; how a tree spans worktrees; and what the person sees when a
branch's files have changed since.

## Several machines

Each host stays independent. A device keeps its list of machines and holds
one workspace stream per connected machine. Because every value names its
host, one sidebar can group projects under their machine, with that
machine's connection state.

## Order of work

Each step ships on its own and keeps older clients and hosts working.

1. **Host identity and the workspace stream**, with projects, hidden
   projects, worktree ownership, and unread state moved to the host. This
   Mac's existing lists are imported into its own host once. The desktop
   shows the machine it is on from the host's identity. *Built
   ([Workspace](HOST.md#workspace)).*
2. **Git and files through the host**, with the checkout watcher and the
   speed check, then removal of the desktop's local Git path.
3. **Per-device tabs, layout, and selection on the host**, with drafts and
   an offline send queue on the device.
4. **Intents with operation IDs** for every write.
5. **Prompt-tree exploration**, then its data model and view.
6. **Several machines in one sidebar.**
7. **Runs that survive a host restart:** a supervisor and agent runners
   separate from the request server, so updating the host never stops work.

## Verification limits

Nothing here is built. Each step lands with its own tests and documentation
in [HOST.md](HOST.md), [ARCHITECTURE.md](ARCHITECTURE.md), and
[MACOS.md](MACOS.md), and states what was and was not checked live.
