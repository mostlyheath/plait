//! Tab preferences are disposable; draft durability remains with the journal.
use super::Plait;
use gpui::*;
use plait_client::{
    session::Journal,
    tabs::{
        self, CheckoutKey, TabId, TabTarget,
        bookmark::{BookmarkError, BookmarkRecord, ThreadBookmark},
        persistence::{self, RestoreContext, RestoreStatus, SavedWindow},
        storage,
    },
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Where this device's tabs for the connected machine are kept: a host that
/// keeps a workspace stores them for this device; otherwise a file on this Mac.
#[derive(Clone)]
pub(super) enum Target {
    File(PathBuf),
    Host {
        client: plait_client::host::Client,
        device: String,
        /// The device state revision last read or written, for compare-and-set.
        revision: Arc<AtomicU64>,
    },
}

/// The host keeps at most this much state per device; tabs over it are not saved.
const HOST_TABS_LIMIT: usize = 56_000;

#[derive(Default)]
pub(super) struct Persistence {
    /// Set once the connected host's tabs for this device have been read.
    pub host_loaded: bool,
    pub host_loading: bool,
    /// Advances when the window moves to another machine; older reads are dropped.
    pub host_generation: u64,
    pub target: Option<Target>,
    pub requested: bool,
    pub loaded: bool,
    pub restored: bool,
    pub writable: bool,
    pub edited: bool,
    pub path: Option<PathBuf>,
    pub bytes: Option<Vec<u8>>,
    pub window: Option<AnyWindowHandle>,
    save: Option<Task<()>>,
    error: Option<String>,
    gate: Arc<WriteGate>,
}
#[derive(Default)]
struct WriteGate {
    revision: AtomicU64,
    serial: Mutex<()>,
    last: Mutex<Option<Vec<u8>>>,
    /// The extended view this saver last stored, so unchanged views are not rewritten.
    view: Mutex<Option<(Vec<u8>, String)>>,
}
pub(super) struct SaveRequest {
    document: SavedWindow,
    active: Option<(TabId, ThreadBookmark)>,
    journal: Arc<Journal>,
    target: Target,
    gate: Arc<WriteGate>,
    revision: u64,
}
#[cfg(test)]
impl Persistence {
    pub(super) fn save_revision(&self) -> u64 {
        self.gate.revision.load(Ordering::Acquire)
    }
}
impl SaveRequest {
    pub(super) fn write(mut self) -> Result<(), String> {
        let _guard = self
            .gate
            .serial
            .lock()
            .map_err(|_| "Tab writer is unavailable.")?;
        if self.revision != self.gate.revision.load(Ordering::Acquire) {
            return Ok(());
        }
        // Save drafts first when possible. A journal over its limit reports itself;
        // the tab file stays independent, and the decoder omits drafts not yet durable.
        let _ = self.journal.flush();
        let mut view = self
            .gate
            .view
            .lock()
            .map_err(|_| "Tab writer is unavailable.")?;
        let mut stored = None;
        if let Some((id, bookmark)) = self.active {
            let record = match BookmarkRecord::capture(&bookmark) {
                Ok(record) => record,
                Err(BookmarkError::NeedsStorage) => {
                    let bytes = tabs::bookmark::encode_extended(&bookmark)
                        .map_err(|_| "The current tab view could not be saved.")?;
                    let blob = match view.as_ref() {
                        Some((old, name)) if *old == bytes => name.clone(),
                        _ => {
                            let blob = super::tab_lifecycle::record_name("view");
                            self.journal.write_record_bytes(
                                &blob,
                                &bytes,
                                tabs::bookmark::EXTENDED_BOOKMARK_LIMIT,
                            )?;
                            blob
                        }
                    };
                    stored = Some((bytes, blob.clone()));
                    BookmarkRecord::stored(bookmark.view, blob)
                        .map_err(|_| "Invalid tab bookmark name.")?
                }
                Err(_) => {
                    return Err(
                        "The current tab view could not be saved; previous saved tabs are intact."
                            .into(),
                    );
                }
            };
            self.document
                .set_bookmark(id, record)
                .map_err(|_| "The current tab view could not be saved.")?;
        }
        let bytes = persistence::encode_document(&self.document)
            .map_err(|_| "Tab preferences exceed their limit; previous saved tabs are intact.")?;
        let mut last = self
            .gate
            .last
            .lock()
            .map_err(|_| "Tab writer is unavailable.")?;
        if last.as_ref() != Some(&bytes) {
            write_target(&self.target, &bytes)?;
            *last = Some(bytes);
        }
        // Only the record this saver wrote earlier is removed; restored and parked
        // views belong to the tabs that still name them.
        let retired = view
            .take()
            .map(|(_, name)| name)
            .filter(|name| stored.as_ref().is_none_or(|(_, blob)| blob != name));
        *view = stored;
        if let Some(name) = retired {
            let _ = self.journal.remove_record(&name);
        }
        Ok(())
    }
}

/// Stores the tab document. A host keeps it as this device's state; a write
/// that lost a race with another of this device's windows rereads and retries once.
fn write_target(target: &Target, bytes: &[u8]) -> Result<(), String> {
    use plait_client::host::device;
    let (client, device, revision) = match target {
        Target::File(path) => return storage::write(path, bytes),
        Target::Host {
            client,
            device,
            revision,
        } => (client, device, revision),
    };
    if bytes.len() > HOST_TABS_LIMIT {
        return Err("Too many tabs to keep on the host; previous saved tabs are intact.".into());
    }
    let saved = match device::set_field(
        client,
        device,
        revision.load(Ordering::Acquire),
        "tabs",
        bytes,
    ) {
        Err(error) if device::is_conflict(&error) => {
            let (current, _) =
                device::get_field(client, device, "tabs").map_err(|e| e.to_string())?;
            device::set_field(client, device, current, "tabs", bytes)
        }
        other => other,
    }
    .map_err(|e| format!("The host did not keep this device's tabs: {e}"))?;
    revision.store(saved, Ordering::Release);
    Ok(())
}

impl Plait {
    /// This device's stable ID on every host, made once.
    pub(super) fn device_id(&mut self, cx: &mut Context<Self>) -> String {
        if self.preferences.value.device_id.is_empty() {
            let mut preferences = self.preferences.value.clone();
            preferences.device_id = plait_client::host_state::request_id();
            self.set_preferences(preferences, super::jump::PreferenceGroup::View, cx);
        }
        self.preferences.value.device_id.clone()
    }

    /// Reads this device's tabs from a host that keeps them. The first time on
    /// this Mac's own host, the tabs saved on this Mac stand in for them.
    pub(super) fn load_host_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let device = self.device_id(cx);
        let client = self.host_client();
        let local = self.host_is_local();
        let generation = self.tabs.persistence.host_generation;
        let task = cx.background_executor().spawn({
            let client = client.clone();
            let device = device.clone();
            async move { plait_client::host::device::get_field(&client, &device, "tabs") }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let state = &mut this.tabs.persistence;
                if state.host_generation != generation {
                    return;
                }
                state.host_loaded = true;
                state.host_loading = false;
                match result {
                    Ok((revision, tabs)) => {
                        if tabs.is_some() || !local {
                            state.bytes = tabs;
                        }
                        state.target = Some(Target::Host {
                            client,
                            device,
                            revision: Arc::new(AtomicU64::new(revision)),
                        });
                        state.writable = true;
                    }
                    // An older host keeps no device state: this Mac's file, or nothing for another machine.
                    Err(_) if local => {}
                    Err(_) => {
                        state.bytes = None;
                        state.writable = false;
                    }
                }
                this.try_restore_tabs(window, cx);
            });
        })
        .detach();
    }

    /// Save this machine's tab document where it belongs. Runs while this
    /// machine is still selected, so its own drafts are in the document.
    pub(super) fn flush_machine_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(request) = self.flush_tab_preferences(Some(window), cx) {
            cx.background_executor()
                .spawn(async move {
                    let _ = request.write();
                })
                .detach();
        }
    }

    /// Another machine's tabs are its own: start empty until the new machine's
    /// tabs are read from its host. `flush_machine_tabs` saved these first.
    pub(super) fn switch_machine_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.model = tabs::WindowTabs::new(super::tabs::fresh_window_id());
        let state = &mut self.tabs.persistence;
        state.host_generation += 1;
        state.host_loaded = false;
        state.host_loading = false;
        state.target = None;
        state.bytes = None;
        state.restored = false;
        state.edited = false;
        self.refresh_tab_rows();
        // An older local host keeps tabs in this Mac's file; read it again.
        self.load_tab_preferences(window, cx);
    }

    /// Whether the connected machine's tab store is known yet.
    fn tab_store_ready(&self) -> bool {
        use super::host_workspace::Source;
        match self.host_workspace.source {
            Source::Host(_) => self.tabs.persistence.host_loaded,
            Source::Legacy => true,
            // No stream was asked for, or it failed: this Mac's file.
            Source::Pending => !self.host_workspace_started() || self.host_workspace_failed(),
        }
    }

    pub(super) fn tab_save_input(&self, cx: &Context<Self>) -> impl IntoElement {
        let owner = cx.weak_entity();
        // Scrollers consume bubbling events; capture before they do and save after settling.
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_mouse_event(move |_: &ScrollEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = owner.update(cx, |this, cx| this.queue_tab_save(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    pub(super) fn load_tab_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.persistence.requested = true;
        self.tabs.persistence.window = Some(window.window_handle());
        let task = cx.background_executor().spawn(async move {
            let path = storage::default_path();
            let read = path.as_deref().map(storage::read).transpose();
            (path, read)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (path, read) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let state = &mut this.tabs.persistence;
                state.loaded = true;
                state.path = path;
                state.writable = read.is_ok() && state.path.is_some();
                state.bytes = read.ok().flatten().flatten();
                this.try_restore_tabs(window, cx);
            });
        })
        .detach();
    }

    pub(super) fn recoverable_tab_drafts(&self) -> Vec<TabTarget> {
        let Some(data) = &self.session.data else {
            return Vec::new();
        };
        // Another machine's drafts stay saved for it; only this machine's return here.
        data.draft_tabs
            .iter()
            .filter(|(_, tab)| self.draft_is_here(tab))
            .map(|(id, tab)| TabTarget::Draft {
                id: id.clone(),
                checkout: self.tab_checkout(tab.checkout.clone()),
            })
            .collect()
    }

    fn verified_tab_checkouts(&self) -> Vec<CheckoutKey> {
        let local = self
            .tabs
            .identity
            .checkouts
            .iter()
            .map(|(root, identity)| CheckoutKey {
                host: self.tabs.host.clone(),
                root: root.clone(),
                directory: Some(*identity),
            });
        let remote = self.tabs.identity.remote.iter().map(|root| CheckoutKey {
            host: self.tabs.host.clone(),
            root: root.clone(),
            directory: None,
        });
        local.chain(remote).collect()
    }

    pub(super) fn try_restore_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.persistence.requested
            || self.tabs.persistence.restored
            || !self.tabs.persistence.loaded
            || !self.tabs.identity.verified
            || !self.tab_identity_current()
            || !self.tabs.identity.catalog_ready
            || self.session.loading
            || self.session.data.is_none()
            || self.tabs.lifecycle.busy
            || !self.tab_store_ready()
        {
            return;
        }
        let drafts = self.recoverable_tab_drafts();
        let checkouts = self.verified_tab_checkouts();
        let context = RestoreContext {
            host: &self.tabs.host,
            threads: self.host.connection.threads(),
            checkouts: &checkouts,
            drafts: &drafts,
            connection: self
                .host
                .connection
                .connected
                .then_some(self.host.connection.epoch),
        };
        let bytes = self.tabs.persistence.bytes.take();
        let restored = bytes
            .as_deref()
            .map(|bytes| persistence::decode(bytes, super::tabs::fresh_window_id(), &context));
        let connection = context.connection;
        self.tabs.persistence.restored = true;
        if let Some(restored) = restored {
            self.tabs.persistence.writable &= restored.status != RestoreStatus::UnknownVersion;
            self.session.restore_thread = None;
            if !self.tabs.persistence.edited {
                let covered = self.settings_open;
                self.tabs.model = restored.window;
                self.park_workspace(cx);
                self.tabs.lifecycle.presenting = true;
                if let Some(id) = self.tabs.model.active() {
                    let result = self.tabs.model.activate(id);
                    self.tab_error(result);
                }
                self.present_tab(window, cx);
                self.tabs.lifecycle.presenting = false;
                self.keep_beneath_settings(covered, window, cx);
            } else if self.tabs.model.active().is_some() {
                // Keep the person's early action in front while restoring the other tabs.
                let active = self.tabs.model.active();
                for tab in restored.window.tabs() {
                    let target = tab.session().target().clone();
                    let already_open = self
                        .tabs
                        .model
                        .tabs()
                        .iter()
                        .any(|old| old.session().target() == &target);
                    if !already_open && let Ok(id) = self.tabs.model.open(target, connection) {
                        let _ = self
                            .tabs
                            .model
                            .save_bookmark(id, tab.session().bookmark().clone());
                    }
                }
                if let Some(id) = active {
                    let _ = self.tabs.model.activate(id);
                }
                self.refresh_tab_rows();
            }
        } else if !self.tabs.persistence.edited {
            self.restore_thread_selection(window, cx);
        }
        // Closed drafts and legacy journals have no tab preference record. Keep
        // their text reachable even after the session-only reopen history expires.
        let active = self.tabs.model.active();
        let mut recovered = false;
        for target in drafts {
            if !checkouts.contains(target.checkout())
                || self
                    .tabs
                    .model
                    .tabs()
                    .iter()
                    .any(|tab| tab.session().target() == &target)
            {
                continue;
            }
            let TabTarget::Draft { id, checkout } = &target else {
                continue;
            };
            if !self
                .session
                .data
                .as_ref()
                .and_then(|s| s.draft_tab(id, &checkout.root))
                .is_some_and(|tab| {
                    tab.creation.is_some()
                        || !tab.draft.text.is_empty()
                        || !tab.draft.images.is_empty()
                })
            {
                continue;
            }
            if self.tabs.model.open(target, connection).is_ok() {
                recovered = true;
            }
        }
        if let Some(active) = active {
            let _ = self.tabs.model.activate(active);
        }
        if recovered {
            let covered = self.settings_open;
            self.park_workspace(cx);
            self.tabs.lifecycle.presenting = true;
            self.present_tab(window, cx);
            self.tabs.lifecycle.presenting = false;
            self.keep_beneath_settings(covered, window, cx);
        }
        self.forget_unopened_empty_drafts(cx);
        self.queue_tab_save(cx);
        cx.notify();
    }

    /// Empty drafts are saved only so their open tabs return. Restore is the one
    /// point that knows which returned; drop the rest so they do not accumulate.
    fn forget_unopened_empty_drafts(&mut self, cx: &mut Context<Self>) {
        let mut kept: std::collections::HashSet<String> = self
            .tabs
            .model
            .tabs()
            .iter()
            .filter_map(|tab| match tab.session().target() {
                TabTarget::Draft { id, .. } => Some(id.clone()),
                TabTarget::Existing { .. }
                | TabTarget::Worktrees { .. }
                | TabTarget::Issues { .. } => None,
            })
            .collect();
        kept.extend(self.draft.id.clone());
        kept.extend(self.tabs.lifecycle.drafts.keys().cloned());
        // Restore here says nothing about another machine's open tabs.
        let elsewhere: Vec<String> = self
            .session
            .data
            .iter()
            .flat_map(|data| &data.draft_tabs)
            .filter(|(_, tab)| !self.draft_is_here(tab))
            .map(|(id, _)| id.clone())
            .collect();
        kept.extend(elsewhere);
        let Some(data) = &mut self.session.data else {
            return;
        };
        let unopened: Vec<String> = data
            .draft_tabs
            .iter()
            .filter(|(id, tab)| {
                !kept.contains(*id)
                    && tab.creation.is_none()
                    && tab.draft.creation.is_none()
                    && tab.draft.text.is_empty()
                    && tab.draft.images.is_empty()
            })
            .map(|(id, _)| id.clone())
            .collect();
        if unopened.is_empty() {
            return;
        }
        for id in &unopened {
            data.clear_draft_tab(id);
        }
        if let Some(journal) = &self.session.journal {
            journal.queue(data.clone());
        }
        self.schedule_session_save(false, cx);
    }

    pub(super) fn tab_save_request(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<SaveRequest> {
        let state = &self.tabs.persistence;
        if !state.restored
            || !state.writable
            || !self.tabs.identity.verified
            || !self.tab_identity_current()
        {
            return None;
        }
        let drafts = self.recoverable_tab_drafts();
        let checkouts = self.verified_tab_checkouts();
        let context = RestoreContext {
            host: &self.tabs.host,
            threads: self.host.connection.threads(),
            checkouts: &checkouts,
            drafts: &drafts,
            connection: self
                .host
                .connection
                .connected
                .then_some(self.host.connection.epoch),
        };
        Some(SaveRequest {
            document: persistence::capture(&self.tabs.model, &context),
            active: self
                .tabs
                .model
                .active()
                .filter(|_| !self.worktrees_visible())
                .map(|id| (id, self.capture_tab_bookmark(window, cx))),
            target: state
                .target
                .clone()
                .or_else(|| state.path.clone().map(Target::File))?,
            journal: self.session.journal.clone()?,
            gate: state.gate.clone(),
            revision: state.gate.revision.load(Ordering::Acquire),
        })
    }

    pub(super) fn queue_tab_save(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.tabs.persistence;
        if !state.restored
            || !state.writable
            || !self.tabs.identity.verified
            || self.session.closing
        {
            return;
        }
        let Some(window) = state.window else {
            return;
        };
        let revision = state.gate.revision.fetch_add(1, Ordering::AcqRel) + 1;
        state.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let request = window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| this.tab_save_request(Some(window), cx))
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(request) = request else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { request.write() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if revision != this.tabs.persistence.gate.revision.load(Ordering::Acquire) {
                    return;
                }
                match result {
                    Err(error) => {
                        let note = format!("Tabs not saved: {error}");
                        this.tabs.persistence.error = Some(note.clone());
                        this.notice = Some(note);
                        cx.notify();
                    }
                    Ok(()) => {
                        if let Some(error) = this.tabs.persistence.error.take()
                            && this.notice.as_ref() == Some(&error)
                        {
                            this.notice = None;
                            cx.notify();
                        }
                    }
                }
            });
        }));
    }

    pub(super) fn flush_tab_preferences(
        &mut self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<SaveRequest> {
        self.tabs.persistence.save = None;
        self.tabs
            .persistence
            .gate
            .revision
            .fetch_add(1, Ordering::AcqRel);
        self.tab_save_request(window, cx)
    }
}
