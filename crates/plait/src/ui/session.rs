//! Desktop recovery and save scheduling. Host execution remains independent.

use super::{Plait, composer::Composer};
use gpui::*;
use plait_client::{
    host::{MessageRole, ThreadSummary},
    machines::MachineId,
    session::{self, Draft, Journal, MachineWork, Session, Thread},
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

/// What a thread is called before it has a first message or a chosen name.
pub(super) const UNTITLED: &str = "New thread";

#[derive(Default)]
pub(super) struct SessionState {
    pub required: bool,
    pub loading: bool,
    pub closing: bool,
    pub journal: Option<Arc<Journal>>,
    pub data: Option<Session>,
    pub note: Option<String>,
    pub(super) restore_thread: Option<String>,
    save: Option<Task<()>>,
}

impl SessionState {
    pub fn ready(&self) -> bool {
        !self.closing && (!self.required || (!self.loading && self.journal.is_some()))
    }
}

impl Plait {
    pub(super) fn acknowledge_saved_draft(
        &mut self,
        id: &str,
        text: &str,
        revision: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        // Only the record this thread's composer, or else its send ledger,
        // saves to: the machine it is bound to, at the listed folder.
        let Some(checkout) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|summary| summary.id == id)
            .map(|summary| summary.checkout.clone())
        else {
            return;
        };
        let ledger = self.ledger_machine();
        let machine = self
            .workspace
            .bindings
            .get(id)
            .map(|binding| binding.machine.clone())
            .unwrap_or_else(|| ledger.clone());
        let Some(saved) = self
            .saved_work
            .machines
            .get(&machine)
            .and_then(|work| work.thread(id, &checkout))
        else {
            return;
        };
        let mut thread = saved.thread.clone();
        let draft = &mut thread.draft;
        let revision = revision.or_else(|| {
            draft
                .submitted
                .as_ref()
                .filter(|(s, _)| s == text)
                .map(|(_, r)| *r)
        });
        if revision == Some(draft.revision) && draft.text == text {
            draft.text.clear();
            draft.images.clear();
            draft.submitted = None;
            draft.revision += 1;
        }
        if machine == ledger {
            thread.pending = self.host.submissions.pending(id).cloned();
            thread.pending_submissions = self
                .host
                .submissions
                .all(id)
                .iter()
                .skip(1)
                .cloned()
                .collect();
            thread.queued_submissions = self.host.submissions.accepted(id).to_vec();
        }
        let accepted = self.accepting_hosts();
        let mut works = BTreeMap::new();
        super::saved_work::touch(&mut works, &self.saved_work.machines, &machine)
            .put_thread(id, thread, &accepted);
        let (changed, _) = self.store_machine_work(works);
        if changed {
            self.schedule_session_save(false, cx);
        }
    }

    pub fn restore_desktop(
        &mut self,
        requested: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| {
                this.close_desktop(false, window, cx);
                false
            })
            .unwrap_or(true)
        });
        self.load_tab_preferences(window, cx);
        self.session.required = true;
        self.session.loading = true;
        self.session.note = Some("Restoring saved workspace…".into());
        self.sync_composer(cx);
        let checkout_generation = self.host.creation.generation;
        let task = cx.background_executor().spawn(async move {
            let (directory, host) = session::locations()?;
            let (journal, data) = Journal::open(&directory, host)?;
            let path = requested
                .or_else(|| data.checkout.clone().filter(|path| path.is_dir()))
                .unwrap_or_else(|| PathBuf::from("."));
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|root| root.join(&path))
                    .map_err(|error| format!("Cannot restore checkout: {error}"))?
            };
            let recovered = journal.recovery_files();
            Ok::<_, String>((journal, data, path, recovered))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.session.loading = false;
                match result {
                    Ok((journal, data, path, recovered)) => {
                        // This observer outlives the window. Closing it must flush the latest draft.
                        let quit_journal = journal.clone();
                        let quit_view = cx.weak_entity();
                        let quit_window = window.window_handle();
                        let app: &mut App = cx;
                        app.on_app_quit(move |cx| {
                            let journal = quit_journal.clone();
                            let (tabs, drafts) = quit_window.update(cx, |_, window, cx| {
                                quit_view.update(cx, |this, cx| {
                                    this.capture_session(cx);
                                    let tabs = this.flush_tab_preferences(Some(window), cx);
                                    (tabs, this.draft_flush(cx))
                                })
                            }).ok().and_then(Result::ok).unwrap_or((None, None));
                            cx.background_executor().spawn(async move {
                                // This quit cannot be cancelled (a user quit goes through
                                // close_desktop, which can). Local saves come first, within
                                // GPUI's quit timeout; tabs, which may go to a remote host, last.
                                if let Err(error) = journal.flush() {
                                    match journal.keep_unsaved() {
                                        Ok(kept) => eprintln!("Plait workspace: {error} Unsaved work was kept in {kept:?}."),
                                        Err(kept) => eprintln!("Plait workspace: {error} {kept}"),
                                    }
                                }
                                // Unsaved Linear issue text, as the close path writes it; also local.
                                if let Some((writer, revision, drafts)) = drafts
                                    && let Err(error) = writer.write(&journal, revision, &drafts)
                                {
                                    eprintln!("Plait issue drafts: {error}");
                                }
                                if let Some(tabs) = tabs && let Err(error) = tabs.write() { eprintln!("Plait tabs: {error}"); }
                            })
                        }).detach();
                        let selected = data.selected.clone();
                        this.saved_work.machines = journal.take_machines();
                        this.session.journal = Some(journal);
                        this.session.data = Some(data);
                        // A quit that could not be cancelled kept unsaved work aside.
                        this.session.note = recovered.first().and_then(|file| file.parent()).map(|folder| {
                            format!("Work that could not be saved when Plait last quit is kept in {}.", folder.display())
                        });
                        // Nothing was saved before the journal loaded: composers
                        // mounted earlier bind now, to what they load.
                        this.workspace.bindings.clear();
                        let composers: Vec<_> = this.workspace.composers.iter().map(|(id, c)| (id.clone(), c.clone())).collect();
                        for (id, composer) in composers {
                            this.restore_composer(&id, &composer, cx);
                        }
                        this.adopt_saved_sends();
                        // A folder chosen while disk state loaded wins over the saved folder.
                        if this.host.creation.generation == checkout_generation
                            && !this.host_workspace.checkout_carried
                        {
                            this.change_checkout(path, window, cx);
                            this.session.restore_thread = selected;
                        }
                        this.capture_session(cx);
                        this.verify_tab_identity(window, cx);
                        this.try_restore_tabs(window, cx);
                    }
                    Err(error) => this.session.note = Some(format!("Workspace recovery unavailable: {error} Sending is disabled to protect saved work.")),
                }
                this.sync_composer(cx);
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn close_desktop(
        &mut self,
        quit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.closing {
            return;
        }
        self.capture_session(cx);
        let Some(journal) = self.session.journal.clone() else {
            window.remove_window();
            if quit {
                cx.defer(super::quit_app);
            }
            return;
        };
        let tabs = self.flush_tab_preferences(Some(window), cx);
        // Unsaved Linear issue text, newest revision, without waiting for its pause.
        let drafts = self.draft_flush(cx);
        self.session.closing = true;
        self.sync_composer(cx);
        let task = cx.background_executor().spawn(async move {
            journal.flush()?;
            if let Some(tabs) = tabs {
                tabs.write()?;
            }
            if let Some((writer, revision, drafts)) = drafts {
                writer.write(&journal, revision, &drafts)?;
            }
            Ok::<_, String>(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    window.remove_window();
                    // A quit closes every window, each saving first; it ends
                    // only once none is left, and stops at one that cannot save.
                    if quit {
                        cx.defer(super::quit_app);
                    }
                }
                Err(error) => {
                    this.session.closing = false;
                    this.session.note = Some(format!(
                        "Window kept open because work could not be saved: {error}"
                    ));
                    this.sync_composer(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn change_checkout(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tabs.lifecycle.presenting
            && self.tabs.model.active().is_some()
            && self.host.creation.checkout != path
        {
            self.open_draft_in(path, window, cx);
            return;
        }
        if self.host.creation.creating || self.host.creation.pending_create.is_some() {
            self.notice =
                Some("Resolve the pending thread creation before changing checkout.".into());
            cx.notify();
            return;
        }
        self.clear_turn_review();
        // A draft belongs to its checkout, including while another project is open.
        let drafting = self.draft_visible();
        self.capture_session(cx);
        if self.host.creation.checkout != path {
            self.draft = super::new_thread::Draft {
                generation: self.draft.generation + 1,
                ..Default::default()
            };
        }
        self.update_lists(|lists| lists.open(&path), cx);
        let live = match self.host.change_checkout(path.clone()) {
            Ok(live) => live,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        self.host_workspace.checkout_carried = false;
        self.workspace.file_viewer.clear_checkout();
        self.workspace.line_pick = None;
        self.workspace.thread_files.reset();
        self.policy = Default::default();
        self.forget_agent();
        self.workspace.reset_for_checkout_change();
        self.session.restore_thread = None;
        self.navigation.managing = None;
        self.navigation.rename = None;
        self.navigation.menu = None;
        self.settings_open = false;
        self.new_thread_open = false;
        if drafting {
            self.open_draft(window, cx);
        }
        self.notice = None;
        self.state.open_checkout(path);
        self.rebuild_projects(cx);
        if !live {
            self.reconnect_host(window, cx);
        }
        self.pump(cx);
        self.capture_session(cx);
    }

    pub(super) fn restore_thread_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.persistence.requested && !self.tabs.persistence.restored {
            return;
        }
        if let Some(id) = self.session.restore_thread.take()
            && self.host.thread.selected.is_none()
            && let Some(summary) = self
                .host
                .connection
                .threads()
                .iter()
                .find(|thread| thread.id == id)
            && self.saved_thread(summary).is_some()
        {
            self.select_host_thread(id, window, cx);
        }
    }

    /// Whether thread `id` is listed and its sends in memory are for its folder.
    pub(super) fn session_accepts_thread(&self, id: &str) -> bool {
        self.host
            .connection
            .threads()
            .iter()
            .find(|summary| summary.id == id)
            .is_some_and(|summary| !self.ledger_conflict(summary))
    }

    /// Mount thread `id`'s composer: bind it, and fill it from the current
    /// machine's saved text, on every mount.
    pub(super) fn restore_composer(
        &mut self,
        id: &str,
        composer: &Entity<Composer>,
        cx: &mut Context<Self>,
    ) {
        if let Some(saved) = self.bind_thread_composer(id) {
            let draft = saved.draft;
            let client = self.host_client();
            composer.update(cx, |composer, cx| {
                composer.restore_draft(draft.text, draft.revision, draft.submitted, cx);
                composer.restore_thread_images_with_client(draft.images, id.to_owned(), client, cx)
            });
        }
    }

    pub(super) fn capture_session(&mut self, cx: &mut Context<Self>) {
        self.refresh_draft_title(cx);
        self.queue_tab_save(cx);
        if self.session.data.is_none() {
            return;
        }
        // Everything typed saves to the machine it is bound to, in memory now.
        self.bind_draft();
        // A composer mounted without a restore is restored, bound and filled
        // from its saved text, before it can save over that text.
        let unbound: Vec<_> = self
            .workspace
            .composers
            .iter()
            .filter(|(id, _)| !self.workspace.bindings.contains_key(*id))
            .map(|(id, composer)| (id.clone(), composer.clone()))
            .collect();
        for (id, composer) in unbound {
            self.restore_composer(&id, &composer, cx);
        }
        self.adopt_saved_sends();
        let mut works = BTreeMap::new();
        self.capture_open_draft(&mut works, cx);
        self.capture_parked_drafts(&mut works);
        self.capture_threads(&mut works, cx);
        let (mut changed, mut removed_image) = self.store_machine_work(works);
        // desktop.json keeps only the window's folder and selection; its older
        // records stay as they are for older builds.
        let data = self.session.data.as_ref().unwrap();
        let mut next = data.clone();
        if self.host_is_local()
            && !self.host_workspace.checkout_carried
            && self.host.creation.checkout.is_absolute()
        {
            next.checkout = Some(self.host.creation.checkout.clone());
        }
        next.selected = self
            .host
            .thread
            .selected
            .clone()
            .or_else(|| self.session.restore_thread.clone());
        if *data != next {
            removed_image |= data
                .image_ids()
                .any(|id| !next.image_ids().any(|next| next == id));
            if let Some(journal) = &self.session.journal {
                journal.queue(next.clone());
            }
            self.session.data = Some(next);
            changed = true;
        }
        if changed {
            self.schedule_session_save(removed_image, cx);
        }
    }

    /// The open draft, into its machine's work.
    fn capture_open_draft(&self, works: &mut BTreeMap<MachineId, MachineWork>, cx: &App) {
        let (Some(id), Some(binding)) = (&self.draft.id, &self.draft.binding) else {
            return;
        };
        let draft = if let Some(composer) = &self.draft.composer {
            let composer = composer.read(cx);
            let (text, revision, submitted) = composer.saved_draft();
            Draft {
                text: text.into(),
                revision,
                submitted: submitted.clone(),
                images: composer.saved_images().to_vec(),
                creation: self.draft.legacy_creation(),
            }
        } else if self.tabs.model.tabs().iter().any(|tab| matches!(tab.session().target(), plait_client::tabs::TabTarget::Draft { id: open, .. } if open == id)) {
            // An open empty tab still survives restart before its composer is mounted.
            self.draft.saved.clone()
        } else {
            return;
        };
        let tab = plait_client::session::DraftTab {
            legacy: false,
            creation: self.draft.saved_creation(),
            checkout: self.host.creation.checkout.clone(),
            settings: self.draft.settings(),
            draft,
        };
        super::saved_work::touch(works, &self.saved_work.machines, &binding.machine)
            .drafts
            .insert(id.clone(), tab);
    }

    /// Drafts parked behind other tabs, into their machines' work, so a parked
    /// draft dropped from memory later is still saved.
    fn capture_parked_drafts(&self, works: &mut BTreeMap<MachineId, MachineWork>) {
        for (id, draft) in &self.tabs.lifecycle.drafts {
            let Some(binding) = &draft.binding else {
                continue;
            };
            let saved =
                super::saved_work::touch(works, &self.saved_work.machines, &binding.machine);
            let checkout = saved
                .drafts
                .get(id)
                .map(|tab| tab.checkout.clone())
                .or_else(|| {
                    self.tabs
                        .model
                        .tabs()
                        .iter()
                        .find_map(|tab| match tab.session().target() {
                            plait_client::tabs::TabTarget::Draft { id: open, checkout }
                                if open == id =>
                            {
                                Some(checkout.root.clone())
                            }
                            _ => None,
                        })
                });
            let Some(checkout) = checkout else {
                continue;
            };
            let mut text = draft.saved.clone();
            text.creation = draft.legacy_creation();
            saved.drafts.insert(
                id.clone(),
                plait_client::session::DraftTab {
                    legacy: false,
                    creation: draft.saved_creation(),
                    checkout,
                    settings: draft.settings(),
                    draft: text,
                },
            );
        }
    }

    /// Listed threads' composer text, approval choice and saved sends, each into
    /// its bound machine's work: a composer's text where it mounted, sends where
    /// the ledger is.
    fn capture_threads(&mut self, works: &mut BTreeMap<MachineId, MachineWork>, cx: &App) {
        let ledger = self.ledger_machine();
        let accepted = self.accepting_hosts();
        let none = BTreeMap::new();
        // Only threads with something to save: a mounted composer, sends in
        // memory or saved, or an approval choice made here. Each check is a
        // lookup, and only those threads are copied, so a large catalog costs
        // little more than its threads with work.
        let summaries: Vec<_> = self
            .host
            .connection
            .threads()
            .iter()
            .filter(|summary| {
                let id = &summary.id;
                self.workspace.composers.contains_key(id)
                    || self.host.submissions.pending(id).is_some()
                    || !self.host.submissions.accepted(id).is_empty()
                    || self.saved_work.adopted.contains_key(id)
                    || self.workspace.thread_controls.approvals.contains_key(id)
            })
            .cloned()
            .collect();
        let mut written = Vec::new();
        for summary in &summaries {
            let id = &summary.id;
            let in_memory = self.host.submissions.pending(id).is_some()
                || !self.host.submissions.accepted(id).is_empty();
            let chosen = self.workspace.thread_controls.approvals.get(id).copied();
            // Sends in memory are for another folder's thread with this ID.
            if self.ledger_conflict(summary) {
                continue;
            }
            let approval = match chosen {
                Some(chosen) => chosen,
                None => self
                    .saved_thread(summary)
                    .and_then(|thread| thread.approval),
            };
            let title = self
                .saved_thread(summary)
                .map(|thread| thread.title.clone())
                .filter(|title| !title.is_empty())
                .or_else(|| {
                    self.host
                        .thread
                        .snapshot
                        .as_ref()
                        .filter(|snapshot| snapshot.id == *id)
                        .and_then(|snapshot| {
                            snapshot
                                .messages
                                .iter()
                                .find(|message| message.role == MessageRole::User)
                        })
                        .map(|message| session::title(&message.text))
                })
                .unwrap_or_default();
            let with_sends = |mut thread: Thread| {
                thread.pending = self.host.submissions.pending(id).cloned();
                thread.pending_submissions = self
                    .host
                    .submissions
                    .all(id)
                    .iter()
                    .skip(1)
                    .cloned()
                    .collect();
                thread.queued_submissions = self.host.submissions.accepted(id).to_vec();
                thread
            };
            let record = |work: &MachineWork| {
                work.thread(id, &summary.checkout)
                    .map(|saved| saved.thread.clone())
                    .unwrap_or_else(|| Thread {
                        checkout: summary.checkout.clone(),
                        model: summary.model.clone(),
                        title: title.clone(),
                        draft: Draft::default(),
                        pending: None,
                        pending_submissions: Vec::new(),
                        queued_submissions: Vec::new(),
                        approval: None,
                    })
            };
            let mut ledger_written = false;
            if let Some(composer) = self.workspace.composers.get(id)
                && let Some(binding) = self.workspace.bindings.get(id)
            {
                let composer = composer.read(cx);
                let (text, revision, submitted) = composer.saved_draft();
                let work =
                    super::saved_work::touch(works, &self.saved_work.machines, &binding.machine);
                let fresh = work.thread(id, &summary.checkout).is_none();
                let mut thread = record(work);
                thread.draft = Draft {
                    images: composer.saved_images().to_vec(),
                    text: text.to_owned(),
                    revision,
                    submitted: submitted.clone(),
                    creation: None,
                };
                thread.title = title.clone();
                thread.model = summary.model.clone();
                thread.approval = approval;
                let sends = binding.machine == ledger;
                if sends {
                    thread = with_sends(thread);
                    ledger_written = true;
                }
                // A record is made only for something worth keeping, which
                // includes the thread's own approval choice.
                let worth = !fresh
                    || !thread.draft.text.is_empty()
                    || !thread.draft.images.is_empty()
                    || thread.approval.is_some()
                    || (sends && in_memory);
                if worth {
                    work.put_thread(id, thread, if sends { &accepted } else { &none });
                }
            }
            if ledger_written {
                written.push((id.clone(), summary.checkout.clone()));
            } else if in_memory || approval.is_some() || self.saved_work.adopted.contains_key(id) {
                let work = super::saved_work::touch(works, &self.saved_work.machines, &ledger);
                let mut thread = with_sends(record(work));
                thread.approval = approval;
                work.put_thread(id, thread, &accepted);
                written.push((id.clone(), summary.checkout.clone()));
            }
        }
        // Sends saved for a folder are that folder's from now on.
        for (id, checkout) in written {
            self.saved_work.adopted.entry(id).or_insert(checkout);
        }
    }

    /// An explicit discard, or a sent draft: removed from its own machine only.
    pub(super) fn clear_new_draft(&mut self, cx: &mut Context<Self>) {
        self.bind_draft();
        let (Some(id), Some(binding)) = (self.draft.id.clone(), self.draft.binding.clone()) else {
            return;
        };
        let mut works = BTreeMap::new();
        super::saved_work::touch(&mut works, &self.saved_work.machines, &binding.machine)
            .drafts
            .remove(&id);
        self.store_machine_work(works);
        self.schedule_session_save(true, cx);
    }

    pub(super) fn schedule_session_save(&mut self, immediately: bool, cx: &mut Context<Self>) {
        let Some(journal) = self.session.journal.clone() else {
            return;
        };
        let timer = cx.background_executor().timer(if immediately {
            Duration::ZERO
        } else {
            Duration::from_millis(250)
        });
        self.session.save = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let result = cx
                .background_executor()
                .spawn(async move { journal.flush() })
                .await;
            let _ = this.update(cx, |this, cx| {
                // A failed write keeps the work in memory and queued for the next
                // save; the notice says so.
                this.session.note = result
                    .err()
                    .map(|error| format!("Workspace not saved: {error}"));
                cx.notify();
            });
        }));
    }

    /// The host's title, else the one saved from its first message.
    fn chosen_title(&self, summary: &ThreadSummary) -> Option<String> {
        summary
            .title
            .as_ref()
            .filter(|title| !title.is_empty())
            .cloned()
            .or_else(|| {
                self.saved_thread(summary)
                    .map(|thread| thread.title.clone())
                    .filter(|title| !title.is_empty())
            })
    }

    pub(super) fn summary_title(&self, summary: &ThreadSummary) -> String {
        self.chosen_title(summary)
            .unwrap_or_else(|| UNTITLED.to_owned())
    }

    pub(super) fn thread_title(&self, id: &str) -> String {
        self.thread_index(id)
            .map(|index| self.summary_title(&self.host.connection.threads()[index]))
            .unwrap_or_else(|| UNTITLED.to_owned())
    }

    /// A thread's name in a control's label. Untitled threads all read "New
    /// thread", so theirs adds the start of the ID to tell them apart.
    pub(super) fn thread_name(&self, id: &str) -> String {
        match self
            .thread_index(id)
            .map(|index| &self.host.connection.threads()[index])
            .and_then(|summary| self.chosen_title(summary))
        {
            Some(title) => title,
            None => format!("{UNTITLED} {}", id.chars().take(8).collect::<String>()),
        }
    }

    /// The name a rename starts from: empty for an untitled thread.
    pub(super) fn rename_text(&self, id: &str) -> String {
        self.host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .and_then(|summary| self.chosen_title(summary))
            .unwrap_or_default()
    }

    pub(super) fn record_snapshot(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.host.thread.snapshot.as_ref() else {
            return;
        };
        let id = snapshot.id.clone();
        if !self.session_accepts_thread(&id) {
            return;
        }
        let accepted = self.host.submissions.observe(snapshot);
        let accepted_any = !accepted.is_empty();
        for pending in accepted {
            if let Some(composer) = self.workspace.composers.get(&id) {
                composer.update(cx, |composer, cx| {
                    composer.acknowledge(&pending.text, pending.draft_revision, cx);
                });
            }
        }
        let title_missing = self
            .session
            .data
            .as_ref()
            .and_then(|data| data.threads.get(&id))
            .is_none_or(|thread| thread.title.is_empty());
        let pending_changed = self
            .session
            .data
            .as_ref()
            .and_then(|data| data.threads.get(&id))
            .is_some_and(|thread| {
                thread.pending.as_ref() != self.host.submissions.pending(&id)
                    || thread.pending_submissions.as_slice()
                        != self.host.submissions.all(&id).get(1..).unwrap_or_default()
                    || thread.queued_submissions.as_slice() != self.host.submissions.accepted(&id)
            });
        if title_missing || pending_changed || accepted_any {
            self.capture_session(cx);
        }
    }
}
