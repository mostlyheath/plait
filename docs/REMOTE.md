# Remote hosts

Plait connects a Mac window to one local or remote machine at a time. Remote
hosts run on macOS or Linux and are reached over SSH or an opt-in Tailscale
listener. The host owns execution, threads, approvals, Git, files, and recovery;
closing the window or losing the link does not stop accepted work. See
[Host](HOST.md) for the protocol and safety contract.

## Set up a Linux host

Download the archive and its `.sha256` file for your Linux architecture from
[Releases](https://github.com/heath0xFF/plait/releases), verify it with
`sha256sum -c`, and extract it. Run `<build-id>/bin/plait-host install-service`
from its permanent location. The archive includes Node and the runtime.

Alternatively, install Rust, Node, npm, and Git on the host and build from a source checkout:

```sh
cargo build --release -p plait-host
(cd runtime && npm ci)
target/release/plait-host install-service
```

A source-built systemd user service uses the Node on the current PATH; an archived host uses its bundled Node. The host stores data in
`~/.local/share/plait`. Run `plait-host run` for a foreground host, or
`plait-host uninstall-service` to remove the service. Without lingering, the
user service stops at logout; `loginctl enable-linger $USER` keeps it running.
Linux host release archives include Node and the runtime; in-app Linux host updates are planned.

Linux runs trusted commands and trusted MCP servers as the host user. It
refuses the sandboxed command policy; see [Linux sandbox](LINUX-SANDBOX.md).
Codex and Claude Code use their own permission modes. Turn records and undo
work on Linux; worktree build-output cloning is macOS-only.

## Connect over SSH

In **Settings → Machines**, add a name and an SSH destination. Open a remote
project by entering its path on that machine. The sidebar's top row names and
switches machines; the window title names the remote machine.

The client runs `ssh -T` in batch mode and starts `plait-host bridge` as the
remote user. The bridge connects to the host's owner-only socket; `--store PATH`
selects a different store. SSH configuration, keys, agent, and `known_hosts`
apply. Plait does not store a password, and an unknown host key fails. A lost
connection appears offline and reconnects with backoff; host work continues.

## Connect over Tailscale

On the Linux host, allow the owner's login and start the listener:

```sh
plait-host tailnet allow --me
plait-host install-service --tailnet
```

The owner host service must also run. Alternatively start the listener with
`plait-host serve --tailnet [--port 7419] [--store PATH]`. Use
`plait-host tailnet allow <login>` for another login; tagged nodes also require
`plait-host tailnet allow-node <name>`. Configure the machine in Settings with
its tailnet `host:port`.

The listener binds only its Tailscale addresses, never a wildcard address. It
asks the local Tailscale service to identify each peer before reading protocol
bytes and admits only allowlisted identities. Tagged and shared nodes are
refused unless named. It fails closed when the service is unavailable. The
listener currently requires the Linux Tailscale socket.

## Working remotely

Changes, History, Files, branches, Worktrees graphs and sizes, and agreed Git
writes run on the host. Reads are bounded and paged; the host checks checkout
identity and prevents file reads through symlinks. Git reads disable optional
locks, external diff, textconv, filters, fsmonitor, and untracked cache. Writes
use the host repository's configuration and hooks. Open checkouts are watched
and status changes are pushed to clients.

Network Git operations use the host's credentials; the connection does not
forward this Mac's SSH agent. A systemd user service needs its own available
credentials. Missing credentials fail rather than prompting. Editor, terminal,
and file-manager handoffs are unavailable for remote checkouts. Browser sign-in
uses the pasted-code path because the callback is on the host's loopback.

Each write carries an operation ID; retrying returns its first outcome. If a
link drops during a write, the host finishes it and the client can recover the
outcome and refresh state. Each device's tabs, layout, and selection are
stored on each host. Several machines in one sidebar and runs surviving a host
restart are planned.

## Transport limits and verification

Both transports multiplex framed streams to the owner-only socket. A link has
at most 32 streams; requests are at most 64 KiB and replies or events at most
1 MiB. A partial frame has a 15-second deadline. Large Git results use bounded
pages; results above 64 MiB are refused, and files above the viewer's 8 MiB
limit return only their stamp. Secret writes cross authenticated links; replies
do not contain secrets.

Scripted transport checks do not establish a live Mac-to-Linux connection or
verify a run continuing while the Mac is closed. See
[Verification](VERIFICATION.md) for current evidence.
