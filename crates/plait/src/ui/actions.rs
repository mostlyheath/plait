//! Every application action in one table.
//!
//! The table is the single source for key bindings, the menu bar, the command
//! palette's entries and shortcut hints, and the keyboard table in
//! docs/SHORTCUTS.md (a test compares them). Shortcuts are bound only by `bind`,
//! which takes a map of action ID to keystroke, so a saved keymap can replace
//! any default without touching the table.
//!
//! Editing keys that belong to one control (the composer, the text field, and
//! the palette's list) are bound by those controls and are not listed here.
//!
//! The second half of this file is the rules of the shortcut editor as plain
//! functions over that map: what a row is bound to, which keystrokes are
//! refused, which actions a keystroke would collide with, and how an
//! assignment changes the map. The editor's view is in settings_keyboard.rs.

use super::{
    jump::*,
    tabs::*,
    ways_out::{OpenInEditor, OpenTerminal, ShowInFinder},
    *,
};
use std::{collections::BTreeMap, rc::Rc};

/// Where an action appears besides its shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    /// A menu item and a command palette entry.
    Menu,
    /// A command palette entry with an optional shortcut.
    Palette,
    /// The same, starting a new group in its menu.
    MenuGroup,
    /// Shortcut only: movement that needs no menu item.
    Hidden,
}

pub(super) struct AppAction {
    /// Stable identifier, the key of a keymap override. Never rename or reuse one.
    pub id: &'static str,
    pub title: &'static str,
    /// The menu the action sits in, and its group in a shortcut editor.
    pub category: &'static str,
    /// Default keystroke in GPUI notation, such as `cmd-shift-n`.
    pub key: Option<&'static str>,
    pub place: Place,
    build: fn() -> Box<dyn Action>,
}

impl AppAction {
    pub fn action(&self) -> Box<dyn Action> {
        (self.build)()
    }

    /// Quit must work with no window focused; everything else needs the root view.
    fn context(&self) -> Option<&'static str> {
        (self.id != "app.quit").then_some("Plait")
    }

    /// The default as a keystroke. Defaults are single keystrokes; a test checks.
    pub fn default_key(&self) -> Option<Keystroke> {
        parsed(self.key?)?.into_iter().next()
    }

    /// Focus movement is how the keyboard reaches every control, and it has no
    /// menu item, so it always keeps a shortcut.
    pub fn required(&self) -> bool {
        self.id.starts_with("focus.")
    }
}

const fn entry(
    id: &'static str,
    title: &'static str,
    category: &'static str,
    key: Option<&'static str>,
    place: Place,
    build: fn() -> Box<dyn Action>,
) -> AppAction {
    AppAction {
        id,
        title,
        category,
        key,
        place,
        build,
    }
}

use Place::{Hidden, Menu as Item, MenuGroup as Group};

