//! Desktop-only preferences. Blocking file access: call on the background executor.
//!
//! The host owns model defaults and credentials. This file holds presentation
//! choices that belong to one desktop installation, so it never leaves the Mac.

use crate::{
    handoff::{self, EditorChoice, Purpose, TerminalChoice},
    host::{ModelChoice, ThinkingLevel, harness::HarnessId},
    model::Project,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// Larger files are not ours; refuse them rather than parse arbitrary input.
const FILE_LIMIT: u64 = 512 * 1024;

/// How requests and replies are drawn on this desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Turns {
    #[default]
    Bubbles,
    Line,
}

/// What Cmd-1 through Cmd-9 select, counted in sidebar order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JumpTarget {
    #[default]
    Threads,
    Machines,
    Projects,
}

impl JumpTarget {
    pub const ALL: [Self; 3] = [Self::Threads, Self::Machines, Self::Projects];

    pub fn label(self) -> &'static str {
        match self {
            Self::Threads => "Threads",
            Self::Machines => "Machines",
            Self::Projects => "Projects",
        }
    }
}

/// Favorites kept in the picker. The oldest is dropped beyond this.
pub const FAVORITE_LIMIT: usize = 64;
/// Recently used models kept, newest first.
pub const RECENT_LIMIT: usize = 8;
/// Longer identifiers are not remembered, so full lists always fit the file limit.
const MODEL_KEY_LIMIT: usize = 512;

/// Shortcut entries kept. A full keymap and full model lists fit the file limit.
pub const KEYMAP_LIMIT: usize = 128;
/// Longer action IDs and keystrokes are not remembered.
const KEYMAP_ID_LIMIT: usize = 64;
const KEYMAP_KEYS_LIMIT: usize = 48;

/// Shortcuts that replace the defaults: action ID to keystroke, such as
/// `"thread.new": "cmd-t"`. An empty keystroke leaves the action unbound.
/// The desktop decides what an ID and a keystroke mean; this is only storage.
pub type Keymap = BTreeMap<String, String>;

/// Bounds and default for the code font size, in points.
pub const CODE_SIZE_DEFAULT: u8 = 12;
pub const CODE_SIZE_MIN: u8 = 12;
pub const CODE_SIZE_MAX: u8 = 14;

/// Interface font family for labels, controls, and conversation text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceFont {
    #[default]
    Plex,
    System,
}

impl InterfaceFont {
    pub const ALL: [Self; 2] = [Self::Plex, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::Plex => "Plex Sans",
            Self::System => "System",
        }
    }
}

/// Clamp a point size to the allowed code size range (12–14).
pub fn clamp_code_size(size: u8) -> u8 {
    size.clamp(CODE_SIZE_MIN, CODE_SIZE_MAX)
}

/// A model chosen on this desktop: an identifier, never a credential.
/// Unlike the wire type, unknown fields are ignored so a newer file still loads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelKey {
    pub provider: String,
    pub model_id: String,
}

impl ModelKey {
    fn fits(&self) -> bool {
        self.provider.len() + self.model_id.len() <= MODEL_KEY_LIMIT
    }
}

impl From<&ModelChoice> for ModelKey {
    fn from(choice: &ModelChoice) -> Self {
        Self {
            provider: choice.provider.clone(),
            model_id: choice.model_id.clone(),
        }
    }
}

impl PartialEq<ModelChoice> for ModelKey {
    fn eq(&self, choice: &ModelChoice) -> bool {
        self.provider == choice.provider && self.model_id == choice.model_id
    }
}

/// Unknown fields are ignored so a newer client's file still loads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub navigation: crate::navigation::Preferences,
    /// This device's ID on every host it uses, for per-device state there.
    pub device_id: String,
    /// Hosts that already hold this device's project lists, by host ID.
    pub imported_workspaces: Vec<String>,
    pub machines: crate::machines::Machines,
    /// The machine selected on this desktop; older preferences default to local.
    #[serde(deserialize_with = "lenient")]
    pub selected_machine: crate::machines::MachineId,
    pub jump_target: JumpTarget,
    /// Ask before each edit and command. Off by default: runs approve their own
    /// edits and sandboxed commands, and file protections still apply.
    pub ask_before_actions: bool,
    /// Hide the Git inspector beside the conversation on wide windows.
    pub hide_inspector: bool,
    /// Hide the card above the composer listing the files a thread changed.
    pub hide_changed_files: bool,
    /// Widths dragged on this desktop, in points. `None` keeps the default;
    /// the desktop fits a saved width to the window it is drawn in.
    pub sidebar_width: Option<u16>,
    pub inspector_width: Option<u16>,
    /// Models pinned to the top of the picker, oldest first.
    pub favorite_models: Vec<ModelKey>,
    /// Models used to start threads from this desktop, newest first.
    pub recent_models: Vec<ModelKey>,
    /// The thinking level new threads start at; see `default_thinking`.
    #[serde(deserialize_with = "lenient_level")]
    pub default_thinking: Option<ThinkingLevel>,
    /// Shortcuts changed on this desktop. Action IDs this client does not know
    /// are kept, so an entry written by a newer client survives an older one.
    pub keymap: Keymap,
    /// Follow macOS, or keep one appearance.
    #[serde(deserialize_with = "lenient")]
    pub appearance: crate::theme::Appearance,
    /// Theme names for each appearance; `None` is the default (Plait Light, Stillwater).
    pub light_theme: Option<String>,
    pub dark_theme: Option<String>,
    /// Interface font family (`plex` or `system`). Older files default to `plex`.
    #[serde(deserialize_with = "lenient")]
    pub interface_font: InterfaceFont,
    /// The font family code, paths, and commands are drawn in; `None` is the
    /// system's monospaced font. Read it through `code_font`.
    pub code_font: Option<String>,
    /// The code font size, in points (12–14); `None` is the default (12).
    /// Read it through `code_size` and change it through `set_code_size`.
    #[serde(deserialize_with = "lenient_code_size")]
    pub code_size: Option<u8>,
    #[serde(deserialize_with = "lenient")]
    pub turns: Turns,
    /// Fold turns before the latest in the conversation. `None` means on; read
    /// through `fold_earlier_turns`.
    #[serde(deserialize_with = "lenient_bool")]
    pub fold_earlier_turns: Option<bool>,
    #[serde(deserialize_with = "lenient_bool")]
    pub fold_finished_work: Option<bool>,
    /// The harness a new thread's draft starts in, when it is ready. Never `Other`:
    /// one this client does not know reads, and is written, as Pi.
    #[serde(deserialize_with = "lenient_harness", serialize_with = "known_harness")]
    pub default_harness: HarnessId,
    /// What Open in Editor runs. Set it through `set_open_files_with`.
    #[serde(deserialize_with = "lenient")]
    pub open_files_with: EditorChoice,
    /// What Open Terminal Here runs. Set it through `set_open_terminals_with`.
    #[serde(deserialize_with = "lenient")]
    pub open_terminals_with: TerminalChoice,
}

