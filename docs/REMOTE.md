# Remote hosts

**Agreed design for M3. Built: transport framing, the bridge, SSH and tailnet transports, one remote machine per window, remote Git and files, and the Linux user service.** The maintainer chose the direction:
reach a Plait host on another machine over SSH or over a tailnet, with this
Linux workstation as the first remote host, and remote threads before remote
Git. [HOST.md](HOST.md) remains the contract for what is implemented.

## Goals and boundaries

- A Mac client works with threads on a host it reaches through the person's
  own SSH keys or their tailnet. The host still owns execution, durable
  threads, approvals, checkout safety, and recovery. Closing or losing the
  client never stops accepted work.
- The local socket protocol is never exposed on an unauthenticated or public
  listener. SSH runs the bridge as the person on the remote machine. The
  tailnet listener is opt-in, binds only Tailscale addresses, and authorizes
  each connection by Tailscale identity before reading a protocol byte.
- No central coordinator, relay, or account service. Each host stays
  independent.
- Out of scope for M3: projects spanning several machines (M6), remote
  previews, and any command sandbox on Linux.

## Transport

### One stream format for both paths

Today each request opens its own Unix socket, and up to three watches stay
open. A remote link carries many logical streams over one connection, with
this frame:

```text
[u32 stream id][u8 kind][u32 length][payload]   (integers big-endian)
```

| Kind | Direction | Payload |
| --- | --- | --- |
| 1 Open | client → host | One newline-terminated request, at most 64 KiB |
| 2 Frame | host → client | One newline-terminated reply or event, at most 1 MiB |
| 3 Close | either | Empty. From the client it ends a watch; from the host, EOF |
| 4 Error | host → client | Fixed transport text for that stream; the link stays up |
| 5 Ping | either, stream 0 | Empty. Answered by a Ping, for liveness |

- The remote end opens one connection to the host's owner-only socket per
  stream, so `plait-host run`, its uid check, and its 64-client cap are
  unchanged.
- A link carries at most 32 open streams. Sizes are checked before
  allocation, and a frame that does not complete within 15 seconds closes
  the link.
- Payloads are the existing newline JSON, forwarded byte for byte. Protocol
  versions 1–6 and every reply shape are unchanged.
- In plait-core, `Client` gains a transport: the local socket, or a stream
  on a shared link. `Frames`, watches, and cancellation work the same over
  either.

### SSH

- The client keeps one child per machine:
  `ssh -T -o BatchMode=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -- <destination> plait-host bridge`. The arguments
  are passed as a list and never through a shell.
- The person's SSH configuration (aliases, keys, agent, `ProxyJump`,
  Tailscale SSH) applies as usual. Plait never asks for or stores a
  password. Host keys are checked by the person's `known_hosts`, and an
  unknown host fails with a clear message.
- `plait-host bridge` connects each stream to the default store's socket,
  or to `--store PATH`.
- A Ping every 15 seconds, with a 45-second deadline, detects a dead link.
  The client then shows the machine offline and reconnects with backoff.
  Accepted work keeps running on the host.

### Tailnet

