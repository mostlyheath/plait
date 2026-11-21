# macOS installation and independent updates

Plait is an early local preview for **Apple Silicon and macOS 15 or later**.
The client and host are separate programs. Closing or updating the client does
not stop accepted host work. Model requests require access to the configured
provider; installing Plait does not include a model or offline inference engine.
The bundle declares macOS 15, but packaging CI uses macOS 26. Older-system
runtime acceptance is not yet complete.

## Download and install

Published previews live on the [GitHub releases page](https://github.com/heath0xFF/plait/releases).
Download the **DMG**, open it, and drag **Plait** into **Applications**. Open
it from Applications: on first launch it sets up its local server in the
per-user layout used by in-app updates. No source checkout, Xcode, Rust,
separately installed Node, or Terminal command is required.

For a command-line installation or update, download and inspect the installer:

```sh
curl --proto '=https' -fL \
  https://raw.githubusercontent.com/heath0xFF/plait/main/scripts/install.sh \
  -o install-plait.sh
less install-plait.sh
bash install-plait.sh --allow-unsigned
```

The short form is:

```sh
curl --proto '=https' -fsSL https://raw.githubusercontent.com/heath0xFF/plait/main/scripts/install.sh | bash -s -- --allow-unsigned
```

Run it as your normal user, not under `sudo`. Only migrating an older
system-wide installation requires administrator access, once. The flag accepts
the unsigned preview; the script does not change Gatekeeper or remove quarantine
attributes. Finish or Stop active tasks before installing. Threads, drafts, and
credentials are preserved; the app reopens after installation.

The script uses the newest published release, including previews.
`--version v0.1.0-preview.7` pins a release. `--download ~/Downloads` verifies and
saves the app archive without installing it. Initial downloads are checked
against the release's SHA-256 metadata over HTTPS; this detects corruption, not
publisher identity. Later in-app updates verify signed release manifests.

`--component all`, `client`, and `host` retain the older system-wide package
installation paths for manual component management. Those packages do not
create an installation eligible for in-app updates.

## Packages

`bash scripts/build-macos.sh` produces:

| Artifact | Purpose |
| --- | --- |
| `Plait-…-macos-arm64.dmg` | Plait.app beside an Applications shortcut, the system-wide migration command, and instructions |
| `Plait-Installer-…-macos-arm64.pkg` | Install both components; Customize can select either one |
| `Plait-Client-…-macos-arm64.pkg` | Install or update only `/Applications/Plait.app` |
| `Plait-Host-…-macos-arm64.pkg` | Install or stage only the host, Node, and pinned runtime |
| `Plait-…-macos-arm64.zip` | App bundle with its host archive and prebuilt-install script |
| `Plait-Release-….json` and `.sha256` files | Component metadata and artifact checksums |

**These are unsigned preview installers, not notarized releases.** The client
and Rust host have ad-hoc signatures. The bundled Node binary retains its
upstream signature. A checksum detects corruption; it does not prove publisher
identity. Check the source and artifact origin before using macOS's explicit
per-app or per-installer approval. Do not disable Gatekeeper. CI installation
does not test the downloaded-file approval flow on your Mac. If macOS blocks the
installer, review its origin, then use the explicit **Open Anyway** option in
System Settings → Privacy & Security if offered. If machine policy forbids it,
stop; this preview provides no security-policy bypass.

## First installation

1. Do not start two hosts against the same store. First launch never stops or
   replaces a host that is already running, so a manually started development
   host keeps serving.
2. Drag **Plait** from the DMG into **Applications**, then open it from
   Applications. The first time, macOS asks you to approve the unsigned preview
   in System Settings → Privacy & Security → **Open Anyway**.
3. With no server on this Mac, Plait sets one up: it installs its bundled host
   under `~/Library/Application Support/Plait/Host` and registers it as your
   LaunchAgent. The new thread view says so while it works, and shows **Retry**
   if it fails. Then use **Open repository…** (Cmd-O), and **Settings**
   (Cmd-comma) to choose an authenticated model.
4. Start a real thread. Settings → New threads selects automatic approval or
   asking for each action. Whether a project's commands are sandboxed or
   trusted is that project's own choice, in its settings (the project's **…**
   menu in the sidebar → **Project settings…**). A Codex or Claude Code thread
   uses that harness's own permissions instead, chosen with **Permissions**
   under a new or open thread's message box, with a default for each harness
   in Settings → Agents. The app shows only real threads.

### What first launch sets up

First launch only installs a server that is missing. It runs in the
background, one launch at a time, and leaves alone:

- a host launchd already runs, of any version, including one with active work;
- a host that answers on the default socket, such as one you started by hand;
- an installed release that is not running: Plait registers that release, not
  its own bundled copy, so a newer host is never downgraded.

It does nothing for a source build or when `PLAIT_HOST_STORE` selects a
development store. Opened from the mounted DMG or from Downloads (where macOS
runs a quarantined app from a temporary copy), Plait asks you to drag it to
Applications first, so no service points at a disk that goes away. An earlier
system-wide package install needs **Move from system-wide install.command**
once; first launch does not remove root-owned files or ask for an
administrator password. It does not write through a symbolic link at the
host folder or LaunchAgents folder.

