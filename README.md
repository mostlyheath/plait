<p align="center">
  <img src="assets/branding/app-icon/plait-app-icon-1024.png" width="96" height="96" alt="Plait app icon" />
</p>

<h1 align="center">Plait</h1>

<p align="center">
  <strong>A native Mac app for working with coding agents.</strong><br/>
  Start threads in your repositories, approve what the agent does, then review
  and commit the result, on this Mac or on another machine.
</p>

<p align="center">
  <a href="https://github.com/heath0xFF/plait/releases"><strong>Download</strong></a> ·
  <a href="docs/GUIDE.md">Guide</a> ·
  <a href="docs/README.md">Docs</a> ·
  <a href="docs/ROADMAP.md">Roadmap</a> ·
  <a href="CONTRIBUTING.md">Contribute</a>
</p>

<p align="center">
  <img src="docs/images/conversation.png" width="100%" alt="Plait with a finished thread that added retries to a sample Rust crate: the agent's plan, its file reads, edits, and test run, and the changed files beside it" />
</p>

<p align="center"><sub>Plait's real views, drawn over a sample project. The thread is sample data, not a recorded agent run.</sub></p>

## How it works

Plait has two parts. The **host** runs on the machine that holds your code: it
runs the agents, keeps every thread, and owns your projects, Git, worktrees,
and undo history. The **app** is a fast native window onto a host, on this Mac
or on another Mac or Linux machine over SSH or Tailscale. Closing the window,
or losing the connection, never stops accepted work; reopening it picks up
where the host is.

Plait is written in Rust and drawn with [GPUI](https://www.gpui.rs/). There is
no web view.

Follow one message from Send to Undo in the [system walkthrough](docs/WALKTHROUGH.md).

## What you get

- **Work that outlives the window.** The host owns each run, its history, and
  its approvals. Quit, update, or reconnect; everything is where you left it.
- **Any machine, the same app.** Each machine keeps its own projects and your
  tabs on it. The control at the bottom of the sidebar shows the current
  machine and lets you switch machines.
- **Approvals you can read.** Let the agent work, or approve each edit and
  command. On a Mac, a project can run commands in a sandbox that cannot write
  the checkout or reach the network.
- **Undo a run.** The host records the checkout before and after every run.
  One press puts back what that run changed and leaves your own later edits
  alone.
- **Git review built in.** Stage and discard by file or hunk, commit, push,
  pull, and switch branches. The host watches the checkout, so Changes stays
  current without polling.
- **Several threads in one project.** A busy project offers a worktree of its
  own or a place in line. The Worktrees tab draws every branch with its
  thread's state and the space it holds.
- **Tabs, keyboard first.** Cmd-1 to Cmd-9, a command palette, quick open
  (Cmd-P), and shortcuts you can rebind.
- **Your agents and models.** Threads run on Pi, built in, or your own Codex or
  Claude Code. Sign in to a provider or add a key; the host keeps credentials.
- **Linear issues.** Browse a project's issues and start a thread from one.

<table>
<tr>
<td width="50%"><img src="docs/images/changes.png" width="100%" alt="The Changes view with four staged files and the diff of src/client.rs, ready to commit" /></td>
<td width="50%"><img src="docs/images/worktrees.png" width="100%" alt="The Worktrees tab with four branches, thread states, storage sizes, and the selected checkout's removal details" /></td>
</tr>
<tr>
<td width="50%"><img src="docs/images/approval.png" width="100%" alt="A thread waiting for approval to run cargo test in the command sandbox" /></td>
<td width="50%"><img src="docs/images/conversation-light.png" width="100%" alt="The same thread in Plait Light" /></td>
</tr>
</table>

## Install

Plait runs on **Apple Silicon with macOS 15 or later**, with Git and an account
with a model provider. The download includes the host and its runtime.

Download the **DMG** from [Releases](https://github.com/heath0xFF/plait/releases),
drag **Plait** into **Applications**, and open it. First launch sets up the host
for your account. Or from Terminal:

```sh
curl --proto '=https' -fsSL \
  https://raw.githubusercontent.com/heath0xFF/plait/main/scripts/install.sh |
  bash -s -- --allow-unsigned
```

**Plait is ad-hoc signed and not notarized.** The first open needs
**System Settings → Privacy & Security → Open Anyway**, once. Read the
[guide](docs/GUIDE.md#install-and-updates) first, and do not disable Gatekeeper.
Updates come from **Plait → Check for Updates…** and are checked against signed
manifests. To run a host on Linux, see [Remote hosts](docs/REMOTE.md).

## Get started

1. **Open project…** (Cmd-O): a repository or any folder.
2. **Settings** (Cmd-comma) → **Providers**: sign in or add a key.
3. Start a thread and describe the change. Watch it work, or approve each step.
4. Review in **Changes**, commit, and keep going in the same thread.

## Status

Plait is an early preview, built in the open and used daily to build itself.
Previews are published from `main` when a release is started; see [Releasing](docs/RELEASING.md).

- A window works with one machine at a time; several in one sidebar is next.
- Plait is not an IDE: no editor, embedded terminal, merge, or rebase. Files are
  read-only, and Open in Editor or Terminal uses the commands you choose.
- What is verified, and what is not yet, is in
  [Verification](docs/VERIFICATION.md).

## Development

The app is **Rust + GPUI**, the host is **Rust** with a **TypeScript** agent
runtime. See [Contributing](CONTRIBUTING.md), [Architecture](docs/ARCHITECTURE.md),
and [Design](DESIGN.md).

## License

Plait's code and documentation are [MIT licensed](LICENSE). Dependencies keep
their own licenses, including GPUI's Apache-2.0. The bundled Base16 schemes in
`assets/themes/base16` are from tinted-theming/schemes (MIT, license kept beside
them); the Stillwater themes are from
[Stillwater](https://github.com/heath0xFF/stillwater) (MIT).
