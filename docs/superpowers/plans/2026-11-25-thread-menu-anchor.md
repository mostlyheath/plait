# Thread menu anchor, shortcuts and archive confirmation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The sidebar thread `⋯` menu opens next to its row, Rename and Archive get default shortcuts acting on the open thread, and the Archive shortcut asks first with a "Remember this setting" checkbox.

**Architecture:** Shortcuts are table entries in `ui/actions.rs`; their handlers already exist in `ui/mod.rs` (~1126-1145) and act on `host.thread.selected`. The menu and the confirmation card become GPUI `deferred(anchored(...))` children of the thread row built in `ui/navigation.rs`, so no bounds tracking is needed. The remembered choice is one boolean in `plait-core` preferences.

**Tech Stack:** Rust, GPUI (zed rev pinned in `Cargo.toml`).

**Spec:** `docs/superpowers/specs/2026-11-25-thread-menu-anchor-design.md`

## Global Constraints

- Defaults are single keystrokes in GPUI notation; a test checks collisions (`actions.rs` test `ids_are_unique_and_default_keystrokes_parse_without_collisions`).
- The keyboard table in `docs/MACOS.md` must equal the action table (test `the_documented_keyboard_table_is_this_table`).
- Never rename or reuse an action ID.
- Selection and hover use neutral fills; color only for state (`DESIGN.md`).
- Thread management requires `host.protocol() >= 3` and a connected host (`begin_thread_management`).
- Preferences ignore unknown fields and default missing ones (`#[serde(default)]`).
- Final gate: `bash scripts/check.sh`.

## Review Focus

- Archive shortcut with no thread open: nothing happens, no card.
- Archive shortcut on an already archived thread: unarchives at once, no question.
- Remember ticked, then Cancel: nothing archived and the preference is **not** saved.
- Menu opened on the last visible row or near the window bottom: it flips, stays fully on screen.
- Preferences file written before this change loads with the question still on.

---

### Task 1: Default shortcuts for Rename and Archive

**Files:**
- Modify: `crates/plait/src/ui/actions.rs:130-131`
- Modify: `docs/MACOS.md` (keyboard table; also the sentence near line 309 naming the shortcuts)

**Interfaces:**
- Produces: `thread.rename` bound to `cmd-alt-r`, `thread.archive` bound to `cmd-alt-a` (both free in the current table; `cmd-shift-e` is Every Thread upstream).

- [ ] **Step 1:** Change the two `entry(...)` calls from `None` to `Some("cmd-alt-r")` and `Some("cmd-alt-a")`.
- [ ] **Step 2:** Run `cargo test -p plait actions` — expect the doc-table test `the_documented_keyboard_table_is_this_table` to FAIL.
- [ ] **Step 3:** Add the two rows to the `docs/MACOS.md` keyboard table in the same format as neighbours.
- [ ] **Step 4:** Run `cargo test -p plait actions` — expect PASS (collision and doc tests included). If the collision test names a clash, pick another free `cmd-shift-<letter>` and update both files.
- [ ] **Step 5:** Commit `Give Rename and Archive Thread default shortcuts`.

### Task 2: Remembered-choice preference

**Files:**
- Modify: `crates/plait-core/src/preferences.rs` (struct at ~94; its tests)
- Modify: the Settings section that holds `ask_before_actions` / `hide_changed_files` (find with `grep -rn hide_changed_files crates/plait/src/ui`)

**Interfaces:**
- Produces: `Preferences.skip_archive_confirmation: bool` (default `false`), documented "Archive a thread from its shortcut without asking."

- [ ] **Step 1:** Test in `preferences.rs` tests: `skip_archive_confirmation_defaults_off_and_survives_older_files` — `serde_json::from_str::<Preferences>("{}")` gives `false`; a value set to `true` round-trips through serialize/deserialize.
- [ ] **Step 2:** Run `cargo test -p plait-core skip_archive` — FAIL (no field).
- [ ] **Step 3:** Add the field with its doc comment next to `hide_changed_files`.
- [ ] **Step 4:** Run the test — PASS.
- [ ] **Step 5:** Add a toggle row "Ask before archiving a thread from its shortcut" in the same Settings section as `hide_changed_files`, written through `set_preferences(value, PreferenceGroup::View, cx)` like the existing toggles. Inverted: on = ask.
- [ ] **Step 6:** `cargo build -p plait`; commit `Remember whether to ask before archiving by shortcut`.

