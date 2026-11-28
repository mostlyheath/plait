# Changelog

What changed in each Plait release, newest first. Notes for the next release
wait in [changes/](changes/README.md) until a release collects them.

## v0.1.0-preview.85

### Added

- Settings → About lists the licenses for the software, fonts and artwork bundled with Plait. SECURITY.md explains how to report a vulnerability privately.

### Fixed

- Rendered Markdown wraps to the panel and reflows as you resize it; long code lines scroll sideways instead. Editor opens the file you're looking at, at the line you're on, including in worktrees.
- A thread started in a plain folder that later becomes a Git repository now protects your uncommitted edits and records its runs for Undo.
- Discarding a hunk is refused if the change has moved on since you reviewed it, Delete keeps a copy of an untracked file before removing it, and Unstage All no longer cancels a merge in progress.
- Installs and updates come from mostlyheath/plait, and hosts report their full preview version.
- A leftover steering message keeps its own approval setting, and when Codex or Claude Code interrupts a turn, queued follow-ups wait instead of starting.
- A slow commit, such as one with a long pre-commit hook, no longer disconnects every thread on that host.
- A failed or cancelled Wait message no longer loses its attached images when the draft already has content. Plait keeps your draft in place, copies any recovered text to the clipboard, and attaches recovered images that fit within the 4-image and 5 MiB limits.

## v0.1.0-preview.84

### Added

- Markdown files open rendered, with a Source switch, heading outline, Find, checkout images, and a check for broken local links.
- Stillwater Light joins the light themes in Settings → Appearance.

### Changed

- The disk image now holds only Plait and an Applications shortcut, so installing is one drag. The one-time Open Anyway step is described in the guide.

### Fixed

- If Plait can’t read your saved Linear drafts, it protects them and tries again when you reopen Issues.
- Cmd-Y opens History by default, instead of the macOS screenshot key. Cmd-Shift-C commits staged changes when the commit message already has focus and Commit is enabled; otherwise it takes you to the message.

## v0.1.0-preview.83

### Added

- Create Linear issues and edit their title and description from the Issues tab, with images uploaded to Linear shown in the description. Nothing is sent until you press Save, unsaved text survives quitting, and Plait checks for changes in Linear before sending an edit.

### Changed

- Worktrees and Linear issues have a close button in their header that returns you to the tab you were on, so you can get back to your threads when thread tabs are hidden.

### Fixed

- In the Linear issues tab, long issue titles are shortened inside the list instead of running underneath the issue details.
- Unsent drafts, message-box text, waiting messages and each thread's approval choice are kept separately for each machine, so work typed for one machine never appears on, or is sent to, another, and nothing you type is lost when you switch machines, reconnect or quit.

## v0.1.0-preview.82

### Added

- Claude Code can be set up straight from a new thread's harness list or the command palette; both open Settings → Agents and start the one-time download.
- Threads can be named by a model you choose in Settings → New threads → Thread naming. It's off until you pick a model, sends only the first message, and never renames a thread you named yourself.

### Changed

- The message box's model, permissions and harness controls stay on one line in narrow windows, and the "patch changed" note on a restored tab is shown as information beside the diff, not as an error.

### Fixed

- Check for Updates opens Settings on the About tab.
- The thread header keeps its title readable and shortens long project and branch names in the middle instead of collapsing them to dots; the branch icon draws correctly.
- The Linear issues view opens as a tab straight away, shows issue descriptions as formatted Markdown that scrolls, and lets you change an issue's status.
- Folding a project, and the result of removing a worktree, apply only to the machine they happened on.
- API keys your shell exports, used as $NAME or ${NAME} in ~/.pi/agent/models.json or by built-in providers, now work in the app. They're used only for provider requests and never reach commands or Git hooks. Changing a custom endpoint's server forgets its saved key.
- A rename or generated name made while a queued message was starting is no longer reverted.
- After a host restart, an interrupted run is recorded against the right turn, even when a follow-up was queued behind it.
- A run that fails to start explains why, even on a slow machine, and a tailnet host that refuses this device says so clearly.
- Creating a thread in its own worktree, in a project with a running thread, no longer warns that the worktree changed on disk.