/// A value this client does not know (written by a newer one) reads as the
/// default rather than failing the whole file.
fn lenient<'de, D: serde::Deserializer<'de>, T: Default + serde::de::DeserializeOwned>(
    deserializer: D,
) -> Result<T, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}

/// A level this client does not know (written by a newer one) reads as unset
/// rather than failing the whole file.
fn lenient_level<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ThinkingLevel>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

/// A stored code size outside 12–14 is clamped rather than rejected, and an
/// unreadable value (e.g. from a newer client) reads as unset.
fn lenient_code_size<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u8>, D::Error> {
    let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let clamped = if let Some(n) = value.as_i64() {
        Some(n.clamp(CODE_SIZE_MIN as i64, CODE_SIZE_MAX as i64) as u8)
    } else if let Some(n) = value.as_u64() {
        Some(n.clamp(CODE_SIZE_MIN as u64, CODE_SIZE_MAX as u64) as u8)
    } else {
        value
            .as_f64()
            .filter(|n| n.is_finite())
            .map(|n| n.round().clamp(CODE_SIZE_MIN as f64, CODE_SIZE_MAX as f64) as u8)
    };
    Ok(clamped)
}

/// An unexpected value for a boolean preference reads as unset rather than
/// failing the whole file.
fn lenient_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| value.as_bool()))
}

/// A harness this client cannot run (from a newer one) reads as Pi rather
/// than failing the whole file.
fn lenient_harness<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HarnessId, D::Error> {
    let id: HarnessId = lenient(deserializer)?;
    Ok(if id == HarnessId::Other {
        HarnessId::Pi
    } else {
        id
    })
}

/// `Other` has no name to write, and a failed encode would block every save.
fn known_harness<S: serde::Serializer>(id: &HarnessId, serializer: S) -> Result<S::Ok, S::Error> {
    let id = if *id == HarnessId::Other {
        HarnessId::Pi
    } else {
        *id
    };
    id.serialize(serializer)
}

impl Preferences {
    /// Unset means Medium: with thinking off, reasoning models answer worse,
    /// and that should not be what a new thread gets without asking.
    pub fn default_thinking(&self) -> ThinkingLevel {
        self.default_thinking.unwrap_or(ThinkingLevel::Medium)
    }

    /// The chosen code font family, or `None` for the system's. A blank name
    /// is no choice.
    pub fn code_font(&self) -> Option<&str> {
        self.code_font
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    /// The chosen code font size in points, clamped to 12–14 (default 12).
    pub fn code_size(&self) -> u8 {
        self.code_size
            .map(clamp_code_size)
            .unwrap_or(CODE_SIZE_DEFAULT)
    }

    /// Choose the code font size in points, clamped to 12–14.
    /// The default is stored as unset so it stays aligned with `Default`.
    pub fn set_code_size(&mut self, size: u8) {
        let clamped = clamp_code_size(size);
        self.code_size = (clamped != CODE_SIZE_DEFAULT).then_some(clamped);
    }

    /// Whether turns before the latest draw folded. On by default, including
    /// for files saved before the preference existed.
    pub fn fold_earlier_turns(&self) -> bool {
        self.fold_earlier_turns.unwrap_or(true)
    }

    pub fn fold_finished_work(&self) -> bool {
        self.fold_finished_work.unwrap_or(true)
    }

    pub fn is_favorite(&self, choice: &ModelChoice) -> bool {
        self.favorite_models.iter().any(|key| key == choice)
    }

    /// Add or remove a favorite. Returns whether the model is a favorite now.
    pub fn toggle_favorite(&mut self, choice: &ModelChoice) -> bool {
        let before = self.favorite_models.len();
        self.favorite_models.retain(|key| key != choice);
        let key = ModelKey::from(choice);
        if self.favorite_models.len() != before || !key.fits() {
            return false;
        }
        self.favorite_models.push(key);
        trim_oldest(&mut self.favorite_models, FAVORITE_LIMIT);
        true
    }

    /// Move a model to the front of the recent list.
    pub fn record_recent(&mut self, choice: &ModelChoice) {
        let key = ModelKey::from(choice);
        if !key.fits() {
            return;
        }
        self.recent_models.retain(|recent| recent != choice);
        self.recent_models.insert(0, key);
        self.recent_models.truncate(RECENT_LIMIT);
    }

    /// Choose what opens files. A custom command that cannot be used is not
    /// kept; the error says why.
    pub fn set_open_files_with(&mut self, choice: EditorChoice) -> Result<(), String> {
        if let EditorChoice::Custom(command) = &choice {
            handoff::template(command, Purpose::Editor)?;
        }
        self.open_files_with = choice;
        Ok(())
    }

    /// Choose what opens terminals, as `set_open_files_with` does.
    pub fn set_open_terminals_with(&mut self, choice: TerminalChoice) -> Result<(), String> {
        if let TerminalChoice::Custom(command) = &choice {
            handoff::template(command, Purpose::Terminal)?;
        }
        self.open_terminals_with = choice;
        Ok(())
    }

    /// Replace the shortcut overrides, within the limits a load applies.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
        self.trim_keymap();
    }

