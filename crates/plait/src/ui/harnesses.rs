//! Agent harnesses: which harnesses can run a thread here, and which one runs
//! the open thread. A version 4 host lists them once per connection; Settings
//! checks again on demand. Every string the model picker, the composer line,
//! and Settings draw is made when the reply arrives; rendering formats nothing.
//!
//! An older host lists none, so every thread runs in Pi and nothing changes.

use super::{
    Plait,
    commands::Command,
    model_picker::{model_key, model_label},
    palette::Item,
    settings,
    theme::*,
};
use gpui::{prelude::*, *};
use plait_core::host::{
    self, Model, ModelChoice, Snapshot,
    harness::{Harness, HarnessId, HarnessState, Harnesses, Request, ThreadHarness},
    providers,
};
use std::{sync::Arc, time::Duration};

#[derive(Default)]
pub(super) struct HarnessesState {
    /// The host connection `view` was read from.
    epoch: Option<u64>,
    view: Option<Arc<HarnessesView>>,
    /// Bumped whenever the list changes, so an open picker and the composer line rebuild.
    pub(super) generation: u64,
    busy: bool,
    /// The harness a running `set_up_harness` request is for.
    setting_up: Option<HarnessId>,
    /// The latest request; an earlier reply that lands after it is dropped.
    serial: u64,
    note: Option<SharedString>,
    /// The harness of the model last chosen for a new thread, and its permission mode.
    chosen: Option<(HarnessId, ModelChoice, Option<String>)>,
    /// The harness and mode `host.pending_create` was made for, so a retry repeats them.
    pending: HarnessId,
    pending_mode: Option<String>,
    /// The open thread's harness, the list generation, and what its composer line says.
    line: Option<(ThreadHarness, u64, ThreadLine)>,
}

/// What the composer line says about a thread that does not run in Pi.
pub(super) struct ThreadLine {
    /// The harness and the version of the thread's latest run, as "Codex 0.169.0".
    pub(super) label: SharedString,
    /// The logo's hover text: `label`, and whether that version was tested.
    pub(super) tip: SharedString,
    pub(super) protection: Option<SharedString>,
    /// The harness's own permission mode, when the host lists its modes.
    pub(super) mode: Option<ModeLine>,
}

/// A harness thread's own permission mode as the composer shows it.
#[derive(Debug, PartialEq)]
pub(super) struct ModeLine {
    /// The thread's chosen mode, or the harness default before it has one.
    pub(super) id: String,
    pub(super) label: SharedString,
    /// What the mode lets the harness do, or that the harness used another.
    pub(super) note: SharedString,
    /// Whether `note` says the harness is not using the chosen mode.
    pub(super) overridden: bool,
    /// The Permissions chip's hover text: what the mode allows, what protects
    /// the checkout, and when a change applies.
    pub(super) tip: SharedString,
    /// Under the Permissions menu: when a choice applies.
    pub(super) applies: SharedString,
}

/// One of a harness's own permission modes, as the host names it.
pub(super) struct ModeRow {
    pub(super) id: String,
    pub(super) label: SharedString,
    pub(super) detail: SharedString,
}

/// One harness as the picker, the composer, and Settings show it.
pub(super) struct HarnessRow {
    pub(super) id: HarnessId,
    pub(super) name: SharedString,
    pub(super) ready: bool,
    pub(super) state: SharedString,
    pub(super) note: Option<SharedString>,
    /// Version, the oldest Plait runs, and the one it was tested with.
    pub(super) version: SharedString,
    pub(super) path: Option<SharedString>,
    pub(super) protection: SharedString,
    /// Why a thread in this harness cannot send, when it is not ready.
    blocked: Option<SharedString>,
    /// Empty for Pi, whose models are the status catalog.
    pub(super) models: Vec<Model>,
    /// The harness's own permission modes; empty for Pi, which uses Plait's approvals.
    pub(super) modes: Vec<ModeRow>,
    pub(super) default_mode: Option<String>,
    /// Settings → Agents: what the default mode lets the harness do.
    pub(super) default_detail: SharedString,
    /// Settings → Agents: why the harness cannot be the default now, as hover text.
    pub(super) unstartable: Option<SharedString>,
    /// The harness needs a download the person starts from Settings → Agents.
    pub(super) setup: bool,
    /// The version the host can install, offered while the harness is not installed.
    pub(super) installable: Option<SharedString>,
    pub(super) signed_out: bool,
    /// The host installed the copy that runs.
    pub(super) managed: bool,
}

pub(super) struct HarnessesView {
    pub(super) rows: Vec<HarnessRow>,
}

/// One harness request in flight. Its reply lands only while both still match.
#[derive(Clone, Copy, Debug)]
pub(super) struct HarnessesRequest {
    epoch: u64,
    serial: u64,
}

