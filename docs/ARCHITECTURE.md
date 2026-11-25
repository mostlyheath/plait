# Architecture

Plait is a native macOS client for agents running on a local or remote host.
The desktop is Rust and GPUI; the host is Rust with a TypeScript runtime
sidecar. [HOST.md](HOST.md) defines the host protocol, persistence, safety
rules, and operation details. [REMOTE.md](REMOTE.md) covers remote setup.

The [system walkthrough](WALKTHROUGH.md) follows a message across these boundaries.

## Crate boundaries

The Rust code is split into crates with a checked dependency direction:

- `plait-protocol`: wire data and serialization with no platform I/O,
  including host requests, snapshots and provider data, update manifests and
  their signature check, and remote frame encoding. Both host and clients use it.
- `plait-transport`: shared frame and owner-checked local socket I/O for hosts and clients.
- `plait-release`: release fetching, artifact verification, and host installation for hosts and clients.
- `plait-engine`: Git, leases, records, build output, and worktree size
  measurement. Only the host links it.
- `plait-client`: transports, the host client, the snapshot mirror and event
  stream, and GPUI-free view models, reusable by other clients such as a
  future iOS app.
- `plait`: GPUI rendering on top of `plait-client`.

`plait-client` has two default features: `local-host` (finding and connecting
to a host on the same machine over its socket) and `ssh` (starting `ssh` to
reach, install and probe remote hosts). Mac-only work, such as opening editors
and terminals and installing app updates, lives in `plait`. Without the
features, `plait-client` still builds for iOS with machine records, state and
host protocol models; a client without them has no transport yet, since an
authenticated network transport is not built.

Neither the desktop nor `plait-client` links `plait-engine`; everything a
client needs from a machine goes through a host. `scripts/check.sh` enforces
the direction: `plait-host` never depends on `plait-client`, and the desktop's
scene tests reach a host through the `plait-client` `test-host` feature, which
only the desktop's dev-dependencies enable.

## Boundaries

- `crates/plait` owns native presentation, input, focus, and dispatch of
  background work. Views show host and repository state; they do not invent
  runs or sample data.
- `crates/plait-client` owns GPUI-independent models, state transitions, host
  clients and recovery; Git and file operations reach it from a host. It checks that asynchronous
  results still belong to the selected checkout and request. Its split is
  described in [Crate boundaries](#crate-boundaries).
- `crates/plait-host` owns the host process and transport boundary, peer
  checks, and filesystem and process helpers. It can outlive a desktop window.
- `runtime/` owns durable agent execution and adapters for Pi, Codex, and
  Claude Code, including approvals, queues, records, and command policy.

State owns transitions; UI owns presentation. Filesystem and Git work runs on
background executors, never during render. The host owns accepted work, so
closing a client or a subscription does not cancel a run.

## Host-first data flow

The host exposes a workspace snapshot and subscription with host identity,
sequence numbers, and events for shared workspace state. Slow subscribers
must resynchronize from a snapshot. The workspace operations and their
contract are described in [HOST.md](HOST.md#workspace). Desktop adoption is
in progress; existing thread listing and watches remain in use.

The host supports bounded Git and file reads and explicit Git writes for a
checkout. Remote desktop views use these operations, with checkout identity
checks and paged results. Reads disable optional locks, external diff,
textconv, filters, fsmonitor, and untracked cache; writes use repository
configuration and hooks. Network Git operations use the host's credentials.
The permitted writes are listed in [HOST.md](HOST.md) and
[ROADMAP.md](ROADMAP.md).

Local and remote checkouts go through the same host operations: the desktop's
only backend is the host (`crates/plait-client/src/state/backend.rs`), so this
Mac's Git and file reads and writes also go through its local host. The host's
checkout watcher (`runtime/src/runtime.ts`) publishes status changes, so
visible views refresh and hidden views become stale. See
[HOST-FIRST.md](HOST-FIRST.md) for the migration and its limits.

## Connections

A local client connects over a local socket. Remote connections use an SSH
bridge or an opt-in Tailscale listener. A window selects one machine at a time;
remote paths refer to that machine. The host remains the authority for its
own work and credentials. See [REMOTE.md](REMOTE.md) for transport and Linux
limits.

## Threads, runs, and worktrees

The host owns threads, accepted runs, history, approvals, and the checkout
queue. A checkout admits one writer at a time. A thread can wait for a busy
checkout or use an optional per-thread worktree. Worktree creation starts
from a committed checkout; uncommitted work is not copied. Removal checks for
work that would be lost. The host also manages approved build-output seeds.
Details and recovery rules are in [HOST.md](HOST.md).

The host records checkout state before and after a turn. Turn review reads
those records, and undo restores eligible files changed by the turn while
protecting later changes. Records and undo do not promise to reverse every
external effect of a command. See [HOST.md](HOST.md) for the exact scope and
conflict rules.

## Approvals and command policy

Approval mode governs edits and commands; approval is tied to the proposed
call. Command policy is separate. On macOS the host can run commands in a
sandbox or under the trusted policy. Linux supports trusted commands and
refuses the sandboxed policy until a containment backend is available. These
rules, including harness-specific behavior and limits, are defined in
[HOST.md](HOST.md) and [LINUX-SANDBOX.md](LINUX-SANDBOX.md).

## Desktop state

The desktop keeps personal preferences, drafts, connection choices, and
presentation state. It currently also persists tabs, selections, and other
workspace state as the host-first migration proceeds. **Planned:** shared
workspace state lives on the host, and tabs, layout, and selection move to
host storage scoped by device. Unsent drafts and personal preferences remain
on the device. See [HOST-FIRST.md](HOST-FIRST.md).

Editor and terminal handoffs use configured commands with placeholders; they
are unavailable for remote checkouts. The file viewer is read-only.

## UI and performance

Large lists and content use bounded or virtualized presentation. Work is
cached and refreshed by identity and revision where applicable; stale
asynchronous replies must not replace newer state. Render must not perform
filesystem or Git operations. Headless scenes inject sample state solely for
tests and capture wide and narrow layouts without a live provider. They do
not substitute for native window inspection. See [VERIFICATION.md](VERIFICATION.md).

## Dependencies

Dependencies are declared and pinned in manifests and lockfiles. Check the
upstream source at the pinned revision before using an API. Keep dependency
licenses and notices; original application code and design remain Plait's
own. GPUI stays out of the headless core and host crates.
