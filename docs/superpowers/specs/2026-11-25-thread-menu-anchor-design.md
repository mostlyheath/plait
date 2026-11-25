# Thread menu: anchored popover, shortcuts, confirmation

## Why
The sidebar's thread `⋯` menu renders in the sidebar column (`navigation.rs`,
`navigation_menu`), so it opens at the bottom left whichever row was clicked.
The cursor has to travel to reach it. Rename and Archive have no shortcuts.

## Goals
- The menu opens next to the row's `⋯` button, flips to stay inside the window,
  and closes on Esc or an outside click (no Close button).
- Rename and Archive get default shortcuts. They act on the **open** thread.
- A shortcut that archives asks first: "Archive this thread?" with a
  "Remember this setting" checkbox. Ticked, later invocations skip the question.

## Non-goals
- Acting on a hovered or selected row. Mouse-driven menu items (no question
  asked today) keep behaving as they do.
- A "Close thread" action: none exists today, so none is added. Say so if wanted.
- Confirmation on Rename; it opens an editor and is already reversible.

## Approaches
1. **Anchored popover (chosen):** GPUI `anchored()` + `deferred()` at the button.
   First use of both in this codebase.
2. Inline expansion under the row: less code, but the list shifts.

## Design
- Menu: same contents, rendered as an overlay anchored below the button.
- Keys: `thread.rename` and `thread.archive` in `actions.rs` get defaults chosen
  to avoid collisions with the existing keymap (a test checks); `docs/MACOS.md`
  keyboard table updated (a test compares it). Menu rows show their shortcut.
- Confirmation: a small card anchored to the open thread's row with Cancel and
  Archive, Return confirms, Esc cancels. The checkbox saves a preference in the
  existing preferences store; it is reversible in Settings.
- Unarchive needs no question.

## Left to the builder
Exact keys, preference name and Settings location, flip behavior near edges.