/// In menu order. Categories are contiguous; each becomes one menu.
#[rustfmt::skip]
pub(super) const ACTIONS: &[AppAction] = &[
    entry("app.settings", "Settings…", "Plait", Some("cmd-,"), Item, || Box::new(Settings)),
    entry("settings.providers", "Provider Settings", "Plait", None, Item, || Box::new(ProviderSettings)),
    entry("app.check_for_updates", "Check for Updates…", "Plait", None, Item, || Box::new(CheckForUpdates)),
    entry("app.quit", "Quit Plait", "Plait", Some("cmd-q"), Group, || Box::new(Quit)),
    entry("tab.new", "New Tab", "File", Some("cmd-t"), Item, || Box::new(NewTab)),
    entry("thread.new", "New Thread", "File", Some("cmd-n"), Item, || Box::new(NewThread)),
    entry("thread.new_with_model", "New Thread with Model…", "File", Some("cmd-shift-n"), Item, || Box::new(NewThreadWithModel)),
    entry("thread.new_worktree", "New Thread in Worktree", "File", Some("cmd-alt-n"), Item, || Box::new(NewWorktreeThread)),
    entry("thread.switch_model", "Change Thread Model…", "File", Some("cmd-shift-m"), Item, || Box::new(SwitchThreadModel)),
    entry("repository.open", "Open Project…", "File", Some("cmd-o"), Group, || Box::new(OpenRepository)),
    entry("project.worktrees", "Worktrees", "File", None, Item, || Box::new(super::worktrees_view::Worktrees)),
    entry("project.issues", "Linear Issues", "File", None, Item, || Box::new(super::issues_view::Issues)),
    entry("project.settings", "Project Settings…", "File", None, Item, || Box::new(ProjectSettings)),
    entry("file.check_links", "Check Links", "File", None, Item, || Box::new(CheckDocumentLinks)),
    entry("file.open", "Open File…", "File", Some("cmd-p"), Item, || Box::new(QuickOpen)),
    entry("git.refresh", "Refresh Git Status", "File", Some("cmd-r"), Item, || Box::new(Refresh)),
    // Ways out. Option and Command, with E for editor and T for terminal.
    entry("file.open_in_editor", "Open in Editor", "File", Some("cmd-alt-e"), Group, || Box::new(OpenInEditor)),
    entry("file.show_in_finder", "Show in Finder", "File", None, Item, || Box::new(ShowInFinder)),
    entry("project.open_terminal", "Open Terminal Here", "File", Some("cmd-alt-t"), Item, || Box::new(OpenTerminal)),
    entry("tab.close", "Close Tab", "File", Some("cmd-w"), Group, || Box::new(CloseTab)),
    entry("tab.reopen", "Reopen Closed Tab", "File", Some("cmd-shift-t"), Item, || Box::new(ReopenTab)),
    entry("window.close", "Close Window", "File", Some("cmd-shift-w"), Group, || Box::new(Close)),
    entry("palette.toggle", "Command Palette…", "View", Some("cmd-k"), Item, || Box::new(CommandPalette)),
    entry("view.toggle_sidebar", "Toggle Sidebar", "View", Some("cmd-b"), Group, || Box::new(ToggleSidebar)),
    entry("view.overview", "Every Thread", "View", Some("cmd-shift-e"), Item, || Box::new(ToggleOverview)),
    entry("view.graph", "Graph", "View", None, Place::Palette, || Box::new(super::graph_view::Graph)),
    entry("view.toggle_work", "Fill Panel", "View", Some("cmd-\\"), Item, || Box::new(FillPanel)),
    entry("pane.next", "Focus Next Pane", "View", Some("cmd-alt-right"), Item, || Box::new(FocusNextPane)),
    entry("pane.previous", "Focus Previous Pane", "View", Some("cmd-alt-left"), Item, || Box::new(FocusPreviousPane)),
    entry("view.toggle_inspector", "Toggle Panel", "View", Some("cmd-l"), Item, || Box::new(ToggleInspector)),
    entry("view.toggle_changed_files", "Toggle Changed Files", "View", None, Item, || Box::new(ToggleChangedFiles)),
    entry("view.toggle_diagnostics", "Toggle Diagnostics", "View", Some("cmd-shift-d"), Item, || Box::new(ToggleDiagnostics)),
    entry("view.conversation", "Conversation", "View", Some("cmd-shift-1"), Item, || Box::new(Conversation)),
    entry("view.changes", "Changes", "View", Some("cmd-shift-2"), Item, || Box::new(Changes)),
    entry("view.history", "History", "View", Some("cmd-y"), Item, || Box::new(CommitHistory)),
    entry("view.files", "Files", "View", None, Item, || Box::new(ShowFiles)),
    entry("file.find", "Find", "View", Some("cmd-f"), Group, || Box::new(FindInFile)),
    entry("file.find_next", "Find Next", "View", Some("cmd-g"), Item, || Box::new(FindNext)),
    entry("file.find_previous", "Find Previous", "View", Some("cmd-shift-g"), Item, || Box::new(FindPrevious)),
    entry("conversation.quote_selection", "Quote selection", "View", None, Place::Palette, || Box::new(super::selection::QuoteSelection)),
    entry("thread.next", "Next Thread", "View", Some("cmd-alt-down"), Group, || Box::new(NextThread)),
    entry("thread.next_needing_me", "Next Thread Needing Me", "View", Some("cmd-alt-j"), Item, || Box::new(NextThreadNeedingMe)),
    entry("thread.previous", "Previous Thread", "View", Some("cmd-alt-up"), Item, || Box::new(PreviousThread)),
    entry("git.next_file", "Next Changed File", "View", Some("alt-down"), Item, || Box::new(NextFile)),
    entry("git.previous_file", "Previous Changed File", "View", Some("alt-up"), Item, || Box::new(PreviousFile)),
    entry("thread.rename", "Rename Thread…", "Thread", None, Item, || Box::new(RenameThread)),
    entry("thread.archive", "Archive or Unarchive Thread", "Thread", None, Item, || Box::new(ArchiveThread)),
    entry("thread.show_archived", "Show or Hide Archived Threads", "Thread", None, Item, || Box::new(ToggleArchived)),
    entry("thread.filter", "Find a Thread", "Thread", None, Item, || Box::new(FilterThreads)),
    entry("thread.notifications", "Mute or Unmute Thread Alerts", "Thread", None, Item, || Box::new(ToggleNotifications)),
    entry("thread.refresh_list", "Refresh Threads and Folders", "Thread", None, Item, || Box::new(RefreshThreadList)),
    entry("project.show_hidden", "Show Hidden Projects", "Thread", None, Item, || Box::new(ShowHiddenProjects)),
    // Answers to the open thread's approval. Both need Command and Option, so
    // no typing slip sends one, and their keys sit far apart.
    entry("thread.undo_last_turn", "Undo Last Turn", "Thread", Some("cmd-alt-u"), Group, || Box::new(UndoLastTurn)),
    entry("approval.deny_once", "Deny Once", "Thread", Some("cmd-alt-."), Group, || Box::new(DenyOnce)),
    entry("approval.allow_once", "Allow Once", "Thread", Some("cmd-alt-y"), Item, || Box::new(AllowOnce)),
    entry("git.stage_all", "Stage All Changes", "Git", None, Item, || Box::new(StageAll)),
    entry("git.unstage_all", "Unstage All Changes", "Git", None, Item, || Box::new(UnstageAll)),
    entry("git.commit", "Commit Staged Changes…", "Git", Some("cmd-shift-c"), Group, || Box::new(CommitStaged)),
    entry("git.push", "Push", "Git", None, Group, || Box::new(Push)),
    entry("git.pull", "Pull", "Git", None, Item, || Box::new(Pull)),
    entry("git.fetch", "Fetch", "Git", None, Item, || Box::new(Fetch)),
    entry("git.switch_branch", "Switch Branch…", "Git", Some("cmd-shift-b"), Group, || Box::new(SwitchBranch)),
    entry("git.new_branch", "New Branch…", "Git", None, Item, || Box::new(NewBranch)),
    entry("window.minimize", "Minimize", "Window", Some("cmd-m"), Item, || Box::new(Minimize)),
    entry("tab.next", "Next Tab", "Window", Some("cmd-shift-]"), Item, || Box::new(NextTab)),
    entry("tab.previous", "Previous Tab", "Window", Some("cmd-shift-["), Item, || Box::new(PreviousTab)),
    entry("tab.move_right", "Move Tab Right", "Window", Some("ctrl-shift-right"), Item, || Box::new(MoveTabRight)),
    entry("tab.move_left", "Move Tab Left", "Window", Some("ctrl-shift-left"), Item, || Box::new(MoveTabLeft)),
    entry("tab.list", "Show All Tabs…", "Window", Some("cmd-shift-\\"), Item, || Box::new(ListTabs)),
    entry("tab_jump.1", "Switch to Tab 1", "Navigation", Some("cmd-1"), Hidden, || Box::new(TabJump1)),
    entry("tab_jump.2", "Switch to Tab 2", "Navigation", Some("cmd-2"), Hidden, || Box::new(TabJump2)),
    entry("tab_jump.3", "Switch to Tab 3", "Navigation", Some("cmd-3"), Hidden, || Box::new(TabJump3)),
    entry("tab_jump.4", "Switch to Tab 4", "Navigation", Some("cmd-4"), Hidden, || Box::new(TabJump4)),
    entry("tab_jump.5", "Switch to Tab 5", "Navigation", Some("cmd-5"), Hidden, || Box::new(TabJump5)),
    entry("tab_jump.6", "Switch to Tab 6", "Navigation", Some("cmd-6"), Hidden, || Box::new(TabJump6)),
    entry("tab_jump.7", "Switch to Tab 7", "Navigation", Some("cmd-7"), Hidden, || Box::new(TabJump7)),
    entry("tab_jump.8", "Switch to Tab 8", "Navigation", Some("cmd-8"), Hidden, || Box::new(TabJump8)),
    entry("tab_jump.9", "Switch to Tab 9", "Navigation", Some("cmd-9"), Hidden, || Box::new(TabJump9)),
    entry("jump.1", "Jump to Sidebar Item 1", "Navigation", Some("cmd-alt-1"), Hidden, || Box::new(Jump1)),
    entry("jump.2", "Jump to Sidebar Item 2", "Navigation", Some("cmd-alt-2"), Hidden, || Box::new(Jump2)),
    entry("jump.3", "Jump to Sidebar Item 3", "Navigation", Some("cmd-alt-3"), Hidden, || Box::new(Jump3)),
    entry("jump.4", "Jump to Sidebar Item 4", "Navigation", Some("cmd-alt-4"), Hidden, || Box::new(Jump4)),
    entry("jump.5", "Jump to Sidebar Item 5", "Navigation", Some("cmd-alt-5"), Hidden, || Box::new(Jump5)),
    entry("jump.6", "Jump to Sidebar Item 6", "Navigation", Some("cmd-alt-6"), Hidden, || Box::new(Jump6)),
    entry("jump.7", "Jump to Sidebar Item 7", "Navigation", Some("cmd-alt-7"), Hidden, || Box::new(Jump7)),
    entry("jump.8", "Jump to Sidebar Item 8", "Navigation", Some("cmd-alt-8"), Hidden, || Box::new(Jump8)),
    entry("jump.9", "Jump to Sidebar Item 9", "Navigation", Some("cmd-alt-9"), Hidden, || Box::new(Jump9)),
    entry("focus.next", "Focus Next Control", "Navigation", Some("tab"), Hidden, || Box::new(Tab)),
    entry("focus.previous", "Focus Previous Control", "Navigation", Some("shift-tab"), Hidden, || Box::new(BackTab)),
];