    /// Entries past the limits are dropped from the end of the ID order.
    fn trim_keymap(&mut self) {
        self.keymap
            .retain(|id, keys| id.len() <= KEYMAP_ID_LIMIT && keys.len() <= KEYMAP_KEYS_LIMIT);
        while self.keymap.len() > KEYMAP_LIMIT {
            self.keymap.pop_last();
        }
    }

    /// A hand-edited file may repeat entries or exceed the limits.
    fn normalize(mut self) -> Self {
        self.navigation.normalize();
        for list in [&mut self.favorite_models, &mut self.recent_models] {
            let mut seen = std::collections::HashSet::new();
            list.retain(|key| key.fits() && seen.insert(key.clone()));
        }
        trim_oldest(&mut self.favorite_models, FAVORITE_LIMIT);
        self.recent_models.truncate(RECENT_LIMIT);
        self.trim_keymap();
        self.code_size = self.code_size.map(clamp_code_size);
        // A hand-edited command that cannot be used reads as the default.
        let files = std::mem::take(&mut self.open_files_with);
        let _ = self.set_open_files_with(files);
        let terminals = std::mem::take(&mut self.open_terminals_with);
        let _ = self.set_open_terminals_with(terminals);
        self
    }
}

fn trim_oldest(list: &mut Vec<ModelKey>, limit: usize) {
    let excess = list.len().saturating_sub(limit);
    list.drain(..excess);
}

/// Overlay changes made before disk loading finished without losing untouched
/// settings from disk. New preference fields participate through serialization.
pub fn merge_startup(loaded: Preferences, edited: &Preferences) -> Result<Preferences, String> {
    fn merge(
        loaded: serde_json::Value,
        defaults: serde_json::Value,
        edited: serde_json::Value,
    ) -> serde_json::Value {
        if edited == defaults {
            return loaded;
        }
        match (loaded, defaults, edited) {
            (
                serde_json::Value::Object(mut loaded),
                serde_json::Value::Object(defaults),
                serde_json::Value::Object(edited),
            ) => {
                for (key, value) in edited {
                    let old = loaded.remove(&key).unwrap_or(serde_json::Value::Null);
                    let default = defaults
                        .get(&key)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    loaded.insert(key, merge(old, default, value));
                }
                serde_json::Value::Object(loaded)
            }
            (_, _, edited) => edited,
        }
    }
    let opened = loaded.navigation.opened.clone();
    let encode = |preferences: &Preferences| {
        serde_json::to_value(preferences).map_err(|_| "Cannot merge saved preferences.".to_owned())
    };
    let value = merge(
        encode(&loaded)?,
        encode(&Preferences::default())?,
        encode(edited)?,
    );
    let mut preferences: Preferences =
        serde_json::from_value(value).map_err(|_| "Cannot merge saved preferences.".to_owned())?;
    for checkout in opened {
        if !preferences.navigation.hidden.contains(&checkout) {
            preferences.navigation.open(&checkout);
        }
    }
    Ok(preferences)
}

/// One atomic write at a time; edits during it coalesce to the newest snapshot.
#[derive(Default)]
pub struct SaveQueue {
    pending: Option<(u64, Preferences)>,
    running: Option<u64>,
}

impl SaveQueue {
    pub fn queue(&mut self, revision: u64, value: Preferences) {
        self.pending = Some((revision, value));
    }

    pub fn next_write(&mut self) -> Option<(u64, Preferences)> {
        if self.running.is_some() {
            return None;
        }
        let next = self.pending.take()?;
        self.running = Some(next.0);
        Some(next)
    }

    pub fn finish(&mut self, revision: u64) {
        if self.running == Some(revision) {
            self.running = None;
        }
    }
}

/// The per-user location, from the environment. `None` without a home directory.
pub fn default_path() -> Option<PathBuf> {
    // A development instance must not write the installed desktop's choices.
    if let Some(path) = std::env::var_os("PLAIT_DESKTOP_PREFERENCES_FILE") {
        let path = PathBuf::from(path);
        return path.is_absolute().then_some(path);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|home| home.join("Library/Application Support/Plait/desktop.json"));
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))
        .map(|config| config.join("plait/desktop.json"))
}

/// A missing file means defaults. A malformed one is an error, not silently reset.
pub fn load(path: &Path) -> Result<Preferences, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Preferences::default());
        }
        Err(error) => return Err(format!("Cannot read preferences: {error}")),
    };
    let size = file
        .metadata()
        .map_err(|error| format!("Cannot read preferences: {error}"))?
        .len();
    if size > FILE_LIMIT {
        return Err("Preferences file is too large; defaults are in use.".into());
    }
    serde_json::from_reader(std::io::BufReader::new(file))
        .map(Preferences::normalize)
        .map_err(|error| format!("Preferences file is invalid ({error}); defaults are in use."))
}

/// Replace the file atomically so a crash never leaves a half-written file.
pub fn save(path: &Path, preferences: &Preferences) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("Cannot save preferences: {error}");
    let bytes = serde_json::to_vec_pretty(preferences)
        .map_err(|error| format!("Cannot encode preferences: {error}"))?;
    if bytes.len() as u64 + 1 > FILE_LIMIT {
        return Err("Preferences exceed the file limit; the saved file was left intact.".into());
    }
    let directory = path
        .parent()
        .ok_or_else(|| "Cannot save preferences: no parent directory.".to_owned())?;
    fs::create_dir_all(directory).map_err(fail)?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(fail)
}

/// One sidebar thread and the project and machine rows that contain it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jump {
    pub project: usize,
    pub machine: usize,
    /// Index into the host's thread list. Host threads belong to the local machine.
    pub thread: usize,
}

