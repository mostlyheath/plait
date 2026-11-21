<p align="center">
  <img src="assets/branding/app-icon/plait-app-icon-1024.png" width="96" height="96" alt="Plait app icon" />
</p>

<h1 align="center">Plait</h1>

<p align="center">
  <strong>A native Mac app for working with coding agents.</strong><br/>
  Start threads in your repositories, approve what the agent does, then review
  and commit the result, all in one fast window.
</p>

<p align="center">
  <a href="https://github.com/heath0xFF/plait/releases"><strong>Download</strong></a> ·
  <a href="docs/MACOS.md">Mac guide</a> ·
  <a href="docs/README.md">Docs</a> ·
  <a href="docs/ROADMAP.md">Roadmap</a> ·
  <a href="CONTRIBUTING.md">Contribute</a>
</p>

<p align="center">
  <img src="docs/images/conversation.png" width="100%" alt="Plait with a finished thread that added retries to a sample Rust crate: the agent's plan, its file reads, edits, and test run, and the changed files beside it" />
</p>

<p align="center"><sub>Plait's real views, drawn over a sample project. The thread is sample data, not a recorded agent run.</sub></p>

Plait is written in Rust and drawn with [GPUI](https://www.gpui.rs/). There is no
web view. Agents run in a separate host process on your Mac, so closing the
window never stops accepted work, and reopening it returns you to the same
tabs, threads, and drafts.

## What you get

- **Native and fast.** Long threads, large diffs, and 20,000-line files scroll
  in virtualized views. Git and file reads run off the UI thread, and frame cost
  is measured on every view.
- **Work that outlives the window.** The host owns each run and its history.
  Quit, update, or reboot the app; threads, drafts, and pending approvals are
  there when you come back.
- **Tabs.** Keep several threads open and switch with Cmd-1 to Cmd-9. Each tab
  says whether its thread is working, needs you, finished, or failed, and keeps
  its own view, scroll position, find, and draft. Open as many New thread tabs
  as you like; each is its own draft, named by its first line. Closing a tab
  never stops its run, and your tabs come back after a restart. See
  [tabs](docs/MACOS.md#tabs).
- **Several threads in one project.** Start a thread where another is already
  running and Plait offers it its own worktree, a new branch and checkout made
  from HEAD beside the repository, or a place in line for the checkout. A
  project can make the worktree automatic, and can start each one with the
  build output you approve, cloned copy-on-write. See
  [a busy project](docs/MACOS.md#starting-a-thread-while-a-project-is-busy).
- **Worktrees as a branch graph.** A project's Worktrees tab draws each
  worktree's branch off the trunk, with its thread's state, what is
  uncommitted or not pushed, and the space it holds. Remove one from there
  when nothing would be lost. See [worktrees](docs/MACOS.md#worktrees).
- **Images in messages.** Paste or drop PNG/JPEG images, remove
  attachments before sending, and open conversation thumbnails larger. Drafts
  and sent originals persist privately. Requires host protocol 6; see the
  [limits and clipboard behavior](docs/MACOS.md#images-in-messages),
  including a 5 MiB combined image limit per message.
- **Approvals you can read.** Let the agent work, or ask before each edit and
  command. Commands run as you, as they would in your terminal, or opt a
  project into a macOS sandbox that cannot write your checkout or reach the
  network.
- **Undo a run.** The host records your checkout before and after every run,
  whichever harness ran it. One press puts back the files that run changed,
  including what its commands wrote, and leaves alone anything it finds you
  changed since. See [what undo does not restore](docs/MACOS.md#undo-a-run).
- **Git review built in.** Stage and discard by file or hunk (with undo),
  commit, push, fetch, fast-forward pull, and create or switch branches. Click
  diff line numbers to ask the agent about exactly those lines.
- **Every project in one sidebar.** Threads across checkouts with live status,
  a "Needs me" filter, alerts (toasts while Plait is in front, notifications
  when it is not), rename and archive, and optional
  per-thread worktrees.
- **Keyboard first.** A command palette, quick open (Cmd-P), find in file, and
  shortcuts you can rebind.
- **Your models.** Sign in to a provider or add an API key or custom server from
  Settings. The app never sees your credentials; the host keeps them.

<table>
<tr>
<td width="50%" valign="top">

### Review and commit

The Changes view shows staged and unstaged changes side by side with the
conversation that made them. Commit without leaving the thread.

</td>
<td width="50%">
<img src="docs/images/changes.png" width="100%" alt="The Changes view with four staged files and the diff of src/client.rs, ready to commit" />
</td>
</tr>
<tr>
<td width="50%" valign="top">

### Approve each step

In Ask mode, every edit and command waits for you, with the exact call and
what the sandbox allows. Threads that need you rise to the top of the sidebar.

</td>
<td width="50%">
<img src="docs/images/approval.png" width="100%" alt="A thread waiting for approval to run cargo test in the command sandbox" />
</td>
</tr>
<tr>
<td width="50%" valign="top">

### Read any file

Open any file in the checkout with Cmd-P, highlighted and searchable. It
reloads when the agent changes it on disk.

</td>
<td width="50%">
<img src="docs/images/files.png" width="100%" alt="The Files view showing src/retry.rs with find open on the word delay" />
</td>
</tr>
<tr>
<td width="50%" valign="top">

### See every worktree

The Worktrees graph shows each branch's commits, its thread's state, and the
space its checkout holds. Remove is available when nothing would be lost, or
with your say for unpushed commits that stay on their branch.

</td>
<td width="50%">
<img src="docs/images/worktrees.png" width="100%" alt="The sample forecast project's Worktrees tab, with four branches, thread states, storage sizes, and the selected checkout's removal details" />
</td>
</tr>
<tr>
<td width="50%" valign="top">

### Light, dark, and your themes

Follows macOS appearance. Stillwater is the default dark theme; Plait Light
and Dark, bundled Base16 schemes, and any Base16 file you add are one click away.

</td>
<td width="50%">
<img src="docs/images/conversation-light.png" width="100%" alt="The same thread in Plait Light" />
</td>
</tr>
</table>

## Install

Plait runs on **Apple Silicon with macOS 15 or later**. You need Git and an
account with a model provider. The installer includes the host and its Node
runtime; you do not need Xcode, Rust, or Node.

Download the **DMG** from [Releases](https://github.com/heath0xFF/plait/releases),
open it, and drag **Plait** into **Applications**. Open it from there; on first
launch Plait sets up its local server for your account. Or from Terminal:

```sh
curl --proto '=https' -fsSL \
  https://raw.githubusercontent.com/heath0xFF/plait/main/scripts/install.sh |
  bash -s -- --allow-unsigned
```

**Previews are unsigned and not notarized.** Read the [Mac guide](docs/MACOS.md)
before installing, and do not disable Gatekeeper. After the first install,
updates come from **Plait → Check for Updates…** and are checked against signed
release manifests. Threads, drafts, and credentials are kept across updates.

## Get started

1. Open Plait and choose **Open repository…** (Cmd-O).
2. Open **Settings** (Cmd-comma) → **Providers** and sign in, or add an API key.
3. Start a thread and describe the change. Watch it work, or approve each step.
4. Review the result in **Changes**, commit, and keep going in the same thread.

The [Mac guide](docs/MACOS.md) covers every view and shortcut.
[Build Plait with Plait](docs/SELF-HOSTING.md) shows the full edit, test,
review loop on a real project.

## How it works

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.svg">
  <img alt="Architecture diagram. The Plait desktop, written in Rust with GPUI, has four views today: Conversation for agent threads and approvals, Changes for Git status, diffs, staging, commits, and history, a read-only Files viewer, and Settings for models, providers, approvals, and agent skills. An editor, logs and processes, and reviews are marked as planned for M4. The desktop saves its own drafts and recovery state. It talks to the host daemon on your Mac over a local socket for requests and streaming updates, and runs hardened Git on your repository itself. On your Mac, the Rust host daemon runs the Pi runtime with durable threads, applies approved file edits, runs sandboxed commands that can only read the repository, and calls your model providers. Pi is built in, and other agent harnesses run through your own installs (M7). Two more machines, another Mac and a Linux machine, are drawn dashed as planned for M6, reached over an authenticated connection that is also planned." src="docs/images/architecture-light.svg" width="880">
</picture>

The desktop is a client. The host runs the agent, keeps every thread, and
applies only the edits and commands you allowed. Files you had already changed
are protected in both approval modes. See [host and sandbox rules](docs/HOST.md).
Dashed boxes are planned, not shipped.

## Status

Plait is an early preview, built in the open and used daily to build itself.
The latest preview is on the
[Releases page](https://github.com/heath0xFF/plait/releases); every merge that
changes the app publishes the next one.

- **Mac only, local only.** Remote hosts and Linux execution are
  [planned](docs/ROADMAP.md).
- **Agent harnesses.** A thread runs on Pi, built in, or in your own Codex or
  Claude Code, with its own sign-in and permissions; Claude Code needs a
  one-time setup in Settings → Agents. Other harnesses are planned (M7).
- **Your Pi setup, partly.** Threads get your Pi skills (including
  `/skill:name`), prompt templates, and user-level
  `AGENTS.md`, listed in each project's settings and offered when you type `/`.
  Pi threads find and call tools from configured stdio and header-auth HTTP MCP
  servers, with per-project switches and per-call approvals. MCP browser sign-in
  and named CLIs are planned (M8).
- **One thread writes a checkout at a time.** A second thread in a busy
  project can use its own worktree or explicitly wait in a durable queue.
  A worktree starts from HEAD: uncommitted files are never copied, build
  output only from the directories you approve for the project, and removing
  one is manual. A host restart holds queued messages for Resume.
  Reading while another thread writes and cleanup rules are
  [planned](docs/ROADMAP.md).
- **Not an IDE.** There is no file editor, embedded terminal, merge, or rebase.
  The Files view is read-only. Instead Plait hands off: Open in Editor opens the
  file on screen at its line in your editor, Show in Finder reveals it, and
  Open Terminal Here opens the project's checkout, each with what you choose in
  Settings → Editor.
- **Verification is public.** What CI and installed-app checks cover, and what
  they do not yet, is in [Verification](docs/VERIFICATION.md).

## Development

The desktop is **Rust + GPUI**, the host is **Rust**, and the agent runtime is a
**TypeScript sidecar** built on Pi Durable. Start with
[Contributing](CONTRIBUTING.md) for build requirements and checks;
[Architecture](docs/ARCHITECTURE.md) and [Design](DESIGN.md) describe the
boundaries. `bash scripts/readme-screenshots.sh` regenerates the images above.

## License

Plait's original code and documentation are [MIT licensed](LICENSE).
Dependencies keep their own licenses, including GPUI's Apache-2.0 license. The
bundled Base16 schemes in `assets/themes/base16` are from tinted-theming/schemes
(MIT, license kept beside them). Stillwater's colors come from
[Stillwater](https://github.com/heath0xFF/stillwater) (MIT).