/// Keystrokes that replace the defaults, by action ID. An empty keystroke
/// leaves the action unbound. Ordered, so binding is deterministic.
pub(super) type KeyOverrides = BTreeMap<String, String>;

/// The platform whose key names label shortcuts.
pub(super) const KEY_LABELS: OperatingSystem = if cfg!(target_os = "macos") {
    OperatingSystem::Mac
} else {
    OperatingSystem::Linux
};

/// A binding written in GPUI notation as keystrokes. `None` when it is empty
/// or any part of it does not parse.
fn parsed(keystrokes: &str) -> Option<Vec<Keystroke>> {
    let keys = keystrokes
        .split_whitespace()
        .map(|keystroke| Keystroke::parse_with_os(keystroke, KEY_LABELS).ok())
        .collect::<Option<Vec<_>>>()?;
    (!keys.is_empty()).then_some(keys)
}

fn parses(keystrokes: &str) -> bool {
    parsed(keystrokes).is_some()
}

/// The keystroke an action is bound to: its override when that is usable,
/// otherwise its default. `None` means unbound.
pub(super) fn keystroke<'a>(action: &'a AppAction, overrides: &'a KeyOverrides) -> Option<&'a str> {
    match overrides.get(action.id).map(String::as_str) {
        Some("") => None,
        // A keystroke that does not parse must not cost the user the default.
        Some(custom) if parses(custom) => Some(custom),
        _ => action.key,
    }
}

/// Bind every application shortcut. This is the only place they are bound.
pub(super) fn bind(cx: &mut App, overrides: &KeyOverrides) {
    let bindings = ACTIONS
        .iter()
        .zip(winning_bindings(overrides))
        .filter_map(|(action, wins)| wins.then_some(action))
        .filter_map(|action| {
            let context = action
                .context()
                .and_then(|context| KeyBindingContextPredicate::parse(context).ok())
                .map(Rc::new);
            let keys = keystroke(action, overrides)?;
            KeyBinding::load(keys, action.action(), context, false, None, cx).ok()
        })
        .collect::<Vec<_>>();
    cx.bind_keys(bindings);
}

