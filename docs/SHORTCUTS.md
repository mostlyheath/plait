# Keyboard shortcuts

Default shortcuts for Plait on macOS. Customize application shortcuts in **Settings → Keyboard**, which records keystrokes, searches actions, flags conflicts (requiring **Reassign**), clears bindings, and restores defaults.

## Application shortcuts

This table matches `ACTIONS` in `crates/plait/src/ui/actions.rs` (checked by automated tests). The **ID** column is the stable override key for `keymap` in `~/Library/Application Support/Plait/desktop.json` (for example, `{"thread.new": "cmd-t", "app.quit": ""}`—an empty string unbinds, an invalid keystroke falls back to the default, and unknown IDs are preserved).

| Shortcut | Action | ID |
| --- | --- | --- |
| Cmd-comma | Settings… | `app.settings` |
| Cmd-Q | Quit Plait | `app.quit` |
| Cmd-T | New Tab | `tab.new` |
| Cmd-N | New Thread | `thread.new` |
| Cmd-Shift-N | New Thread with Model… | `thread.new_with_model` |
| Cmd-Option-N | New Thread in Worktree | `thread.new_worktree` |
| Cmd-Shift-M | Change Thread Model… | `thread.switch_model` |
| Cmd-O | Open Project… | `repository.open` |
| Cmd-P | Open File… | `file.open` |
| Cmd-R | Refresh Git Status | `git.refresh` |
| Cmd-Option-E | Open in Editor | `file.open_in_editor` |
| Cmd-Option-T | Open Terminal Here | `project.open_terminal` |
| Cmd-W | Close Tab | `tab.close` |
| Cmd-Shift-T | Reopen Closed Tab | `tab.reopen` |
| Cmd-Shift-W | Close Window | `window.close` |
| Cmd-K | Command Palette… | `palette.toggle` |
| Cmd-B | Toggle Sidebar | `view.toggle_sidebar` |
| Cmd-Shift-E | Every Thread | `view.overview` |
| Cmd-Backslash | Fill Panel | `view.toggle_work` |
| Cmd-Option-Right | Focus Next Pane | `pane.next` |
| Cmd-Option-Left | Focus Previous Pane | `pane.previous` |
| Cmd-L | Toggle Panel | `view.toggle_inspector` |
| Cmd-Shift-D | Toggle Diagnostics | `view.toggle_diagnostics` |
| Cmd-Shift-1 | Conversation | `view.conversation` |
| Cmd-Shift-2 | Changes | `view.changes` |
| Cmd-Shift-3 | History | `view.history` |
| Cmd-F | Find | `file.find` |
| Cmd-G | Find Next | `file.find_next` |
| Cmd-Shift-G | Find Previous | `file.find_previous` |
| Cmd-Option-Down | Next Thread | `thread.next` |
| Cmd-Option-J | Next Thread Needing Me | `thread.next_needing_me` |
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

Notes on application shortcuts:

- **Thread tabs.** All tab shortcuts, including **Show All Tabs…**, work with either **Hide** or **Show** in Settings → Appearance.
- **Rebinding rules:** Text-editing keys and reserved macOS system shortcuts (such as `Cmd-Space`, `Cmd-Tab`, and `Cmd-Shift-3`–`5`) cannot be assigned. `Tab` and `Shift-Tab` always keep a binding. Each numbered jump set (`tab_jump.1`–`9` and `jump.1`–`9`) changes modifiers as one set.
- **Approvals (`Cmd-Option-Y`, `Cmd-Option-period`) and Undo (`Cmd-Option-U`):** Act only in the visible conversation with no modal or palette open over it. Holding an approval key answers at most one card.
- **Unbound menu actions:** Actions without a default shortcut (such as **Worktrees**, **Linear Issues**, **Project Settings…**, **Show in Finder**, **Files**, **Rename Thread…**, **Archive or Unarchive Thread**, **Stage All Changes**, **Push**, **Pull**, **Fetch**, and **New Branch…**) can still be assigned a shortcut in **Settings → Keyboard**.

Work view also has these assignable pane actions.

| Shortcut | Action | ID |
| --- | --- | --- |
| — | Split Pane Right | `pane.split_right` |
| — | Split Pane Down | `pane.split_down` |
| — | Close Pane | `pane.close` |

## Every thread

| Key | Action |
| --- | --- |
| Arrow keys | Move focus by row or column |
| Home / End | Focus the first or last card |
| Enter | Open the focused thread |
| Escape | Return to the workspace |

## Composer and text fields

Control-local bindings defined in `crates/plait/src/ui/composer/mod.rs` and `crates/plait/src/ui/text_field.rs`:

| Key | Action |
| --- | --- |
| Enter | Send message, queue a follow-up during a run, or accept an open suggestion |
| Cmd-Enter | Send or queue a follow-up even when a suggestion list is open |
| Option-Enter | Steer the running agent at its next step (protocol 3+ host) |
| Shift-Enter | Insert a line break (composer) |
| Tab | Accept an open suggestion; otherwise focus the next control |
| `@` / `/` | Open file mentions at a word start / local commands and Pi skills at the start of an empty draft |
| Up / Down | Move in a suggestion list; on the first line of the composer, recall earlier sent messages |
| Escape | Dismiss suggestion list, exit message recall, or close an empty new-thread tab |
| Escape twice (within 1.5 s) | Stop the running agent |
| Option-Backspace / Option-Delete | Delete previous / next word |
| Cmd-Backspace / Cmd-Delete | Delete to start / end of line |
| Option-Left / Right | Move by word (add `Shift` to select) |
| Cmd-Left / Right | Move to start / end of line (add `Shift` to select) |
| Home / End, Cmd-Up / Cmd-Down | Move to start / end of draft (composer); start / end of field (single-line fields) |
| Cmd-A / Cmd-C / Cmd-X / Cmd-V | Select all / copy / cut / paste |
| Cmd-Z / Cmd-Shift-Z | Undo / redo text edits |

## Command palette and model picker

Control-local bindings defined in `crates/plait/src/ui/palette.rs`:

| Key | Action |
| --- | --- |
| Up / Down | Move selection (wraps at ends) |
| Page Up / Page Down | Move by one page |
| Enter | Run the selected item |
| Cmd-Enter | Model picker: save selected model as the default |
| Cmd-D | Model picker: add or remove favorite |
| Escape | Close overlay and return focus |

## Sidebar

| Key | Action |
| --- | --- |
| Up / Down | Move focus between sidebar project labels, thread cards, and drafts, scrolling them into view |
| Tab / Shift-Tab | Move through sidebar controls, including project and thread menus |
| Enter / Space | Activate the focused project, thread, draft, or menu |
| Enter / Escape | Save or cancel an inline thread rename |

## Conversation, Find, and views

Bindings defined in `crates/plait/src/ui/selection.rs`, `crates/plait/src/ui/file_view.rs`, and `crates/plait/src/ui/worktrees_view/render.rs`:

| Key | Action |
| --- | --- |
| Cmd-C | Copy selected text in Conversation |
| Home / Cmd-Up | Jump to start of loaded conversation history (requests an earlier page if available) |
| End / Cmd-Down | Jump to latest messages in Conversation |
| Enter / Shift-Enter | Find bar: jump to next / previous match |
| Escape | Close Find bar, enlarged image overlay, or Settings modal |
| Enter / Space | Activate focused control (including opening a folded earlier turn) or select a Worktrees lane |