fn version_line(harness: &Harness) -> String {
    let mut parts = Vec::new();
    if let Some(version) = &harness.version {
        parts.push(format!("Version {version}"));
    }
    if let Some(supported) = &harness.supported {
        parts.push(format!("needs {} or newer", supported.min));
        parts.push(format!("tested with {}", supported.tested));
    }
    if parts.is_empty() {
        return "Built into the host".into();
    }
    let line = parts.join(" · ");
    // "needs" leads only when no version is known.
    let mut chars = line.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl HarnessRow {
    fn new(harness: Harness) -> Self {
        let ready = harness.state == HarnessState::Ready;
        let state = harness.state.label();
        let version = version_line(&harness).into();
        let note = harness.note.filter(|note| !note.trim().is_empty());
        let modes: Vec<ModeRow> = harness
            .modes
            .into_iter()
            .map(|mode| ModeRow {
                id: mode.id,
                label: mode.label.into(),
                detail: mode.detail.into(),
            })
            .collect();
        let default_detail = harness
            .default_mode
            .as_deref()
            .and_then(|id| modes.iter().find(|mode| mode.id == id))
            .map(|mode| mode.detail.clone())
            .unwrap_or_default();
        let blocked = (!ready).then(|| {
            let fix = note.as_deref().unwrap_or("See Settings → Agents.");
            SharedString::from(format!("{}: {}. {fix}", harness.name, state.to_lowercase()))
        });
        let models = harness.models.unwrap_or_default();
        let unstartable = if harness.id == HarnessId::Other {
            Some(format!("{}: update Plait to use it.", harness.name))
        } else if !ready {
            Some(match note.as_deref() {
                Some(note) => format!("{state}. {note}"),
                None => state.to_owned(),
            })
        } else if harness.id != HarnessId::Pi && models.is_empty() {
            Some(format!("{} lists no models.", harness.name))
        } else {
            None
        };
        Self {
            id: harness.id,
            version,
            path: harness
                .path
                .as_deref()
                .map(|path| home_relative(path).into()),
            name: harness.name.into(),
            ready,
            state: state.into(),
            note: note.map(Into::into),
            protection: harness.protection.into(),
            blocked,
            models,
            modes,
            default_mode: harness.default_mode,
            default_detail,
            unstartable: unstartable.map(Into::into),
            setup: harness.state == HarnessState::SetupRequired,
            installable: harness
                .installable
                .filter(|_| harness.state == HarnessState::NotInstalled)
                .map(Into::into),
            signed_out: harness.state == HarnessState::SignedOut,
            managed: harness.managed,
        }
    }

    /// Whether a new thread can start in this harness now: ready, known to
    /// this client, and with a model to run.
    pub(super) fn startable(&self) -> bool {
        self.unstartable.is_none()
    }

    pub(super) fn mode(&self, id: &str) -> Option<&ModeRow> {
        self.modes.iter().find(|mode| mode.id == id)
    }

    /// Only an installed harness can be pointed at another executable; Pi ships
    /// with the host. One that needs setup offers that first.
    pub(super) fn has_path(&self) -> bool {
        !matches!(self.id, HarnessId::Pi | HarnessId::Other) && !self.setup
    }
}

impl HarnessesView {
    pub(super) fn new(reply: Harnesses) -> Self {
        Self {
            rows: reply.harnesses.into_iter().map(HarnessRow::new).collect(),
        }
    }
}

/// One model of a harness other than Pi as a picker row. Favorites and the host's
/// default model are Pi's, so these rows have neither.
fn model_item(row: &HarnessRow, model: &Model) -> Item {
    let choice = model.choice();
    let item = Item::new(
        model_key(model),
        model.name.clone(),
        Command::HarnessModel(row.id, choice.clone()),
    )
    .detail(model_label(&choice))
    .section(row.name.clone())
    .enabled(model.authenticated);
    if model.authenticated {
        item
    } else {
        item.badge("Not signed in", colors().amber)
    }
}

/// Rows a harness thread's own model menu lists: that harness's models only.
pub(super) fn thread_items(row: &HarnessRow) -> Vec<Item> {
    row.models
        .iter()
        .map(|model| model_item(row, model))
        .collect()
}

/// The harness list a new thread chooses from: Pi, then every harness the host
/// lists. One that cannot start a thread is listed, disabled, with its state.
pub(super) fn harness_items(rows: &[HarnessRow], current: HarnessId) -> Vec<Item> {
    let item = |id: HarnessId, name: SharedString, detail: SharedString| {
        let item = Item::new(
            format!("harness\u{1f}{name}"),
            name,
            Command::DraftHarness(id),
        )
        .detail(detail)
        .section("Agent harnesses");
        match harness_glyph(id) {
            Some(glyph) => item.glyph(glyph, colors().text),
            None => item,
        }
    };
    let current_badge = |item: Item, id: HarnessId| {
        if id == current {
            item.badge("Current", colors().blue)
        } else {
            item
        }
    };
    // An older host lists none; every thread runs in Pi.
    if !rows.iter().any(|row| row.id == HarnessId::Pi) {
        let pi = item(HarnessId::Pi, "Pi".into(), "Built into the host".into());
        let mut items = vec![current_badge(pi, HarnessId::Pi)];
        items.extend(
            rows.iter()
                .map(|row| harness_item(row, &item, &current_badge)),
        );
        return items;
    }
    rows.iter()
        .map(|row| harness_item(row, &item, &current_badge))
        .collect()
}

fn harness_item(
    row: &HarnessRow,
    item: &impl Fn(HarnessId, SharedString, SharedString) -> Item,
    current: &impl Fn(Item, HarnessId) -> Item,
) -> Item {
    let detail = row.note.clone().unwrap_or_else(|| row.version.clone());
    let listed = item(row.id, row.name.clone(), detail);
    if row.startable() {
        return current(listed, row.id);
    }
    let badge = if row.ready {
        SharedString::from("No models")
    } else {
        row.state.clone()
    };
    listed.badge(badge, colors().amber).enabled(false)
}

/// The harness a snapshot's thread runs in. Hosts before version 4 run only Pi.
pub(super) fn harness_of(snapshot: &Snapshot) -> HarnessId {
    snapshot
        .harness
        .as_ref()
        .map_or(HarnessId::Pi, |harness| harness.id)
}

/// What the composer says about a harness thread's own mode. The host clears
/// `reported` when the mode changes, so a report always describes the chosen mode.
pub(super) fn mode_line(row: &HarnessRow, key: &ThreadHarness) -> Option<ModeLine> {
    let id = key.mode.clone().or_else(|| row.default_mode.clone())?;
    let chosen = row.mode(&id);
    let label = chosen.map_or_else(|| SharedString::from(id.clone()), |mode| mode.label.clone());
    let name = &row.name;
    let (own, sets) = own_settings(row.id);
    let reported = key.reported.as_deref().filter(|reported| *reported != id);
    let (note, overridden) = match reported.map(|reported| (reported, row.mode(reported))) {
        None => (
            chosen.map(|mode| mode.detail.clone()).unwrap_or_default(),
            false,
        ),
        Some((_, Some(used))) => (
            format!(
                "{name} is using {} instead of {label}, set by your {name} {own}. {}",
                used.label, used.detail
            )
            .into(),
            true,
        ),
        // `custom`, or a mode this list does not name.
        Some((_, None)) => (
            format!("{name} is using permissions its own {own} {sets}, not {label}.").into(),
            true,
        ),
    };
    let protection = row.protection.trim();
    let (when, applies) = if applies_at_once(row.id) {
        ("at once".to_owned(), "Applies at once".to_owned())
    } else {
        (
            format!("from {name}'s next turn"),
            format!("Applies from {name}'s next turn"),
        )
    };
    // The chip names the mode and nothing more; what the mode allows is said
    // here, on hover. A mode the harness is not using is said in the open.
    let allows = if overridden { "" } else { note.trim() };
    let lead = [allows, protection]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let tip = format!(
        "{lead}{}Choose what {name} may do in this thread. A change applies {when}, even during a run.",
        if lead.is_empty() { "" } else { " " }
    );
    Some(ModeLine {
        id,
        label,
        note,
        overridden,
        tip: tip.into(),
        applies: format!("{applies}, even during a run.").into(),
    })
}

/// Whether a permissions change reaches a running thread at once. The host
/// lists no capability for it; docs/HOST.md says Claude Code takes a change
/// at once, and Codex and others from their next turn.
fn applies_at_once(id: HarnessId) -> bool {
    id == HarnessId::Claude
}

/// What a harness calls the person's own setup of it, and the verb that agrees with it.
fn own_settings(id: HarnessId) -> (&'static str, &'static str) {
    match id {
        HarnessId::Claude => ("settings", "set"),
        _ => ("configuration", "sets"),
    }
}

/// What installing a harness downloads, and whose software it stays.
fn install_terms(id: HarnessId, version: &str) -> String {
    let owner = match id {
        HarnessId::Codex => "OpenAI's software under the Apache-2.0 license",
        HarnessId::Claude => "Anthropic's software under Anthropic's terms",
        _ => "its vendor's software under the vendor's terms",
    };
    format!(
        "Not installed on this machine. Plait can install {} {version}, the version it is tested with, into the host's own folder; a copy you install yourself is used first. It stays {owner}.",
        id.label()
    )
}

fn terms_url(id: HarnessId) -> Option<&'static str> {
    match id {
        HarnessId::Codex => Some("https://github.com/openai/codex/blob/main/LICENSE"),
        HarnessId::Claude => Some("https://code.claude.com/docs/en/legal-and-compliance"),
        _ => None,
    }
}