/// A saved binding takes priority over a new default. For ties, GPUI
/// ranks a binding with a context above one without, then a later binding
/// above an earlier. Only that winner is bound: the held-key guard lets a key
/// go on to lower bindings, so a loser would act in the winner's place.
fn winning_bindings(overrides: &KeyOverrides) -> Vec<bool> {
    let mut ranked = (0..ACTIONS.len()).collect::<Vec<_>>();
    ranked.sort_by_cached_key(|&ix| {
        let action = &ACTIONS[ix];
        let custom = overrides.get(action.id).is_some_and(|key| parses(key));
        std::cmp::Reverse((custom, action.context().is_some(), ix))
    });
    let mut taken = Vec::new();
    let mut wins = vec![false; ACTIONS.len()];
    for ix in ranked {
        let Some(keys) = keystroke(&ACTIONS[ix], overrides).and_then(parsed) else {
            continue;
        };
        if !taken.contains(&keys) {
            taken.push(keys);
            wins[ix] = true;
        }
    }
    wins
}

/// Whether the connected host reported graph support and `View → Graph` should show.
struct GraphMenuEnabled(bool);

impl Global for GraphMenuEnabled {}

/// The menu bar: one menu per category, in table order.
#[cfg(test)]
fn menus() -> Vec<Menu> {
    menus_with_graph(false)
}

fn menus_with_graph(include_graph: bool) -> Vec<Menu> {
    let mut menus: Vec<(&str, Vec<MenuItem>)> = Vec::new();
    for action in ACTIONS.iter().filter(|action| {
        matches!(action.place, Item | Group) || (include_graph && action.id == "view.graph")
    }) {
        if menus
            .last()
            .is_none_or(|(name, _)| *name != action.category)
        {
            menus.push((action.category, Vec::new()));
        }
        let Some((_, items)) = menus.last_mut() else {
            continue;
        };
        if action.place == Group && !items.is_empty() {
            items.push(MenuItem::separator());
        }
        items.push(MenuItem::Action {
            name: action.title.into(),
            action: action.action(),
            os_action: None,
            checked: false,
            disabled: false,
        });
    }
    menus
        .into_iter()
        .map(|(name, items)| Menu::new(name).items(items))
        .collect()
}

/// Shows or hides `View → Graph` in the native menu bar based on host support.
pub(super) fn set_graph_menu(cx: &mut App, enabled: bool) {
    cx.set_global(GraphMenuEnabled(enabled));
    cx.set_menus(menus_with_graph(enabled));
}

/// Bind all keys and build the menus. Call again with a new keymap to rebind:
/// the previous bindings are cleared first, and the menus pick up the new keys.
pub(super) fn install(cx: &mut App, overrides: &KeyOverrides) {
    cx.clear_key_bindings();
    // Controls bind their own editing keys.
    composer::bind_keys(cx);
    text_field::bind_keys(cx);
    // Everything bound so far belongs to text entry.
    let editing = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter_map(|binding| match binding.keystrokes() {
            [only] => Some(only.inner().clone()),
            _ => None,
        })
        .collect();
    cx.set_global(EditingKeys(editing));
    palette::bind_keys(cx);
    selection::bind_keys(cx);
    bind(cx, overrides);
    let include_graph = cx.try_global::<GraphMenuEnabled>().is_some_and(|g| g.0);
    cx.set_menus(menus_with_graph(include_graph));
}

/// The keys the composer and text fields bind for themselves, read from their
/// own bindings each time the keymap is installed.
struct EditingKeys(Vec<Keystroke>);

impl Global for EditingKeys {}

/// Keys that edit text. An application shortcut on one of them would not fire
/// while typing, so the shortcut editor refuses them.
pub(super) fn editing_keys(cx: &App) -> &[Keystroke] {
    cx.try_global::<EditingKeys>()
        .map_or(&[], |keys| keys.0.as_slice())
}

/// What an action is bound to now. Empty when it is unbound.
pub(super) fn bound(action: &AppAction, overrides: &KeyOverrides) -> Vec<Keystroke> {
    keystroke(action, overrides)
        .and_then(parsed)
        .unwrap_or_default()
}

/// Keystrokes as the menus show them: `⌘⇧N`.
pub(super) fn label(keys: &[Keystroke]) -> String {
    let shown = keys.iter().map(|key| key.display(KEY_LABELS).to_string());
    shown.collect::<Vec<_>>().join(" ")
}

