# Changelog

What changed in each Plait preview, newest first. Notes for the next preview
wait in [changes/](changes/README.md) until a release collects them.

## v0.1.0-preview.84

### Added

- Markdown files open rendered, with a Source switch, heading outline, Find, checkout images, and a check for broken local links.

### Fixed

- If Plait can’t read your saved Linear drafts, it protects them and tries again when you reopen Issues.

## v0.1.0-preview.83

### Added

- Create Linear issues and edit their title and description from the Issues tab, with images uploaded to Linear shown in the description. Nothing is sent until you press Save, unsaved text survives quitting, and a change someone else made in Linear is shown before anything is overwritten.

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