/// What a refused or failed harness request says. Host messages are shown as sent.
fn failure(error: &host::Error) -> String {
    if providers::needs_host_update(error) {
        return "Update the host to choose an agent harness.".into();
    }
    match error {
        host::Error::Rejected(rejected) => {
            format!("The host did not accept this: {}", rejected.message)
        }
        other => format!("{other} Nothing was changed."),
    }
}

impl Plait {
    /// Scene state, as if this connection's host had answered `harnesses`.
    #[cfg(test)]
    pub(super) fn inject_harnesses(&mut self, reply: Option<Harnesses>) {
        self.harnesses.epoch = Some(self.host.connection.epoch);
        self.harnesses.view = reply.map(|reply| Arc::new(HarnessesView::new(reply)));
        self.harnesses.note = None;
        self.harnesses.generation += 1;
    }

    /// The harness list for this connection; empty before it is read and for older hosts.
    pub(super) fn harness_rows(&self) -> &[HarnessRow] {
        match &self.harnesses.view {
            Some(view)
                if self.harnesses.epoch == Some(self.host.connection.epoch)
                    && self.host.connection.connected
                    && self.host.protocol() >= 4 =>
            {
                &view.rows
            }
            _ => &[],
        }
    }

    pub(super) fn harness_row(&self, id: HarnessId) -> Option<&HarnessRow> {
        self.harness_rows().iter().find(|row| row.id == id)
    }

    /// A model of a harness other than Pi, as that harness listed it.
    pub(super) fn harness_model(&self, id: HarnessId, choice: &ModelChoice) -> Option<&Model> {
        self.harness_row(id)?
            .models
            .iter()
            .find(|model| model.choice() == *choice)
    }

    /// Whether `choice` can start or continue a thread in harness `id` now.
    pub(super) fn usable_in(&self, id: HarnessId, choice: &ModelChoice) -> bool {
        if id == HarnessId::Pi {
            return self.host.available(Some(choice));
        }
        self.harness_row(id).is_some_and(|row| row.ready)
            && self
                .harness_model(id, choice)
                .is_some_and(|model| model.authenticated)
    }

    /// The harness the open thread runs in.
    pub(super) fn thread_harness(&self) -> HarnessId {
        self.host
            .thread
            .snapshot
            .as_deref()
            .map_or(HarnessId::Pi, harness_of)
    }

    /// The harness for a new thread with `choice`: the one it was chosen from, else Pi.
    pub(super) fn new_thread_harness(&self, choice: &ModelChoice) -> HarnessId {
        match &self.harnesses.chosen {
            Some((id, chosen, _)) if chosen == choice => *id,
            _ => HarnessId::Pi,
        }
    }

    /// The mode chosen with `choice`, for a harness other than Pi.
    pub(super) fn new_thread_mode(&self, choice: &ModelChoice) -> Option<String> {
        match &self.harnesses.chosen {
            Some((id, chosen, mode)) if chosen == choice && *id != HarnessId::Pi => mode.clone(),
            _ => None,
        }
    }

    pub(super) fn set_pending_harness(&mut self, id: HarnessId, mode: Option<String>) {
        self.harnesses.pending = id;
        self.harnesses.pending_mode = mode;
    }

    pub(super) fn pending_harness(&self) -> HarnessId {
        self.harnesses.pending
    }

    pub(super) fn pending_mode(&self) -> Option<String> {
        self.harnesses.pending_mode.clone()
    }

    /// Why the open thread cannot send because of its harness, if it cannot.
    pub(super) fn harness_reason(&self) -> Option<String> {
        let id = self.thread_harness();
        if id == HarnessId::Pi {
            return None;
        }
        if self.host.protocol() < 4 {
            return Some("Update the host to continue this thread.".into());
        }
        match self.harness_row(id) {
            Some(row) => row.blocked.as_ref().map(ToString::to_string),
            None if self.harnesses.note.is_some() => {
                Some("Agent harnesses could not be checked · see Settings → Agents.".into())
            }
            None => Some(format!("Checking {}…", id.label())),
        }
    }