fn key_name(key: &str) -> String {
    let mut letters = key.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// The modifiers held for `key`, then the key, each named by `names`.
fn named(key: &Keystroke, names: [&str; 5]) -> Vec<String> {
    let held = [
        key.modifiers.control,
        key.modifiers.alt,
        key.modifiers.platform,
        key.modifiers.shift,
        key.modifiers.function,
    ];
    let mut parts = held
        .into_iter()
        .zip(names)
        .filter(|(down, _)| *down)
        .map(|(_, name)| name.to_owned())
        .collect::<Vec<_>>();
    parts.push(key_name(&key.key));
    parts
}

/// Keystrokes in words, for assistive technology: `Command Shift N`.
pub(super) fn spoken(keys: &[Keystroke]) -> String {
    let names = ["Control", "Option", "Command", "Shift", "Function"];
    let words = keys.iter().map(|key| named(key, names).join(" "));
    words.collect::<Vec<_>>().join(", then ")
}

impl super::Plait {
    pub(super) fn shortcut_label(&self, id: &str) -> String {
        ACTIONS
            .iter()
            .find(|action| action.id == id)
            .map(|action| label(&bound(action, &self.preferences.value.keymap)))
            .unwrap_or_default()
    }

    pub(super) fn shortcut_aria(&self, id: &str) -> String {
        ACTIONS
            .iter()
            .find(|action| action.id == id)
            .map(|action| {
                bound(action, &self.preferences.value.keymap)
                    .iter()
                    .map(|key| named(key, ["Control", "Alt", "Meta", "Shift", "Fn"]).join("+"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }
}

const JUMP: &str = "jump.";

/// One line of the shortcut editor and of the documented table: one action,
/// or the nine sidebar jumps, which share their modifiers and change as a set.
#[derive(Clone, Copy)]
pub(super) struct Shortcut {
    pub actions: &'static [AppAction],
}

/// Every action as editor rows, in table order.
pub(super) fn shortcuts() -> Vec<Shortcut> {
    ACTIONS
        .chunk_by(|a, b| {
            (a.id.starts_with(JUMP) && b.id.starts_with(JUMP))
                || (a.id.starts_with("tab_jump.") && b.id.starts_with("tab_jump."))
        })
        .map(|actions| Shortcut { actions })
        .collect()
}

/// What a row is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Binding {
    Unbound,
    Keys(Vec<Keystroke>),
    /// The jump set, from its first keystroke to its last.
    Range(Keystroke, Keystroke),
    /// Jumps that no longer share modifiers: edited in the file, or one of
    /// them gave its keystroke to another action.
    Mixed,
}

impl Binding {
    /// As the menus would show it. Empty when unbound.
    pub fn label(&self) -> String {
        let one = |key: &Keystroke| label(std::slice::from_ref(key));
        match self {
            Self::Unbound => String::new(),
            Self::Keys(keys) => label(keys),
            Self::Range(first, last) => format!("{} – {}", one(first), one(last)),
            Self::Mixed => "Mixed".into(),
        }
    }

    /// In words, for assistive technology.
    pub fn spoken(&self) -> String {
        let one = |key: &Keystroke| spoken(std::slice::from_ref(key));
        match self {
            Self::Unbound => "none".into(),
            Self::Keys(keys) => spoken(keys),
            Self::Range(first, last) => format!("{} to {}", one(first), one(last)),
            Self::Mixed => "mixed".into(),
        }
    }
}

impl Shortcut {
    pub fn is_set(&self) -> bool {
        self.actions.len() > 1
    }

    pub fn title(&self) -> &'static str {
        if self.is_set() {
            if self.actions[0].id.starts_with("tab_jump.") {
                "Switch to Tab 1–9"
            } else {
                "Jump to Sidebar Item 1–9"
            }
        } else {
            self.actions[0].title
        }
    }

    pub fn category(&self) -> &'static str {
        self.actions[0].category
    }

    /// The keystroke each action would get if `pressed` were recorded on this
    /// row. The jump set takes the modifiers and keeps its numbers.
    pub fn candidate(&self, pressed: &Keystroke) -> Result<Vec<Keystroke>, String> {
        let with_key = |key: &str| Keystroke {
            modifiers: pressed.modifiers,
            key: key.to_owned(),
            // What the key would type is not part of a binding.
            key_char: None,
        };
        if !self.is_set() {
            return Ok(vec![with_key(&pressed.key)]);
        }
        let numbers = self
            .actions
            .iter()
            .map(|action| action.id.rsplit_once('.').expect("numbered action").1);
        if !numbers.clone().any(|number| number == pressed.key) {
            return Err(
                "The nine jumps change together. Hold the modifiers you want and press a number, such as ⌃1."
                    .into(),
            );
        }
        Ok(numbers.map(with_key).collect())
    }

    /// The default keystroke of each action. Empty unless every action has one.
    pub fn defaults(&self) -> Vec<Keystroke> {
        let keys = self.actions.iter().map(AppAction::default_key);
        keys.collect::<Option<Vec<_>>>().unwrap_or_default()
    }

    pub fn binding(&self, overrides: &KeyOverrides) -> Binding {
        let mut bound = self.actions.iter().map(|action| bound(action, overrides));
        if !self.is_set() {
            return match bound.next() {
                Some(keys) if !keys.is_empty() => Binding::Keys(keys),
                _ => Binding::Unbound,
            };
        }
        let bound = bound.collect::<Vec<_>>();
        if bound.iter().all(Vec::is_empty) {
            return Binding::Unbound;
        }
        // A set is its first keystroke's modifiers on every number.
        let expected = bound[0]
            .first()
            .and_then(|first| self.candidate(first).ok());
        let single = bound.iter().map(|keys| match keys.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        });
        match (expected, single.collect::<Option<Vec<_>>>()) {
            (Some(expected), Some(actual)) if expected == actual => {
                let last = actual.len() - 1;
                Binding::Range(actual[0].clone(), actual[last].clone())
            }
            _ => Binding::Mixed,
        }
    }

    /// Whether any action of the row is bound to something other than its default.
    pub fn changed(&self, overrides: &KeyOverrides) -> bool {
        self.actions.iter().any(|action| {
            let default = action.default_key().into_iter().collect::<Vec<_>>();
            bound(action, overrides) != default
        })
    }

    /// A saved entry for this row that is not a keystroke. Its default applies.
    pub fn unusable<'a>(&self, overrides: &'a KeyOverrides) -> Option<&'a str> {
        self.actions.iter().find_map(|action| {
            let saved = overrides.get(action.id)?;
            (!saved.is_empty() && !parses(saved)).then_some(saved.as_str())
        })
    }

    /// Whether every search term is in the row's title, menu, or shortcut,
    /// written as the menus show it (`⌘N`), as the file does (`cmd-n`), or in
    /// words (`command n`).
    pub fn matches(&self, overrides: &KeyOverrides, query: &str) -> bool {
        let binding = self.binding(overrides);
        let written = self.actions.iter().map(|action| {
            let keys = bound(action, overrides);
            let notation = keys.iter().map(|key| key.unparse(KEY_LABELS));
            notation.collect::<Vec<_>>().join(" ")
        });
        let text = [
            self.title().to_owned(),
            self.category().to_owned(),
            binding.label(),
            binding.spoken(),
            written.collect::<Vec<_>>().join(" "),
        ]
        .join("\n")
        .to_lowercase();
        let query = query.to_lowercase();
        query.split_whitespace().all(|term| text.contains(term))
    }
}

