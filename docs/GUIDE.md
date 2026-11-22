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

**Plait → Check for Updates…** (and **Settings → About**) checks GitHub Releases for the newest signed manifest (`update.json`, verified with an embedded Ed25519 public key before trusting versions, sizes, or SHA-256 digests). Plait also checks quietly at launch and shows an **Update** button in the sidebar when a release is available.

**Install and Restart** replaces `/Applications/Plait.app`, installs the new host, restarts it, and reopens Plait with your tabs and drafts intact. Updates refuse to install while any thread is running or awaiting approval; finish or stop active runs first. Independent client and server updates are planned ([Releasing](RELEASING.md#independent-client-and-server-releases)).

On a headless Mac host, run `plait-host update --check` and `plait-host update`. For Linux hosts, see [Remote hosts](REMOTE.md#the-linux-host).

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

The client negotiates host protocol versions **1 through 6** per request and degrades gracefully when connected to an older host, disabling only controls that host does not support.

## Machines

**Settings → Machines** selects which machine's host this window works with: **This Mac**, or another Mac or Linux machine over **SSH** or **Tailscale**. Each window works with one machine at a time; showing projects from several machines in one sidebar is planned ([Host-first Plait](HOST-FIRST.md)).

To add a remote machine, choose **Add machine…**:

- **SSH:** Give it a name, an SSH destination (such as `you@workstation` or an alias from `~/.ssh/config`), and optional port and host store path. Plait runs `ssh -T -o BatchMode=yes … -- <destination> plait-host bridge` using your own SSH keys or agent and never asks for or stores a password. The remote machine needs `plait-host` running (for example via `plait-host run` or its systemd user service).
- **Tailscale (Tailnet):** On a Linux host with `plait-host` running, authorize your login with `plait-host tailnet allow --me` (and `plait-host tailnet allow-node <name>` for tagged devices) and start the listener with `plait-host serve --tailnet` (or `plait-host install-service --tailnet`). In Plait, enter a name, the machine's tailnet host, and port (`7419` by default).

Choose **Use** to connect in the background; if connecting fails, Plait stays on the current machine and reports the error. The window remembers its last machine and reconnects at launch without blocking. **Remove…** forgets a machine locally without affecting its host or threads. Live remote verification against a separate Linux host remains open; see [Remote hosts](REMOTE.md) for transport and Linux service details.

**How the window shows which machine it is on:** The workspace header displays `project / machine · branch` (hover to see the full checkout path). The sidebar footer and Settings header show `<machine> connected`, `Connecting to <machine>…`, or `<machine> disconnected`. The machine name comes from the host's own identity (defaulting to its hostname; **This Mac** locally), falling back to the saved machine name while connecting.

**Each machine's own projects:** Hosts supporting the workspace stream ([HOST.md](HOST.md#workspace)) keep their own opened and hidden projects, worktree ownership, and read/unread state. On this Mac, existing desktop lists import into this Mac's host once. Switching machines shows that machine's projects. Choose **Open repository…** (`Cmd-O`) to pick a folder on this Mac or enter an absolute checkout path on a remote machine.

## Projects, threads, tabs, and drafts

Opening a repository adds it to the sidebar. Selecting a thread switches its conversation, draft, and Git workspace together. Hovering or focusing a project row reveals **+** for a new thread; its **…** menu opens **Project settings…**, **Worktrees**, **Linear issues**, or hides the project (its host threads stay intact; use the eye button to show hidden projects again). Missing folders stay visible as **Missing** with readable history.

Filter the sidebar by search text, **Needs me** (unread activity, pending approvals, failures, interrupted runs), or **Archived**. Rename or archive threads from thread actions or the **Thread** menu; archiving only hides a thread from the default list and never stops an active run. The host streams live updates for the newest 200 threads; use **Refresh** for older threads and folder state.

The bell button toggles alerts (unread markers stay when muted). While Plait is active, alerts for other threads appear as toasts in the upper right (up to three, 8 seconds each). While inactive, a bundled app posts macOS notifications (subject to system permission and Focus).

### Tabs

The 40-point top bar holds the window buttons, sidebar toggle, open tabs (threads, drafts, Worktrees, and Linear Issues), and **+**. When the sidebar is open, tabs align with its right edge; drag empty bar space to move the window. The 36-point second row shows the active checkout context, view switcher (**Conversation**, **Changes**, **History**, **Files**), and Git inspector toggle.

- Switch tabs with `Cmd-1`–`Cmd-8` (`Cmd-9` selects the last), `Cmd-Shift-[` / `Cmd-Shift-]`, or the overflow menu (`Cmd-Shift-Backslash`). Reorder tabs by dragging or with `Control-Shift-Left` / `Control-Shift-Right`.
- Close a tab with `Cmd-W` or middle-click; closing never stops a running thread. Reopen closed tabs (and older nonempty drafts) with `Cmd-Shift-T`. Closing the last tab leaves an empty workspace; pressing `Cmd-W` again (or `Cmd-Shift-W`) closes the window.
- Tab order and per-view bookmarks are stored in `~/Library/Application Support/Plait/tabs.json` (moving per-device tabs to the host is planned under [Host-first Plait](HOST-FIRST.md)).

### Drafts, messaging, and search

`Cmd-N`, `Cmd-T`, and **+** each open a new independent draft tab without creating anything on the host until you send. A draft takes its title from its first nonblank line (up to 120 characters; empty or image-only drafts read **New thread**). Choose its project, harness, model, thinking level, and permissions directly around the message box. **Discard** deletes the draft and closes its tab; `Escape` only closes an empty draft tab.

During a run:

- Press `Enter` (or `Cmd-Enter`) to queue a **Follow-up** for after the run finishes, or `Option-Enter` to **Steer** the running agent at its next step. Queued items appear above the message box with **Cancel** and **Edit**.
- Press **Stop** (or `Escape` twice within 1.5 seconds) to stop the run and cancel that thread's queued inputs.
- Type `@` at a word start to insert a checkout file path, `/` at the start of an empty draft for local commands and Pi skills/templates, and `Up`/`Down` on the first line to recall earlier prompts.
- Context usage appears below the composer when known (marking 75% as high and 90% as nearly full; hover for token counts).
- Press `Cmd-F` to search loaded conversation messages (`Enter`/`Cmd-G` next, `Shift-Enter`/`Cmd-Shift-G` previous, up to 1,000 matches, case-insensitive). Earlier pages load as you scroll to the top or press `Home`/`Cmd-Up`; Find searches loaded messages only.
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

Credentials are stored on the host in `~/.pi/agent/auth.json` (mode `0600`) and custom servers in `~/.pi/agent/models.json`. You can also authenticate via the bundled Pi CLI (`/login`) and reload providers without restarting. Set the default harness, default model, and default thinking level in **Settings → New threads**; change an open thread's model (`Cmd-Shift-M`) or thinking level beneath the composer between runs.

### Agents (Pi, Codex, Claude Code)

- **Pi** (built in): Uses Plait's approval mode and per-project command policy. Pi threads load your user-level `AGENTS.md`, Pi skills, and prompt templates (listed in **Project settings** and offered via `/`). Pi also supports configured stdio and anonymous or header-key HTTP **MCP servers**: toggle servers per project in **Project settings → MCP servers**. Project-defined entries in `.pi/mcp.json` start disabled as **Needs review** and bind to their exact definition (changing the file disables them as **Changed**). MCP browser OAuth and named CLIs are planned.
- **Codex** and **Claude Code:** Configured in **Settings → Agents**. Plait runs your own installed `codex` or `claude` binary (from login `PATH` or a chosen executable path) with its own sign-in, configuration, instructions, and MCP servers.
  - **Claude Code one-time setup:** Plait does not bundle Anthropic's Agent SDK. On first use, Claude Code reads **Needs setup**; click **Set up** to download the single pinned SDK tarball from npm, verify its checksum, and store it beside the host database.

### Approvals and command safety

- **Pi threads:** Choose **Ask each time** or **Automatic** in **Settings → New threads**, or override per thread beneath the composer. In **Ask** mode, every file edit and command waits for your decision (**Allow once** `Cmd-Option-Y`, **Deny once** `Cmd-Option-period`, acting only when the card is visible and focused in conversation). In both modes, Pi's file tools refuse to modify pre-existing dirty files.
  - **Commands policy** (**Project settings → Commands**): **Trusted** (default) runs commands as your user with network and full checkout access. On macOS, **Sandboxed** runs commands offline with read-only checkout access and a private per-thread cache (sandboxed projects always ask approval before MCP tool calls, though an enabled local MCP server still runs as a local process). Linux hosts support trusted commands only; Linux command sandboxing is planned ([LINUX-SANDBOX.md](LINUX-SANDBOX.md)).
- **Codex and Claude Code threads:** Use the harness's own permission modes (defaults in **Settings → Agents**, per-thread under **Permissions**):
  - **Codex:** `Read only`, `Ask`, `Approve for me`, `Full access` (default). Mode changes apply on the next turn.
  - **Claude Code:** `Ask`, `Accept edits`, `Plan`, `Auto` (default), `Bypass`. Mode changes apply immediately.
  - Harness permission requests appear as one-shot Plait approval cards. Plait's pre-existing dirty-file guard does not block Codex or Claude Code edits (approval cards list dirty paths as a warning). If the harness's own configuration overrides the chosen mode, an amber notice beneath the composer says so.

## Changes, History, Files, and Worktrees

Git operations run on the machine that owns the checkout. On a remote machine, that machine's host executes every Git read and write (`git_read` / `git_write`) using its own Git configuration, hooks, and credentials (your Mac's SSH agent is not forwarded). On this Mac, Git currently runs locally and moving local Git and file reads through this Mac's host with a checkout watcher is planned ([Host-first Plait](HOST-FIRST.md)). All Git reads are hardened (no optional locks, external diff, textconv, or filters); explicit writes run under the checkout lease and are refused while a thread is writing there.

- **Changes** (`Cmd-Shift-2`): Stage or unstage files or individual `@@` hunks, **Stage all** / **Unstage all**, or **Mark resolved** on merge conflicts. **Discard…** / **Discard hunk…** (or **Delete…** for untracked files) asks for confirmation and saves the exact bytes in the repository's Git object store so **Undo** can restore them until your next Git action. Type a one-line commit message and press **Commit** (`Cmd-Shift-C`); hooks and filters run as configured. Use **Fetch**, **Push** (or **Publish** when no upstream exists), and fast-forward **Pull** (no merge, rebase, amend, or force push). Click the branch name (`Cmd-Shift-B`) to switch or create local branches. Click or Shift-click diff line numbers and choose **Ask about this** to append a quoted line reference to your draft. Above the composer, a toggleable **Changed files** card tracks uncommitted files edited by the thread's file tools.
- **History** (`Cmd-Shift-3`): Lists commits on the current branch, newest first, 100 at a time, with message and first-parent patch.
- **Files** (`Cmd-P`): Read-only syntax-highlighted viewer rooted strictly inside the checkout (no symlinks; files over 8 MiB, binary, or non-UTF-8 are described instead). Supports **Find** (`Cmd-F`, up to 1,000 matches) and reloads changed files on focus, status refresh, or tab return.
- **External handoffs** (this Mac only): **Open in Editor** (`Cmd-Option-E`) opens the active file in Files or Changes at its current/picked line; **Show in Finder** reveals it; **Open Terminal Here** (`Cmd-Option-T`) opens the checkout folder. Configure them in **Settings → Editor** using macOS's default app/Terminal or a single-line custom command with `{file}`, `{line}`, and `{project}` placeholders. Custom commands do not run through a shell (`$`, `~`, and globs stay literal), so pass full executable paths for binaries outside `/usr/bin` and `/bin`.

### Worktrees

Open a project's **Worktrees** tab from its sidebar menu, **Project settings**, the command palette, or a worktree thread's header. The graph draws the main checkout's branch as the trunk and each worktree as a lane (capped at 300 commits), showing thread state, uncommitted counts, publication state as of the last fetch, and logical source and build-output sizes (walks stop at 50,000 entries or 250 ms and show **over** when clipped).

Host-owned worktrees are created on `plait/<uuid-prefix>` branches under `<main-repo>.worktrees/` from a captured `HEAD`. Nothing is removed automatically: select a lane and choose **Remove** to confirm (**Remove if pushed**, or **Keep local commits only** when only unpushed/unknown upstream state blocks removal). Active work, uncommitted changes, unapproved ignored files, and detached-HEAD commits block removal. Branches, thread history, and private caches remain.

**Build output seeding** (macOS only; Linux worktrees start cold): In **Project settings → New threads → Build output**, approve reproducible ignored directories (for example `target` or `node_modules`) to clone copy-on-write from an idle main checkout seed into new worktrees. Approve only reproducible output you are willing to remove with a worktree (everything inside an approved directory is cloned and excluded from removal blocks). Choose **Skip** during creation to start cold.

## Turn records, review, and undo

With a protocol 5+ host, the host snapshots your checkout before and after every run—across all harnesses and permission modes, including files written by commands—into a private Git store kept for 30 days. If the before-record fails, the run does not start.

- **Review a run:** Click any filename or the step count on a finished run's closing line to open **Changes** scoped to that turn's saved before/after diff (**Recorded**), even for files later committed, edited, or deleted. Switch between **Recorded** and **Current** to compare against your live working tree, or open the scope picker to choose **All changes**, **Original** (the run's changes), or **Restored** (what an undo put back). Staging by turn and "since last reviewed" baselines are planned.

### Undo a run

Click **Undo** on a run's closing line or press `Cmd-Option-U` (**Thread → Undo Last Turn**) in the visible conversation. Without a confirmation dialog, the host restores file contents, executable bits, and symlinks from the before-record and switches the line to **Undone** with **Redo**. Steering messages share their run's undo; queued follow-ups get their own.

If any file changed after the run ended, Undo changes nothing and opens **Changed since this turn**, offering **Undo the rest** (restoring only unchanged paths) or **Cancel**. Undo does not restore Git history (`HEAD moved`), other file permissions, extended attributes, hard links, empty directories, or effects outside the checkout. A record marked **After restart** may include edits made while the host was down.

## Linear

- **Sign in:** Open **Settings → Linear** and paste a Linear personal API key (read access is enough to browse). The host verifies the key with Linear, stores it privately (`0600`) on that host, and never returns it to the desktop. Use **Replace key** or **Sign out…** anytime (sign-out removes the key, cached issues, and project links while preserving local thread-to-issue labels).
- **Link a project:** Open **Project settings → Linear** and select a team (Linear caps team listings at 50).
- **Browse and start threads:** Open **Linear issues** from the project's sidebar menu, **File → Linear Issues**, or the command palette. Browse 50 issues at a time (up to 500) with **Active**, **Assigned to me**, **Backlog**, **Completed**, and **All** filters, search by title/number, or paste an exact identifier (such as `PLA-142`) or issue URL. Details display as plain text with **Open in Linear** and fall back to the host's cached copy when offline.
- **Start thread** opens a new draft filled with the issue's identifier, title, URL, and description (up to 4,000 characters) and records a local host link once sent; **Add to thread…** appends the reference to an existing thread's draft. Nothing is sent or written to Linear automatically. Live verification against Linear's API remains open.

## Settings

Press `Cmd-comma` (or **Plait → Settings…**) for application Settings, and open **Project settings…** from any project's sidebar **…** menu, **File → Project Settings…**, or the command palette. Opening Project settings from a sidebar project menu targets that project without switching the active thread underneath. Press `Escape`, **Done**, or click outside to close.

- **Application tabs:** **New threads** (default harness, model, thinking, Pi approval mode), **Providers** (subscriptions, API keys, custom servers), **Linear** (API key), **Machines** (This Mac and remote hosts), **Agents** (harness paths, setup, default modes), **Keyboard** (record, search, and reset shortcuts; see [SHORTCUTS.md](SHORTCUTS.md)), **Editor** (file and terminal handoffs), **Appearance** (light/dark/follow macOS, **Stillwater** default dark, **Plait Dark**, **Plait Light** default light, bundled Base16 schemes, custom `.yaml` Base16 files in `~/Library/Application Support/Plait/themes/`, monospaced **Code font** defaulting to SF Mono or any installed monospaced family such as `Menlo`, and toggling the Changed files card), and **About** (build revision, host protocol, updates).
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