    /// Read the list once per host connection. Settings checks again on demand.
    pub(super) fn sync_harnesses(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) || !self.host.connection.connected || self.host.protocol() < 4 {
            return;
        }
        if self.harnesses.epoch == Some(self.host.connection.epoch) {
            return;
        }
        self.reset_harnesses();
        self.refresh_harnesses(cx);
    }

    /// Forget the list read from an earlier connection. A request still in
    /// flight for it can no longer land, since its epoch no longer matches.
    pub(super) fn reset_harnesses(&mut self) {
        self.harnesses = HarnessesState {
            epoch: Some(self.host.connection.epoch),
            generation: self.harnesses.generation + 1,
            serial: self.harnesses.serial,
            chosen: self.harnesses.chosen.take(),
            pending: self.harnesses.pending,
            pending_mode: self.harnesses.pending_mode.take(),
            ..Default::default()
        };
    }

    /// Check every harness again: an install, sign-in, or update can change while the app is open.
    pub(super) fn refresh_harnesses(&mut self, cx: &mut Context<Self>) {
        self.send_harnesses(Request::harnesses(), cx);
    }

    /// Run harness `id` from `path`, or from the login shell's PATH when `None`.
    fn set_harness_path(
        &mut self,
        id: HarnessId,
        path: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.send_harnesses(Request::set_harness_path(id, path), cx);
    }

    /// Start new threads in harness `id` in `mode`. The reply is the whole list.
    pub(super) fn set_harness_mode(&mut self, id: HarnessId, mode: String, cx: &mut Context<Self>) {
        self.send_harnesses(Request::set_harness_mode(id, mode), cx);
    }

    /// Download what harness `id` needs before it can run threads. The host
    /// replies with the whole list, `id` checked again: ready, another state,
    /// or still needing setup with a note saying what went wrong.
    pub(super) fn set_up_harness(&mut self, id: HarnessId, cx: &mut Context<Self>) {
        self.send_harnesses(Request::set_up_harness(id), cx);
    }

    pub(super) fn install_harness(&mut self, id: HarnessId, cx: &mut Context<Self>) {
        self.send_harnesses(Request::install_harness(id), cx);
    }

    /// Whether harness `id` is being set up or installed now, for Settings → Agents.
    pub(super) fn harness_setting_up(&self, id: HarnessId) -> bool {
        self.harnesses.busy && self.harnesses.setting_up == Some(id)
    }

    /// Mark a request as sent, one at a time; `None` when it cannot be sent now.
    /// `setup` names the harness a `set_up_harness` request is for.
    pub(super) fn begin_harnesses(&mut self, setup: Option<HarnessId>) -> Option<HarnessesRequest> {
        if self.harnesses.busy || !self.host.connection.connected || self.host.protocol() < 4 {
            return None;
        }
        self.harnesses.busy = true;
        self.harnesses.setting_up = setup;
        self.harnesses.serial += 1;
        Some(HarnessesRequest {
            epoch: self.host.connection.epoch,
            serial: self.harnesses.serial,
        })
    }

    /// Every request answers with the whole list, checked again.
    fn send_harnesses(&mut self, mut request: Request, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let setup = match &request {
            Request::SetUpHarness { harness, .. } | Request::InstallHarness { harness, .. } => {
                Some(*harness)
            }
            _ => None,
        };
        if self.host.protocol() >= 6 {
            match &mut request {
                Request::Harnesses { protocol_version }
                | Request::SetHarnessPath {
                    protocol_version, ..
                }
                | Request::SetHarnessMode {
                    protocol_version, ..
                }
                | Request::SetUpHarness {
                    protocol_version, ..
                }
                | Request::InstallHarness {
                    protocol_version, ..
                } => *protocol_version = 6,
            }
        }
        let Some(key) = self.begin_harnesses(setup) else {
            return;
        };
        cx.notify();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            // An install downloads about 170 MB on the host before it answers.
            let wait = if matches!(request, Request::InstallHarness { .. }) {
                Duration::from_secs(16 * 60)
            } else {
                Duration::from_secs(20)
            };
            let reply: Harnesses = client.send_within(&request, wait)?;
            Ok::<_, host::Error>(HarnessesView::new(reply))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| this.finish_harnesses(key, result, cx));
        })
        .detach();
    }

    /// Apply a reply, unless the connection or a later request has replaced the one it answers.
    pub(super) fn finish_harnesses(
        &mut self,
        key: HarnessesRequest,
        result: Result<HarnessesView, host::Error>,
        cx: &mut Context<Self>,
    ) {
        if self.host.connection.epoch != key.epoch
            || self.harnesses.epoch != Some(key.epoch)
            || self.harnesses.serial != key.serial
        {
            return;
        }
        self.harnesses.busy = false;
        self.harnesses.setting_up = None;
        match result {
            Ok(view) => {
                self.harnesses.note = None;
                self.harnesses.view = Some(Arc::new(view));
                self.harnesses.generation += 1;
            }
            Err(error) => self.harnesses.note = Some(failure(&error).into()),
        }
        self.sync_composer(cx);
        cx.notify();
    }

    /// Ask for an executable with the system file picker, then send it to the host.
    fn choose_harness_path(&mut self, id: HarnessId, name: SharedString, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("Use this {name}").into()),
        });
        let epoch = self.host.connection.epoch;
        cx.spawn(async move |this, cx| {
            let result = prompt.await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch != epoch {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.set_harness_path(id, Some(path), cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        this.harnesses.note =
                            Some(format!("Cannot open the file picker: {error}").into())
                    }
                    Err(error) => {
                        this.harnesses.note =
                            Some(format!("The file picker closed unexpectedly: {error}").into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep the composer line in step with the open thread and the list. Runs when
    /// either changes, never in render.
    pub(super) fn sync_thread_harness(&mut self) {
        let key = self
            .host
            .thread
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.harness.clone())
            .filter(|harness| harness.id != HarnessId::Pi);
        let Some(key) = key else {
            self.harnesses.line = None;
            return;
        };
        let generation = self.harnesses.generation;
        if matches!(&self.harnesses.line, Some((line, made, _)) if *line == key && *made == generation)
        {
            return;
        }
        let row = self.harness_row(key.id);
        let name = row.map_or_else(|| key.id.label().into(), |row| row.name.clone());
        let label: SharedString = match &key.version {
            Some(version) => format!("{name} {version}").into(),
            None => name,
        };
        let tip = match row.and_then(|row| row.note.as_ref()) {
            Some(note) => format!("{label}. {note}").into(),
            None => label.clone(),
        };
        let protection = row
            .map(|row| row.protection.clone())
            .filter(|protection| !protection.is_empty());
        let mode = row.and_then(|row| mode_line(row, &key));
        self.harnesses.line = Some((
            key,
            generation,
            ThreadLine {
                label,
                tip,
                protection,
                mode,
            },
        ));
    }

    /// The open thread's harness line, when it runs in a harness other than Pi.
    pub(super) fn thread_harness_line(&self) -> Option<&ThreadLine> {
        let (key, _, line) = self.harnesses.line.as_ref()?;
        let snapshot = self.host.thread.snapshot.as_ref()?;
        (snapshot.harness.as_ref() == Some(key)).then_some(line)
    }

    /// Settings → Agents: each harness, its state and version, and what protects the checkout.
    pub(super) fn harness_settings(&self, cx: &mut Context<Self>) -> Div {
        let usable = self.host.connection.connected && self.host.protocol() >= 4;
        let busy = self.harnesses.busy;
        // A setup says so on its own row; Refresh just waits.
        let checking = busy && self.harnesses.setting_up.is_none();
        let refresh = toned_button(
            "harness-refresh",
            if checking { "Checking…" } else { "Refresh" },
            Tone::Filled,
        )
        .disabled(busy || !usable)
        .when(usable && !busy, |el| {
            el.on_activate(cx, |this, _, cx| this.refresh_harnesses(cx))
        });
        let mut page = settings::section("Agent harnesses").child(settings::setting(
            "What can run a thread",
            "Pi is built in. Codex and Claude Code run through your own installs, or ones Plait installs on this machine, with their own sign-in and settings. A new thread starts in the default from New threads; choose another under its message box, and the thread stays in it.",
            refresh,
        ));
        if !self.host.connection.connected {
            return page.child(tag(
                "harnesses-disconnected",
                "Connect the host to list agent harnesses.",
                colors().subtle,
            ));
        }
        if self.host.protocol() < 4 {
            return page.child(
                tag(
                    "harnesses-older-host",
                    "This host runs every thread in Pi. Update the host to choose another harness.",
                    colors().subtle,
                )
                .line_height(px(16.)),
            );
        }
        if let Some(note) = &self.harnesses.note {
            page = page
                .child(tag("harnesses-note", note.clone(), colors().amber).line_height(px(16.)));
        }
        let rows = self.harness_rows();
        if rows.is_empty() {
            return page.when(self.harnesses.note.is_none(), |el| {
                el.child(tag(
                    "harnesses-checking",
                    "Checking agent harnesses…",
                    colors().subtle,
                ))
            });
        }
        let mut list = column().gap_4();
        for (index, row) in rows.iter().enumerate() {
            list = list.child(self.harness_entry(index, row, busy, cx));
        }
        page.child(list)
    }

    fn harness_entry(
        &self,
        index: usize,
        harness: &HarnessRow,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let tint = if harness.ready {
            colors().mint
        } else {
            colors().amber
        };
        let mut text = column()
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .child(
                row()
                    .gap_2()
                    .children(
                        harness_logo(harness.id).map(|logo| logo.text_color(rgb(colors().text))),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors().text))
                            .child(a11y_text(("harness-name", index), harness.name.clone())),
                    )
                    .child(row().child(dot(tint)).child(tag(
                        ("harness-state", index),
                        harness.state.clone(),
                        tint,
                    ))),
            )
            .child(tag(
                ("harness-version", index),
                harness.version.clone(),
                colors().muted,
            ));
        if let Some(path) = &harness.path {
            let path: SharedString = if harness.managed {
                format!("Installed by Plait · {path}").into()
            } else {
                path.clone()
            };
            text = text.child(tag(("harness-path-text", index), path, colors().subtle).truncate());
        }
        if let Some(version) = &harness.installable {
            text = text.child(
                tag(
                    ("harness-install-terms", index),
                    install_terms(harness.id, version),
                    colors().subtle,
                )
                .line_height(px(16.)),
            );
        }
        if let Some(note) = &harness.note {
            text = text.child(
                tag(
                    ("harness-note", index),
                    note.clone(),
                    if harness.ready {
                        colors().muted
                    } else {
                        colors().amber
                    },
                )
                .line_height(px(16.)),
            );
        }
        if !harness.protection.is_empty() {
            text = text.child(
                tag(
                    ("harness-protection", index),
                    harness.protection.clone(),
                    colors().subtle,
                )
                .line_height(px(16.)),
            );
        }
        let mut entry = row().items_start().gap_3().child(text);
        let modes = (!harness.setup)
            .then(|| self.default_mode_setting(index, harness, busy, cx))
            .flatten();
        if harness.setup {
            let id = harness.id;
            let setting_up = self.harness_setting_up(id);
            entry = entry.child(
                toned_button(
                    ("harness-setup", index),
                    if setting_up {
                        "Setting up…"
                    } else {
                        "Set up"
                    },
                    Tone::Primary,
                )
                .flex_shrink_0()
                .aria_label(format!("Set up {} threads", harness.name))
                .disabled(busy)
                .when(!busy, |el| {
                    el.on_activate(cx, move |this, _, cx| this.set_up_harness(id, cx))
                }),
            );
        }
        if let Some(version) = &harness.installable {
            let id = harness.id;
            let installing = self.harness_setting_up(id);
            entry = entry.child(
                row()
                    .flex_shrink_0()
                    .gap_1()
                    .children(terms_url(id).map(|url| {
                        toned_button(("harness-terms", index), "Terms", Tone::Ghost)
                            .aria_label(format!("Read the terms {} comes under", harness.name))
                            .on_activate(cx, move |_, _, cx| cx.open_url(url))
                    }))
                    .child(
                        toned_button(
                            ("harness-install", index),
                            if installing {
                                "Installing…"
                            } else {
                                "Install"
                            },
                            Tone::Primary,
                        )
                        .aria_label(format!("Install {} {version} on the host", harness.name))
                        .disabled(busy)
                        .when(!busy, |el| {
                            el.on_activate(cx, move |this, _, cx| this.install_harness(id, cx))
                        }),
                    ),
            );
        }
        if harness.signed_out {
            let id = harness.id;
            let signing_in = self.harness_flow() == Some(id);
            entry = entry.child(
                toned_button(("harness-sign-in", index), "Sign in", Tone::Primary)
                    .flex_shrink_0()
                    .aria_label(format!("Sign in to {} on the host", harness.name))
                    .disabled(busy || signing_in)
                    .when(!busy && !signing_in, |el| {
                        el.on_activate(cx, move |this, _, cx| this.start_harness_sign_in(id, cx))
                    }),
            );
        }
        if harness.has_path() {
            let (id, name) = (harness.id, harness.name.clone());
            entry = entry.child(
                row()
                    .flex_shrink_0()
                    .gap_1()
                    .child(
                        toned_button(("harness-choose", index), "Choose…", Tone::Ghost)
                            .aria_label(format!("Choose the {} executable", harness.name))
                            .disabled(busy)
                            .when(!busy, |el| {
                                el.on_activate(cx, move |this, _, cx| {
                                    this.choose_harness_path(id, name.clone(), cx)
                                })
                            }),
                    )
                    .child(
                        toned_button(("harness-path", index), "Use PATH", Tone::Ghost)
                            .aria_label(format!("Find {} on the login shell's PATH", harness.name))
                            .disabled(busy)
                            .when(!busy, |el| {
                                el.on_activate(cx, move |this, _, cx| {
                                    this.set_harness_path(id, None, cx)
                                })
                            }),
                    ),
            );
        }
        let flow = (self.harness_flow() == Some(harness.id)).then(|| {
            column()
                .gap_2()
                .when_some(self.providers.note.clone(), |el, note| {
                    el.child(tag(("harness-sign-in-note", index), note, colors().amber))
                })
                .child(self.provider_form(cx))
        });
        let entry = match modes {
            Some(modes) => column().gap_3().child(entry).child(modes),
            None => column().child(entry),
        };
        entry.when_some(flow, |el, flow| el.gap_3().child(flow))
    }

    /// Settings → New threads: the harness a new thread's draft starts in: Pi,
    /// then each harness the host lists. One that cannot start a thread now is
    /// shown, not offered.
    pub(super) fn default_harness_setting(&self, cx: &mut Context<Self>) -> Div {
        let saved = self.preferences.value.default_harness;
        let rows = self.harness_rows();
        let mut options: Vec<(HarnessId, SharedString, Option<SharedString>)> = Vec::new();
        if !rows.iter().any(|row| row.id == HarnessId::Pi) {
            options.push((HarnessId::Pi, HarnessId::Pi.label().into(), None));
        }
        options.extend(
            rows.iter()
                .map(|row| (row.id, row.name.clone(), row.unstartable.clone())),
        );
        // Kept in view while the host does not list it, so the choice is not lost from sight.
        if !options.iter().any(|(id, ..)| *id == saved) {
            options.push((
                saved,
                saved.label().into(),
                Some("Not listed by this host now.".into()),
            ));
        }
        let mut choices = segmented();
        for (index, (id, name, unstartable)) in options.into_iter().enumerate() {
            let selected = id == saved;
            let tip = unstartable.clone();
            let enabled = unstartable.is_none() && !selected;
            choices = choices.child(
                toned_button(
                    ("default-harness", index),
                    "",
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Ghost
                    },
                )
                .h(px(24.))
                .gap(px(6.))
                .role(Role::RadioButton)
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .aria_label(format!("Default harness for new threads: {name}"))
                .children(harness_logo(id).map(|logo| {
                    logo.size(px(13.)).text_color(rgb(if selected {
                        colors().text
                    } else {
                        colors().muted
                    }))
                }))
                .child(name)
                .disabled(unstartable.is_some())
                .when_some(tip, |el, tip| {
                    el.aria_description(tip.clone()).tooltip(move |_, cx| {
                        cx.new(|_| super::thread_controls::TextTooltip(tip.to_string()))
                            .into()
                    })
                })
                .when(enabled, |el| {
                    el.on_activate(cx, move |this, _, cx| this.set_default_harness(id, cx))
                }),
            );
        }
        settings::setting(
            "Default harness for new threads",
            "New threads start here when it is ready, otherwise in Pi, with its default model and permissions.",
            choices,
        )
    }

    /// Save the harness new drafts start in. A draft that chose its own keeps it.
    pub(super) fn set_default_harness(&mut self, id: HarnessId, cx: &mut Context<Self>) {
        if id == HarnessId::Other {
            return;
        }
        let mut value = self.preferences.value.clone();
        value.default_harness = id;
        self.set_preferences(value, super::jump::PreferenceGroup::Agent, cx);
        // A draft following the default now starts elsewhere.
        self.sync_composer(cx);
    }

    /// The mode new threads in a harness with its own modes start in.
    fn default_mode_setting(
        &self,
        index: usize,
        harness: &HarnessRow,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if harness.modes.is_empty() {
            return None;
        }
        let mut choices = segmented();
        for (choice, mode) in harness.modes.iter().enumerate() {
            let selected = harness.default_mode.as_deref() == Some(mode.id.as_str());
            let (id, value) = (harness.id, mode.id.clone());
            choices = choices.child(
                toned_button(
                    SharedString::from(format!("harness-mode-{index}-{choice}")),
                    mode.label.clone(),
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Ghost
                    },
                )
                .h(px(24.))
                .role(Role::RadioButton)
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .aria_label(format!(
                    "{} default permissions for new threads: {}",
                    harness.name, mode.label
                ))
                .aria_description(mode.detail.clone())
                .disabled(busy)
                .when(!busy && !selected, |el| {
                    el.on_activate(cx, move |this, _, cx| {
                        this.set_harness_mode(id, value.clone(), cx)
                    })
                }),
            );
        }
        Some(settings::setting(
            "Default permissions for new threads",
            harness.default_detail.clone(),
            choices,
        ))
    }
}