/// Threads grouped by machine, in sidebar order.
fn machines(projects: &[Project], host_threads: usize) -> Vec<Vec<Jump>> {
    let mut rows = Vec::new();
    for (project, p) in projects.iter().enumerate() {
        for (machine, m) in p.machines.iter().enumerate() {
            let threads = if m.local { host_threads } else { 0 };
            rows.push(
                (0..threads)
                    .map(|thread| Jump {
                        project,
                        machine,
                        thread,
                    })
                    .collect(),
            );
        }
    }
    rows
}

/// The thread for Cmd-`number` (1-based). A machine or project selects its
/// first thread. Returns `None` when there is no such row or it has no thread.
pub fn jump(
    projects: &[Project],
    host_threads: usize,
    target: JumpTarget,
    number: usize,
) -> Option<Jump> {
    let index = number.checked_sub(1)?;
    let machines = machines(projects, host_threads);
    match target {
        JumpTarget::Threads => machines.into_iter().flatten().nth(index),
        JumpTarget::Machines => machines.get(index)?.first().copied(),
        JumpTarget::Projects => machines
            .into_iter()
            .flatten()
            .find(|jump| jump.project == index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Machine;

    #[test]
    fn preferences_machines_old_and_bad_entries() {
        let old: Preferences = serde_json::from_str(r#"{"hide_inspector":true}"#).unwrap();
        assert!(old.hide_inspector);
        assert_eq!(old.machines.list().len(), 1);
        let saved: Preferences = serde_json::from_str(r#"{"hide_inspector":true,"machines":[{"id":"bad id","name":"Bad","transport":{"type":"local"}},{"id":"remote","name":"Remote","transport":{"type":"tailnet","host":"node","port":123}},{"id":"bad","name":"Bad","transport":{"type":"tailnet","host":"node","port":0}}]}"#).unwrap();
        assert!(saved.hide_inspector);
        assert_eq!(saved.machines.list().len(), 2);
        assert_eq!(
            serde_json::from_str::<Preferences>(&serde_json::to_string(&saved).unwrap()).unwrap(),
            saved
        );
    }

    #[test]
    fn preferences_selected_machine_defaults_and_round_trips() {
        let old: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(old.selected_machine.as_str(), "local");
        let invalid: Preferences =
            serde_json::from_str(r#"{"selected_machine":"bad id","hide_inspector":true}"#).unwrap();
        assert_eq!(invalid.selected_machine.as_str(), "local");
        assert!(invalid.hide_inspector);
        let saved: Preferences = serde_json::from_str(r#"{"selected_machine":"remote"}"#).unwrap();
        let restored: Preferences =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(restored.selected_machine.as_str(), "remote");
    }

    #[test]
    fn default_thinking_is_medium_until_chosen_and_survives_newer_levels() {
        let preferences = Preferences::default();
        assert_eq!(preferences.default_thinking(), ThinkingLevel::Medium);
        let saved: Preferences = serde_json::from_str(r#"{"default_thinking":"high"}"#).unwrap();
        assert_eq!(saved.default_thinking(), ThinkingLevel::High);
        // A level from a newer client leaves the rest of the file intact.
        let newer: Preferences =
            serde_json::from_str(r#"{"default_thinking":"extreme","hide_inspector":true}"#)
                .unwrap();
        assert_eq!(newer.default_thinking(), ThinkingLevel::Medium);
        assert!(newer.hide_inspector);
    }

    #[test]
    fn appearance_follows_the_system_until_chosen_and_survives_newer_values() {
        use crate::theme::Appearance;
        assert_eq!(Preferences::default().appearance, Appearance::System);
        let saved: Preferences =
            serde_json::from_str(r#"{"appearance":"light","light_theme":"Nord"}"#).unwrap();
        assert_eq!(saved.appearance, Appearance::Light);
        assert_eq!(saved.light_theme.as_deref(), Some("Nord"));
        let newer: Preferences =
            serde_json::from_str(r#"{"appearance":"dusk","hide_inspector":true}"#).unwrap();
        assert_eq!(newer.appearance, Appearance::System);
        assert!(newer.hide_inspector);
    }

    #[test]
    fn interface_font_defaults_to_plex_and_reads_leniently() {
        assert_eq!(Preferences::default().interface_font, InterfaceFont::Plex);
        // A file saved before the key existed loads as plex and keeps other fields.
        let older: Preferences = serde_json::from_str(r#"{"hide_inspector":true}"#).unwrap();
        assert_eq!(older.interface_font, InterfaceFont::Plex);
        assert!(older.hide_inspector);

        // Unknown or malformed values fall back to plex without failing the file.
        for raw in [r#""inter""#, "42", "null", "true", "{}"] {
            let loaded: Preferences = serde_json::from_str(&format!(
                r#"{{"interface_font":{raw},"hide_inspector":true}}"#
            ))
            .unwrap();
            assert_eq!(loaded.interface_font, InterfaceFont::Plex, "raw={raw}");
            assert!(loaded.hide_inspector);
        }

        // Valid values serialize as snake_case and round-trip through disk.
        let root = scratch("interface-font");
        let path = root.join("desktop.json");
        for (choice, expected) in [
            (InterfaceFont::Plex, "plex"),
            (InterfaceFont::System, "system"),
        ] {
            let chosen = Preferences {
                interface_font: choice,
                ..Default::default()
            };
            let encoded = serde_json::to_value(&chosen).unwrap();
            assert_eq!(encoded["interface_font"], expected);
            save(&path, &chosen).unwrap();
            let loaded = load(&path).unwrap();
            assert_eq!(loaded, chosen);
            assert_eq!(loaded.interface_font, choice);
        }

        // Startup merge preserves a saved System choice when untouched during startup.
        let saved_system = Preferences {
            interface_font: InterfaceFont::System,
            ..Default::default()
        };
        let edited_elsewhere = Preferences {
            hide_inspector: true,
            ..Default::default()
        };
        let merged = merge_startup(saved_system, &edited_elsewhere).unwrap();
        assert_eq!(merged.interface_font, InterfaceFont::System);
        assert!(merged.hide_inspector);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn code_font_is_the_systems_until_a_family_is_named() {
        // A file saved before the setting existed keeps the system font.
        let older: Preferences = serde_json::from_str(r#"{"hide_inspector":true}"#).unwrap();
        assert_eq!(older.code_font(), None);
        let saved: Preferences =
            serde_json::from_str(r#"{"code_font":" JetBrains Mono "}"#).unwrap();
        assert_eq!(saved.code_font(), Some("JetBrains Mono"));
        // A blank name is no choice.
        let blank: Preferences = serde_json::from_str(r#"{"code_font":"  "}"#).unwrap();
        assert_eq!(blank.code_font(), None);
        let written = serde_json::to_string(&saved).unwrap();
        let read: Preferences = serde_json::from_str(&written).unwrap();
        assert_eq!(read.code_font(), Some("JetBrains Mono"));
    }

    #[test]
    fn code_size_defaults_clamps_and_round_trips() {
        // A file saved before the setting existed loads at 12 points.
        let older: Preferences = serde_json::from_str(r#"{"hide_inspector":true}"#).unwrap();
        assert_eq!(older.code_size(), CODE_SIZE_DEFAULT);
        assert!(older.hide_inspector);

        // Values outside 12–14 are clamped rather than rejected, and an
        // unreadable value falls back to 12 without failing the rest of the file.
        for (raw, expected) in [
            ("0", 12),
            ("-4", 12),
            ("11", 12),
            ("12", 12),
            ("13", 13),
            ("14", 14),
            ("15", 14),
            ("999", 14),
            ("null", 12),
            (r#""huge""#, 12),
        ] {
            let loaded: Preferences =
                serde_json::from_str(&format!(r#"{{"code_size":{raw},"hide_inspector":true}}"#))
                    .unwrap();
            assert_eq!(loaded.code_size(), expected, "raw={raw}");
            assert!(loaded.hide_inspector);
        }

        // Saving and reloading round-trips every valid size and clamps out-of-range sets.
        let root = scratch("code-size");
        let path = root.join("desktop.json");
        for (input, expected) in [(10, 12), (12, 12), (13, 13), (14, 14), (20, 14)] {
            let mut chosen = Preferences::default();
            chosen.set_code_size(input);
            assert_eq!(chosen.code_size(), expected);
            save(&path, &chosen).unwrap();
            let loaded = load(&path).unwrap();
            assert_eq!(loaded, chosen);
            assert_eq!(loaded.code_size(), expected);
        }
        // A hand-edited out-of-range value on disk loads clamped, not rejected.
        fs::write(&path, r#"{"code_size": 24, "hide_inspector": true}"#).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.code_size(), 14);
        assert!(loaded.hide_inspector);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fold_earlier_turns_defaults_to_on_and_survives_older_files() {
        assert!(Preferences::default().fold_earlier_turns());
        // A file saved before the setting existed still loads with folding on.
        let older: Preferences = serde_json::from_str(r#"{"hide_inspector":true}"#).unwrap();
        assert!(older.fold_earlier_turns());
        assert!(older.hide_inspector);
        // Explicit false and true round-trip cleanly.
        let off: Preferences = serde_json::from_str(r#"{"fold_earlier_turns":false}"#).unwrap();
        assert!(!off.fold_earlier_turns());
        let written = serde_json::to_string(&off).unwrap();
        let restored: Preferences = serde_json::from_str(&written).unwrap();
        assert!(!restored.fold_earlier_turns());
        // A malformed value falls back to on without dropping other settings.
        let bad: Preferences =
            serde_json::from_str(r#"{"fold_earlier_turns":"always","hide_inspector":true}"#)
                .unwrap();
        assert!(bad.fold_earlier_turns());
        assert!(bad.hide_inspector);
        // Startup merge preserves a saved false when untouched during startup.
        let untouched = Preferences {
            hide_inspector: true,
            ..Default::default()
        };
        let merged = merge_startup(off, &untouched).unwrap();
        assert!(!merged.fold_earlier_turns());
        assert!(merged.hide_inspector);
    }

    #[test]
    fn finished_work_preference_is_optional_and_lenient() {
        for json in ["{}", r#"{"fold_finished_work":"unknown"}"#] {
            assert!(
                serde_json::from_str::<Preferences>(json)
                    .unwrap()
                    .fold_finished_work()
            );
        }
        let saved: Preferences = serde_json::from_str(r#"{"fold_finished_work":false}"#).unwrap();
        let restored: Preferences =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert!(!restored.fold_finished_work());
    }

    #[test]
    fn editor_and_terminal_choices_round_trip_and_refuse_unusable_commands() {
        // A file saved before the settings existed opens with the system's choices.
        let older: Preferences = serde_json::from_str(r#"{"hide_inspector":true}"#).unwrap();
        assert_eq!(older.open_files_with, EditorChoice::System);
        assert_eq!(older.open_terminals_with, TerminalChoice::Terminal);
        let root = scratch("ways-out");
        let path = root.join("desktop.json");
        let editors = [
            EditorChoice::System,
            EditorChoice::RexNeovim,
            EditorChoice::Custom("zed '{file}:{line}'".into()),
        ];
        let terminals = [
            TerminalChoice::Terminal,
            TerminalChoice::Rex,
            TerminalChoice::Custom("wezterm start --cwd {project}".into()),
        ];
        for (editor, terminal) in editors.into_iter().zip(terminals) {
            let mut chosen = Preferences::default();
            chosen.set_open_files_with(editor.clone()).unwrap();
            chosen.set_open_terminals_with(terminal.clone()).unwrap();
            save(&path, &chosen).unwrap();
            let loaded = load(&path).unwrap();
            assert_eq!(loaded.open_files_with, editor);
            assert_eq!(loaded.open_terminals_with, terminal);
        }
        // An unusable command is refused and the previous choice stays.
        let mut chosen = Preferences::default();
        chosen.set_open_files_with(EditorChoice::RexNeovim).unwrap();
        let error = chosen
            .set_open_files_with(EditorChoice::Custom("zed 'unclosed {file}".into()))
            .unwrap_err();
        assert!(error.contains("single quote"), "{error}");
        assert!(
            chosen
                .set_open_terminals_with(TerminalChoice::Custom("kitty {file}".into()))
                .is_err()
        );
        assert_eq!(chosen.open_files_with, EditorChoice::RexNeovim);
        assert_eq!(chosen.open_terminals_with, TerminalChoice::Terminal);
        // A hand-edited command that cannot be used, or a choice from a newer
        // client, reads as the default and leaves the rest of the file.
        fs::write(
            &path,
            r#"{"open_files_with":{"with":"custom","command":"zed"},"open_terminals_with":{"with":"someday"},"hide_inspector":true}"#,
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.open_files_with, EditorChoice::System);
        assert_eq!(loaded.open_terminals_with, TerminalChoice::Terminal);
        assert!(loaded.hide_inspector);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_harness_is_pi_until_chosen_and_survives_unknown_harnesses() {
        assert_eq!(Preferences::default().default_harness, HarnessId::Pi);
        let saved: Preferences = serde_json::from_str(r#"{"default_harness":"codex"}"#).unwrap();
        assert_eq!(saved.default_harness, HarnessId::Codex);
        // A harness from a newer client, or a malformed value, leaves the rest intact.
        for value in [r#""someday""#, "7", "null"] {
            let newer: Preferences = serde_json::from_str(&format!(
                r#"{{"default_harness":{value},"hide_inspector":true}}"#
            ))
            .unwrap();
            assert_eq!(newer.default_harness, HarnessId::Pi, "{value}");
            assert!(newer.hide_inspector);
        }
        // `Other` is never written, so saving cannot fail on it.
        let other = Preferences {
            default_harness: HarnessId::Other,
            ..Default::default()
        };
        let written = serde_json::to_value(&other).unwrap();
        assert_eq!(written["default_harness"], "pi");
    }

    #[test]
    fn a_default_harness_chosen_at_startup_merges_with_the_saved_one() {
        let loaded = Preferences {
            default_harness: HarnessId::Codex,
            ..Default::default()
        };
        // An edit elsewhere keeps the saved harness.
        let edited = Preferences {
            hide_inspector: true,
            ..Default::default()
        };
        let merged = merge_startup(loaded.clone(), &edited).unwrap();
        assert_eq!(merged.default_harness, HarnessId::Codex);
        assert!(merged.hide_inspector);
        // Choosing one before loading finished wins.
        let mut chose = Preferences {
            default_harness: HarnessId::Codex,
            ..Default::default()
        };
        let merged = merge_startup(Preferences::default(), &chose).unwrap();
        assert_eq!(merged.default_harness, HarnessId::Codex);
        // Choosing Pi again matches the default, so the saved choice stays.
        chose.default_harness = HarnessId::Pi;
        let merged = merge_startup(loaded, &chose).unwrap();
        assert_eq!(merged.default_harness, HarnessId::Codex);
    }

    fn choice(provider: &str, model_id: &str) -> ModelChoice {
        ModelChoice {
            provider: provider.into(),
            model_id: model_id.into(),
        }
    }

    fn keymap(entries: &[(&str, &str)]) -> Keymap {
        let owned = |(id, keys): &(&str, &str)| (id.to_string(), keys.to_string());
        entries.iter().map(owned).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("plait-preferences-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn startup_edits_keep_other_saved_settings_and_projects() {
        let mut loaded = Preferences {
            ask_before_actions: true,
            ..Default::default()
        };
        loaded.navigation.open(std::path::Path::new("/previous"));
        loaded.toggle_favorite(&choice("p", "m"));
        let mut edited = Preferences {
            hide_inspector: true,
            ..Default::default()
        };
        edited.navigation.open(std::path::Path::new("/current"));
        let merged = merge_startup(loaded, &edited).unwrap();
        assert!(merged.ask_before_actions && merged.hide_inspector);
        assert!(merged.is_favorite(&choice("p", "m")));
        assert_eq!(merged.navigation.opened.len(), 2);
    }

    #[test]
    fn preference_writes_coalesce_without_overlapping_or_finishing_the_wrong_write() {
        let mut writes = SaveQueue::default();
        writes.queue(1, Preferences::default());
        assert_eq!(writes.next_write().unwrap().0, 1);
        let mut latest = Preferences {
            hide_inspector: true,
            ..Default::default()
        };
        writes.queue(2, latest.clone());
        latest.ask_before_actions = true;
        writes.queue(3, latest.clone());
        assert!(writes.next_write().is_none());
        writes.finish(0);
        assert!(writes.next_write().is_none());
        writes.finish(1);
        assert_eq!(writes.next_write(), Some((3, latest)));
        writes.finish(1);
        assert!(writes.next_write().is_none());
        writes.finish(3);
        assert!(writes.next_write().is_none());
    }

    #[test]
    fn missing_file_is_default_and_save_round_trips() {
        let root = scratch("round-trip");
        let path = root.join("nested/desktop.json");
        assert_eq!(load(&path), Ok(Preferences::default()));
        assert!(!Preferences::default().ask_before_actions);
        let mut chosen = Preferences {
            jump_target: JumpTarget::Machines,
            ask_before_actions: true,
            hide_inspector: true,
            sidebar_width: Some(318),
            inspector_width: Some(402),
            ..Preferences::default()
        };
        chosen.toggle_favorite(&choice("openai-codex", "gpt-5.6-luna"));
        chosen.record_recent(&choice("openrouter", "anthropic/claude-fable-5"));
        chosen.set_keymap(keymap(&[("thread.new", "cmd-t"), ("app.quit", "")]));
        chosen.default_harness = HarnessId::Codex;
        save(&path, &chosen).unwrap();
        assert_eq!(load(&path), Ok(chosen));
        // No temporary file is left beside the saved one.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_and_oversized_files_are_reported_not_reset() {
        let root = scratch("malformed");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("desktop.json");
        fs::write(&path, "{\"jump_target\": \"tabs\"}").unwrap();
        assert!(load(&path).unwrap_err().contains("invalid"));
        fs::write(&path, " ".repeat(FILE_LIMIT as usize + 1)).unwrap();
        assert!(load(&path).unwrap_err().contains("too large"));
        // The bad file is left for the user to inspect.
        assert!(path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_fields_and_missing_fields_load() {
        let root = scratch("fields");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("desktop.json");
        fs::write(&path, "{\"jump_target\": \"projects\", \"later\": 1}").unwrap();
        assert_eq!(load(&path).unwrap().jump_target, JumpTarget::Projects);
        fs::write(&path, "{}").unwrap();
        assert_eq!(load(&path), Ok(Preferences::default()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_from_before_model_lists_loads_with_empty_lists() {
        let root = scratch("older");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("desktop.json");
        // Exactly what the previous release wrote.
        fs::write(
            &path,
            "{\n  \"jump_target\": \"machines\",\n  \"ask_before_actions\": true\n}\n",
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.jump_target, JumpTarget::Machines);
        assert!(loaded.ask_before_actions);
        assert!(!loaded.hide_inspector);
        assert_eq!((loaded.sidebar_width, loaded.inspector_width), (None, None));
        assert!(loaded.favorite_models.is_empty());
        assert!(loaded.recent_models.is_empty());
        assert!(loaded.keymap.is_empty());
        assert_eq!(loaded.default_harness, HarnessId::Pi);
        // Saving it again keeps the old settings and adds the new fields.
        save(&path, &loaded).unwrap();
        assert_eq!(load(&path), Ok(loaded));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_entries_ignore_unknown_fields_and_reject_malformed_ones() {
        let root = scratch("models");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("desktop.json");
        fs::write(
            &path,
            r#"{"favorite_models": [{"provider": "p", "model_id": "m", "later": true}]}"#,
        )
        .unwrap();
        assert!(load(&path).unwrap().is_favorite(&choice("p", "m")));
        fs::write(&path, r#"{"recent_models": [{"provider": "p"}]}"#).unwrap();
        assert!(load(&path).unwrap_err().contains("invalid"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_keymap_loads_as_written_and_keeps_entries_this_client_does_not_know() {
        let root = scratch("keymap");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("desktop.json");
        // The file is storage only: an unknown ID, an empty keystroke, and a
        // keystroke no client can parse all load and save unchanged.
        fs::write(
            &path,
            r#"{"keymap": {"thread.new": "cmd-t", "later.action": "cmd-l", "app.quit": "", "view.changes": "not a key"}}"#,
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        let expected = keymap(&[
            ("app.quit", ""),
            ("later.action", "cmd-l"),
            ("thread.new", "cmd-t"),
            ("view.changes", "not a key"),
        ]);
        assert_eq!(loaded.keymap, expected);
        save(&path, &loaded).unwrap();
        assert_eq!(load(&path).unwrap().keymap, expected);
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"later.action\": \"cmd-l\""), "{written}");
        // A keystroke must be a string; anything else is a malformed file.
        fs::write(&path, r#"{"keymap": {"thread.new": 7}}"#).unwrap();
        assert!(load(&path).unwrap_err().contains("invalid"));
        fs::write(&path, r#"{"keymap": ["cmd-t"]}"#).unwrap();
        assert!(load(&path).unwrap_err().contains("invalid"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_keymap_is_bounded_in_entries_and_length() {
        let long = "x".repeat(KEYMAP_ID_LIMIT + 1);
        let mut preferences = Preferences::default();
        let mut entries = (0..KEYMAP_LIMIT + 5)
            .map(|index| (format!("action.{index:03}"), "cmd-k".to_owned()))
            .collect::<Keymap>();
        entries.insert(long.clone(), "cmd-l".into());
        entries.insert("too.long".into(), "k".repeat(KEYMAP_KEYS_LIMIT + 1));
        preferences.set_keymap(entries);
        assert_eq!(preferences.keymap.len(), KEYMAP_LIMIT);
        assert!(!preferences.keymap.contains_key(&long));
        assert!(!preferences.keymap.contains_key("too.long"));
        // The first entries in ID order are the ones kept.
        assert!(preferences.keymap.contains_key("action.000"));
        assert!(!preferences.keymap.contains_key("action.130"));
    }

    #[test]
    fn loading_removes_duplicates_and_applies_the_limits() {
        let root = scratch("limits");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("desktop.json");
        let entries = (0..FAVORITE_LIMIT + 3)
            .map(|index| format!(r#"{{"provider": "p", "model_id": "m{index}"}}"#))
            .chain([r#"{"provider": "p", "model_id": "m5"}"#.to_owned()])
            .collect::<Vec<_>>()
            .join(",");
        fs::write(
            &path,
            format!(r#"{{"favorite_models": [{entries}], "recent_models": [{entries}]}}"#),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.favorite_models.len(), FAVORITE_LIMIT);
        // Favorites keep the newest; recents keep the front of the list.
        assert_eq!(loaded.favorite_models[0].model_id, "m3");
        assert_eq!(loaded.recent_models.len(), RECENT_LIMIT);
        assert_eq!(loaded.recent_models[0].model_id, "m0");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn favorites_toggle_and_recents_move_to_the_front() {
        let mut preferences = Preferences::default();
        let (a, b) = (choice("p", "a"), choice("p", "b"));
        assert!(preferences.toggle_favorite(&a));
        assert!(preferences.toggle_favorite(&b));
        assert!(preferences.is_favorite(&a));
        assert!(!preferences.toggle_favorite(&a));
        assert!(!preferences.is_favorite(&a));
        assert_eq!(preferences.favorite_models, vec![ModelKey::from(&b)]);

        preferences.record_recent(&a);
        preferences.record_recent(&b);
        preferences.record_recent(&a);
        assert_eq!(
            preferences.recent_models,
            vec![ModelKey::from(&a), ModelKey::from(&b)]
        );
        for index in 0..RECENT_LIMIT + 2 {
            preferences.record_recent(&choice("p", &format!("m{index}")));
        }
        assert_eq!(preferences.recent_models.len(), RECENT_LIMIT);
        assert_eq!(
            preferences.recent_models[0].model_id,
            format!("m{}", RECENT_LIMIT + 1)
        );
        // The same model under another provider is a different model.
        assert!(!preferences.is_favorite(&choice("q", "b")));
    }

    #[test]
    fn full_lists_stay_within_the_file_limit() {
        let mut preferences = Preferences::default();
        let long = "m".repeat(MODEL_KEY_LIMIT - 4);
        for index in 0..FAVORITE_LIMIT + 5 {
            let model = choice("p", &format!("{long}{index:03}"));
            assert!(preferences.toggle_favorite(&model));
            preferences.record_recent(&model);
        }
        assert_eq!(preferences.favorite_models.len(), FAVORITE_LIMIT);
        // An identifier too long to remember is skipped, not stored.
        let oversized = choice("p", &"x".repeat(MODEL_KEY_LIMIT));
        assert!(!preferences.toggle_favorite(&oversized));
        preferences.record_recent(&oversized);
        assert!(
            !preferences
                .recent_models
                .iter()
                .any(|key| *key == oversized)
        );
        // With the longest keymap a load keeps, the file still loads.
        let id = "i".repeat(KEYMAP_ID_LIMIT - 3);
        let keys = "k".repeat(KEYMAP_KEYS_LIMIT);
        preferences.set_keymap(
            (0..KEYMAP_LIMIT + 5)
                .map(|index| (format!("{id}{index:03}"), keys.clone()))
                .collect(),
        );
        assert_eq!(preferences.keymap.len(), KEYMAP_LIMIT);
        let bytes = serde_json::to_vec_pretty(&preferences).unwrap();
        assert!((bytes.len() as u64) < FILE_LIMIT, "{} bytes", bytes.len());
    }

    #[test]
    fn an_oversized_save_does_not_replace_a_readable_file() {
        let root = scratch("oversized-save");
        let path = root.join("desktop.json");
        save(&path, &Preferences::default()).unwrap();
        let mut value = Preferences::default();
        value
            .navigation
            .observed
            .insert("x".repeat(FILE_LIMIT as usize), Default::default());
        assert!(save(&path, &value).unwrap_err().contains("left intact"));
        assert_eq!(load(&path).unwrap(), Preferences::default());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_failure_is_reported() {
        let root = scratch("blocked");
        fs::create_dir_all(&root).unwrap();
        // A file where the directory should be.
        fs::write(root.join("plait"), "").unwrap();
        assert!(save(&root.join("plait/desktop.json"), &Preferences::default()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    /// Two projects: this machine and a remote one, then a remote-only project.
    fn layout() -> Vec<Project> {
        let machine = |name: &str, local| Machine {
            name: name.into(),
            local,
        };
        vec![
            Project {
                checkout: "/fixture".into(),
                name: "plait".into(),
                machines: vec![machine("workstation", true), machine("laptop", false)],
            },
            Project {
                checkout: "/fixture".into(),
                name: "notes".into(),
                machines: vec![machine("laptop", false)],
            },
        ]
    }

    fn thread(thread: usize) -> Jump {
        Jump {
            project: 0,
            machine: 0,
            thread,
        }
    }

    #[test]
    fn threads_are_counted_in_sidebar_order() {
        let projects = layout();
        assert_eq!(jump(&projects, 3, JumpTarget::Threads, 1), Some(thread(0)));
        assert_eq!(jump(&projects, 3, JumpTarget::Threads, 3), Some(thread(2)));
        assert_eq!(jump(&projects, 3, JumpTarget::Threads, 4), None);
        assert_eq!(jump(&projects, 3, JumpTarget::Threads, 0), None);
        assert_eq!(jump(&projects, 0, JumpTarget::Threads, 1), None);
    }

    #[test]
    fn machines_and_projects_select_their_first_thread() {
        let projects = layout();
        assert_eq!(jump(&projects, 2, JumpTarget::Machines, 1), Some(thread(0)));
        // A machine or project with no threads selects nothing.
        assert_eq!(jump(&projects, 2, JumpTarget::Machines, 2), None);
        assert_eq!(jump(&projects, 2, JumpTarget::Machines, 3), None);
        assert_eq!(jump(&projects, 2, JumpTarget::Projects, 1), Some(thread(0)));
        assert_eq!(jump(&projects, 2, JumpTarget::Projects, 2), None);
        assert_eq!(jump(&projects, 2, JumpTarget::Projects, 9), None);
    }
}

#[cfg(test)]
mod turns_tests {
    use super::*;

    #[test]
    fn old_or_unknown_turn_styles_keep_other_preferences() {
        for json in [
            r#"{"hide_inspector":true}"#,
            r#"{"turns":"future","hide_inspector":true}"#,
            r#"{"turns":42,"hide_inspector":true}"#,
        ] {
            let preferences: Preferences = serde_json::from_str(json).unwrap();
            assert!(preferences.hide_inspector);
            assert_eq!(preferences.turns, Turns::Bubbles);
        }
        let preferences: Preferences = serde_json::from_str(r#"{"turns":"line"}"#).unwrap();
        assert_eq!(preferences.turns, Turns::Line);
        assert_eq!(serde_json::to_value(preferences).unwrap()["turns"], "line");
    }
}
