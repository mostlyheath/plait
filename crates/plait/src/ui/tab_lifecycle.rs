//! Only the selected tab owns a GPUI workspace. Parking is a save boundary.
use super::{Plait, composer::ParkedEditor, tabs::TabNavigation};
use gpui::*;
use plait_client::tabs::{
    TabId, TabTarget,
    bookmark::{self, BookmarkRecord, ThreadBookmark},
};
use std::sync::atomic::{AtomicU64, Ordering};

const EDITOR_LIMIT: usize = 8 * 1024 * 1024;
static RECORD: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(super) enum Intent {
    Open(TabTarget),
    Activate(TabId),
    Close(TabId),
    Navigate(TabNavigation),
}
#[derive(Clone)]
pub(super) enum EditorRecord {
    Memory(Box<ParkedEditor>),
    Stored(String),
}
#[derive(Default)]
pub(super) struct Lifecycle {
    pub busy: bool,
    pub view_revision: u64,
    pub presenting: bool,
    pub hydrating: bool,
    hydrate_generation: u64,
    pending: std::collections::VecDeque<Intent>,
    /// Parked editors, by the machine they were typed on and their tab: two
    /// machine entries can name the same host, so the target alone is not enough.
    pub editors: Vec<(plait_client::machines::MachineId, TabTarget, EditorRecord)>,
    pub drafts: std::collections::BTreeMap<String, super::new_thread::Draft>,
}

impl Lifecycle {
    pub(super) fn reopen_checkout(&mut self, key: &plait_client::tabs::CheckoutKey) {
        let update = |target: &mut TabTarget| {
            let checkout = match target {
                TabTarget::Existing { checkout, .. }
                | TabTarget::Draft { checkout, .. }
                | TabTarget::Worktrees { checkout }
                | TabTarget::Issues { checkout }
                | TabTarget::Graph { checkout } => checkout,
            };
            if checkout.host == key.host && checkout.root == key.root {
                *checkout = key.clone();
            }
        };
        for (_, target, _) in &mut self.editors {
            update(target);
        }
        for intent in &mut self.pending {
            if let Intent::Open(target) = intent {
                update(target);
            }
        }
    }

    pub(super) fn verify_pending(
        &mut self,
        previous: &plait_client::tabs::HostKey,
        host: &plait_client::tabs::HostKey,
        checkouts: &[plait_client::tabs::CheckoutKey],
    ) {
        for intent in &mut self.pending {
            if let Intent::Open(target) = intent {
                let checkout = match target {
                    TabTarget::Existing { checkout, .. }
                    | TabTarget::Draft { checkout, .. }
                    | TabTarget::Worktrees { checkout }
                    | TabTarget::Issues { checkout }
                    | TabTarget::Graph { checkout } => checkout,
                };
                if &checkout.host == previous {
                    checkout.host = host.clone();
                    checkout.directory = checkouts
                        .iter()
                        .find(|c| c.root == checkout.root)
                        .and_then(|c| c.directory);
                }
            }
        }
    }
}