#[cfg(test)]
mod tests {
    // Named imports: a glob would bring in GPUI's own `test` attribute.
    use super::{
        HarnessId, Harnesses, HarnessesView, ThreadHarness, harness_items, mode_line, thread_items,
    };

    fn sample() -> Harnesses {
        serde_json::from_str(
            r#"{"harnesses":[
                {"id":"pi","name":"Pi","state":"ready","note":null,"version":null,"supported":null,"path":null,
                 "capabilities":{"steer":true,"approvals":true,"thinking":true,"usage":true},
                 "protection":"Plait's sandbox and approvals.","models":null},
                {"id":"codex","name":"Codex","state":"ready","note":"Newer than tested.","version":"0.170.0",
                 "supported":{"min":"0.169.0","tested":"0.169.0"},"path":"/opt/sample/bin/codex",
                 "capabilities":{"steer":true,"approvals":true,"thinking":true,"usage":true},
                 "protection":"Codex's sandbox.","models":[
                    {"provider":"codex","model_id":"a","name":"A","authenticated":true},
                    {"provider":"codex","model_id":"b","name":"B","authenticated":false}],
                 "modes":[{"id":"read_only","label":"Read only","detail":"Only reads."},
                          {"id":"ask","label":"Ask","detail":"Asks first."},
                          {"id":"full_access","label":"Full access","detail":"Never asks."}],
                 "default_mode":"full_access"},
                {"id":"someday","name":"Someday","state":"signed_out","note":"Sign in first.","version":null,
                 "supported":{"min":"1.0","tested":"1.2"},"path":null,
                 "capabilities":{"steer":false,"approvals":false,"thinking":false,"usage":false},
                 "protection":"","models":[]}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn strings_are_made_once_from_the_reply() {
        let view = HarnessesView::new(sample());
        let [pi, codex, other] = &view.rows[..] else {
            panic!("three rows")
        };
        assert_eq!(pi.version.as_ref(), "Built into the host");
        assert!(pi.models.is_empty() && pi.blocked.is_none() && !pi.has_path());
        assert_eq!(
            codex.version.as_ref(),
            "Version 0.170.0 · needs 0.169.0 or newer · tested with 0.169.0"
        );
        assert_eq!(codex.path.as_deref(), Some("/opt/sample/bin/codex"));
        assert!(codex.ready && codex.has_path());
        assert_eq!(
            other.version.as_ref(),
            "Needs 1.0 or newer · tested with 1.2"
        );
        assert_eq!(
            other.blocked.as_deref(),
            Some("Someday: signed out. Sign in first.")
        );
        assert!(!other.has_path());
    }

    #[test]
    fn the_harness_list_offers_ready_harnesses_and_shows_the_rest_disabled() {
        let view = HarnessesView::new(sample());
        let listed = |items: Vec<super::Item>| {
            items
                .iter()
                .map(|item| {
                    (
                        item.title.to_string(),
                        item.enabled,
                        item.badge.as_ref().map(|(badge, _)| badge.to_string()),
                        item.glyph.map(|(glyph, _)| glyph),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            listed(harness_items(&view.rows, HarnessId::Codex)),
            vec![
                ("Pi".into(), true, None, Some("harness-pi")),
                (
                    "Codex".into(),
                    true,
                    Some("Current".into()),
                    Some("harness-openai")
                ),
                ("Someday".into(), false, Some("Signed out".into()), None),
            ]
        );
        let items = harness_items(&view.rows, HarnessId::Pi);
        assert_eq!(items[2].detail.as_ref(), "Sign in first.");
        // An older host lists nothing: Pi alone, current.
        assert_eq!(
            listed(harness_items(&[], HarnessId::Pi)),
            vec![(
                "Pi".into(),
                true,
                Some("Current".into()),
                Some("harness-pi")
            )]
        );
        // A Codex thread's menu lists only Codex models.
        assert_eq!(thread_items(&view.rows[1]).len(), 2);
        assert!(thread_items(&view.rows[0]).is_empty());
    }

    /// Pi and Claude Code as the host lists them, Claude Code in `state`.
    fn claude_list(state: &str, note: Option<&str>) -> Harnesses {
        let ready = state == "ready";
        serde_json::from_value(serde_json::json!({"harnesses": [
            {"id":"pi","name":"Pi","state":"ready","note":null,"version":null,"supported":null,"path":null,
             "capabilities":{"steer":true,"approvals":true,"thinking":true,"usage":true},
             "protection":"Plait's sandbox and approvals.","models":null},
            {"id":"claude","name":"Claude Code","state":state,"note":note,
             "version": ready.then_some("2.1.326"),
             "supported":{"min":"2.1.290","tested":"2.1.326"},
             "path": ready.then_some("/opt/sample/bin/claude"),
             "capabilities":{"steer":true,"approvals":true,"thinking":true,"usage":true},
             "protection":"Claude Code's mode.",
             "models": if ready {
                 serde_json::json!([{"provider":"claude","model_id":"default","name":"Default","authenticated":true}])
             } else {
                 serde_json::json!([])
             },
             "modes":[{"id":"ask","label":"Ask","detail":"Asks first."},
                      {"id":"plan","label":"Plan","detail":"Plans first."},
                      {"id":"auto","label":"Auto","detail":"Decides itself."}],
             "default_mode":"auto"}]}))
        .unwrap()
    }

    fn with(mut list: Harnesses, installable: Option<&str>, managed: bool) -> Harnesses {
        list.harnesses[1].installable = installable.map(Into::into);
        list.harnesses[1].managed = managed;
        list
    }

    #[test]
    fn a_missing_harness_offers_its_pinned_install_and_a_signed_out_one_offers_sign_in() {
        let view = HarnessesView::new(with(
            claude_list("not_installed", None),
            Some("2.1.326"),
            false,
        ));
        let claude = &view.rows[1];
        assert_eq!(claude.installable.as_deref(), Some("2.1.326"));
        assert!(!claude.signed_out);
        assert!(super::install_terms(claude.id, "2.1.326").contains("Claude Code 2.1.326"));
        // A host without a build for its platform offers nothing to install.
        let view = HarnessesView::new(with(claude_list("not_installed", None), None, false));
        assert_eq!(view.rows[1].installable, None);
        // An installed harness is not offered again, even when the host could.
        let view = HarnessesView::new(with(claude_list("signed_out", None), Some("2.1.326"), true));
        let claude = &view.rows[1];
        assert_eq!(claude.installable, None);
        assert!(claude.signed_out && claude.managed);
        assert!(
            super::terms_url(HarnessId::Codex).is_some()
                && super::terms_url(HarnessId::Pi).is_none()
        );
    }

    #[test]
    fn a_harness_that_needs_setup_offers_setup_and_nothing_else() {
        let view = HarnessesView::new(claude_list("setup_required", Some("Set up first.")));
        let claude = &view.rows[1];
        assert!(claude.setup && !claude.ready && !claude.startable());
        assert_eq!(claude.state.as_ref(), "Needs setup");
        assert!(
            !claude.has_path(),
            "setup comes before choosing an executable"
        );
        assert_eq!(
            claude.version.as_ref(),
            "Needs 2.1.290 or newer · tested with 2.1.326"
        );
        assert_eq!(
            claude.unstartable.as_deref(),
            Some("Needs setup. Set up first.")
        );
        assert_eq!(
            claude.blocked.as_deref(),
            Some("Claude Code: needs setup. Set up first.")
        );
        let items = harness_items(&view.rows, HarnessId::Pi);
        assert_eq!(items[1].title.as_ref(), "Claude Code");
        assert!(!items[1].enabled);
        assert_eq!(
            items[1].glyph.map(|(glyph, _)| glyph),
            Some("harness-claude")
        );
        assert_eq!(
            items[1].badge.as_ref().map(|(badge, _)| badge.as_ref()),
            Some("Needs setup")
        );

        let view = HarnessesView::new(claude_list("ready", None));
        let claude = &view.rows[1];
        assert!(claude.ready && !claude.setup && claude.startable() && claude.has_path());
        assert_eq!(claude.default_detail.as_ref(), "Decides itself.");
        assert_eq!(thread_items(claude).len(), 1);
    }

    #[test]
    fn a_claude_code_mode_change_applies_at_once() {
        let view = HarnessesView::new(claude_list("ready", None));
        let row = &view.rows[1];
        let key = |mode: &str, reported: Option<&str>| ThreadHarness {
            id: HarnessId::Claude,
            version: Some("2.1.326".into()),
            mode: Some(mode.into()),
            reported: reported.map(Into::into),
        };
        let line = mode_line(row, &key("auto", None)).unwrap();
        assert_eq!(
            (line.label.as_ref(), line.note.as_ref()),
            ("Auto", "Decides itself.")
        );
        // Hover says what the mode allows; the chip itself only names it.
        assert_eq!(
            line.tip.as_ref(),
            "Decides itself. Claude Code's mode. Choose what Claude Code may do in this thread. A change applies at once, even during a run."
        );
        assert_eq!(line.applies.as_ref(), "Applies at once, even during a run.");
        let line = mode_line(row, &key("auto", Some("plan"))).unwrap();
        assert!(line.overridden);
        // A mode the harness is not using is drawn under the controls, so
        // hover does not repeat it.
        assert!(line.tip.starts_with("Claude Code's mode."), "{}", line.tip);
        assert_eq!(
            line.note.as_ref(),
            "Claude Code is using Plan instead of Auto, set by your Claude Code settings. Plans first."
        );
        let line = mode_line(row, &key("ask", Some("custom"))).unwrap();
        assert_eq!(
            line.note.as_ref(),
            "Claude Code is using permissions its own settings set, not Ask."
        );
    }

    fn thread(mode: Option<&str>, reported: Option<&str>) -> ThreadHarness {
        ThreadHarness {
            id: HarnessId::Codex,
            version: None,
            mode: mode.map(Into::into),
            reported: reported.map(Into::into),
        }
    }

    #[test]
    fn the_mode_line_names_the_choice_and_says_when_the_harness_used_another() {
        let view = HarnessesView::new(sample());
        let (pi, codex) = (&view.rows[0], &view.rows[1]);
        assert_eq!(codex.default_detail.as_ref(), "Never asks.");
        // Pi uses Plait's approvals, so it has no mode line.
        assert!(mode_line(pi, &thread(None, None)).is_none());
        // Before the first run, the chosen mode says what it does.
        let line = mode_line(codex, &thread(Some("ask"), None)).unwrap();
        assert_eq!((line.id.as_str(), line.label.as_ref()), ("ask", "Ask"));
        assert_eq!(
            (line.note.as_ref(), line.overridden),
            ("Asks first.", false)
        );
        // Hover leads with what the mode allows, then what protects the checkout.
        assert!(
            line.tip
                .starts_with("Asks first. Codex's sandbox. Choose what Codex may do"),
            "{}",
            line.tip
        );
        // A thread without a mode yet shows the harness default.
        let line = mode_line(codex, &thread(None, None)).unwrap();
        assert_eq!(line.label.as_ref(), "Full access");
        // The run used what was chosen.
        let line = mode_line(codex, &thread(Some("ask"), Some("ask"))).unwrap();
        assert!(!line.overridden);
        // Codex's configuration ran another mode.
        let line = mode_line(codex, &thread(Some("ask"), Some("read_only"))).unwrap();
        assert!(line.overridden);
        assert_eq!(
            line.note.as_ref(),
            "Codex is using Read only instead of Ask, set by your Codex configuration. Only reads."
        );
        assert_eq!(
            line.applies.as_ref(),
            "Applies from Codex's next turn, even during a run."
        );
        let line = mode_line(codex, &thread(Some("full_access"), Some("custom"))).unwrap();
        assert_eq!(
            line.note.as_ref(),
            "Codex is using permissions its own configuration sets, not Full access."
        );
    }
}