### Task 3: Anchored thread menu

**Files:**
- Modify: `crates/plait/src/ui/navigation.rs` (row builder ~412-558, `navigation_menu` 695-796, `navigation()` 1115-1131)

**Interfaces:**
- Produces: `fn thread_menu(&self, id: &str, cx: &mut Context<Self>) -> AnyElement` — the existing thread branch of `navigation_menu`, returned as `deferred(anchored().snap_to_window_with_margin(px(8.)).child(panel))`. Project menu keeps its current inline rendering.

- [ ] **Step 1:** Extract the thread branch (727-787) into `thread_menu`. Drop the Close button for threads. Panel gets `on_mouse_down_out` closing it (pattern: `palette.rs:950`) and an Escape binding in a `"ThreadMenu"` key context that sets `navigation.menu = None`.
- [ ] **Step 2:** In the row builder, when `self.navigation.menu.as_deref() == Some(&id)`, add `thread_menu` as a child of the `⋯` button's wrapper so the anchor is the button. `navigation_menu()` renders nothing for thread ids any more.
- [ ] **Step 3:** Each menu row shows its keymap shortcut at the right (source: the same lookup the palette uses for hints; grep `shortcut` in `palette.rs`). Rows without one show none.
- [ ] **Step 4:** Manual check (`cargo run -p plait` on a connected host, protocol 3): menu opens under the clicked `⋯`; Esc closes; outside click closes; clicking `⋯` on a row near the window bottom flips upward and stays on screen; project menu unchanged.
- [ ] **Step 5:** `cargo test -p plait`; commit `Open the thread menu next to its row`.

### Task 4: Archive confirmation card

**Files:**
- Modify: `crates/plait/src/ui/navigation.rs` (nav state struct near line 25; row builder; new `archive_card`)
- Modify: `crates/plait/src/ui/mod.rs:1131-1145` (the `ArchiveThread` handler)

**Interfaces:**
- Consumes: `Preferences.skip_archive_confirmation` (Task 2), the anchoring pattern from Task 3.
- Produces: `navigation.confirm_archive: Option<String>` (thread id); `fn archive_card(&self, id: &str, remember: bool, cx) -> AnyElement`; `fn request_archive(&mut self, id: String, window, cx)` which archives at once when the thread is already archived or the preference is on, else sets `confirm_archive`.

- [ ] **Step 1:** Unit-test the decision as a pure function `needs_archive_question(archived: bool, skip: bool) -> bool` (true only when not archived and not skip), cases for all four inputs. Run — FAIL; implement in `navigation.rs`; run — PASS.
- [ ] **Step 2:** The `ArchiveThread` handler calls `request_archive` for the selected thread instead of `archive_thread` directly.
- [ ] **Step 3:** `archive_card` is an anchored child of the open thread's row: text "Archive this thread?", a "Remember this setting" checkbox (local `remember` state, default off, built from existing control primitives as in `palette.rs:694`), buttons Cancel and Archive. Return confirms, Esc cancels (`"ArchiveConfirm"` key context).
- [ ] **Step 4:** Confirm: if `remember`, write `skip_archive_confirmation = true` through `set_preferences(.., PreferenceGroup::View, cx)`; then `archive_thread(id, true, ..)`; clear `confirm_archive`. Cancel only clears `confirm_archive` and saves nothing.
- [ ] **Step 5:** Manual check: shortcut with no thread open does nothing; first use shows the card; Cancel with box ticked leaves the setting off; Archive with box ticked archives and the next use archives silently; the Settings toggle turns the question back on; archived thread's shortcut unarchives silently.
- [ ] **Step 6:** `bash scripts/check.sh`; commit `Ask before archiving a thread by shortcut`.