/// Shortcuts macOS acts on before, or instead of, the application.
const SYSTEM_KEYS: &[(&str, &str)] = &[
    ("cmd-tab", "switches applications"),
    ("cmd-shift-tab", "switches applications"),
    ("cmd-space", "opens Spotlight"),
    ("ctrl-space", "switches the input source"),
    ("cmd-`", "cycles windows"),
    ("cmd-shift-3", "takes a screenshot"),
    ("cmd-shift-4", "takes a screenshot"),
    ("cmd-shift-5", "takes a screenshot"),
    ("cmd-shift-q", "logs out"),
    ("ctrl-cmd-q", "locks the screen"),
    ("cmd-alt-escape", "opens Force Quit"),
    ("cmd-alt-d", "hides the Dock"),
    ("ctrl-up", "opens Mission Control"),
    ("ctrl-down", "shows application windows"),
    ("ctrl-left", "moves between Spaces"),
    ("ctrl-right", "moves between Spaces"),
];

/// Why one keystroke must not become an application shortcut, if anything
/// stops it. `editing` are the keys the composer and text fields bind.
fn reserved(key: &Keystroke, editing: &[Keystroke]) -> Option<String> {
    let only = std::slice::from_ref(key);
    let shown = label(only);
    let held = key.modifiers;
    if key.is_modifier_key() {
        return Some("A modifier alone is not a shortcut. Hold it and press a key.".into());
    }
    // Shift and Option still type: a capital, or a character such as ß.
    let types = key.key.chars().count() == 1 || key.key == "space";
    if types && !(held.platform || held.control || held.function) {
        return Some(format!(
            "{shown} types a character, so it would stop text entry. Add Command or Control."
        ));
    }
    if matches!(key.key.as_str(), "enter" | "tab" | "escape") && !held.modified() {
        return Some(format!(
            "{shown} confirms, moves focus, or cancels throughout Plait. Add a modifier."
        ));
    }
    if editing.contains(key) {
        return Some(format!(
            "{shown} edits text in the composer and text fields, where it would not reach this action."
        ));
    }
    let system = |(keys, effect): &(&str, &'static str)| {
        (parsed(keys)?.as_slice() == only).then_some(*effect)
    };
    if let Some(effect) = SYSTEM_KEYS.iter().find_map(system) {
        return Some(format!(
            "{shown} {effect} on macOS and does not reach Plait."
        ));
    }
    // What is saved must read back as the same keystroke.
    if parsed(&key.unparse(KEY_LABELS)).as_deref() != Some(only) {
        return Some("That key cannot be saved as a shortcut.".into());
    }
    None
}

/// Why `keys` must not become the shortcuts of `shortcut`'s actions, if
/// anything stops it. An action's own default is always allowed: Tab moves
/// focus by default, though no other action may take it.
pub(super) fn refusal(
    shortcut: &Shortcut,
    keys: &[Keystroke],
    editing: &[Keystroke],
) -> Option<String> {
    let mut pairs = shortcut.actions.iter().zip(keys);
    pairs.find_map(|(action, key)| {
        (action.default_key().as_ref() != Some(key))
            .then(|| reserved(key, editing))
            .flatten()
    })
}

/// Other actions bound to one of `keys` now. A longer sequence that starts
/// with one counts: the two would wait on each other.
pub(super) fn conflicts(
    shortcut: &Shortcut,
    keys: &[Keystroke],
    overrides: &KeyOverrides,
) -> Vec<&'static AppAction> {
    let own = |action: &AppAction| shortcut.actions.iter().any(|own| own.id == action.id);
    ACTIONS
        .iter()
        .filter(|action| !own(action))
        .filter(|action| {
            bound(action, overrides)
                .first()
                .is_some_and(|first| keys.contains(first))
        })
        .collect()
}

/// Record one action's binding. An entry equal to the default is removed, so
/// the map holds only what differs. `None` unbinds.
fn set(overrides: &mut KeyOverrides, action: &AppAction, key: Option<&Keystroke>) {
    if key == action.default_key().as_ref() {
        overrides.remove(action.id);
        return;
    }
    let keys = key.map(|key| key.unparse(KEY_LABELS)).unwrap_or_default();
    overrides.insert(action.id.to_owned(), keys);
}

