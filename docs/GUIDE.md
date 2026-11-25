# Plait user guide

Plait is a native Mac app (Apple Silicon, macOS 15 or later) for working with coding agents through a separate host process. Closing or updating the app never stops accepted work. The bundle declares macOS 15, while packaging CI runs on macOS 26; older-system runtime acceptance is not yet complete.

## Install and updates

Download the **DMG** from [GitHub Releases](https://github.com/heath0xFF/plait/releases), open it, and drag **Plait** into **Applications**. Open Plait from Applications; on first launch it sets up its local host automatically. You need Git and a model provider account. No Xcode, Rust, or separate Node installation is required; the host bundles pinned Node **26.12.0** and its runtime.

For a command-line install or update (run as your normal user, never under `sudo`):

```sh
curl --proto '=https' -fsSL \
  https://raw.githubusercontent.com/heath0xFF/plait/main/scripts/install.sh |
  bash -s -- --allow-unsigned
```

Use `--version <tag>` to pin a release or `--download <dir>` to verify and save the archive without installing. Initial script downloads verify SHA-256 checksums over HTTPS (detecting corruption, not publisher identity).

**Unsigned previews:** Previews are ad-hoc signed and not notarized. On first launch, review the source and approve the app in **System Settings → Privacy & Security → Open Anyway** if prompted. Do not disable Gatekeeper. Earlier root-owned system-wide package installs (`.pkg`) must migrate once using **Move from system-wide install.command** from the DMG (or the script above), which asks for administrator access once; threads, drafts, and credentials are preserved.

### In-app and headless updates

**Plait → Check for Updates…** (and **Settings → About**) checks GitHub Releases for the newest signed manifest (`update.json`, verified with an embedded Ed25519 public key before trusting versions, sizes, or SHA-256 digests). Plait also checks quietly at launch and shows an **Update** button at the foot of the sidebar when a release is available.

**Install and Restart** replaces `/Applications/Plait.app`, installs the new host, restarts it, and reopens Plait with your tabs and drafts intact. Updates refuse to install while any thread is running or awaiting approval; finish or stop active runs first. Independent client and server updates are planned ([Releasing](RELEASING.md#independent-client-and-server-releases)).

On a headless Mac host, run `plait-host update --check` and `plait-host update`. For Linux hosts, see [Remote hosts](REMOTE.md#set-up-a-linux-host).

### From a source checkout

Contributors can install a working tree as this Mac's per-user app and host:

```sh
scripts/update-installed-macos.sh
```

If runs are active, a changed host is staged rather than restarted; activate it later with `--activate-host` (or stage explicitly with `--stage-host`). See [Releasing](RELEASING.md#packaging-and-signing) for building distributable packages.

## First launch and the local host

Plait separates the desktop client from the host daemon (`plait-host`). On first launch from Applications, if no local host is running or installed, Plait unpacks its bundled host into `~/Library/Application Support/Plait/Host`, registers a per-user LaunchAgent (`dev.plait.host`), and connects to its owner-only socket at `~/.local/share/plait/host.sock`.

First launch only installs a missing local server. It leaves alone:

- any host launchd already runs (including one with active work);
- any host answering on the default socket (such as a manually started development host);
- an existing installed host release that is stopped (it registers that release rather than downgrading).

First launch does nothing for source builds or when `PLAIT_HOST_STORE` points to a development store. If opened directly from a mounted DMG or Downloads, Plait asks you to drag it to Applications first.

The client negotiates host protocol versions **1 through 7** per request and degrades gracefully when connected to an older host, disabling only controls that host does not support.

## Machines

**Settings → Machines** selects which machine's host this window works with: **This Mac**, or another Mac or Linux machine over **SSH** or **Tailscale**. Each window works with one machine at a time; showing projects from several machines in one sidebar is planned ([Host-first Plait](HOST-FIRST.md)). The open folder belongs to the machine it was chosen on: after a switch, the window opens one of the new machine's projects and never lists or saves the other machine's folder as its own. Folding a project folds it only on that machine. A worktree removal that finishes after a switch or a reconnect changes the lists, and shows its result, only on the machine it ran on, once that machine's host is checked. Unsent drafts and saved thread work are not yet kept per machine ([#132](https://github.com/mostlyheath/plait/issues/132)).

To add a remote machine, choose **Add machine…**:

- **SSH:** Give it a name, an SSH destination (such as `you@workstation` or an alias from `~/.ssh/config`), and optional port and host store path. Plait runs `ssh -T -o BatchMode=yes … -- <destination> plait-host bridge` using your own SSH keys or agent and never asks for or stores a password. If the machine has no Plait host, connecting offers **Install Plait on _machine_**: the machine downloads the release's prebuilt host for Linux x86_64 or arm64, with Node and the runtime included, checks it against the signed manifest, and starts it as your user. Nothing is compiled there and nothing uses `sudo`.
- **Tailscale (Tailnet):** On a Linux host with `plait-host` running, authorize your login with `plait-host tailnet allow --me` (and `plait-host tailnet allow-node <name>` for tagged devices) and start the listener with `plait-host serve --tailnet` (or `plait-host install-service --tailnet`). In Plait, enter a name, the machine's tailnet host, and port (`7419` by default).

Choose **Use** to connect in the background; if connecting fails, Plait stays on the current machine and reports the error. The window remembers its last machine and reconnects at launch without blocking. **Remove…** forgets a machine locally without affecting its host or threads. Live remote verification against a separate Linux host remains open; see [Remote hosts](REMOTE.md) for transport and Linux service details.

**Which machine you are on:** The control at the foot of the sidebar names the machine and shows its connection state. Open it to switch machines or choose **Manage machines…**. The window title also names a remote machine. A remote machine's name comes from its host (its hostname unless renamed); while it connects, the name you saved is shown.

Press **Cmd-Option-J** (Next Thread Needing Me) to open the oldest waiting approval, followed by failures or interruptions, then unread results. Archived threads are excluded. Settings and any available Update sit beside the machine control.

A blue open ring marks an active thread in the sidebar and tabs, and appears
beside the current turn’s state and any tool still running. The ring turns
while work is in progress. It stays still when macOS **System Settings →
Accessibility → Display → Reduce motion** is enabled. If the host disconnects, cached activity stops animating until
the connection returns.

**Each machine's own projects:** Hosts supporting the workspace stream ([HOST.md](HOST.md#workspace)) keep their own opened and hidden projects, worktree ownership, and read/unread state. On this Mac, existing desktop lists import into this Mac's host once. Switching machines shows that machine's projects. Choose **Open project…** (`Cmd-O`) to pick a folder on this Mac, or browse another machine's folders through its host (repositories open in one step; a typed absolute path also works).

**Plain folders:** A folder that is not a Git repository opens too. Threads, approvals, and the file viewer work there; Changes, History, branches, worktrees, and undo need Git and say so. Nothing an agent changes in a plain folder can be undone, and no file is protected as already changed.

## Projects, threads, tabs, and drafts

Opening a repository adds it to the sidebar. Selecting a thread switches its conversation, draft, and Git workspace together. Hovering or focusing a project row reveals **+** for a new thread; its **…** menu opens **Project settings…**, **Worktrees**, **Linear issues**, or hides the project (its host threads stay intact; use **Show Hidden Projects** in the **Thread** menu or the palette to list them again). Missing folders stay visible as **Missing** with readable history.

Choose **New thread** to open a draft. **Needs you** opens Every thread. Its sidebar count includes unread activity, pending approvals, failures, and interrupted runs, excluding archived threads. **Find anything** opens the palette to search threads and commands. Expand a project to see its threads and their states, or fold it to see its count. **Open project…** is below the list.

**Every thread** shows the current machine’s unarchived threads by project, including threads in folded projects. Its bar counts threads that need a decision or recovery, threads that are working, and finished threads with unread activity. Open it with **Needs you**, `Cmd-Shift-E`, or **Every Thread** in the command palette. Use the arrow keys to move between cards, Home or End to jump to the first or last card, and Enter or a click to open a thread. **Back**, Escape, or the same shortcut returns to the open workspace. **Find** opens the command palette.

Each thread card shows its title and a state mark. Within a project, approvals come first, then failures and interruptions, finished unread threads, working threads, and everything else. Each group is ordered by most recent update. Cards show the harness, model, and update age when available. A branch appears when Plait knows the branch of that thread's own Plait-created worktree.

Use **Show or Hide Archived Threads** in the menu or palette. Rename or archive threads from thread actions or the **Thread** menu; archiving only hides a thread from the default list and never stops an active run. The host streams live updates for the newest 200 threads; use **Refresh Threads and Folders** in the **Thread** menu or the palette for older threads and folder state. **Mute or Unmute Thread Alerts** and **Show Hidden Projects** are there too.

**Mute or Unmute Thread Alerts** toggles alerts (unread markers stay when muted). While Plait is active, alerts for other threads appear as toasts in the upper right (up to three, 8 seconds each). While inactive, a bundled app posts macOS notifications (subject to system permission and Focus).

### Tabs

The sheet opens with the thread title. Hover over a shortened title to read it in full or over the project chip for the checkout path. The branch chip opens **Switch Branch**. As space tightens, the header reserves up to 160 points for the title. The project and branch names shorten in the middle, then the branch disappears, followed by the project. Below a window width of 760 points, both chips are hidden. The sidebar toggle sits beside the window buttons when the sidebar is hidden. Drag empty space in the header or the sidebar’s top strip to move the window.

Choose **Settings → Appearance → Thread tabs → Show** to see the open tabs and the **+** control while viewing a thread. The project and branch then sit below that thread’s message box. Drafts, Worktrees and Linear issues show their own title. With either setting, the header names a working thread, a pending approval, a failure or an interruption. **Toggle Panel** opens or closes the thread’s review panel.

- Switch tabs with `Cmd-1`–`Cmd-8` (`Cmd-9` selects the last), `Cmd-Shift-[` / `Cmd-Shift-]`, or **Show All Tabs** (`Cmd-Shift-Backslash`). These shortcuts also work with tabs hidden. Reorder tabs by dragging or with `Control-Shift-Left` / `Control-Shift-Right`.
- Close a tab with `Cmd-W` or middle-click; closing never stops a running thread. Reopen closed tabs (and older nonempty drafts) with `Cmd-Shift-T`. Closing the last tab leaves an empty workspace; pressing `Cmd-W` again (or `Cmd-Shift-W`) closes the window.
- Hiding the tabs preserves their saved order, per-view bookmarks, and unsent drafts.

### Review panel

Open **Toggle Panel** (`Cmd-L`) on a thread. Choose **Changes**, **Files**, or **History** inside the panel. The close control or **Conversation** (`Cmd-Shift-1`) returns focus to the thread. New and restored tabs start with the panel closed.

**This turn** shows the selected turn’s recorded diff. **Uncommitted** shows the checkout’s current changes and commit controls. A thread without a recorded turn opens on **Uncommitted**. Use **Cmd-Option-Right** or **Cmd-Option-Left** to move focus between the conversation and panel. Find searches the focused conversation, diff, or file. Both focus shortcuts stay within the panel when it fills the sheet.

Drag the divider to resize the panel. Its width is saved on this device. Double-click the divider to return to half the sheet. **Cmd-Backslash** toggles fill. On sheets narrower than 1020 points the panel fills the sheet automatically.

### Drafts, messaging, and search

`Cmd-N`, `Cmd-T`, and **+** each open a new independent draft tab without creating anything on the host until you send. A draft takes its title from its first nonblank line (up to 120 characters; empty or image-only drafts read **New thread**). Choose its project and harness in the row above the message box, and its model, effort, and permissions inside the message box. **Discard** deletes the draft and closes its tab; `Escape` only closes an empty draft tab.

Requests appear in bubbles on the right. Replies use the full conversation width. Choose **Settings → Appearance → Turns → Line** for author labels and a line connecting each turn.

During a run:

- Press `Enter` (or `Cmd-Enter`) to queue a **Follow-up** for after the run finishes, or `Option-Enter` to **Steer** the running agent at its next step. Queued items appear above the message box with **Cancel** and **Edit**.
- While a run is active and the draft is empty, the Send button becomes **Stop** (or press `Escape` twice within 1.5 seconds) to stop the run and cancel that thread's queued inputs. Typing into the draft switches the button back to queue the message.
- Type `@` at a word start to insert a checkout file path, `/` at the start of an empty draft for local commands and Pi skills/templates, and `Up`/`Down` on the first line to recall earlier prompts.
- Context usage appears at the right of the message box's settings row when known (marking 75% as high and 90% as nearly full; hover for token counts).
- Press `Cmd-F` to search loaded conversation messages (`Enter`/`Cmd-G` next, `Shift-Enter`/`Cmd-Shift-G` previous, up to 1,000 matches, case-insensitive). Earlier pages load as you scroll to the top or press `Home`/`Cmd-Up`; Find searches loaded messages only and opens a folded turn when it holds the current match.
- Select text within one reply and choose **Quote selection** beside **Copy message** or in the command palette. Plait appends the quote and focuses the unsent draft. An existing draft keeps its caret. In an empty draft, you can type your response on the blank line after the quote.
- When the host records a finished turn, its reasoning, tool calls, steering messages, and intermediate replies fold under one step summary. Open it to read the work in order. Find opens it for the current match. Failed, stopped, and interrupted runs keep their work visible. Turn off **Settings → Appearance → Fold finished work** to show the rows individually.
- Earlier finished turns fold to their request line (with age) and closing line. Click a folded request or press `Enter` / `Space` to open it for the life of the tab; the latest turn, active or failed/stopped turns, and the current Find match stay open. Toggle this in **Settings → Appearance → Fold earlier turns**.
- Press `Cmd-K` for the command palette (threads, commands with shortcuts, and "New thread with" signed-in models).

### Busy projects and the queue

Only one thread writes a checkout at a time. Sending a new draft in a busy project opens **Project busy**:

- **Own worktree:** Creates a new branch and checkout from `HEAD` beside the main repository and sends the draft there. Uncommitted files are never copied.
- **Wait:** Places the message in the durable checkout queue (up to 32 pending messages and 32 MiB of queued text and images per checkout) to start in order when the checkout is free.
- **Cancel:** Keeps the draft unsent.

An existing thread in a busy checkout offers **Wait** or **Continue separately** (which creates a linked worktree thread from `HEAD` without copying conversation history). You can set **Project settings → New threads → When this project is busy** to **Own worktree** to skip the prompt for new threads, or press `Cmd-Option-N` anytime to create a worktree thread directly.

A host restart marks not-started queued messages as **Held**; choose **Resume** to release held checkout waiters in order (interrupted runs are never replayed). Changing a queued thread's model, thinking level, permissions, or command policy causes that waiter to fail when its turn arrives and returns its text/images to the draft (or clipboard if the draft is nonempty).

### Images in messages

Paste (`Cmd-V`) or drop PNG or JPEG images onto the message box (format is detected from bytes; TIFF-only clipboard data is refused, and when the clipboard holds both text and an image, text is pasted). Click a conversation thumbnail for a larger view (`Escape` closes it).

- **Limits:** Up to 4 distinct images per message, at most 5 MiB each and 5 MiB combined, up to 8,192 px on either side and 16,777,216 total pixels. Plait never resizes or recompresses originals.
- **States:** **Refused** (limit or format; hover for reason), **Failed** (missing draft original; remove before sending), **Uploading**, **Text only** (selected model rejects images), and **Update host** (requires a protocol 6 host).
- **Retention:** Draft originals stay in the desktop session directory until sent or removed. Sent originals stay in the private host store for its lifetime (never inside your repository or undo records); unused uploads expire after 24 hours.

## Models, providers, agents, and approvals

### Providers and models

The host owns all provider credentials; the desktop never reads or saves them. Open **Settings → Providers** to add:

- **Subscription:** Sign in via browser (on a remote host, paste the returned code or redirect URL into Plait).
- **API key:** Paste once; the key is masked, sent directly to the host, and cleared from the form.
- **Custom server:** Add an OpenAI-compatible base URL, optional key, and model IDs (**Check connection** lists reported models without saving the entered key).

Credentials are stored on the host in `~/.pi/agent/auth.json` (mode `0600`) and custom servers in `~/.pi/agent/models.json`. You can also authenticate via the bundled Pi CLI (`/login`) and reload providers without restarting. A key or provider header written as `$NAME` or `${NAME}` in `models.json`, and a built-in provider's key variable such as `OPENAI_API_KEY`, are read from your login shell, as in a new terminal. Only key lookup sees these values; commands, Git hooks, and agents do not. Shortly after the host starts, such providers show as signed in; after changing a startup file, reload providers. Cloud credentials for Amazon Bedrock, Google Vertex, and Azure OpenAI endpoints still need to be in the host's own environment. An endpoint whose key or headers read a variable or run a command can only be moved to another server by editing `models.json`. Set the default harness, default model, and default thinking level in **Settings → New threads**; change an open thread's model (`Cmd-Shift-M`) or effort in the message box between runs.

**Thread naming:** In **Settings → New threads → Thread naming**, choose a
naming model to enable short, task-focused names. This model is separate from
the coding model and works for Pi, Codex, and Claude Code threads. For a local
model, add its OpenAI-compatible endpoint in **Providers**, then select it here.
The address must be reachable from the selected host; `localhost` means that
host, not the desktop.

Naming is off until you choose a model. When enabled, the host sends up to
8,000 characters of the first message text in one extra model call. It sends
no images, files, tools, or later conversation. The coding run does not wait
for the name. A timeout, unavailable model, or full naming queue leaves the
prompt-based title in place; no other provider is used. Existing conversations
and names you set yourself stay unchanged. Turn **Automatic names** off to stop
new naming calls and cancel pending ones. These settings belong to the host,
so naming continues when the desktop closes.

### Agents (Pi, Codex, Claude Code)

- **Pi** (built in): Uses Plait's approval mode and per-project command policy. Pi threads load your user-level `AGENTS.md`, Pi skills, and prompt templates (listed in **Project settings** and offered via `/`). Pi also supports configured stdio and anonymous or header-key HTTP **MCP servers**: toggle servers per project in **Project settings → MCP servers**. Project-defined entries in `.pi/mcp.json` start disabled as **Needs review** and bind to their exact definition (changing the file disables them as **Changed**). MCP browser OAuth and named CLIs are planned.
- **Codex** and **Claude Code:** Configured in **Settings → Agents**. Plait runs your own installed `codex` or `claude` binary (from login `PATH` or a chosen executable path) with its own sign-in, configuration, instructions, and MCP servers. Without one, **Install** puts the tested version in the host's own folder, and **Sign in** runs the CLI's sign-in from the app.
  - **Claude Code one-time setup:** Plait does not bundle Anthropic's Agent SDK. On first use, Claude Code reads **Needs setup**. To start setup, click **Set up** in **Settings → Agents**, choose **Claude Code** in a new thread's harness list, or run **Set up Claude Code threads** from the command palette. Plait downloads the single pinned SDK tarball from npm, verifies its checksum, and stores it beside the host database. The harness list and the palette open **Settings → Agents**, which shows progress and any failure. When Claude Code is ready, choose it for the new thread.

### Approvals and command safety

- **Pi threads:** Choose **Ask each time** or **Automatic** in **Settings → New threads**, or override per thread in the message box. In **Ask** mode, every file edit and command waits for your decision in an approval card drawn in the message box's place (**Allow once** `Cmd-Option-Y`, **Deny once** `Cmd-Option-period`, acting only when the card is visible in conversation). Choose **Reply instead** below the card to show the message box beneath it for that approval; any unsent draft stays intact while the box is hidden and after the approval is answered. In both modes, Pi's file tools refuse to modify pre-existing dirty files.
  - **Commands policy** (**Project settings → Commands**): **Trusted** (default) runs commands as your user with network and full checkout access. On macOS, **Sandboxed** runs commands offline with read-only checkout access and a private per-thread cache (sandboxed projects always ask approval before MCP tool calls, though an enabled local MCP server still runs as a local process). Linux hosts support trusted commands only; Linux command sandboxing is planned ([LINUX-SANDBOX.md](LINUX-SANDBOX.md)).
- **Codex and Claude Code threads:** Use the harness's own permission modes (defaults in **Settings → Agents**, per-thread under **Permissions**):
  - **Codex:** `Read only`, `Ask`, `Approve for me`, `Full access` (default). Mode changes apply on the next turn.
  - **Claude Code:** `Ask`, `Accept edits`, `Plan`, `Auto` (default), `Bypass`. Mode changes apply immediately.
  - Harness permission requests appear as one-shot Plait approval cards. Plait's pre-existing dirty-file guard does not block Codex or Claude Code edits (approval cards list dirty paths as a warning). If the harness's own configuration overrides the chosen mode, an amber notice inside the message box says so.

## Changes, History, Files, and Worktrees

Git operations run on the machine that owns the checkout. On a remote machine, that machine's host executes every Git read and write (`git_read` / `git_write`) using its own Git configuration, hooks, and credentials (your Mac's SSH agent is not forwarded). On this Mac, Git currently runs locally and moving local Git and file reads through this Mac's host with a checkout watcher is planned ([Host-first Plait](HOST-FIRST.md)). All Git reads are hardened (no optional locks, external diff, textconv, or filters); explicit writes run under the checkout lease and are refused while a thread is writing there.

In a thread header, **Commit…** opens **Changes → Uncommitted** and focuses the commit message. Stage and commit from that panel. With a clean working tree, the header offers **Publish** for an unpublished branch, **Push** for outgoing commits, or **Pull** for a fast-forward update. Hover over a disabled action to read its blocker. An up-to-date branch has no header Git action.

- **Changes** (`Cmd-Shift-2`): Stage or unstage files or individual `@@` hunks, **Stage all** / **Unstage all**, or **Mark resolved** on merge conflicts. **Discard…** / **Discard hunk…** (or **Delete…** for untracked files) asks for confirmation and saves the exact bytes in the repository's Git object store so **Undo** can restore them until your next Git action. Type a one-line commit message and press **Commit** (`Cmd-Shift-C`); hooks and filters run as configured. Use **Fetch**, **Push** (or **Publish** when no upstream exists), and fast-forward **Pull** (no merge, rebase, amend, or force push). Click the branch name (`Cmd-Shift-B`) to switch or create local branches. Click or Shift-click diff line numbers and choose **Ask about this** to append a quoted line reference to your draft. Joined to the top of the message box, a toggleable **Changed files** strip (closed at first, expanding up to six rows before scrolling) tracks uncommitted files edited by the thread's file tools and opens them in Changes.
- **History** (`Cmd-Shift-3`): Lists commits on the current branch, newest first, 100 at a time, with message and first-parent patch.
- **Files** (`Cmd-P`): Read-only syntax-highlighted viewer rooted strictly inside the checkout (no symlinks; files over 8 MiB, binary, or non-UTF-8 are described instead). Supports **Find** (`Cmd-F`, up to 1,000 matches) and reloads changed files on focus, status refresh, or tab return.
- **External handoffs** (this Mac only): **Open in Editor** (`Cmd-Option-E`) opens the active file in Files or Changes at its current/picked line; **Show in Finder** reveals it; **Open Terminal Here** (`Cmd-Option-T`) opens the checkout folder. Configure them in **Settings → Editor** using macOS's default app/Terminal or a single-line custom command with `{file}`, `{line}`, and `{project}` placeholders. Custom commands do not run through a shell (`$`, `~`, and globs stay literal), so pass full executable paths for binaries outside `/usr/bin` and `/bin`.

### Worktrees

Open a project's **Worktrees** tab from its sidebar menu, **Project settings**, the command palette, or the worktree control in a thread’s message box. The graph draws the main checkout's branch as the trunk and each worktree as a lane (capped at 300 commits), showing thread state, uncommitted counts, publication state as of the last fetch, and logical source and build-output sizes (walks stop at 50,000 entries or 250 ms and show **over** when clipped).

Host-owned worktrees are created on `plait/<uuid-prefix>` branches under `<main-repo>.worktrees/` from a captured `HEAD`. Nothing is removed automatically: select a lane and choose **Remove** to confirm (**Remove if pushed**, or **Keep local commits only** when only unpushed/unknown upstream state blocks removal). Active work, uncommitted changes, unapproved ignored files, and detached-HEAD commits block removal. Branches, thread history, and private caches remain.

**Build output seeding** (macOS only; Linux worktrees start cold): In **Project settings → New threads → Build output**, approve reproducible ignored directories (for example `target` or `node_modules`) to clone copy-on-write from an idle main checkout seed into new worktrees. Approve only reproducible output you are willing to remove with a worktree (everything inside an approved directory is cloned and excluded from removal blocks). Choose **Skip** during creation to start cold.

## Turn records, review, and undo

With a protocol 5+ host, the host snapshots your checkout before and after every run—across all harnesses and permission modes, including files written by commands—into a private Git store kept for 30 days. If the before-record fails, the run does not start.

Expand the file count and select a recorded filename or **Review** on a closing line to open that turn in **Changes**. Its recorded diff remains available even if the file was later committed, edited, or deleted. **Latest** returns to the newest turn. Undo and Redo use the same safeguards as the closing line.

### Undo a run

Click **Undo** on a run's closing line or press `Cmd-Option-U` (**Thread → Undo Last Turn**) in the visible conversation. Without a confirmation dialog, the host restores file contents, executable bits, and symlinks from the before-record and switches the line to **Undone** with **Redo**. Steering messages share their run's undo; queued follow-ups get their own.

If any file changed after the run ended, Undo changes nothing and opens **Changed since this turn**, offering **Undo the rest** (restoring only unchanged paths) or **Cancel**. Undo does not restore Git history (`HEAD moved`), other file permissions, extended attributes, hard links, empty directories, or effects outside the checkout. A record marked **After restart** may include edits made while the host was down.

## Linear

- **Sign in:** Open **Settings → Linear** and paste a Linear personal API key (read access is enough to browse). The host verifies the key with Linear, stores it privately (`0600`) on that host, and never returns it to the desktop. Use **Replace key** or **Sign out…** anytime (sign-out removes the key, cached issues, and project links while preserving local thread-to-issue labels).
- **Link a project:** Open **Project settings → Linear** and select a team (Linear caps team listings at 50).
- **Browse and start threads:** Open **Linear issues** from the project's sidebar menu, **File → Linear Issues**, or the command palette. Browse 50 issues at a time (up to 500) with **Active**, **Assigned to me**, **Backlog**, **Completed**, and **All** filters, search by title/number, or paste an exact identifier (such as `PLA-142`) or issue URL. Details show the description and comments as formatted text (headings, lists, code and quotes) with **Open in Linear**, and fall back to the host's cached copy when offline. Links show where they point and are not opened from the issue; images are shown in Linear, not in Plait.
- **Start thread** opens a new draft filled with the issue's identifier, title, URL, and description (up to 4,000 characters) and records a local host link once sent; **Add to thread…** appends the reference to an existing thread's draft. Nothing is sent or written to Linear automatically.
- **Change status:** Choose **Status** in an issue's details to move it to another state of its team. The move happens only when you choose a state, and the host records it before sending it. If the reply is lost, **Try again** returns the saved outcome instead of moving the issue twice. If Linear may have applied it but did not say, Plait says so and does not send it again; check the issue in Linear. A host without durable Linear writes shows the status without the menu.
- Live verification against Linear's API remains open.

## Settings

Press `Cmd-comma` (or **Plait → Settings…**) for application Settings, and open **Project settings…** from any project's sidebar **…** menu, **File → Project Settings…**, or the command palette. Opening Project settings from a sidebar project menu targets that project without switching the active thread underneath. Press `Escape`, **Done**, or click outside to close.

- **Application tabs:** **New threads** (default harness, model, thinking, Pi approval mode), **Providers** (subscriptions, API keys, custom servers), **Linear** (API key), **Machines** (This Mac, remote hosts, and host updates over SSH), **Agents** (install, sign-in, harness paths, setup, default modes), **Keyboard** (record, search, and reset shortcuts; see [SHORTCUTS.md](SHORTCUTS.md)), **Editor** (file and terminal handoffs), **Appearance** (light/dark/follow macOS, **Stillwater Deep** default dark, **Stillwater**, **Plait Dark**, **Plait Light** default light, bundled Base16 schemes, custom `.yaml` Base16 files in `~/Library/Application Support/Plait/themes/`, **Interface font** choosing bundled **Plex Sans** or **System**, monospaced **Code font** defaulting to SF Mono or any installed monospaced family such as `Menlo`, **Code size** from 12 to 14 points defaulting to 12, **Thread tabs**, **Fold earlier turns**, **Fold finished work**, and toggling the Changed files card), and **About** (build revision, host protocol, updates).
- **Project settings:** **Commands** (Trusted vs Sandboxed), Pi skills/templates/instructions and **MCP servers**, **New threads** (busy-project behavior and **Build output** directories), **Linear** team link, and **Worktrees**.

Preferences live in `~/Library/Application Support/Plait/desktop.json`. Custom Base16 themes automatically adjust low-contrast text and accents for readability.

## Recovery and troubleshooting

- **Workspace and draft recovery:** Unsent drafts, image references, and uncertain submission request IDs are saved in `~/Library/Application Support/Plait/session/desktop.json` (private unencrypted JSON; 4 MiB total, 256 KiB per draft). Reopening restores tabs and drafts without sending or replaying anything; **Retry safely** reuses the original request ID so the host never runs a submission twice. If a draft would exceed the 4 MiB journal limit, **Workspace not saved** appears and required saves refuse sending or closing until you shorten or discard drafts. Never delete a damaged journal; close duplicate Plait windows and keep a backup copy first.
- **Host will not connect:** Click **Reconnect** once. On this Mac, inspect the installed host and its last startup record without stopping running work:

```sh
build_id=$(/usr/bin/plutil -extract build_id raw -o - \
  "$HOME/Library/Application Support/Plait/Host/installed.json")
"$HOME/Library/Application Support/Plait/Host/releases/$build_id/bin/plait-host" status
/usr/bin/plutil -p "$HOME/.local/share/plait/startup.json"
```

Do not delete `~/.local/share/plait` to clear an error.

- **Stop unconfirmed:** If Plait cannot confirm that a stopped run's or Git command's processes exited (including after a host restart or a lingering Git hook/credential helper), the checkout header shows **Stop unconfirmed** and blocks new runs and Git writes there. Check for lingering processes using that folder, then click **Stop unconfirmed** and choose **Stop processes** (offered when Plait can verify the surviving Git process group) or **Confirm stopped** after verifying they have exited.
- **Isolated development instances:** Set absolute paths for `PLAIT_HOST_STORE`, `PLAIT_DESKTOP_STATE_DIR`, and `PLAIT_DESKTOP_PREFERENCES_FILE` together so test instances do not share state with your daily install.