impl Plait {
    pub(super) fn request_tab(
        &mut self,
        intent: Intent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tabs.persistence.edited = true;
        self.tabs.identity.opening += 1;
        if self.tabs.lifecycle.busy {
            self.tabs.lifecycle.pending.push_back(intent);
            return;
        }
        if self.host.creation.creating || self.host.creation.pending_create.is_some() {
            let same_checkout = match &intent {
                Intent::Open(target) => target.checkout().root == self.host.creation.checkout,
                Intent::Activate(id) => self.tabs.model.get(*id).is_some_and(|t| {
                    t.session().target().checkout().root == self.host.creation.checkout
                }),
                Intent::Close(id) => self.tabs.model.active() != Some(*id),
                Intent::Navigate(_) => false,
            };
            if !same_checkout {
                self.notice = Some("Resolve the pending thread creation before changing checkout or closing its tab.".into());
                cx.notify();
                return;
            }
        }
        if let Intent::Activate(id) = &intent
            && self
                .tabs
                .model
                .get(*id)
                .is_some_and(|t| t.session().target().checkout().host != self.tabs.host)
        {
            self.notice = Some("This tab belongs to a different host store. Open the thread from the current sidebar.".into());
            cx.notify();
            return;
        }
        let unchanged = match &intent {
            Intent::Close(id) => self.tabs.model.active() != Some(*id),
            Intent::Activate(id) => self.tabs.model.active() == Some(*id),
            Intent::Open(target) => self
                .tabs
                .model
                .active()
                .and_then(|id| self.tabs.model.get(id))
                .is_some_and(|tab| tab.session().target() == target),
            Intent::Navigate(_) => false,
        };
        if unchanged {
            self.perform_tab(intent, window, cx);
            return;
        }
        let Some(active) = self.tabs.model.active() else {
            self.perform_tab(intent, window, cx);
            return;
        };
        if self.worktrees_visible() || self.graph_visible() {
            self.perform_tab(intent, window, cx);
            return;
        }
        let target = self
            .tabs
            .model
            .get(active)
            .unwrap()
            .session()
            .target()
            .clone();
        let editor = self
            .visible_composer()
            .cloned()
            .map(|c| c.update(cx, |c, _| c.park_editor()));
        let machine = self.machines.active.clone();
        self.capture_session(cx);
        let bookmark = self.capture_tab_bookmark(Some(window), cx);
        let record = match BookmarkRecord::capture(&bookmark) {
            Ok(record) => record,
            Err(bookmark::BookmarkError::NeedsStorage) if self.session.journal.is_some() => {
                BookmarkRecord::stored(bookmark.view, record_name("view")).expect("generated name")
            }
            Err(_) => {
                self.notice = Some("Tab kept open: its view could not be saved.".into());
                cx.notify();
                return;
            }
        };
        let Some(journal) = self.session.journal.clone() else {
            let memory: usize = self
                .tabs
                .lifecycle
                .editors
                .iter()
                .filter(|(m, old, _)| !(m == &machine && old == &target))
                .map(|(_, _, e)| match e {
                    EditorRecord::Memory(e) => e.retained_bytes(),
                    EditorRecord::Stored(_) => 0,
                })
                .sum();
            if memory + editor.as_ref().map_or(0, ParkedEditor::retained_bytes) > EDITOR_LIMIT {
                self.notice = Some("Tab kept open: private editor storage is unavailable.".into());
                cx.notify();
                return;
            }
            if let Some(editor) = editor {
                self.remember_editor(machine, target, EditorRecord::Memory(Box::new(editor)), cx);
            }
            let result = self.tabs.model.save_bookmark(active, record);
            if self.tab_error(result) {
                self.perform_tab(intent, window, cx);
            }
            return;
        };
        self.tabs.lifecycle.busy = true;
        let saved = bookmark.clone();
        let saved_record = record.clone();
        let task = cx.background_executor().spawn(async move {
            journal.flush()?;
            if let BookmarkRecord::Stored { blob, .. } = &saved_record {
                let bytes =
                    bookmark::encode_extended(&saved).map_err(|_| "Tab bookmark is too large.")?;
                journal.write_record_bytes(blob, &bytes, bookmark::EXTENDED_BOOKMARK_LIMIT)?;
            }
            let editor = editor
                .map(|editor| {
                    let name = record_name("editor");
                    journal
                        .write_record(&name, &editor, EDITOR_LIMIT)
                        .map(|()| name)
                })
                .transpose()?;
            Ok::<_, String>(editor)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.tabs.lifecycle.busy = false;
                match result {
                    Err(error) => {
                        this.tabs.lifecycle.pending.clear();
                        this.apply_pending_tab_identity(window, cx);
                        this.notice = Some(format!(
                            "Tab kept open because work could not be saved: {error}"
                        ));
                        cx.notify();
                    }
                    Ok(editor) => {
                        // Edits made while flushing belong to a newer save boundary.
                        if this.tabs.model.active() != Some(active)
                            || this.machines.active != machine
                        {
                            return;
                        }
                        if this.capture_tab_bookmark(Some(window), cx) != bookmark {
                            this.request_tab(intent, window, cx);
                            return;
                        }
                        if let Some(editor) = editor {
                            this.remember_editor(machine, target, EditorRecord::Stored(editor), cx);
                        }
                        let result = this.tabs.model.save_bookmark(active, record);
                        if this.tab_error(result) {
                            this.perform_tab(intent, window, cx);
                        }
                    }
                }
            });
        })
        .detach();
    }

    fn remember_editor(
        &mut self,
        machine: plait_client::machines::MachineId,
        target: TabTarget,
        editor: EditorRecord,
        cx: &mut Context<Self>,
    ) {
        let same = |m: &plait_client::machines::MachineId, old: &TabTarget| {
            *m == machine && *old == target
        };
        let retired: Vec<_> = self
            .tabs
            .lifecycle
            .editors
            .iter()
            .filter_map(|(m, old, editor)| match editor {
                EditorRecord::Stored(name) if same(m, old) => Some(name.clone()),
                _ => None,
            })
            .collect();
        self.tabs
            .lifecycle
            .editors
            .retain(|(m, old, _)| !same(m, old));
        self.tabs.lifecycle.editors.push((machine, target, editor));
        if let Some(journal) = self.session.journal.clone() {
            cx.background_executor()
                .spawn(async move {
                    for name in retired {
                        let _ = journal.remove_record(&name);
                    }
                })
                .detach();
        }
    }

    fn perform_tab(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.tabs.model.active();
        // The workspace is rebuilt for the next tab; keep this tab's Find in Changes.
        let find = &self.workspace.diff_find;
        if let Some(field) = &find.field {
            let saved = plait_client::tabs::bookmark::FindBookmark {
                open: find.open,
                query: field.read(cx).text().into(),
                current: find.bookmark_match(),
            };
            if let Some(panel) = self.tabs.model.panel_mut() {
                panel.diff_find = saved;
            }
        }
        let covered = self.settings_open;
        let connection = self
            .host
            .connection
            .connected
            .then_some(self.host.connection.epoch);
        // The current machine's saved drafts, read before the tab model changes.
        let saved_drafts: Vec<(String, std::path::PathBuf)> = self
            .saved_drafts()
            .into_iter()
            .map(|(id, tab)| (id, tab.checkout.clone()))
            .collect();
        let graph_supported = self.graph_supported();
        let result = match intent {
            Intent::Open(target) => self.tabs.model.open(target, connection).map(|_| ()),
            Intent::Activate(id) => self.tabs.model.activate(id),
            Intent::Close(id) => {
                let nonempty = self.tabs.model.get(id).is_some_and(|tab| {
                    let TabTarget::Draft {
                        id: draft_id,
                        checkout,
                    } = tab.session().target()
                    else {
                        return false;
                    };
                    if previous == Some(id) {
                        self.draft.pending.is_some() || self.draft
                            .composer
                            .as_ref()
                            .is_some_and(|c| !c.read(cx).is_empty())
                    } else {
                        // A parked draft's text may not be saved yet: it counts too.
                        self.tabs.lifecycle.drafts.get(draft_id).is_some_and(|parked| {
                            parked.pending.is_some()
                                || !parked.saved.text.is_empty()
                                || !parked.saved.images.is_empty()
                        }) || self
                            .saved_draft(draft_id, &checkout.root)
                            .is_some_and(|tab| !plait_client::session::draft_is_empty(tab))
                    }
                });
                let draft = self.tabs.model.get(id).and_then(|tab| match tab.session().target() { TabTarget::Draft { id, .. } => Some(id.clone()), _ => None });
                let result = self.tabs.model.close(id, nonempty);
                if result.is_ok() && !nonempty && let Some(draft) = draft {
                    // Empty-draft cleanup: only a stored record that is empty, for its own machine.
                    let machine = self.tabs.lifecycle.drafts.get(&draft).and_then(|d| d.binding.clone()).map(|b| b.machine)
                        .unwrap_or_else(|| self.machines.active.clone());
                    let mut works = std::collections::BTreeMap::new();
                    super::saved_work::touch(&mut works, &self.saved_work.machines, &machine).discard_empty_draft(&draft);
                    self.store_machine_work(works);
                    self.tabs.lifecycle.drafts.remove(&draft);
                }
                result
            }
            Intent::Navigate(action) => match action {
                TabNavigation::Next => self.tabs.model.next_tab(),
                TabNavigation::Previous => self.tabs.model.previous_tab(),
                TabNavigation::Jump(n) => self.tabs.model.jump(n),
                TabNavigation::Reopen => self.tabs.model.reopen(|target| match target {
                    TabTarget::Graph { .. } if !graph_supported => None,
                    // As on restore: only a project the sidebar still verifies.
                    TabTarget::Worktrees { checkout }
                    | TabTarget::Issues { checkout }
                    | TabTarget::Graph { checkout } => (checkout.host == self.tabs.host
                        && self.tabs.identity.checkouts.iter().any(|(root, identity)| {
                            *root == checkout.root && checkout.directory == Some(*identity)
                        }))
                    .then_some(connection),
                    TabTarget::Existing { id, checkout } => self
                        .host
                        .connection
                        .threads()
                        .iter()
                        .any(|t| &t.id == id && t.checkout == checkout.root)
                        .then_some(connection),
                    TabTarget::Draft {
                        id: draft_id,
                        checkout,
                    } => saved_drafts
                        .iter()
                        .any(|(id, root)| id == draft_id && *root == checkout.root)
                        .then_some(connection),
                }),
            }
            .and_then(|opened| {
                if opened.is_none() && matches!(action, TabNavigation::Reopen) {
                    // Only the current machine's drafts can reopen here.
                    let fallback = self.saved_drafts().into_iter().find(|(id, tab)| {
                        !plait_client::session::draft_is_empty(tab) && !self.tabs.model.tabs().iter().any(|open| matches!(open.session().target(), TabTarget::Draft { id: open_id, .. } if open_id == id))
                    }).map(|(id, tab)| TabTarget::Draft { id, checkout: self.tab_checkout(tab.checkout.clone()) });
                    if let Some(target) = fallback { self.tabs.model.open(target, connection)?; }
                }
                Ok(())
            }),
        };
        if self.tab_error(result) {
            if previous != self.tabs.model.active() {
                self.park_workspace(cx);
                self.tabs.lifecycle.presenting = true;
                self.present_tab(window, cx);
                self.tabs.lifecycle.presenting = false;
            }
            self.keep_beneath_settings(covered, window, cx);
            if !covered
                && self.workspace.tab == super::Workspace::Conversation
                && self.conversation_snapshot().is_some()
                && window.is_window_active()
                && let Some(id) = self.host.thread.selected.clone()
            {
                self.mark_thread_read(&id, cx);
            }
            self.tabs.list_open = false;
            self.refresh_tab_rows();
            self.retire_editor_cache(cx);
            self.capture_session(cx);
            self.queue_tab_save(cx);
            self.try_restore_tabs(window, cx);
            cx.notify();
        }
        self.apply_pending_tab_identity(window, cx);
        if let Some(intent) = self.tabs.lifecycle.pending.pop_front() {
            self.request_tab(intent, window, cx);
        }
    }

    /// Presenting a tab selects its thread, which would close an open modal.
    pub(super) fn keep_beneath_settings(
        &mut self,
        covered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_open = covered;
        if covered {
            self.settings_restore = self.visible_composer().map(|c| c.focus_handle(cx));
            window.focus(&self.settings_focus, cx);
        }
    }

    pub(super) fn park_workspace(&mut self, cx: &mut Context<Self>) {
        self.worktrees.park();
        self.graph.park();
        self.tabs.lifecycle.hydrating = false;
        self.tabs.lifecycle.hydrate_generation += 1;
        self.workspace.cancelled.store(true, Ordering::Release);
        self.workspace.host_watch = None;
        self.host.select(None);
        let field = self.workspace.commit_message.clone();
        field.update(cx, |f, cx| f.set_text("", cx));
        // Configuration acknowledgements are keyed operations, not view payload.
        let controls = std::mem::take(&mut self.workspace.thread_controls);
        let review_generation = self.workspace.turn_review.generation + 1;
        self.workspace = super::thread_workspace::ThreadWorkspace::new(field);
        self.workspace.turn_review.generation = review_generation;
        self.workspace.thread_controls = controls;
        self.workspace.thread_controls.usage_open = false;
        self.park_draft(cx);
        self.conversation_focus = None;
        self.conversation_find_focus = None;
        self.state.repository.park_view();
    }

    /// The parked editor for `target` on the selected machine only.
    pub(super) fn parked_editor(&self, target: &TabTarget) -> Option<EditorRecord> {
        self.tabs
            .lifecycle
            .editors
            .iter()
            .find(|(m, t, _)| *m == self.machines.active && t == target)
            .map(|(_, _, e)| e.clone())
    }

    pub(super) fn hydrate_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktrees_visible() || self.graph_visible() {
            return;
        }
        let Some(tab) = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
        else {
            return;
        };
        let lifetime = tab.lifetime();
        let view_revision = self.tabs.lifecycle.view_revision;
        let owner = self.host.read_stamp();
        let target = tab.session().target().clone();
        let record = tab.session().bookmark().clone();
        self.workspace.opened_turns = tab.session().opened_turns().clone();
        let editor = self.parked_editor(&target);
        if matches!(record, BookmarkRecord::Inline { .. })
            && !matches!(editor, Some(EditorRecord::Stored(_)))
        {
            if let Some(EditorRecord::Memory(editor)) = editor {
                self.install_editor(*editor, cx);
            }
            self.restore_tab_bookmark(record.inline().unwrap_or_default(), window, cx);
            return;
        }
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        self.tabs.lifecycle.hydrating = true;
        self.tabs.lifecycle.hydrate_generation += 1;
        let generation = self.tabs.lifecycle.hydrate_generation;
        let task = cx.background_executor().spawn(async move {
            let bookmark = match record {
                BookmarkRecord::Inline { .. } => record.inline(),
                BookmarkRecord::Worktrees { .. }
                | BookmarkRecord::Issues { .. }
                | BookmarkRecord::Graph { .. } => None,
                BookmarkRecord::Stored { blob, .. } => journal
                    .read_record_bytes(&blob, bookmark::EXTENDED_BOOKMARK_LIMIT)?
                    .and_then(|bytes| bookmark::decode_extended(&bytes)),
            }
            .ok_or("Saved tab view is unavailable; showing its default view.")?;
            let editor = match editor {
                Some(EditorRecord::Stored(name)) => journal.read_record(&name, EDITOR_LIMIT),
                Some(EditorRecord::Memory(editor)) => Ok(Some(*editor)),
                None => Ok(None),
            };
            Ok::<_, String>((bookmark, editor))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if generation != this.tabs.lifecycle.hydrate_generation {
                    return;
                }
                this.tabs.lifecycle.hydrating = false;
                if this.tabs.lifecycle.view_revision != view_revision
                    || this.tabs.model.active() != Some(lifetime.tab)
                    || !this.host.accepts_read(&owner)
                    || this
                        .tabs
                        .model
                        .get(lifetime.tab)
                        .is_none_or(|tab| tab.lifetime() != lifetime)
                {
                    this.apply_pending_tab_identity(window, cx);
                    return;
                }
                match result {
                    Ok((bookmark, editor)) => {
                        match editor {
                            Ok(Some(editor)) => this.install_editor(editor, cx),
                            Ok(None) => {}
                            Err(_) => this.notice = Some(
                                "The editor history was unavailable. Your saved draft is intact."
                                    .into(),
                            ),
                        }
                        this.restore_tab_bookmark(bookmark, window, cx);
                    }
                    Err(error) => {
                        this.notice = Some(error);
                        this.restore_tab_bookmark(ThreadBookmark::default(), window, cx);
                    }
                }
                this.apply_pending_tab_identity(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn install_editor(&mut self, editor: ParkedEditor, cx: &mut Context<Self>) {
        if editor.retained_bytes() > EDITOR_LIMIT {
            self.notice =
                Some("The editor history exceeded its limit. Your saved draft is intact.".into());
            return;
        }
        if let Some(composer) = self.visible_composer() {
            composer.update(cx, |c, cx| c.restore_editor(editor, cx));
        }
    }

    fn retire_editor_cache(&mut self, cx: &mut Context<Self>) {
        // Closed drafts without an in-flight creation can be restored from the
        // journal: saving first makes that true for a parked draft's latest text.
        self.capture_session(cx);
        self.tabs.lifecycle.drafts.retain(|id, draft| {
            draft.pending.is_some() || self.tabs.model.tabs().iter().any(|tab| {
                matches!(tab.session().target(), TabTarget::Draft { id: open, .. } if open == id)
            }) || self.session.data.is_none() && (!draft.saved.text.is_empty() || !draft.saved.images.is_empty())
        });
        let mut removed = Vec::new();
        self.tabs.lifecycle.editors.retain(|(_, target, editor)| {
            if self.tabs.model.retains_target(target) {
                return true;
            }
            if let EditorRecord::Stored(name) = editor {
                removed.push(name.clone());
            }
            false
        });
        if let Some(journal) = self.session.journal.clone() {
            cx.background_executor()
                .spawn(async move {
                    for name in removed {
                        let _ = journal.remove_record(&name);
                    }
                })
                .detach();
        }
    }
}

pub(super) fn record_name(kind: &str) -> String {
    format!(
        "{kind}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        RECORD.fetch_add(1, Ordering::Relaxed)
    )
}