/// Bind `keys` to the actions of `shortcut`, and unbind the `displaced`
/// actions that held them.
pub(super) fn assign(
    overrides: &mut KeyOverrides,
    shortcut: &Shortcut,
    keys: &[Keystroke],
    displaced: &[&AppAction],
) {
    for action in displaced {
        set(overrides, action, None);
    }
    for (action, key) in shortcut.actions.iter().zip(keys) {
        set(overrides, action, Some(key));
    }
}

/// Leave the row's actions without a shortcut.
pub(super) fn unbind(overrides: &mut KeyOverrides, shortcut: &Shortcut) {
    for action in shortcut.actions {
        set(overrides, action, None);
    }
}

/// Return every action in the table to its default. Entries for IDs this
/// client does not know belong to another version and stay.
pub(super) fn reset_all(overrides: &mut KeyOverrides) {
    for action in ACTIONS {
        overrides.remove(action.id);
    }
}

/// Entries in the saved keymap for actions this client does not have.
pub(super) fn unknown(overrides: &KeyOverrides) -> usize {
    let known = |id: &String| ACTIONS.iter().any(|action| action.id == id);
    overrides.keys().filter(|id| !known(id)).count()
}

/// `cmd-shift-n` as documentation writes it: `Cmd-Shift-N`.
#[cfg(test)]
fn documented(keystroke: &str) -> String {
    let parts = keystroke.split('-').map(|part| match part {
        "cmd" => "Cmd".to_owned(),
        "alt" => "Option".to_owned(),
        "ctrl" => "Control".to_owned(),
        "," => "comma".to_owned(),
        "." => "period".to_owned(),
        "\\" => "Backslash".to_owned(),
        part => {
            let mut letters = part.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(letters).collect()
            })
        }
    });
    parts.collect::<Vec<_>>().join("-")
}

/// The default shortcuts as the Markdown table in docs/SHORTCUTS.md. The nine
/// jumps are one row there, as they are in the shortcut editor.
#[cfg(test)]
fn keyboard_table() -> String {
    let mut table = String::from("| Shortcut | Action | ID |\n| --- | --- | --- |\n");
    for shortcut in shortcuts() {
        let first = &shortcut.actions[0];
        let last = &shortcut.actions[shortcut.actions.len() - 1];
        let (Some(from), Some(to)) = (first.key, last.key) else {
            continue;
        };
        let (from, to) = (documented(from), documented(to));
        let row = if shortcut.is_set() {
            let (title, first, last) = (shortcut.title(), first.id, last.id);
            format!("| {from} to {to} | {title} | `{first}` to `{last}` |\n")
        } else {
            format!("| {from} | {} | `{}` |\n", first.title, first.id)
        };
        table.push_str(&row);
    }
    table
}

#[cfg(test)]
mod tests {
    // Named imports: a glob would bring in GPUI's own `test` attribute.
    use super::{
        ACTIONS, KeyOverrides, documented, keyboard_table, keystroke, menus, parses, reset_all,
        unknown, winning_bindings,
    };
    use std::collections::HashSet;

    #[test]
    fn recording_rejects_text_and_reserved_keys_and_reassigns_conflicts() {
        let shortcuts = super::shortcuts();
        let sidebar = shortcuts
            .iter()
            .find(|row| row.actions[0].id == "view.toggle_sidebar")
            .unwrap();
        let parsed = |key: &str| super::parsed(key).unwrap();
        for key in [
            "a",
            "shift-a",
            "alt-a",
            "enter",
            "tab",
            "escape",
            "cmd-space",
            "cmd-tab",
        ] {
            assert!(
                super::refusal(sidebar, &parsed(key), &[]).is_some(),
                "{key}"
            );
        }
        assert!(super::refusal(sidebar, &parsed("cmd-c"), &parsed("cmd-c")).is_some());
        let mut saved = KeyOverrides::new();
        let keys = parsed("cmd-k");
        let displaced = super::conflicts(sidebar, &keys, &saved);
        assert_eq!(displaced.len(), 1);
        assert_eq!(displaced[0].id, "palette.toggle");
        super::assign(&mut saved, sidebar, &keys, &displaced);
        assert_eq!(saved["palette.toggle"], "");
        assert_eq!(super::bound(&sidebar.actions[0], &saved), keys);
        saved.insert("future.action".into(), "cmd-y".into());
        super::reset_all(&mut saved);
        assert_eq!(saved.len(), 1);
        assert!(saved.contains_key("future.action"));
    }

    #[test]
    fn sidebar_jump_keys_change_as_one_set() {
        let shortcuts = super::shortcuts();
        let jumps = shortcuts.iter().find(|row| row.is_set()).unwrap();
        let pressed = super::parsed("ctrl-4").unwrap().remove(0);
        let keys = jumps.candidate(&pressed).unwrap();
        assert_eq!(keys.len(), 9);
        let mut saved = KeyOverrides::new();
        super::assign(&mut saved, jumps, &keys, &[]);
        assert!(matches!(jumps.binding(&saved), super::Binding::Range(_, _)));
        assert_eq!(keys[0].key, "1");
        assert_eq!(keys[8].key, "9");
        super::unbind(&mut saved, jumps);
        assert_eq!(jumps.binding(&saved), super::Binding::Unbound);
    }

    #[test]
    fn ids_are_unique_and_default_keystrokes_parse_without_collisions() {
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        let mut names = HashSet::new();
        for action in ACTIONS {
            assert!(ids.insert(action.id), "duplicate ID {}", action.id);
            assert!(
                names.insert(action.action().name()),
                "{} repeats an action",
                action.id
            );
            assert!(!action.title.is_empty() && !action.category.is_empty());
            if let Some(key) = action.key {
                assert!(parses(key), "{key} does not parse");
                assert!(
                    !super::SYSTEM_KEYS
                        .iter()
                        .any(|(reserved, _)| super::parsed(reserved) == super::parsed(key)),
                    "{key} is reserved by macOS"
                );
                assert!(keys.insert(key), "{key} is bound twice");
            }
        }
    }

