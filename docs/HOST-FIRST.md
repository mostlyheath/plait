# Host-first Plait

The host owns shared work and workspace state; the desktop presents what it
streams and keeps personal preferences and unsent drafts. Steps 1–4 below are
built. [Host](HOST.md) defines the implemented contract.

## Principles

1. **One way in.** Host operations serve local and remote clients; an older
   local host can fall back to desktop Git.
2. **Snapshot, then numbered events.** A client that falls behind resubscribes
   for a fresh snapshot rather than guessing.
3. **Separate structure and content.** Small revision events signal changes;
   transcripts, diffs, and files are read in bounded pages when needed.
4. **Name the host.** Snapshots and events carry its stable identity.
5. **Writes are intents with an ID.** Retrying an operation returns its first
   outcome rather than repeating it.
6. **Present carefully.** Cache by revision, render locally, retain settled
   views during reconnection, and never invent host state.

## Who owns what

| State | Owner |
| --- | --- |
| Projects, hidden projects, worktree ownership, project settings | Host, shared |
| Threads, turns, records, undo and redo | Host, shared |
| Prompt tree | Host, planned |
| Git, files, worktree sizes | Host; status changes pushed |
| Unread and observed activity | Host, shared |
| Tabs, layout, selection, bookmarks | Host, per device |
| Unsent drafts and offline send queue | Device |
| Theme, keymap, fonts, editor and terminal choice, panel widths | Device |
| Machine connection list | Device |

A device ID scopes its workspace on each host. Reading a thread on one device
marks it read on the others.

## Workspace and Git

`workspace_subscribe {device}` sends a snapshot with host identity, sequence,
projects, worktree ownership, thread summaries, unread state, watched Git
summaries, and device state. Numbered revision events follow. A slow subscriber
receives `resync` and starts again; older hosts retain their existing listing
and watch behavior. This Mac's existing lists import into its host once.

Every machine's Git and file reads go through its host, including this Mac;
an older local host falls back to desktop Git. The host watches open checkouts
and pushes debounced status changes. The sidebar names and switches machines;
the window title names a remote machine. Writes carry operation IDs and replay
their first outcome on retry.

The `scripts/speed-check.sh` check measured 8–15 ms added on Linux and passed
its 20 ms gate. This is not an installed Mac measurement. See
[performance goals](ROADMAP.md#performance-goals).

## The prompt tree

**Planned exploration:** mockups precede agreement on the model and view. The
working model is:

- A node is a prompt and its answering turn, with a parent it continues.
- Editing, rewinding, forking into a worktree, retrying, or undoing branches
  instead of overwriting history.
- Each node links its before and after records, worktree, and any commits.
- Undo and redo move the thread's position and restore eligible changed files;
  the client shows and compares branches.

Open questions: whether retries and edits are siblings or distinct branch
kinds, how the tree spans worktrees, and how to handle files changed since a
branch was recorded.

## Order of work

1. **Built:** host identity and workspace stream, shared project and unread
   state, one-time import, and machine identity.
2. **Built:** host Git and file reads, checkout watching, and speed check;
   retain compatibility fallback for an older local host.
3. **Built:** per-device tabs, layout, and selection on the host.
4. **Built:** operation IDs for writes and replay of their first outcome.
5. **Planned:** prompt-tree exploration, then an agreed model and view.
6. **Planned:** several independent machines in one sidebar.
7. **Planned:** runners that survive host restart, separate from the request
   server.

Each step preserves compatibility and needs its own verification. See
[Verification](VERIFICATION.md) for evidence and limits.

## Runs that outlive the host (agreed, in progress)

The agreed design separates each accepted run into a runner that can outlive
its request server. On restart the server will rediscover a runner as live when
its lock is held, finished when its lock is free and a terminal receipt exists,
or dead when neither is present. Interrupted effects are never replayed.

The supervision and transport foundation is in progress; current runs still
stop on host restart. Runner execution and rediscovery are not yet connected.