- `plait-host serve --tailnet` is opt-in. It binds the machine's own
  Tailscale addresses (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`) as reported by
  `tailscaled` on a configured port, never a wildcard address. It fails
  closed when `tailscaled` cannot be reached.
- For each connection, the host asks `tailscaled`'s local API who the peer
  is before reading protocol bytes. It accepts only an explicit allowlist:
  the owner's Tailscale login, and optionally named nodes. Tagged and shared
  nodes are refused unless listed by name.
- The same stream format and limits apply.
- The client dials `host:port` over the tailnet and needs no SSH.
- The listener asks `tailscaled` at `/var/run/tailscale/tailscaled.sock`, where
  Linux installs it. Elsewhere it cannot reach `tailscaled` and refuses to
  start, so the tailnet listener is a Linux host feature for now.
- On the host, run `plait-host tailnet allow --me` to allow your login, or
  `plait-host tailnet allow <login>` to allow another login. Tagged nodes also
  require `plait-host tailnet allow-node <name>`. Start the listener with
  `plait-host serve --tailnet [--port 7419] [--store PATH]`. On Linux,
  `plait-host install-service --tailnet` installs the separate listener unit.
  The owner host service must also be running.

### What crosses the link

The same as the local socket: requests, replies, and events. One-way secret
writes (provider keys, sign-in answers, the Linear key) cross only these
authenticated links, and replies never contain secrets. Host-private
descriptors and environment are never sent. Browser sign-in to a remote host
uses the pasted-code path, because the host's callback listens on that
machine's loopback.

## The Linux host

- **Trusted commands and trusted MCP servers run on Linux,** as they do on a
  Mac in a trusted project: as the person, with their tools.
  - A sandboxed command policy is refused on Linux, with a message pointing
    to [LINUX-SANDBOX.md](LINUX-SANDBOX.md).
  - Codex and Claude Code threads use their own permission modes, as
    everywhere.
  - The runtime's platform gate is split into "can sandbox" (macOS) and "can
    run trusted" (macOS and Linux).
  - The login-shell fallback becomes `/bin/sh` on Linux.
  - Process-group cleanup and cleanup receipts are unchanged.
- **Running the host on Linux (systemd user service built):**
  - `plait-host run` in the foreground, and a systemd user unit installed by
    `plait-host install-service`.
  - From a source checkout, run:

    ```sh
    cargo build --release -p plait-host
    (cd runtime && npm ci)
    target/release/plait-host install-service
    ```

    The service uses the Node found on your current PATH. Without lingering,
    it stops when you log out; `loginctl enable-linger $USER` keeps it running.
    Remove the service with `plait-host uninstall-service`.
  - The store stays `~/.local/share/plait`.
  - Published Linux host archives follow
    [independent releases](RELEASING.md#independent-client-and-server-releases).
- **Recording and turn undo** already have non-macOS paths. Build-output
  cloning stays macOS-only; Linux worktrees start cold.

## The client

- **Machines.** Settings lists machines: this Mac, plus each remote machine
  with a name and either an SSH destination or a tailnet `host:port`.
  - Machines are stored in desktop preferences.
  - Each machine has its own connection, epoch, catalog, watches, and
    offline state. Losing one never resets another.
- **Identity.**
  - The two host keys in core (`ownership::HostKey` and `tabs::HostKey`)
    become one machine-scoped identity.
  - Every host request goes through that machine's client; nothing calls
    `Client::from_environment()` directly.
  - Checkout identity for a remote checkout comes from the host, never from
    a local `stat`.
  - Client-side `canonicalize`, `verify`, and `is_dir` checks are skipped
    for remote checkouts, because the host checks the identity itself.
- **Sidebar.** Projects group under their machine, with the machine's state
  (connected, connecting, offline) and the time since it was last seen.
  Tabs, alerts, and notification tags are scoped by machine.
- **First remote slice.** Remote threads have their conversation,
  approvals, Stop, queue, models, and history. Remote Git, below, adds
  Changes, History, and Files; the editor and terminal handoffs stay
  unavailable for remote checkouts, with the reason.

### First desktop slice: one machine at a time

**Built:** request routing, Settings → Machines with SSH, opening a remote project by path, and unavailable states for local Git and file views. The live check remains.

Decided while building step 4, to reach usable remote threads sooner.

- **One machine per window.** The first desktop slice works with one
  machine at a time, chosen in **Settings → Machines**. The desktop already
  treats a different host store as another host, and parks that host's tabs
  as offline, so switching reuses that path.
- **Routing.** Every host request goes through the selected machine's
  client.
- **Local-only work.**
  - The folder picker, the client-side checkout checks, and first-launch
    setup apply only to this Mac.
  - A remote project is opened by typing its path on that machine.
- **Git views.** At first, Changes, History, and Files said they were
  unavailable on a remote machine. [Remote Git](#remote-git-and-files)
  replaced that. The editor and terminal handoffs remain unavailable.
  Worktrees measures no sizes there, because sizes read this Mac's disk.
- **Later.** Projects from several machines in one sidebar at once, as
  described above, follow as their own slice.

## Remote Git and files

**Built, except Worktrees sizes; the live check remains.** A remote checkout
gets the same Changes, History, Files, branch picker, Worktrees graph, and
agreed Git writes as a local one, run by the host on its own machine. The
desktop's local path is unchanged.

### Where the Git runs

- The host runs the same `plait-core` functions the desktop runs today
  (`git::status`, `diff`, `files`, `log`, `show`, `branches`,
  `line_counts`, `log::graph`, `file_view::read` and `stamp`,
  `worktree_size`, and `git::write_checked`). It runs them in the Rust
  helper, as it already does for turn records, review, and worktrees, so
  both paths share one hardened command builder and one set of limits.
- No Git crosses the link: only requests and bounded, typed results.
- Reads keep the desktop's hardening: no optional locks, external diff,
  textconv, filters, fsmonitor, or untracked cache. Writes keep the
  repository's own configuration, filters, and hooks, as local writes do.

### Operations

Versionless, additive operations, like turn review. A host that does not
know them answers `invalid_request`, and the desktop keeps the views
unavailable with "Update this machine's host to see its Git."

- **Reads (built):** one operation, `git_read`, names the read: `status`,
  `diff`, `files`, `log`, `show`, `branches`, `line_counts`, `graph`,
  `file`, or `file_stamp`. `git_read_page` fetches the rest of a large
  result. Worktree sizes come with the desktop step, because they measure
  the host's store too.
- **Writes (built on the host):** `git_write` carries one `WriteOp` (stage,
  unstage, hunk, commit, push, publish, fetch, pull, discard, restore,
  create or switch branch) and the checkout identity the desktop last read.
- Every request names a checkout. The host accepts an absolute path to the
  top of a Git work tree, checks its device and inode before and after the
  call (as `RepositoryJob` does locally), and refuses a changed identity.
  File reads go through the no-symlink helper rooted at that checkout.

### Size and paging

The client accepts replies up to 1 MiB; local reads allow more. So:

- The host runs a read once and keeps its serialized result. A result over
  600 KiB is served in base64 pages of that size under a token, so every
  page comes from the same snapshot. Tokens last two minutes after their
  last use, at most four at a time, and end with their last page.
- A result over 64 MiB is refused, not cut short.
- A file comes back as its stamp and bytes, or only its stamp when it is
  over the viewer's 8 MiB limit. The desktop classifies and highlights it,
  as it does a local file.
- Paths cross as base64 of their bytes, so names that are not UTF-8 survive.
- Every other limit is the local read's own.

### Writes, credentials, and undo

- Network writes run on the host with that machine's credentials: its SSH
  agent and keys, and its Git credential helpers. Prompts stay off, so a
  missing credential fails with Git's own message. The link does not forward
  this Mac's SSH agent.
  A host run as a systemd user service has no SSH agent unless the person
  gives it one, so an SSH remote needs a key without a passphrase there, or
  an HTTPS remote with a credential helper.
- A discard saves the file as an unreferenced blob in the repository's own
  object store, which is on the host, so **Restore** works there unchanged.
- A write is not tied to the view's epoch, as locally. If the link drops
  during a write, the host finishes it, and the desktop rereads status when
  it reconnects rather than assuming either outcome.

### Desktop

- Repository jobs run through a backend chosen by the machine: local Git
  for this Mac, host operations otherwise. `pump` stops being gated.
- Editor, Terminal, and Finder handoffs stay unavailable for remote
  checkouts; they would open this Mac's paths.
- Refresh is unchanged: host events, **Refresh**, and focus trigger reads.
  Nothing watches the remote file system.

### Order

1. **Built:** host read operations with paging, and their tests: results
   equal to local reads, paging, identity changes, symlinked files,
   non-UTF-8 names, and an older host.
2. **Built:** a desktop backend per machine for Changes, History, the
   branch picker, Files, the composer file list, line counts, the Worktrees
   graph, and writes. Worktrees sizes still need a host read that measures
   the host's store too.
3. **Built:** `git_write` on the host, including network writes, under the
   checkout's leases.
4. The live check from a Mac to the Linux host.

## Order of work

1. **Linux trusted commands and MCP — built.**
   - Includes the documentation changes.
   - Tests: trusted execution, Stop and process-group cleanup, trusted MCP,
     and continued refusal of a sandboxed policy on Linux.
2. **Built: stream format, `plait-host bridge`, and the client's transport
   abstraction.**
   - Tests: round trips over socket pairs, concurrent requests and watches,
     Close ending a watch without stopping a run, size limits, a stalled
     partial frame, and bridge EOF.
3. **Built: SSH transport.**
   - Tests: with a scripted `ssh`, connect, reconnect, dead-link detection,
     and arguments that cannot reach a shell.
4. **Machines in core (part 1 built).**
   - The saved machine list and per-machine connection model are built; the
     desktop still connects locally. One identity, projects under machines,
     and remote checkout identity remain.
   - Tests: headless grouping, jumps across machines, independent epochs,
     and one machine going offline.
5. **Desktop remote threads.**
   - Covers machine-routed clients, the sidebar, Settings → Machines,
     unavailable Git views, and scoped alerts.
   - Tests: scenes with mixed local, remote, and offline machines, and
     approval and Stop routing.
6. **Tailnet listener and client — built.**
   - Tests: a scripted `tailscaled` answering for an allowed owner, the
     wrong user, a tagged peer, and being down; refusal to bind a
     non-Tailscale address; and limits.
7. **Linux host releases, the systemd unit, and a live check** from the
   Mac to the Linux workstation. The systemd user service is built; releases
   and the live check remain planned.

Remote Git and files follow as their own slice, designed above.

## Verification limits

Until step 7, remote checks use scripted peers on one machine. A remote
claim needs the live check: the Mac reaching this Linux host over SSH and
over the tailnet, and the host continuing a run while the Mac is closed.