    #[test]
    fn a_saved_shortcut_survives_the_new_history_default() {
        let index = |id| ACTIONS.iter().position(|action| action.id == id).unwrap();
        let history = index("view.history");
        let new_thread = index("thread.new");
        let mut saved = KeyOverrides::from([("thread.new".into(), "cmd-y".into())]);
        let wins = winning_bindings(&saved);
        assert!(wins[new_thread]);
        assert!(!wins[history]);

        saved.insert("view.history".into(), "ctrl-alt-y".into());
        assert_eq!(keystroke(&ACTIONS[history], &saved), Some("ctrl-alt-y"));
        assert!(winning_bindings(&saved)[history]);
    }

    #[test]
    fn each_category_is_one_contiguous_menu() {
        let mut seen = Vec::new();
        for action in ACTIONS {
            if seen.last() != Some(&action.category) {
                assert!(
                    !seen.contains(&action.category),
                    "{} is split",
                    action.category
                );
                seen.push(action.category);
            }
        }
        let names = menus()
            .iter()
            .map(|menu| menu.name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["Plait", "File", "View", "Thread", "Git", "Window"]
        );
    }

    #[test]
    fn an_override_replaces_the_default_and_a_bad_one_does_not_remove_it() {
        let new_thread = ACTIONS.iter().find(|a| a.id == "thread.new").unwrap();
        let updates = ACTIONS
            .iter()
            .find(|a| a.id == "app.check_for_updates")
            .unwrap();
        let mut overrides = KeyOverrides::new();
        assert_eq!(keystroke(new_thread, &overrides), Some("cmd-n"));
        assert_eq!(keystroke(updates, &overrides), None);
        overrides.insert("thread.new".into(), "cmd-t".into());
        overrides.insert("app.check_for_updates".into(), "cmd-shift-u".into());
        assert_eq!(keystroke(new_thread, &overrides), Some("cmd-t"));
        assert_eq!(keystroke(updates, &overrides), Some("cmd-shift-u"));
        // Empty unbinds; a keystroke that does not parse falls back.
        overrides.insert("thread.new".into(), String::new());
        assert_eq!(keystroke(new_thread, &overrides), None);
        overrides.insert("thread.new".into(), "cmd-n-x".into());

        assert_eq!(keystroke(new_thread, &overrides), Some("cmd-n"));
        overrides.insert("unknown.action".into(), "cmd-u".into());
        assert_eq!(keystroke(new_thread, &overrides), Some("cmd-n"));
    }

    #[test]
    fn keystrokes_are_documented_in_words() {
        assert_eq!(documented("cmd-shift-n"), "Cmd-Shift-N");
        assert_eq!(documented("cmd-,"), "Cmd-comma");
        assert_eq!(documented("cmd-alt-down"), "Cmd-Option-Down");
        assert_eq!(documented("shift-tab"), "Shift-Tab");
        assert_eq!(documented("cmd-alt-."), "Cmd-Option-period");
    }

    #[test]
    fn retired_pane_actions_never_bind_or_appear_in_menus() {
        let mut overrides = KeyOverrides::from([
            ("pane.split_right".into(), "cmd-n".into()),
            ("pane.split_down".into(), "cmd-8".into()),
            ("pane.close".into(), "cmd-7".into()),
        ]);
        assert_eq!(unknown(&overrides), 3);
        assert_eq!(
            winning_bindings(&overrides),
            winning_bindings(&KeyOverrides::new())
        );
        let previous = overrides.clone();
        reset_all(&mut overrides);
        assert_eq!(overrides, previous);
    }

    /// An approval answer must not come from a typing slip or from the
    /// composer's own keys, and Allow must not be one key away from Deny.
    #[test]
    fn approval_answers_need_command_and_stay_apart() {
        let default = |id: &str| {
            let action = ACTIONS.iter().find(|action| action.id == id).unwrap();
            assert_eq!(action.category, "Thread", "{id}");
            action.default_key().unwrap()
        };
        let (allow, deny) = (
            default("approval.allow_once"),
            default("approval.deny_once"),
        );
        // The composer's keys: send, new line, steer, stop, and send by Command-Enter.
        let composer = ["enter", "shift-enter", "alt-enter", "escape", "cmd-enter"]
            .map(|key| super::parsed(key).unwrap().remove(0));
        for key in [&allow, &deny] {
            assert!(key.modifiers.platform && key.modifiers.alt, "{key:?}");
            assert!(!key.modifiers.shift && !key.modifiers.control, "{key:?}");
            assert!(super::reserved(key, &composer).is_none(), "{key:?}");
        }
        assert_ne!(allow.key, deny.key);
        // Neither rides on a key that confirms, cancels, deletes, or moves focus.
        for key in [&allow.key, &deny.key] {
            assert!(!["enter", "escape", "backspace", "tab"].contains(&key.as_str()));
        }
    }

    #[test]
    fn the_documented_keyboard_table_is_this_table() {
        let documentation = include_str!("../../../../docs/SHORTCUTS.md");
        let table = keyboard_table();
        assert!(
            documentation.contains(&table),
            "docs/SHORTCUTS.md is out of date. Replace its application shortcut table with:\n\n{table}"
        );
    }
}