Replacing a host is not part of first launch: that is
[Check for Updates](#updates), which waits until no run is active.

The host is a **per-user LaunchAgent**, not a root daemon. It starts for the
current graphical user when possible and at later graphical logins. If no
user is logged in, startup waits until login. Service startup resolves the home
from the OS account and rejects root execution; it does not use the installer's
environment. It uses the same default store as
the development host: `~/.local/share/plait`. A development store override is
not copied into the installed agent.

The package includes Node **26.12.0**, the Rust host, runtime source, and locked
production npm dependencies. It needs no Homebrew or global Node installation.
It does **not** include model credentials, user data, project dependencies,
Rust, Git, Xcode, or a general terminal. Keep the development tools needed by
your project installed. Sandboxed Cargo builds remain offline and need cached
sources and an existing lockfile. Full Xcode and its Metal component are needed
for native Plait builds.

### Authentication and status

The host owns provider accounts. Sign-ins and API keys are kept in Pi's
credential file, `~/.pi/agent/auth.json`, readable only by your account. Custom
OpenAI-compatible servers are entries in `~/.pi/agent/models.json`. A login or
API key set only in an interactive shell is not automatically available to a
LaunchAgent. Do not put credentials into the LaunchAgent property list or
the package.

Locate the installed host with native macOS tools:

```sh
build_id=$(/usr/bin/plutil -extract build_id raw -o - \
  "$HOME/Library/Application Support/Plait/Host/installed.json")
host="$HOME/Library/Application Support/Plait/Host/releases/$build_id"
"$host/bin/plait-host" version
"$host/bin/plait-host" status
```

#### Signing in from Plait

Source builds after v0.1.0-preview.2 provide **Settings → Providers**. The host
must support the [provider operations](HOST.md#provider-sign-in); an older host
shows an update requirement. These controls have scripted and scene coverage;
live provider sign-in acceptance remains open.

- **Subscription** (for example Claude Pro or Max, or ChatGPT Plus or Pro): choose
  the provider and sign in. Use **Open in browser** after checking the displayed destination. The
  browser normally returns to the host by itself; if it cannot, paste the code
  or the final address into Plait. Some providers show a short code to type on
  their page instead.
- **API key** (for example OpenRouter): paste the key once. Plait sends it to the
  host and does not keep or show it again. Replace it by entering a new one.
- **Your own server:** give an OpenAI-compatible server a name, base URL,
  optional key, and model IDs. **Check connection** lists the models it reports.
- **Sign out** removes the stored sign-in or key for one provider.

Closing the provider form cancels a pending sign-in. API keys and sign-in answers
are masked, have a reveal control, and are cleared when sent or closed. They are
not copied into desktop preferences, the recovery journal, accessibility values,
or clipboard copy. A connection check consumes an entered key without saving it;
paste it again before saving a new endpoint. Endpoint removal requires confirmation.

**Commands** in a project's settings changes that checkout between Sandboxed
and Trusted. Open the page from the project's **…** menu in the sidebar
(**Project settings…**), or with **File → Project Settings…** for the active
project; a project that is not active becomes the active one first. The page
also lists the Pi skills, prompt templates, and instructions its threads are
offered. Trusted requires confirmation of full user access, network,
Git writes, and the ability to change already-modified files. It affects the next
run; an active run retains its original policy. The approval default is independent.

**MCP servers** on the project's page lists the servers configured for Pi,
with an On/Off switch saved for this checkout. User-level servers default on in
trusted projects and off in sandboxed ones. A server defined or replaced by the
project's `.pi/mcp.json` always starts off as **Needs review**. Read that file,
then turn the server on here. The saved choice is bound to the unresolved
definition; a changed entry is off again and says **Changed**.

Hover shows the defining file and executable with an argument count, or only
the URL host for HTTP. Argument values, env/header values, and credential
commands are hidden. Read the file before opting in; **Open file** opens a
project definition or override in the Files view in one action. Files outside
the checkout show their paths only. A stale enable request is rejected, so
Refresh and read the changed file before enabling it again. Invalid entries
and Pi's disabled MCP extension must be fixed in Pi first.

Project overrides initially say **User limits**: they can disable a server,
hide tools, or defer them, but cannot enable a user-disabled server or expose
user-hidden tools. To permit an override after reading it, turn the server on
explicitly here (off then on if it is already on). That choice has the same
definition binding.

Pi threads now find and call tools from enabled stdio servers and HTTP servers
that need no sign-in or a configured header key. The first tool search connects
them; each call gets its own `mcp:server/tool` row. Servers stop with the run.
Trusted projects follow the thread's approval mode. A sandboxed project always
asks **Use <tool> from <server>**, even in Auto mode, because MCP permits network
access while keeping the command filesystem sandbox.
An enabled local server can read the project and use the network from startup,
before any call is approved, including when sandboxed. Approval gates calls;
it does not gate the server's own behaviour. Nothing from an unapproved project
definition runs, including credential commands and variable expansion.

Refresh shows real connection state and the last observed tool count after a run
has connected a server. Failures appear in Problems with a redacted stderr tail.
A known OAuth server says **Sign in with Pi** or **Sign-in saved**; using saved
OAuth credentials and provider-backed authentication is still planned. An
uncontacted HTTP server is not assumed to need sign-in. Older hosts that do not
report MCP servers show no section. These controls apply to Pi; Codex and Claude
Code keep their own MCP configuration.

The desktop never receives a stored credential. Changes apply to new threads and
new messages at once. A run already in progress continues with the providers it
started with. Existing threads retain their exact saved model; changing the
default affects only new threads.

#### Pi CLI fallback

The host payload includes the pinned Pi CLI:

```sh
"$host/bin/node" \
  "$host/runtime/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js"
```

Use its `/login` command, then exit. Do not enter coding tasks into that CLI if
you intend them to use Plait's approval and sandbox rules.

The v0.1.0-preview.2 host reads credentials only when it starts. Finish any host
work, then restart the host session; the simplest restart is to log out and
back in. A host built from later source can load them without a restart:

```sh
printf '{"op":"reload_providers"}\n' | /usr/bin/nc -U "$HOME/.local/share/plait/host.sock"
```

### If the host does not connect

Use **Reconnect** once. With the installed `$host` path from
[Authentication and status](#authentication-and-status), check the service and its
last startup record. These checks do not stop a run or change saved work:

```sh
"$host/bin/plait-host" status
/usr/bin/plutil -p "$HOME/.local/share/plait/startup.json"
```

The startup record contains phases, elapsed times, process IDs, and OS error
kinds—not credentials or prompts. It describes the last startup attempt, not
current health; a past `ready` phase does not prove that the host is still running.
The `version` command identifies the binary you invoke. After a staged update,
that can be newer than the active host.

Do not delete the host store or force a restart to clear an error. Follow the
[independent update and activation rules](#updates). When reporting a
problem, include the macOS version, build, final startup phase, and error kind.
Do not include credentials, private conversations, or a complete environment dump.

## Workspace recovery

Plait restores the last checkout and selected thread. It also saves draft text
and uncertain submission IDs. Reopening never sends or replays a task. **Retry
safely** uses the original request ID. An acknowledgement cannot clear a draft
that you edited after sending. Settings shows authenticated models first; **Show
all models** includes unavailable choices without making them selectable.

The Mac recovery journal is
`~/Library/Application Support/Plait/session/desktop.json`. It is private local
JSON, not encrypted by Plait, and can contain sensitive prompt text. Normal
window close and Cmd-Q wait for a save. Force quit and power loss can lose the
last unsaved keystrokes. Accepted work remains in the independent host store.
The journal holds at most 4 MiB of serialized recovery data and 256 KiB per
draft. Independent drafts have a 10,000-record decoding bound. The legacy
thread and project-draft fields retain their 512-record bound for older builds. If it would exceed a limit, the
previous good file stays intact and current work stays in memory. Sending and
required close or tab-switch saves report the failure and keep the work open.
A draft that would take the journal over 4 MiB stays in memory with **Workspace
not saved** and the reason. Required saves refuse sending, opening or
switching tabs, or closing until it fits; the previous file is unchanged. Shorten or explicitly
discard drafts to free space. Force quit can lose the unsaved changes. Twenty
drafts of the maximum size are not a supported capacity.

Builds before independent drafts (v0.1.0-preview.19 and earlier) show one New
thread draft per project: the first one opened there. While the journal has
room, every other draft with text, images or an unanswered first send is also
saved in a form those builds keep, image files included, without showing it.
An unanswered first send keeps its request, so retrying it later reaches the
thread the host may already have made. The one draft an older build shows does
not carry its unanswered send there; sending it in that build starts a new
request. Opening this build again
returns those drafts as tabs at the end. If an older build saved the journal
in between, their model, harness and permission choices return to the defaults. The copies are left out when they would take
the journal over 4 MiB, or past 384 saved threads and project drafts, which
leaves older builds room under their own 512-record bound. Drafts left out in
that way are lost if an older build saves the journal.

If recovery reports an error, do not delete or reset the journal. Close duplicate
Plait clients first. Keep a copy of the file before repair; malformed or unknown
versions are left intact. A conflicting saved checkout disables sending
for that thread rather than moving an old draft or request to a new identity.
Changing a thread’s model does not discard its draft or saved selection.
For development with a different `PLAIT_HOST_STORE`, use a separate absolute
`PLAIT_DESKTOP_STATE_DIR`. Reconnect does not erase local state or restart work.

For a prepared development checkout and a first source-change task, see
[Build Plait with Plait](SELF-HOSTING.md).

## Projects and threads

Source builds after v0.1.0-preview.2 list projects across the local host's checkouts.
Opening a folder adds it to the sidebar. Selecting a thread in another project
switches its conversation, draft, and Git workspace together. The **+** on a
project row, shown on hover or keyboard focus, opens a new thread there.
A thread that runs in a harness other than Pi shows that harness's logo beside
its title; under an open thread's message box, the logo names its harness, with
the version on hover. The local machine
appears in workspace context without a redundant row beneath every project.
Missing folders remain visible as **Missing**; their saved conversations can still
be read.

Use the sidebar search to filter titles and project names. **Needs me** includes
unread activity, pending approvals, failures, and interrupted runs. **Archived**
includes archived conversations. Thread actions and the **Thread** menu offer
rename and archive/unarchive. Archiving keeps the conversation and does not stop
an active run. Removing a project from the sidebar keeps its host threads;
the eye button (**Show hidden projects**) beside the filters, or a project entry
in the command palette, brings it back.

The host keeps its newest 200 threads live. When more are loaded, the sidebar
states the limit; use the refresh button (**Refresh threads and folders**) for older threads and
folder availability. A disconnected thread-list watch reports that updates are
paused. Hosts using protocol 1 or 2 retain their one-checkout thread listing and
journal titles; rename and archive require version 3.

The bell button beside the filters (struck through when off) controls alerts:
in-app toasts, system notifications, and window attention requests. Unread
markers remain when alerts are off. Alerts occur for a thread you are not
viewing, or while Plait is inactive, when it finishes, fails, stops, is
interrupted, or needs approval.

While Plait is the active app, an alert is a toast in the window's upper right
instead of a system notification. At most three show at once; a newer alert
for the same thread replaces its older one, and each disappears after eight
seconds. Choose a toast to open its thread, or its **×** to dismiss it; opening
the thread any other way also clears it. While Plait is in the background,
alerts are system notifications, and opening one selects its thread. Quitting
Plait stops alerts, not host work; reopening shows unread markers without
replaying alerts. A bundled Mac app
requests notification authorization on first delivery; an unbundled build cannot
post macOS notifications, and delivery depends on system permission and Focus
settings. The initial listing never posts alerts. Native delivery and activation
remain manual acceptance checks.

### Starting a thread while a project is busy

A checkout runs one thread at a time. Sending a new draft asks the host where
it can start. If the project already has a thread running, **Project busy**
offers **Own worktree**, **Wait**, and **Cancel**.

- **Own worktree** creates a new branch and checkout from HEAD, then sends the
  draft there with its chosen harness, model, and permissions. The draft stays
  editable while the choice is open; what is sent is the text and images shown
  when you choose. Uncommitted files and build output are not copied.
- **Wait** sends the current message and images to the durable checkout queue,
  keeping the chosen harness, model and permissions. It starts in order when
  that checkout is free. Wait is always an explicit choice for a new thread;
  an older host leaves it unavailable.
- **Cancel** keeps the draft.

Another draft can send while the first is preparing its worktree, and drafts
can be closed or moved to another project independently. A thread that
finishes being created never takes the front from a tab you have moved to.

A creation with no answer keeps its request, and so does the saved draft:
after closing the window or quitting while a worktree is being prepared,
sending the reopened draft reaches the thread and worktree the host already
made instead of making a second. A refused creation keeps the draft. Pending
first messages and placement choices are saved with the draft. A restart never
sends them by itself, and **Retry** reuses the same creation ID and captured
message.

If a checkout's cleanup is unconfirmed, the host refuses to start a thread
there with **Stop unconfirmed** and the recorded reason. Resolve that checkout
through its cleanup controls first; the automatic worktree setting does not
bypass the hold.

Project settings → New threads → **When this project is busy** is **Ask** by
default. **Own worktree** makes the worktree automatic for new threads in that
project only. The finished checkout's path and branch are shown, and removing
it stays manual. Cmd-Option-N still creates a worktree thread directly, whether
or not the project is busy.

A thread already bound to a busy checkout offers **Wait** beside **Continue
separately**. Wait sends its draft to the same checkout's queue.
Confirming **Own worktree** there creates a linked thread from HEAD with a new
harness session. The original history and draft stay where they are, and
**Original thread** opens the source. No conversation is replayed or moved. A
disconnected or older host does not offer choices it cannot carry out.

A queued thread says **Waiting** in its tab, sidebar row and above its message
box. Hover shows its place in line and that it waits for the checkout. **Cancel**
removes its first waiting message and leaves the rest in order. If that message
just started, Cancel requests Stop, which also clears that thread's remaining
queued input as usual. A host restart changes
not-started messages to **Held**. **Resume** releases the checkout's held Wait
messages in order; interrupted runs are never replayed. Another thread's held
**Follow-up** messages stay held until that thread's next send. Closing a tab or the desktop
leaves a running host's queue alone. Messages and images remain with their own
thread or pending draft through a lost reply and an explicit retry.

Changing a queued thread's model, thinking, harness permission mode or command policy makes
that waiter fail with a reason when its turn comes. A waiting message that is
cancelled or fails before it starts comes back: its text and images return to the
thread's draft when that draft is empty, or its text is copied to the clipboard
when the draft has other text. Send it again with the choices you intend. The next message's approval choice does not change an already
accepted message's approval. A removed or replaced checkout or an unavailable
harness or model also cannot start it; a failed waiter does not strand the rest
of the line. An unconfirmed cleanup blocks the whole checkout, so it holds the
line instead: the first waiting thread shows the cleanup reason, and after the
cleanup is confirmed, **Resume** continues in order. A checkout holds at most 32 pending messages and 32 MiB of
queued text and images, including held messages and follow-ups. A full queue
refuses the extra send plainly and keeps its draft.

Checkouts that share no files have no default limit on how many run at once.
Before a run starts, the host records the checkout; on one filesystem those
recordings take turns. The selected thread says it is waiting, and Stop
cancels the wait before the harness starts. When the machine runs out of a
resource, the run fails with an explicit error and waits for a deliberate
retry. Only messages explicitly sent with Wait join the checkout queue.

### Worktrees

Choose **Worktrees** from a project's sidebar menu, the command palette,
Project settings, or the worktree line above a thread. One Worktrees tab opens
per project per window; opening it again brings that tab forward. Its branch
mark distinguishes it from thread tabs. Closing it leaves all checkouts intact.
Tab order, selected lane and lane sorting return after restart.

The graph shows the main checkout's current branch as a trunk and each worktree
as a lane. Commit rows show hash, subject and age. Head labels show the owning
thread and its tab state, branch, preservation facts and size. **Merged** means
the head is reachable from the trunk and contains commits beyond its starting
commit. **No new commits** means the head equals or predates that starting commit,
as when a worktree is fresh or reset onto an older commit. **Fork unknown** means the recorded fork
cannot be connected in the available history; **Detached** instead describes
a checkout without a Git branch. Hollow nodes count uncommitted
changes. History is capped at 300 commits; a clipped graph says **Older commits
not shown**. External worktrees are labelled plainly and cannot be removed here.

Select a lane with the pointer or Tab and Return/Space. Its detail appears beside
the graph in wide windows and below in narrow windows. The selected lane is
drawn stronger from its head to its fork; hover gives a quieter preview. Hover
the shortened path to read it in full. **Open** brings its thread
forward. **Finder** shows the checkout folder; **Terminal** uses the terminal
chosen in Settings. **Remove** is available only when the host's current checks
permit at least one confirmation choice. Hover explains a refusal in the host's
words. Removal asks once, shows the known counts and retained branch, and offers
**Remove if pushed**, **Keep local commits only**, or **Cancel**. Only choices
the host permits are offered. **Keep local commits only** explicitly accepts
unpushed or unknown publication status, and is offered only when that status
is what blocks removal; other safety checks still apply.
The view then reports the result. A later refusal is preserved;
a lost reply says unconfirmed. Use **Remove worktree** again on that checkout,
or open its project's **Worktrees** tab and choose **Refresh**, to recover the same request.
Branches, thread history and private build caches remain. Removed records retain
cache accounting and say **Removed**. Nothing is removed automatically.

**Last used** and **Size** sort lanes and show the current order, keeping the main checkout first. Last used
is recorded thread activity, not a filesystem access time. **Refresh** rereads
state and measures again. When a thread's run ends, its checkout is read again
by itself. **Remove** waits while a run works in that checkout. Measurements run only while this tab is visible,
without polling, and are cached until the head, directory identity, recorded
activity or change count changes. Each walk stops after 50,000 entries or a
250 ms deadline checked between entries; a blocked filesystem call can finish
later. A stopped walk says **over** its measured lower bound.

Sizes are logical file bytes, not a promise of reclaimed disk space. Source
excludes Git metadata and top-level `target`, `build`, `dist`, `.next` and
`node_modules`; those directories and the thread's private cache count as build
output. Symlinks are not followed, a walk stays on its checkout's disk and does
not enter another checkout nested inside it, and hard links within a measurement
count once. Copy-on-write sharing and caches shared across checkouts cannot be
apportioned reliably. The neutral split bar compares the selected checkout to
the largest measured checkout; the header totals the available measurements.

An empty project explains **Own worktree** in the busy-project choice and
**New Thread in Worktree**. An older or disconnected host explains what is
needed. Missing folders stay visible and cannot be removed through this view.
Publication facts reflect the last fetch. Work returns through commit, push
and pull request.

### Codex and Claude Code threads

Source builds from `main` with a matching host can run a thread in Codex or
Claude Code instead of Pi. Neither is in a release yet. Each runs your own
install with its own sign-in, settings, instructions, and tools; Plait never
sees their credentials. **Settings → Agents** lists each harness with its state,
version, path, and what protects your checkout. **Choose…** runs a particular
executable, and **Use PATH** returns to the one your login shell finds.
**Refresh** checks again after you install, update, or sign in.

**Setting up Claude Code threads.** Install Claude Code and sign in with
`claude` in a terminal as usual. A Claude subscription, an API key, or a cloud
provider all work. Plait drives `claude` through Anthropic's Agent SDK, which
Plait's downloads do not include, so the Claude Code entry first reads **Needs
setup**. Choose **Set up**. The host downloads that one pinned SDK version from
the npm registry, checks it against a pinned checksum before writing anything,
and keeps it beside its own data. This is the only step that reaches the
network for a harness, and it reads none of Claude Code's files. While it runs
the button reads **Setting up…**. If it fails, the entry stays in **Needs
setup** and says why; choose **Set up** again. Afterwards the entry reads like
Codex's: ready, or what to fix (not installed, too old, or signed out).

**Permissions.** These threads use the harness's own permission modes in place
of Plait's Ask and Auto and the project's command policy. Claude Code offers
Ask, Accept edits, Plan, Auto (the default), and Bypass. Codex offers Read only,
Ask, Approve for me, and Full access (the default). Set each harness's default
for new threads in Settings → Agents, or choose **Permissions** under the
message box, in a new thread or an open one. The control names the mode; hold
the pointer over it to read what the mode allows and what protects the
checkout. In Claude Code a change applies at once, even during a run; in Codex
it applies from the next turn. If the harness's own settings run a different
mode, an amber line under the controls says so. Each request the harness asks you about is an approval card, allowed or
denied once. Plait's guard on files you had already changed does not apply
here: the card names those files, but nothing stops the harness writing them.

**Default harness for new threads** in Settings → New threads chooses where Cmd-N
starts. A harness that is not ready is shown dimmed and cannot be chosen; a
draft whose default is not ready keeps that harness and asks you to choose an available one. What has been checked against
real installs is in [Verification](VERIFICATION.md#claude-code).

## Images in messages

Paste an image with Cmd-V, or drop PNG or JPEG files onto the message box.
Each attachment has a thumbnail and **Remove**. Several images can accompany
text, or be sent on their own. Activate a conversation thumbnail for a larger
view; **Close** or Escape returns to the conversation. Draft images survive
closing the window, and sent images survive a host restart. New thread drafts
stay with their checkout when switching projects. Discard saves immediately.
Removing an image or having its message accepted releases the desktop original
once no draft or pending submission needs it. The host keeps images that entered
the conversation for the life of its store; archiving a thread does not reclaim
them. Cancelling a queued message
releases its images if no other accepted message uses them; unused uploads
expire after 24 hours.

Both desktop and host allow four different images, at most 5 MiB each and
5 MiB combined, up to 8,192 pixels on either side and 16,777,216 pixels total.
The bytes determine PNG or JPEG, not the file extension. **Refused** explains
the reason on hover. Plait never shrinks or recompresses the original to make
it fit. Choose a smaller file yourself if needed. The refusal clears on the
next text edit. **Failed** means that attachment's original could not be
restored; hover for the reason and remove it before sending. Other attachments
keep their thumbnails and can be sent once the failed ones are removed. **Uploading**
keeps Send disabled until the host replies, and pending rows show their image
count. Paste and drop leave a disabled message box unchanged.

**Text only** means the selected model reports that it cannot accept images;
remove them or choose another model. **Update host** means images need a
protocol 6 host. Neither state silently drops an attachment. Claude Code's
model catalog cannot report image support, so sending is allowed and any
harness refusal appears as the run's error.

The pinned GPUI clipboard reader takes PNG first, then JPEG, when there is no
text. TIFF-only images are refused rather than converted. When the clipboard
offers both text and an image, text is pasted and the image is not attached.
Drop the image file onto the message box, or copy the image alone, to attach it.
Copied file paths keep their ordinary text-paste behavior. The clipboard read
itself is synchronous; header validation follows immediately. Hashing and
thumbnail preparation run in the background.

Sent originals live with the thread in the private host store, never in the
repository or an Undo record. Draft originals live in the desktop's private
session directory. Archive does not erase host originals; Remove releases
unreferenced desktop draft originals. Removing the respective state store
removes its originals. [The host contract](HOST.md#limits-and-retention)
gives the paths and upload expiry rules. [Verification](VERIFICATION.md#images-in-messages)
distinguishes offscreen and scripted checks from installed-app acceptance.

## Appearance and themes

**Settings → Appearance → Light or dark** follows macOS between light and dark by
default, or keeps Light or Dark. Choose a theme for each. The dark list starts
with **Stillwater**, the default: a quiet indigo-slate theme mapped by hand.
**Plait Dark** follows it, a neutral graphite alternative. The light list starts
with **Plait Light**, the default. Both lists continue with shipped Base16 schemes such as Catppuccin, Gruvbox, Nord, Rosé Pine,
Solarized, and Tokyo Night. A choice applies at once and is saved in the
desktop preferences.

**Code font** sets the font for code, file paths, and commands. Left empty, it
is the system's monospaced font, SF Mono. Type the name of an installed
monospaced font family, such as `JetBrains Mono`; matching installed names are
offered as you type, and the font changes as soon as the name matches one. A
name that is not installed, or a font whose characters differ in width, changes
nothing, and the line beneath the field says so. Fonts installed while Plait is
running are found the next time Settings opens.

More themes come from Base16 scheme files: put `.yaml` files (for example from
[tinted-theming/schemes](https://github.com/tinted-theming/schemes)) in
`~/Library/Application Support/Plait/themes/`, then choose **Reload**. **Open
themes folder** creates and shows it. Each file appears in the light or dark list
by its own `variant`; a file that cannot be read is named beneath the lists with
the reason. Plait keeps every theme's text readable: if a scheme's text, grays,
or accents are too faint against its background, they are moved toward its text
color until they meet a minimum contrast, so a few schemes look slightly crisper
in Plait than elsewhere. A theme removed from the folder falls back to Plait's
own.

## Review a run

Recorded review is built in the current source. Select a finished run's
filename to read the before/after change saved by that run.
Select its step count or **N more** for the complete file list. A run with no
net file changes has a **Changes** link and says **No recorded changes**.
Committed, clean and deleted files remain in this list while the record is
retained. This view stays on the same record after later runs and undo.

The scope below the branch names the run by its request, such as
**Turn · Document the behavior…**. Without loaded request text it gives the
age, such as **Turn · 5m ago**. Hover and accessible text keep the full request,
age and thread; record IDs are never shown. Open it to choose **All changes**
or another retained record from this thread. Picker rows say **Original** for
files recorded during the run and **Restored** for files an undo put back.
Descriptions give the request, age and thread, with newest records first;
several undos of one run differ by age and order. An undo's closing line opens
the files that undo actually restored; choose **Original** in the scope picker
to see the run before undo. A partial undo's Restored list can be smaller than
Original.

**Recorded** compares saved bytes and offers no staging or discard controls.
**Current** shows the same file's present Git changes, with the usual Staged
and Unstaged sides; it selects a side with changes. **Reading diff…** remains
visible while current status loads; only a known clean file says **No current
changes**. The comparison controls appear once a recorded file is selected.
Neither comparison marks a file reviewed. The commit box still commits the
current index. Origin labels, staging by turn, and review baselines are not built
yet. Recorded changes describe an observed interval, not who wrote the bytes.

**Loading** means the host read is pending. **Unavailable** explains a missing
record or saved content; **Retry** requests it again. **Offline** can retain an
already loaded diff, marked stale. With an older host, **Older host** says
“Turn review needs a newer host”; **Current** returns to ordinary Git review.
Closing-line file links open current Changes, including the first click before
host support is known. Their hover text explains this fallback once the older
host is detected. Supported undo still works. Records remain
subject to the host's 30-day retention.

Refresh is event-driven: entering Changes, selecting a file or record, window
activation, checkout run-state changes, a completed local Git write, or
**Refresh** (**Cmd-R**). There is no periodic review probe.

## Undo a run

With a version 5 host, each finished run ends with its tool-step count and the
files the host recorded changing, including command writes. File links open
the exact recorded change in Changes when the host supports turn review; the
adjacent counts are lines added and removed. Three names fit on one line, with
an **N more** link to all recorded files. Hover previews up to 12 files and
12 not-covered paths, with omitted counts.

Press **Undo** at the line's right end, or **Cmd-Option-U** (Thread → Undo Last
Turn). The latest run always shows its control; earlier controls appear on
hover or keyboard focus. There is no confirmation: the line changes to
**Undone**, lists the files restored, and offers **Redo**. Steering messages
share a run's control; a queued follow-up is its own run. The shortcut acts
only in the visible conversation, with no settings modal or palette over it.
It follows the latest line's Undo or Redo state; Cmd-Z remains text undo.

If a path differs from the run's after record, the default operation changes
nothing and opens **Changed since this turn** at that line. **Undo the rest**
leaves those paths alone and restores only eligible files; **Cancel** closes
the panel. A partial undo lists only the files actually restored. The panel
previews up to 12 changed and 12 not-covered paths and labels omitted counts. Hover text
lists **Not covered** paths, with reasons; these also appear in the panel.

**Recording** means the record is still being taken; **Unavailable** means it
cannot be used; **Wait** means this checkout has an active run or write.
**After restart** warns that capture may include changes made while the host
was down. **HEAD moved** warns that Git history is outside undo's scope.
Hover gives details. Older hosts show no control and explain that undo needs
a newer host. Refresh (Cmd-R) rereads records. If an undo reply is lost, Retry
checks the same request without applying it twice; Incomplete asks you to
inspect Changes and never automatically replays the operation. A failure that
is known to have changed no files keeps the original run available; Retry
starts a new attempt against that run.

If the host cannot record the checkout before a run, that run does not start.
The desktop reports a fixed failure message; the underlying recording reason
is not retained in a user-readable log.

Undo restores file content, executable bits, and symlinks. It does not restore
other permissions, extended attributes, hard-link relationships, empty-directory
history, Git history, or effects outside the checkout. The host checks for
later changes, but an external editor racing the final checks can still lose
a change, and failures after writes begin can be partial. Read the explicit
[verification limits](VERIFICATION.md#turn-undo-desktop-controls).

## Reading messages

Your messages have a **You** label and a neutral background panel. Model
replies have a **Plait** label and stay on the open conversation surface,
including while they stream. Both use regular-weight body text. The rings,
turn lines, closing nodes, and run controls remain beside the conversation.

Quoted lines in your message have muted text and a thin inset rule. Their
outer `>` marks are hidden up to the request block limit; empty `>` lines and
other request syntax stay as written. **Copy message** copies the original source,
including those marks. Select text to
copy only what is drawn. Copy controls appear in the author line on hover or
keyboard focus.

## Keyboard controls

When focus is in the conversation, Home or Cmd-Up goes to the beginning of the
loaded history and requests an earlier page when one is available. End or Cmd-Down
returns to the latest messages. Text fields keep their usual editing shortcuts.
Version 3 history also loads as you scroll toward the top; use **Load earlier**
for an explicit page request and **Retry** after a failed read. Older hosts retain
bounded previews. Completed reasoning expands from its **Reasoning** line.

**Find** (Cmd-F) over the conversation opens the same bar as in Files: it
highlights every match in the requests and replies that are loaded and counts
them ("3 of 17", or "No matches"). Enter or Cmd-G moves to the next match,
Shift-Enter or Cmd-Shift-G to the previous, wrapping at the ends, and the list
scrolls to each one. Escape closes the bar and puts focus back where it was. A
new search starts at the first match from the top of the view, or at the
newest when the conversation is at its end. While the bar is open, a reply
that arrives is searched too, without moving the list. Find ignores letter
case, including outside ASCII, and matches plain text, not patterns. It counts
up to 1,000 matches, keeping the newest. It searches the text as drawn, not
the Markdown behind it, and does not search the reply still being written,
reasoning, tool calls and their output, or a turn's closing line. Only loaded
messages are searched; when earlier messages, or the rest of a long one, are
not loaded, the bar says **Loaded messages only**, with the reason on hover.
Find does not load pages to search them.


### Settings and Project settings

Settings opens as a centered modal over the window from Cmd-comma,
**Plait → Settings…**, the sidebar's Settings control, or the command palette.
**Project Settings…** opens from a project's sidebar menu, the File menu, or
its command-palette action. A sidebar project menu targets that project without
switching the thread underneath. Host status and section navigation stay above
the scrolling Settings content; the project name and checkout identify Project
settings. Both use an 820-point width where it fits, shrinking to leave 24
points on each side; height leaves at least 32 points above and below.

**Done**, **Escape**, or a click outside closes either modal and returns focus
to the previous control. Cmd-comma closes application Settings, or switches
Project settings to application Settings. Escape first closes an open picker,
cancels shortcut recording, closes an open provider form, or keeps the current
command policy at the trust confirmation. Outside clicks
are ignored during shortcut recording. Tab and Shift-Tab cycle inside the
modal, and workspace shortcuts are inactive until it closes. Assistive
technology reaches only the modal and a picker opened over it. The covered view,
scroll position, selection, find and draft remain in place; a running thread
continues receiving replies. Settings changes keep their existing immediate
save and confirmation behavior.

### Application shortcuts

These are the default shortcuts for application actions. The table is generated
from the action table in `crates/plait/src/ui/actions.rs`, and a test fails when
the two differ. The ID is the action's stable name. **Settings → Keyboard**
records shortcuts, searches actions, clears a binding, and restores defaults.
Conflicts name the affected actions and require **Reassign**. Text-editing and
common system shortcuts are refused. Escape cancels recording; the key being
recorded never runs its normal action. The nine sidebar jumps change together.
Menus keep Settings and Quit reachable even when their shortcuts are cleared.

Overrides live in `~/Library/Application Support/Plait/desktop.json` under
`keymap`, for example `"keymap": {"thread.new": "cmd-t", "app.quit": ""}`.
An empty string unbinds; an invalid keystroke keeps the default. Unknown action
IDs survive saves. Menus and palette hints update immediately. For an isolated
development instance, set `PLAIT_DESKTOP_PREFERENCES_FILE` to an absolute file
path alongside separate `PLAIT_HOST_STORE` and `PLAIT_DESKTOP_STATE_DIR` paths.

| Shortcut | Action | ID |
| --- | --- | --- |
| Cmd-comma | Settings… | `app.settings` |
| Cmd-Q | Quit Plait | `app.quit` |
| Cmd-T | New Tab | `tab.new` |
| Cmd-N | New Thread | `thread.new` |
| Cmd-Shift-N | New Thread with Model… | `thread.new_with_model` |
| Cmd-Option-N | New Thread in Worktree | `thread.new_worktree` |
| Cmd-Shift-M | Change Thread Model… | `thread.switch_model` |
| Cmd-O | Open Repository… | `repository.open` |
| Cmd-P | Open File… | `file.open` |
| Cmd-R | Refresh Git Status | `git.refresh` |
| Cmd-Option-E | Open in Editor | `file.open_in_editor` |
| Cmd-Option-T | Open Terminal Here | `project.open_terminal` |
| Cmd-W | Close Tab | `tab.close` |
| Cmd-Shift-T | Reopen Closed Tab | `tab.reopen` |
| Cmd-Shift-W | Close Window | `window.close` |
| Cmd-K | Command Palette… | `palette.toggle` |
| Cmd-B | Toggle Sidebar | `view.toggle_sidebar` |
| Cmd-L | Toggle Git Inspector | `view.toggle_inspector` |
| Cmd-Shift-D | Toggle Diagnostics | `view.toggle_diagnostics` |
| Cmd-Shift-1 | Conversation | `view.conversation` |
| Cmd-Shift-2 | Changes | `view.changes` |
| Cmd-Shift-3 | History | `view.history` |
| Cmd-F | Find | `file.find` |
| Cmd-G | Find Next | `file.find_next` |
| Cmd-Shift-G | Find Previous | `file.find_previous` |
| Cmd-Option-Down | Next Thread | `thread.next` |
| Cmd-Option-Up | Previous Thread | `thread.previous` |
| Option-Down | Next Changed File | `git.next_file` |
| Option-Up | Previous Changed File | `git.previous_file` |
| Cmd-Option-U | Undo Last Turn | `thread.undo_last_turn` |
| Cmd-Option-period | Deny Once | `approval.deny_once` |
| Cmd-Option-Y | Allow Once | `approval.allow_once` |
| Cmd-Shift-C | Commit Staged Changes… | `git.commit` |
| Cmd-Shift-B | Switch Branch… | `git.switch_branch` |
| Cmd-M | Minimize | `window.minimize` |
| Cmd-Shift-] | Next Tab | `tab.next` |
| Cmd-Shift-[ | Previous Tab | `tab.previous` |
| Control-Shift-Right | Move Tab Right | `tab.move_right` |
| Control-Shift-Left | Move Tab Left | `tab.move_left` |
| Cmd-Shift-Backslash | Show All Tabs… | `tab.list` |
| Cmd-1 to Cmd-9 | Switch to Tab 1–9 | `tab_jump.1` to `tab_jump.9` |
| Cmd-Option-1 to Cmd-Option-9 | Jump to Sidebar Item 1–9 | `jump.1` to `jump.9` |
| Tab | Focus Next Control | `focus.next` |
| Shift-Tab | Focus Previous Control | `focus.previous` |

Cmd-N opens a new thread in the active project with the cursor in its message
box. Nothing is created on the host until you send the first message; the
thread is then created and the message submitted to it. If creation fails, the
text stays with the reason, and an unconfirmed creation is retried with the
same request ID, so it cannot make a second thread. Above the message box,
choose the **project** (a filterable list of sidebar projects, most recently
active first, and **Open repository…**), the **model** (only the chosen
harness's models), the **harness** (Pi, and each harness the host lists; one
that is not ready is listed with its state), and **Permissions** (Pi: Ask or
Auto, defaulting to Settings → New threads; another harness: its own modes,
defaulting to its Settings → Agents default). Changing the harness returns the
model and permissions to that harness's defaults: Pi's saved default model, or
the harness's most recently used model, else its first. Switching project keeps
the new thread open and shows that repository in the inspector. While it is
open, a **New thread** row marks where it will land in the sidebar. Opening
another thread sets it aside in its own tab with its text. **Discard** removes
it and closes that tab (see Tabs); Escape closes the tab only when the draft
holds nothing. Draft text and image references are saved per checkout
in the desktop recovery journal across restarts. Cmd-Shift-N opens it with the
model list open. A thread with no title
yet reads **New thread**. **New Thread in Worktree** (Cmd-Option-N) asks the
host to create a checkout and a thread at once, with the draft's selected
harness/model/permissions, or the configured defaults when no draft is open.
It uses the captured committed base, leaving dirty files in the original
checkout. Only approved build output can be cloned into the new checkout. The
branch is `plait/` plus twelve UUID hex digits,
extended if needed, under `<main-repository>.worktrees/` even when started from
a linked worktree. An unavailable chosen harness is refused, never replaced.

You can change tabs while creation finishes; completion appears in the sidebar
without taking focus. An unconfirmed request is retried with Cmd-Option-N using
its original identity, from the same project; in another project Cmd-Option-N
starts a new request. Closing the window leaves accepted creation on the host;
reopening recovers its thread and grouping. A failed checkout reports what
happened and preserves existing files and branches for inspection.

**Remove worktree** asks first. **Remove if pushed** requires a known remote
upstream containing the current commit, as of the last fetch. **Keep local
commits only** explicitly permits unpushed or unknown publication status while
keeping the branch. Both refuse active work, changed directory identity,
uncommitted files, ignored content outside approved build directories, edits
hidden by assume-unchanged or
skip-worktree flags, commits on no branch (made on a detached HEAD in the
worktree) and unfinished Git operations. Old worktrees
created by the desktop remain grouped as imported records and must be removed
with your Git tools. There is no automatic cleanup or force removal.

In **Project settings → New threads → Build output**, choose **Approve** beside
a suggestion or enter a relative directory and choose **Approve**. Suggestions
come from manifests, without running scripts; none is applied automatically.
**Remove** revokes a directory and **Refresh** rereads the host's choices.
Nothing is cloned before approval. An older host shows **Unavailable**.
**Seed · 2.4 GiB** (for example) is the logical size of the copy new worktrees
are cloned from; the Worktrees view's total includes it. Its blocks are shared
with the checkout until either changes, so it is not space that removing it
would free, but once the checkout is rebuilt the seed keeps the old blocks.
Removing every approved directory removes the seed.
Approve only reproducible output you are willing to remove with its worktree.
An approval covers everything inside the directory: any file placed there, a
`.env` included, is cloned into each new worktree and removed with it.
Ignored files inside the current approved directories are **Build output** in
the Worktrees facts and do not block removal. A `.env`, unapproved ignored cache
or other ignored user data still blocks it. Removing a directory from the
approved list makes its existing ignored content block removal again.

During creation, **Cloning build output** shows the file count; **Skip** discards
partial output and proceeds cold. The Worktrees view reports **Warm** when output
was cloned, or **Cold** with the reason on hover, along with time, file count and
skipped entries. Warm means a starting cache, not a guarantee that a build is
already current. Sizes are logical bytes: shared blocks may remain after one
worktree is removed.

The project's main checkout, when quiet, supplies a seed while Plait holds a
lease; a linked worktree never does. After a run, a seed older than an hour is
checked against the source by file metadata alone and cloned again only if
something changed; inside the hour nothing runs. Approving or removing a
directory refreshes the seed at once, and **Refresh** refreshes it whatever its
age. A new run interrupts a refresh promptly. Creating a worktree uses the seed
that is there, however old, and only waits for a fresh one when there is none. No approval, no suitable idle
seed, missing output, a changed approval, an unsupported filesystem or platform,
a different volume, insufficient space or interrupted preparation starts cold.
There is no disk-consuming copy fallback. Builds run in a separate terminal do
not participate in Plait's leases and can still race a seed capture.

Git data, nested repositories and Python virtual environments are refused.
Unapproved ignored directories are never copied. Safe package-manager `.bin`
links are retained; links leaving the approved directory, absolute or unresolved
links, sockets, devices and immutable or append-only files are skipped. Recreate other caches that embed absolute
paths. Sandboxed Pi's Cargo output is seeded in its private thread cache; trusted
projects, Codex and Claude Code use checkout output. Removing a checkout still
retains its separately attributed private cache. Worktrees remain optional and
cleanup remains manual.

Cmd-Option-1 to Cmd-Option-9
count threads in sidebar order; Settings can change this to machines or
projects. Cmd-B hides or shows the sidebar on the left; Cmd-L hides or shows the
Git inspector on the right, and that choice is remembered. On a window too narrow
for the inspector it explains where Git is instead. Drag either panel's inner
edge to resize it; the width is remembered, and double-clicking the edge restores
the default. Both shortcuts can be changed in Settings → Keyboard.

### Tabs

The 40-point top row holds the native window buttons, sidebar toggle, thread,
draft and Worktrees tabs, and New tab control. It stays visible with the sidebar
hidden. While the sidebar is open, the first tab starts at the sidebar's right
edge, so the tabs sit above the content they switch. Drag empty bar space on
either side of the tabs to move the window; double-clicking it follows the
system's title-bar preference. The 36-point second row holds the active
thread's project, branch, four views, and inspector toggle. An empty workspace
has no second row.
Tab order, the tab in front, and each view's bookmark are saved separately in
`~/Library/Application Support/Plait/tabs.json`. Saving is debounced; closing
normally waits for it. Launch waits for verified host and checkout identities
before restoring. A damaged or unsupported file gives a valid empty workspace;
future versions are left untouched. A host that cannot yet provide its identity
still supports tabs for the current session without an extra prompt.

Clicking a sidebar thread opens it at the end or switches to its existing tab.
New tab (+), Cmd-T, Cmd-N, the sidebar's New thread, and a project's New thread
control always append a fresh, empty draft and bring it to the front, even when
the current draft is empty. Each tab keeps its own text, images, project, model,
harness, permissions, thinking level, and Project busy choice. Sending creates
a thread in that tab; other drafts stay as they are. A draft takes its title
from the first nonblank line of its message, trimmed, with each run of spaces
or tabs shown as one space, and limited to 120 characters. Empty and images-only
drafts are called **New thread**. The same title appears in its tab, the overflow
list, and the sidebar's current draft row. Connected drafts with a known model
say **Ready**, including tabs behind the one in front; disconnected tabs say **Offline**. Cmd-1 through Cmd-8 select
tabs by position; Cmd-9 selects the last. Cmd-Shift-[ and Cmd-Shift-] cycle
through tabs; Control-Shift-Left and Control-Shift-Right move the active tab.
Drag a tab to reorder it. The overflow control opens all tabs grouped by state;
large lists draw only the visible rows. Opening tabs has no count ceiling.
Restore reads at most 10,000 entries and 16 MiB of tab preferences to bound damaged
input. All saved open drafts, including empty ones, return in their saved order;
recovered drafts without tab records append after them. Draft content still uses
the separate 4 MiB recovery journal and its visible save-failure rules.

A project’s **Worktrees** command opens its own tab, or selects it if it is
already open. That tab is always called **Worktrees** and shows the project’s
branches, worktrees and storage instead of a thread’s four views. It can be
reordered, closed and reopened like a thread tab. Closing or discarding a draft
beside it can bring Worktrees to the front; the workspace is empty only when no
tabs remain.

Cmd-W or middle-click closes a tab without stopping its run. Cmd-Shift-T reopens
the most recently closed tab, including a nonempty draft or pending creation.
Empty drafts close immediately. The last twenty closes retain their original
position and view for this session. Older nonempty drafts remain in the private
journal: after that history is exhausted, Cmd-Shift-T reopens those drafts at the
end with their content and choices. They also return after restart. Closing the last tab
leaves an empty workspace; another Cmd-W closes the window. Cmd-Shift-W closes
the window directly.

**Discard** removes that draft from recovery and closes its tab. Its right
neighbour comes to the front, or its left neighbour if it was last; discarding
the only tab leaves an empty workspace. Cmd-Shift-T cannot bring a discarded
draft back. Other drafts are kept. Discard first cancels any unanswered
**Project busy** choice. It is also available while creation is unconfirmed: a
creation already sent can finish on the host, but its reply does not open a tab
or send the discarded first message. While a creation that belongs to no draft
is unanswered, Discard is refused with a notice and the draft is kept. Escape in
a draft that holds nothing closes its tab as Cmd-W does; in a draft with text,
images, a **Project busy** choice or an unanswered first send, it does nothing,
so a reflexive key press cannot throw typed work away.

Cmd-Option-period answers the open thread's approval card with **Deny once**
and Cmd-Option-Y with **Allow once**. Each button shows its shortcut as the
keymap binds it. The keys act only when the card is on screen and its buttons
would: in the open thread's conversation, with no palette or picker open over
it, the host connected, no other control request in flight, the approval not
yet answered, and its exact arguments shown. Otherwise they do nothing. A held
key answers one approval at most; release it before answering the next. Both
are in the **Thread** menu and the command palette, and can be changed in
Settings → Keyboard.

Above the message box, a card lists the files the thread wrote that are not
committed yet, with lines added and removed against HEAD. It updates as the
thread's edits finish and as Git status refreshes. Click its header to fold it,
or a file to see that file in Changes. Settings → Appearance, or **Toggle Changed
Files** in the View menu, hides it. Files changed only by commands are not
listed, and the card needs a host from this release or later.

### Command palette and model picker

Cmd-K opens the command palette: threads in the open checkout, every menu
command with its shortcut, and "New thread with" each signed-in model. The model
picker is the same list over the host's model catalog.

| Key | Action |
| --- | --- |
| Type | Filter. Words match in any order, in the title or the detail |
| Up / Down | Move the selection; it wraps at the ends |
| Page Up / Page Down | Move a page |
| Return | Run the selected item |
| Cmd-Return | Model picker: save the selected model as the default |
| Cmd-D | Model picker: add or remove a favorite |
| Escape | Close and return focus to where it was |

### Working during a run

With a version 3 host, Enter queues a follow-up and Option-Enter sends steering.
Steering joins the current run at its next step and uses that run's approval
mode; if it is not taken in time, it becomes a follow-up. Stop stays beside the
send control and clears the host queue as well as stopping the run.

The stack above the composer lists this thread's **Follow-up** and **Steering**
messages in order. These wait on its own run, distinct from **Waiting** for the
checkout. Follow-ups and checkout waiters share acceptance order, so a chain of
follow-ups cannot jump ahead of another thread's waiter. Cancel removes
one; Edit cancels it back into an empty draft. Edit is offered for messages sent by this desktop, whose complete text is
saved locally; the host only sends a bounded preview. If the
draft changes while cancellation is in flight, the cancelled text is copied to
the clipboard and the draft stays intact. Unconfirmed messages each offer Retry
safely, using their original request ID and approval mode. Reopening sends
nothing. A held queue resumes from its oldest input on the next send.

Choose the model beneath the composer, or Cmd-Shift-M, to change this thread's
next run. The adjacent thinking control lists exactly the chosen model's
supported levels. Both are locked during a run: they stay readable, and hover
says when they can be changed. In a Pi thread, changing the approval mode
during a run applies from your next message; until then an amber line says
which mode the run is under. An empty message box names its two triggers, `@`
for files and `/` for commands; a line of key hints appears under the draft
only during a run or when sending is off. New threads start at **Settings → New threads → Default
thinking for new threads** (Medium unless changed), or the closest level their
model offers; a model without reasoning starts with thinking off.

The run-settings menu beside them chooses this thread's approval mode: **Use
default** follows Settings → New threads, or pick **Ask before actions** or
**Auto-approve actions** for this thread alone. The choice is saved with the
thread's draft and applies from its next message; it does not change an active
run or previously queued messages. The button and the menu name only this
thread's approval mode. Whether commands are sandboxed is shared by every
thread in the checkout and is set in the project's settings.

Context usage shows the latest response, not cumulative cost. Hover or activate
it for exact token counts. At 75% it says high; at 90% it says nearly full. It
stays hidden when context usage or the window size is unknown. Older hosts keep
their existing send-when-idle behavior and have no per-thread model controls.

Providers send replies in uneven bursts. Rather than drawing each burst as it
lands, the conversation shows streamed text and thinking flowing in at the
display's own frame rate, trailing what has arrived by about a tenth of a
second; a larger burst drains faster. It redraws every display frame only while
text is catching up, and stops when it has. Opening a thread shows what has
already arrived at once.

### Text and controls

| Key | Action |
| --- | --- |
| Enter / Shift-Enter | Send, or queue a follow-up during a run / insert a line break |
| Cmd-Enter | Send or queue a follow-up, even with a suggestion list open |
| Option-Enter | Steer the running agent at its next step (version 3 host) |
| Up / Down in the draft | On the first line, Up recalls earlier messages sent in this thread; Down returns toward the draft |
| @ | List checkout files at the start of a word; type to filter |
| / | List local commands when it is the first character of an empty draft |
| Up / Down, Enter or Tab, Escape | With a suggestion list open: choose, accept, dismiss |
| Escape | Close the suggestion list, then leave message recall; then, in a new-thread draft that holds nothing, close its tab |
| Escape twice | Stop the running agent, when pressed twice within 1.5 seconds |
| Option-Delete / Option-Fn-Delete | Delete the previous / next word |
| Cmd-Delete / Cmd-Fn-Delete | Delete to the start / end of the line |
| Option-Left / Right | Move by word; add Shift to select |
| Cmd-Left / Right | Move to the start / end of the line; add Shift to select |
| Cmd-A / Cmd-C / Cmd-X / Cmd-V | Select all / copy / cut / paste |
| Cmd-Z / Cmd-Shift-Z | Undo / redo |
| Return / Space | Activate the focused control |

Search fields take one line: pasted line breaks become spaces.

Scroll the conversation, navigation, file list, and diff with the trackpad or
mouse. Long diff lines scroll horizontally. On narrow windows, use Changes to
open Git; the optional conversation inspector is hidden.

### Staging, commits, and history

In **Changes**, hover a file and choose **Stage** or **Unstage**, or use **Stage
file** in the diff header. To take part of a file, hover a hunk's `@@` line and
choose **Stage hunk** (or **Unstage hunk** in the Staged view). If the file
changed since its diff was shown, Plait refuses, rereads it, and asks you to
try again. New, renamed, and conflicted files, and files that pass through a
Git filter such as LFS, stage whole.

**Discard…** in the diff header (Unstaged view) throws away a file's unstaged
changes and returns it to its staged or committed version; staged work is
kept. For an untracked file it reads **Delete…**. Hover a hunk for **Discard
hunk…** to drop just that part. Each asks first in a macOS alert. Before
discarding, Plait saves the file's exact bytes in the repository's object
store, and **Undo** beside the result puts them back until your next Git
action. Conflicted files cannot be discarded here. **Stage all** and **Unstage all** sit above the commit
message. Type a one-line message and press Return or **Commit** (Cmd-Shift-C
moves to the message). Only staged changes are committed. Plait runs your
repository's own Git configuration for these: clean filters such as LFS run when
staging, and commit hooks run when committing; a failing hook's output is shown
and nothing is committed. One write runs at a time.

Every Git write Plait makes (staging and unstaging, by file or hunk, committing,
discarding and restoring a discard, creating or switching a branch, pushing,
publishing, fetching, pulling, undoing or redoing a turn, and removing a
worktree) takes the checkout's lease off the UI thread, the same lease a run
holds; a symlink or another host store does not get around it. While a thread
is running in the checkout, Changes says so and such a write is **refused at
once**: "This checkout is in use by an active run or another Plait write. Wait
until it finishes, then try again." appears under the commit box (in the
sidebar for removing a worktree), and the files, the index and `HEAD` are left
as they were. The other way round, a message sent while one of these writes is
still running (a slow push, say) is refused, saying the checkout is busy with
another Plait operation. Writes that change what linked worktrees of one repository
share (commit, push, publish, fetch, pull, creating or switching a branch, adding
or removing a worktree) wait up to five seconds for another worktree's such write
to finish, then are refused with a message that says so. Creating a worktree
from a checkout where a thread is running is allowed; it is refused only while
a Plait Git write runs there. Repositories never wait for each other. Plait
does not know when a harness has stopped yet: if Codex or Claude Code keeps
running after its run has ended, a write here is not refused.

During a merge, an unmerged file offers **Mark resolved** instead of **Stage**; Commit
stays unavailable until every conflict is marked resolved. If a hook or filter
takes longer than its limit (60 seconds for staging, 120 for a commit), Plait
stops it and says so.

If Plait cannot confirm a run's processes stopped, its checkout stays blocked
for new runs and Git changes. The header shows **Stop unconfirmed**. Hover it
to see the checkout and what Plait could not confirm. The state clears itself
when the missing confirmation arrives. This does not affect other checkouts.

If it remains, check for commands, servers, or builds still using that folder
and stop them. Click **Stop unconfirmed**, then **Confirm stopped** only when
you have checked. This acknowledges the uncertainty; it does not kill a process
or bypass a still-held checkout lease. A host restart during a run can require
this check. A Git change whose hook or credential helper keeps running in
Git's process group (Git's credential cache, for example) keeps the checkout
blocked until that process exits; the state then clears by itself. If a Git command survived the desktop and Plait can still verify
its identity, the second choice is **Stop processes**. Hover it to see the Git
command, its process group, and the checkout. Plait stops only that verified
group and keeps the checkout blocked if its exit remains unconfirmed; when
identity cannot be proved, it offers **Confirm stopped** and sends no signal. Interrupted work is never replayed, and queued inputs remain held.
Trusted commands and external harnesses may deliberately detach a process that
Plait cannot see; the [host guide](HOST.md#cleanup-confirmation-and-quarantine)
describes that boundary.

To ask a thread about specific lines, click a line number in the diff, and
Shift-click another to extend the pick. **Ask about this** puts a reference such
as `src/main.rs:10-12` and the picked lines, as a fenced diff, at the end of the
selected thread's message and opens the conversation. Nothing is sent until you
send it. **Clear** drops the pick; choosing another file or side ignores it.

Above Stage all, Changes shows how the branch stands against its upstream (for
example "2 to push") as of the last fetch, with **Fetch** and **Push**. A branch
with no upstream offers **Publish** instead, which pushes it to `origin` (or
`remote.pushDefault`, or the only remote) and tracks it there. When the branch
is only behind, the button reads **Pull**: Plait fetches and moves the branch
forward, never merging or rebasing, and refuses if that would overwrite
uncommitted changes. When both sides have new commits, Push and Pull are
unavailable; merge or rebase in Terminal. Sign-in uses your existing Git setup: SSH keys, credential
helpers, and, for HTTPS GitHub remotes, `gh auth login` if the GitHub CLI is
installed. Plait never stores or reads the token. Push, Pull, and Fetch are
also in the **Git** menu.

The branch name at the top of Changes, and of the Git inspector, opens the
branch list (**Switch Branch…**, Cmd-Shift-B, in the **Git** menu). It lists the
checkout's local branches, most recently committed first, with the current one
marked; a branch checked out in another worktree says which. Choose one to
switch to it. Type a name that is not a branch and the first row reads
**Create branch** *name*: it makes that branch at the current commit and
switches to it. While what you type is part of a listed name, Enter takes the
best match; **New Branch…** in the Git menu always starts on Create. Uncommitted
changes come along as Git carries them. If switching would overwrite them, Git
refuses and its message is the result line under the commit box; commit,
stash, or discard them and try again. Git checks a new name first (no spaces,
no `..`, no leading dash), and an existing name is refused rather than reused.
The list shows up to 500 branches and says so when there are more. The branch
name is locked, in hint color, while another Git action runs and while a thread
is running in this checkout, since switching would change the files it is
editing; hover says why. Plait acts on the checkout that is open, which may be
a thread's worktree. There is no force, no remote-tracking checkout, and no
branch deletion or renaming.

**History** (Cmd-Shift-3) lists commits on the current branch, newest first, 100
at a time. Select one to see its message and patch against its first parent.
There is no merge, rebase, or amend yet; use your own Git tools for those.

### Files

**Files** shows one checkout file, read-only, with line numbers and the same
syntax colors as the conversation. Open one with **Open File…** (Cmd-P), which
searches the checkout's files by name and folder, or with **Open** in a Changes
diff header. **Find** (Cmd-F) highlights every match and counts them;
Enter or Cmd-G moves to the next, Shift-Enter or Cmd-Shift-G to the previous,
wrapping at the ends, and Escape closes it; opening it again searches the same
text again. As in the conversation, Find ignores letter case, including
outside ASCII, and it counts up to 1,000 matches.

Plait rereads an open file when it may have changed: after a Git status read,
when the window comes forward, and when you return to Files. It compares size
and modification time first, so an unchanged file is not reread, and it never
polls. A file changed on disk is redrawn with **Updated from disk**; one that was
deleted keeps its last version on screen with a note. Binary files, files over
8 MB, and text that is not UTF-8 are described instead of drawn. Files open
inside the checkout only, and never through a symbolic link. There is no editing.

### Editor, Finder, and terminal

Plait hands off to the tools you already use. **Open in Editor**
(Cmd-Option-E) opens the file on screen at its line: in Files, the current find
match, or else the first line on screen; in Changes, the selected file at the
line you picked, or else at its diff's first change. **Show in Finder** reveals
the same file, and **Open Terminal Here** (Cmd-Option-T) opens the open
project's checkout. All three are in the File menu and the command palette;
the Files header has **Editor** and **Finder**, and a Changes diff header has
**Editor**. Hover over a control to see what it will open, at which line, and
with what. A file deleted from the checkout has nothing to open.

**Settings → Editor** chooses what runs:

- **Open files with**: **System default** (the app macOS uses for the file's
  type, without a line), **Neovim in Rex** (`nvim +line -- file` in a new Rex
  tab at the checkout), or **Custom**.
- **Open terminals with**: **Terminal**, **Rex tab** (a Rex tab at the
  checkout), or **Custom**.

A custom command is one line, such as `zed {file}:{line}` or
`/opt/homebrew/bin/wezterm start --cwd {project}`. `{file}` is the file's full
path, `{line}` its line, and `{project}` the checkout's path; a placeholder can
sit inside a word. Single quotes, double quotes, and backslashes group and
escape words as in a shell, but the command is not run through a shell, so `$`,
`~`, globs, and backticks are plain text. A file command needs `{file}`; a
terminal command can use only `{project}`. A command that cannot be used is not
saved, and the line beneath the field says why. Plait does not read your
shell's `PATH`, so name a program outside `/usr/bin` and `/bin` by its full
path. The choices are kept in `desktop.json`.

The Rex choices talk to Rex through AppleScript, addressed by its bundle ID, so
Rex and Rex Beta both answer. The first time, macOS asks whether Plait may
control Rex; the answer can be changed in System Settings → Privacy & Security →
Automation, and until it is allowed, using a Rex choice says so. Plait's builds
are signed ad hoc, so macOS may ask again after an update. Plait does not need
Rex: without it the choice stays, and using it says
that Rex is not installed. Plait starts the command and does not wait for it or
read its output. Only a command that cannot start, or that exits with an error
within two seconds, is reported, in one sentence above the workspace.

## Updates

### In the app

**Plait → Check for Updates…** (also **Settings → About**) looks for the newest
signed release on GitHub Releases, including previews. Plait also checks quietly
at launch; when a release is available, an **Update** button appears beside
Settings in the sidebar.

**Install and Restart** downloads the release, verifies it, replaces
`/Applications/Plait.app`, installs that release's host, restarts the host, and
reopens Plait. It refuses while any thread is running or awaiting approval,
because a host restart would interrupt that run. Finish or stop the run first.
Saved drafts and the workspace layout are kept, as on a normal quit.

Each release publishes `update.json` and a detached Ed25519 signature. Plait
checks the signature against the key built into the app before it trusts the
version or any artifact, then checks each download's signed size and SHA-256.
An edited manifest cannot make an older signed build look newer. The app's code
signature remains ad hoc; downloads are not notarized.

In-app updates need a per-user install: an app bundle you own and a host under
`~/Library/Application Support/Plait/Host`. An install made by the published
Installer packages is root-owned. Open **Move from system-wide
install.command** from the latest DMG, or use the command-line installer above,
to migrate once. It asks
for administrator access only for that migration; no source build is needed. Release builds compare versions; a source build is offered the
newest release.

### Headless hosts

On a Mac that runs only a host, `plait-host update --check` reports whether a
newer signed release exists, and `plait-host update` installs its host archive
with the same checks and the same refusal while runs are active. Linux hosts do
not have published builds yet.

### From a source checkout

Contributors can install their working tree as this Mac's Plait:

```sh
scripts/update-installed-macos.sh
```

It builds the app (uncommitted changes included), quits it, replaces
`/Applications/Plait.app`, and reopens it. The app carries its host; the script
unpacks it into `~/Library/Application Support/Plait/Host/releases/` only when
its content changed and runs it from a per-user LaunchAgent. No administrator
access is needed, except once to remove an earlier system-wide package install;
threads, drafts, and credentials stay where they are.

A new host replaces the running one only while no run is active or awaiting
approval; otherwise it is staged. Activate it later with `--activate-host`, or
keep the current host with `--stage-host`. After activation, releases other than
the running one are removed. **Settings → About** shows the running desktop
revision, build time, and host protocol. Builds are ad-hoc signed and not
notarized.

This per-user install trades one protection for convenience: the system-wide
package keeps host code owned by root, so processes running as you cannot change
it. Commands under the default [trusted command mode](HOST.md#command-policy)
run as you and can write there; a project set to sandboxed commands cannot.

### Installer packages

The published Installer packages predate in-app updates and install system-wide:

- **Client:** quit only the client and run a client-only package. The host and
  accepted work remain running. Reopen the client to reconnect.
- **Host:** run a newer host-only package. It installs a content-addressed release
  directory and updates the LaunchAgent file. Each host release has a distinct
  package receipt, so Installer does not remove the previous release's files.
  The LaunchAgent label stays `dev.plait.host`. Installation does not stop the active host.
  Finish active threads, or Stop them and let command cleanup finish. Then log
  out and back in to activate the update. Shutdown during cleanup leaves an
  **interrupted** thread for inspection; it does not replay the accepted request.
- Installing the **same exact host build again is rejected before payload writes**.
  This prevents Installer from rewriting code used by a running process. Use the
  client-only package if only the client needs installation.
- A manually started host is not adopted or killed. Finish its work and stop it
  before activating the installed agent. A store lock rejects duplicate writers.

The v0.1.0-preview.2 client and host use protocol version **1**. Later source
builds negotiate opt-in [version 3](HOST.md#protocol-version-3), then
[version 2](HOST.md#protocol-version-2), and fall back to
version 1, so either side can be updated first. The client rejects other versions. Future protocol or storage changes need an explicit compatibility and
migration review; arbitrary future versions are not promised to interoperate.
Package installs have no automatic rollback or release-directory eviction. Do
not delete an old release while its host or sidecar can still be running.

Package program files:

```text
/Applications/Plait.app
/Library/LaunchAgents/dev.plait.host.plist
/Library/Application Support/Plait/Host/installed.json
/Library/Application Support/Plait/Host/releases/<content-build-id>/
  bin/plait-host
  bin/node
  runtime/src/
  runtime/node_modules/
  component.json
  LICENSE, NODE_LICENSE.txt, THIRD_PARTY_NOTICES.txt
```

`installed.json` describes the latest installed host, not necessarily the active
host. The active job keeps its original immutable program and runtime paths.
Threads, private build caches, and credentials are outside these program files
and are not installer payloads.

## Build and verification

Use a Git checkout on an Apple Silicon Mac with rustup, Python 3, Node/npm, and
full Xcode with Metal. Run:

```sh
bash scripts/check.sh --require-command-sandbox
bash scripts/build-macos.sh
```

To inspect the built client without installing it, run
`open dist/Plait.app --args /absolute/path/to/repository`. Agent work still needs
a running [local host](HOST.md).

The build compiles the locked workspace, generates client and host Rust notices
with pinned `cargo-about` 0.9.3, verifies the pinned Node download checksum, and
installs locked production npm dependencies without lifecycle scripts. It checks
Mach-O architecture, system-only dynamic links, signatures, property lists,
package contents, and the extracted client ZIP. Building never installs into
`/Applications` or starts a user agent.

The Mac workflow also installs the combined package on a disposable runner. It
checks startup without a developer Node path, reruns the real Mac sandbox suite
with the packaged host and runtime, checks independent component installation
during a scripted command, and checks saved state after host activation. Its
upgrade payload is explicitly labeled a disposable fixture and is not published.
The workflow launches the installed client and captures its window when the
runner has a display. These are scripted checks, not live-model acceptance or
validation on your Mac. See [verification](VERIFICATION.md) for results.

A merge to `main` that changes the app is built once: the run uploads a
`plait-macos-arm64-…` workflow artifact, which expires after 14 days, and a
separate publish job creates an immutable GitHub prerelease with the DMG,
packages, installer script, and checksums, numbered after the last preview.
The tests run before the merge, on the pull request. A failed build or
installer test publishes nothing. The job's only secret is the update signing key, which signs
`update.json` for in-app updates (see [Releasing](RELEASING.md)); the app and
packages are not Developer ID signed. [Issue #6](https://github.com/heath0xFF/plait/issues/6)
tracks Developer ID signing and notarization.
