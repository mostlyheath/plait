# Remote hosts

**Agreed design for M3; transport framing and the bridge are built.** The maintainer chose the direction:
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
  `ssh -T -o BatchMode=yes <destination> plait-host bridge`. The arguments
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
- **Running the host on Linux:**
  - `plait-host run` in the foreground, and a systemd user unit installed by
    `plait-host install-service`.
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
  approvals, Stop, queue, models, and history. Changes, History, Files, and
  the editor and terminal handoffs are shown as unavailable for remote
  checkouts, with the reason, until remote Git is built.

## Remote Git and files (next slice)

Bounded host-side read operations give remote checkouts their status,
diffs, log, file lists, and the file viewer. Then come the agreed Git
writes: staging, commit, and push. They apply the same hardening as the
desktop's local Git reads. That design is written before it is built.

## Order of work

1. **Linux trusted commands and MCP.**
   - Includes the documentation changes.
   - Tests: trusted execution, Stop and process-group cleanup, trusted MCP,
     and continued refusal of a sandboxed policy on Linux.
2. **Built: stream format, `plait-host bridge`, and the client's transport
   abstraction.**
   - Tests: round trips over socket pairs, concurrent requests and watches,
     Close ending a watch without stopping a run, size limits, a stalled
     partial frame, and bridge EOF.
3. **SSH transport.**
   - Tests: with a scripted `ssh`, connect, reconnect, dead-link detection,
     and arguments that cannot reach a shell.
4. **Machines in core.**
   - Covers one identity, per-machine connection state, projects under
     machines, and remote checkout identity.
   - Tests: headless grouping, jumps across machines, independent epochs,
     and one machine going offline.
5. **Desktop remote threads.**
   - Covers machine-routed clients, the sidebar, Settings → Machines,
     unavailable Git views, and scoped alerts.
   - Tests: scenes with mixed local, remote, and offline machines, and
     approval and Stop routing.
6. **Tailnet listener and client.**
   - Tests: a scripted `tailscaled` answering for an allowed owner, the
     wrong user, a tagged peer, and being down; refusal to bind a
     non-Tailscale address; and limits.
7. **Linux host releases, the systemd unit, and a live check** from the
   Mac to the Linux workstation.

Remote Git and files follow as their own design and slice.

## Verification limits

Until step 7, remote checks use scripted peers on one machine. A remote
claim needs the live check: the Mac reaching this Linux host over SSH and
over the tailnet, and the host continuing a run while the Mac is closed.
