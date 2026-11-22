# Remote hosts

Plait connects a Mac window to one local or remote machine at a time. Remote
hosts run on macOS or Linux and are reached over SSH or an opt-in Tailscale
listener. The host owns execution, threads, approvals, Git, files, and recovery;
closing the window or losing the link does not stop accepted work. See
[Host](HOST.md) for the protocol and safety contract.

## Installing a remote host (agreed, not built)

**From the Mac.** In **Add machine… → SSH**, Plait checks whether the machine
answers `plait-host version`. If not, it offers **Install Plait on
_machine_**, over the same SSH connection:

1. Plait reads the machine's platform (`uname -sm`): Linux on x86_64 or arm64,
   or macOS on Apple silicon.
2. Plait fetches that platform's signed host manifest, verifies its signature
   on the Mac, and gives the machine the archive's URL and SHA-256.
3. The machine downloads the archive itself (`curl` or `wget`), checks the
   SHA-256, unpacks it under `~/.local/share/plait/host/`, links
   `~/.local/bin/plait-host`, and starts the user service. Plait then
   connects.

Everything runs as the person; nothing uses `sudo`. Keeping the host running
after logout needs `loginctl enable-linger`, which may need an administrator;
Plait says so rather than asking for a password. A failed step names itself,
shows the machine's own message, and leaves any existing host untouched.

**From the machine.** A one-line script does the same steps on the server:
`curl -fsSL …/install.sh | sh -s -- --host`. Like the Mac script, it checks
the SHA-256 from the release record; it does not verify the publisher
signature.

**Archives.** Each release publishes a host archive per platform, with Node,
the runtime, and its dependencies included, and a signed manifest for it:
`update-host-linux-x86_64.json` and `update-host-linux-arm64.json`, in the
same format as `update-host.json`, each with its own build ID. Existing
manifests are unchanged.

## Agent CLIs on a host (agreed, not built)

Pi is part of the host, so a new machine can run threads with a provider key
or sign-in alone. For Codex and Claude Code, **Settings → Agents** shows each
one's state on the connected machine:

- **Not installed · Install.** The host installs the version Plait is tested
  with into its own folder (`~/.local/share/plait/tools/`), using its bundled
  Node or the vendor's official installer, never `sudo`. A copy the person
  installed themselves is found first and always wins. The CLI stays the
  vendor's software under the vendor's terms, linked before installing.
- **Sign in.** The host runs the CLI's own sign-in; the app shows its link and
  code and passes back anything the person pastes. Credentials stay in the
  CLI's own files on that machine; Plait never reads them.

## Order of work

1. Linux host archives and their signed manifests in the release pipeline.
2. The server-side install script.
3. Installing from the Mac over SSH.
4. Host-installed CLIs and their sign-in in the app.
5. Host updates on Linux: `plait-host update` and from the Mac.

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
