use super::{
    Plait,
    composer::{Composer, ComposerEvent},
};
use gpui::{prelude::*, *};
use plait_client::{
    host::{self, Request, Snapshot},
    host_state::request_id,
    transcript::{self, Update},
};
use serde::de::DeserializeOwned;

/// What a creation request was sent with, to check its reply against.
pub(super) struct Created {
    pub(super) draft_id: Option<String>,
    pub(super) request_id: String,
    pub(super) choice: host::ModelChoice,
    pub(super) harness: host::harness::HarnessId,
    /// The selection when it was sent; a direct creation opens only if it is unchanged.
    pub(super) selection_epoch: u64,
    pub(super) checkout: Option<std::path::PathBuf>,
}

/// Asks the host whether the expected checkout is still the same folder. The
/// host accepts a volume whose device number changed after a restart, and the
/// key comes back refreshed so later requests carry the current identity.
pub(super) fn verified_checkout(
    client: &host::Client,
    expected: Option<&plait_client::ownership::CheckoutKey>,
    supported: bool,
) -> Result<Option<plait_client::ownership::CheckoutKey>, host::Error> {
    let Some(expected) = expected else {
        return Ok(None);
    };
    if !supported {
        return Err(host::Error::Protocol(
            "Update the host to verify the checkout.".into(),
        ));
    }
    let reply = host::git::identity(
        client,
        &expected.root,
        Some(plait_protocol::lease::DirIdentity {
            dev: expected.device,
            ino: expected.inode,
        }),
    )?;
    Ok(
        (reply.identity.dev != expected.device).then(|| plait_client::ownership::CheckoutKey {
            device: reply.identity.dev,
            ..expected.clone()
        }),
    )
}

pub(super) fn is_checkout_identity_changed(error: &host::Error) -> bool {
    matches!(error, host::Error::Rejected(reason)
        if reason.code == "checkout_identity_changed"
            || (reason.code == "invalid_request"
                && reason.message == "This folder changed since it was opened."))
}

pub(super) struct WatchLease {
    pub(super) cancel: Option<host::CancelWatch>,
    pub(super) executor: BackgroundExecutor,
}
impl Drop for WatchLease {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            self.executor
                .spawn(async move {
                    drop(cancel);
                })
                .detach();
        }
    }
}

/// Where a send went: the machine, its verified host and the thread's folder.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Dispatch {
    pub machine: plait_client::machines::MachineId,
    pub host: Option<plait_client::tabs::HostKey>,
    pub checkout: Option<std::path::PathBuf>,
}

impl Plait {
    pub(super) fn host_request<T: DeserializeOwned + Send + 'static>(
        &mut self,
        mut request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
        complete: impl FnOnce(&mut Self, Result<T, host::Error>, &mut Window, &mut Context<Self>)
        + 'static,
    ) {
        let supported = self.host.connection.has_checkout_identity;
        if let Request::ListThreads { checkout, identity }
        | Request::CreateThread {
            checkout, identity, ..
        } = &mut request
        {
            *identity = self.host.connection.checkout_identity(
                self.state
                    .checkout_key()
                    .filter(|key| key.root == *checkout),
            );
        }
        let epoch = self.host.connection.epoch;
        let identity = self.host.identity.clone();
        let acknowledgement = matches!(
            request,
            Request::Submit { .. } | Request::SubmitCheckout { .. } | Request::CreateThread { .. }
        );
        let expected = match &request {
            Request::Submit { thread_id, .. } | Request::SubmitCheckout { thread_id, .. } => {
                self.host.expected_checkout(thread_id).cloned()
            }
            _ => None,
        };
        if let Request::Submit { identity, .. } | Request::SubmitCheckout { identity, .. } =
            &mut request
        {
            *identity = self.host.connection.checkout_identity(expected.as_ref());
        }
        let persist = matches!(
            request,
            Request::Submit { .. } | Request::SubmitCheckout { .. }
        ) && self.session.required;
        if persist {
            self.capture_session(cx);
        }
        let uploads = if let Request::Submit {
            thread_id,
            request_id,
            ..
        }
        | Request::SubmitCheckout {
            thread_id,
            request_id,
            ..
        } = &request
        {
            self.host
                .submissions
                .get(thread_id, request_id)
                .map(|pending| (thread_id.clone(), pending.images.clone()))
        } else {
            None
        };
        // A watch can acknowledge an earlier attempt while this retry is still uploading.
        let image_pins = uploads
            .as_ref()
            .filter(|(_, images)| !images.is_empty())
            .map(|(_, images)| plait_client::images::pin(images))
            .transpose();
        let journal = self.session.journal.clone();
        let client = self.host_client();
        let is_status =
            self.host.connection.connecting && matches!(request, Request::Status { .. });
        let task = cx.background_executor().spawn(async move {
            let capability = if is_status {
                Some(
                    host::release::host_info(&client)
                        .ok()
                        .flatten()
                        .is_some_and(|info| {
                            info.capabilities
                                .iter()
                                .any(|c| c == "checkout_identity_v1")
                        }),
                )
            } else {
                None
            };
            let image_pins = image_pins.map_err(host::Error::Protocol);
            let result = image_pins.and_then(|image_pins| {
                let result = (|| {
                    if persist {
                        let journal = journal.ok_or_else(|| {
                            std::io::Error::other(
                                "Desktop recovery is unavailable. Submission was not sent.",
                            )
                        })?;
                        journal.flush().map_err(std::io::Error::other)?;
                    }
                    if !supported
                        && (expected.is_some()
                            || matches!(
                                request,
                                Request::ListThreads { .. } | Request::CreateThread { .. }
                            ))
                    {
                        return Err(host::Error::Protocol(
                            "Update the host to verify the checkout.".into(),
                        ));
                    }
                    if let Some((thread, images)) = uploads {
                        client.upload_images(&thread, &images)?;
                    }
                    client.request(&request)
                })();
                let collect = image_pins.is_some();
                drop(image_pins);
                if collect && let Err(error) = plait_client::images::collect() {
                    eprintln!("Plait draft images: {error}");
                }
                result
            });
            (result, capability)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (result, capability) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.host.identity != identity
                    || (!acknowledgement && this.host.connection.epoch != epoch)
                {
                    return;
                }
                if let Some(capability) = capability {
                    this.host.connection.has_checkout_identity = capability;
                }
                complete(this, result, window, cx);
                this.sync_composer(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn reconnect_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.connection.connecting {
            return;
        }
        // A remote machine reconnects over a new SSH link; this runs again
        // once that link is made.
        if self.relink_before_reconnect(window, cx) {
            return;
        }
        self.workspace.reset_for_reconnect();
        self.navigation.watch = None;
        self.navigation.generation += 1;
        self.navigation.loading = false;
        if self.navigation.managing.take().is_some() {
            self.navigation.error =
                Some("Thread change not confirmed. Refresh to check before trying again.".into());
        }
        self.tabs.identity.pending = None;
        // A reconnected host may hold other threads; old alerts no longer apply.
        self.clear_toasts();
        self.tabs.model.disconnected(&self.tabs.host);
        self.refresh_tab_rows();
        for draft in std::iter::once(&mut self.draft).chain(self.tabs.lifecycle.drafts.values_mut())
        {
            if let Some(pending) = &mut draft.pending {
                pending.creating = false;
            }
        }
        self.host.reconnect();
        self.sync_composer(cx);
        self.host_request(
            Request::Status {
                protocol_version: Some(host::PROTOCOL_VERSION),
            },
            window,
            cx,
            |this, result, window, cx| match result {
                Ok(status) => {
                    if this.host.apply_status(this.host.connection.epoch, status) {
                        this.verify_tab_identity(window, cx);
                        if !this.tabs.identity.verified {
                            let result = this
                                .tabs
                                .model
                                .reconnected(&this.tabs.host, this.host.connection.epoch);
                            this.tab_error(result);
                        }
                        this.load_threads(window, cx);
                        if !this.tabs.identity.verified {
                            this.refresh_turn_review(cx);
                        }
                        if !this.tabs.identity.verified
                            && let Some(id) = this.host.thread.selected.clone()
                        {
                            this.watch_thread(id, window, cx);
                        }
                    }
                }
                Err(error) => {
                    this.host.connection.connecting = false;
                    this.host.connection.error = Some(error.to_string());
                    // A fresh install has no server yet; set one up once.
                    if matches!(error, host::Error::Transport(_)) {
                        this.offer_server_setup(window, cx);
                    }
                }
            },
        );
        cx.notify();
    }

    pub(super) fn load_threads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.host.connection.connected {
            return;
        }
        if self.host.protocol() >= 3 {
            self.watch_all_threads(window, cx);
            return;
        }
        let checkout = self.host.creation.checkout.clone();
        self.host_request::<host::ThreadsReply>(
            Request::ListThreads {
                checkout,
                identity: None,
            },
            window,
            cx,
            |this, result, window, cx| match result {
                Ok(reply) => {
                    this.host.connection.set_threads(reply.threads);
                    this.rebuild_projects(cx);
                    this.restore_thread_selection(window, cx);
                }
                Err(error) => this.host.connection.error = Some(error.to_string()),
            },
        );
    }

    /// Open a thread that has just been created, with the keyboard in its
    /// composer. A settings modal stays open over it and keeps the keyboard,
    /// since the answer can arrive while the person is typing there; closing
    /// the modal lands in the new composer.
    pub(super) fn open_created_thread(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let covered = self.settings_open;
        let bound = self
            .tabs
            .model
            .active()
            .and_then(|tab| self.tabs.model.get(tab))
            .is_some_and(|tab| {
                matches!(tab.session().target(),
                plait_client::tabs::TabTarget::Existing { id: target, .. } if target == &id)
            });
        if bound {
            // The tab in front already shows this thread, so its checkout change
            // presents that tab. Otherwise a worktree thread would open a stray
            // draft and stay bound to the project's main checkout (#153).
            let presenting = std::mem::replace(&mut self.tabs.lifecycle.presenting, true);
            self.select_host_thread_content(id.clone(), window, cx);
            self.tabs.lifecycle.presenting = presenting;
        } else {
            self.select_host_thread(id.clone(), window, cx);
        }
        if !covered {
            return self.focus_composer(window, cx);
        }
        self.settings_open = true;
        if let Some(composer) = self.workspace.composers.get(&id)
            && self.host.thread.selected.as_ref() == Some(&id)
        {
            self.settings_restore = Some(composer.focus_handle(cx));
        }
        window.focus(&self.settings_focus, cx);
    }

    pub(super) fn select_host_thread(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(checkout) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.checkout.clone())
        else {
            return;
        };
        // Selecting the thread already open changes no tab, so nothing else closes it.
        self.close_overview(window, cx);
        self.open_tab_target(
            plait_client::tabs::TabTarget::Existing {
                id,
                checkout: self.tab_checkout(checkout),
            },
            window,
            cx,
        );
    }

    pub(super) fn select_host_thread_content(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(checkout) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .map(|thread| thread.checkout.clone())
        else {
            return;
        };
        if checkout != self.host.creation.checkout {
            if self.host.creation.creating || self.host.creation.pending_create.is_some() {
                self.notice =
                    Some("Resolve the pending thread creation before changing checkout.".into());
                cx.notify();
                return;
            }
            self.change_checkout(checkout, window, cx);
        }
        self.navigation.menu = None;
        self.navigation.rename = None;
        window.focus(&self.focus, cx);
        self.workspace.reset_for_thread_selection();
        self.host.select(Some(id.clone()));
        self.state.repository.invalidate_view();
        self.workspace.thread_files.reset();
        self.workspace.file_viewer.retire_read();
        self.workspace.line_pick = None;
        self.bind_selected_checkout();
        self.settings_open = false;
        self.new_thread_open = false;
        // Leaving a new-thread draft keeps its text for the next New Thread.
        self.draft.open = false;
        self.refresh_live_list(None);
        self.conversation_rows_changed(cx);
        if !self.workspace.composers.contains_key(&id) {
            let composer = cx.new(|cx| Composer::new(window, cx));
            self.restore_composer(&id, &composer, cx);
            self.attach_composer(id.clone(), composer, window, cx);
            self.reconcile_checkout_waits(window, cx);
        }
        self.capture_session(cx);
        self.sync_composer(cx);
        if self.host.connection.connected && self.tab_identity_current() {
            self.watch_thread(id, window, cx);
        }
        cx.notify();
    }

    /// Make `composer` thread `id`'s: its edits are saved, and its events act
    /// on the thread while it is open.
    pub(super) fn attach_composer(
        &mut self,
        id: String,
        composer: Entity<Composer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Every mount binds the composer, if a restore has not already.
        self.bind_thread_composer(&id);
        let mut last = composer.read(cx).box_state();
        cx.observe(&composer, move |this, composer, cx| {
            this.capture_session(cx);
            let next = composer.read(cx).box_state();
            if next != last {
                last = next;
                cx.notify();
            }
        })
        .detach();
        let focus = composer.read(cx).focus_handle(cx);
        cx.on_focus_in(&focus, window, |_, _, cx| cx.notify())
            .detach();
        cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify())
            .detach();
        let thread_id = id.clone();
        cx.subscribe_in(&composer, window, move |this, _, event, window, cx| {
            if this.host.thread.selected.as_ref() != Some(&thread_id) {
                return;
            }
            match event {
                ComposerEvent::Submit {
                    text,
                    revision,
                    steer,
                } => this.submit_message(text.clone(), *revision, *steer, window, cx),
                ComposerEvent::Stop => this.control_thread(None, window, cx),
                other => this.composer_event(other, window, cx),
            }
        })
        .detach();
        self.workspace.composers.insert(id, composer);
    }

    pub(super) fn watch_thread(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tab_identity_current() || self.workspace.tab != super::Workspace::Conversation {
            return;
        }
        self.workspace.host_watch = None;
        self.host.thread.watch_epoch += 1;
        let epoch = self.host.connection.epoch;
        let watch_epoch = self.host.thread.watch_epoch;
        let watch_id = id.clone();
        let protocol = self.host.protocol();
        let executor = cx.background_executor().clone();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            let snapshots = client.subscribe(watch_id, protocol)?;
            let lease = WatchLease {
                cancel: Some(snapshots.cancel_handle()),
                executor,
            };
            Ok::<_, host::Error>((snapshots, lease))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (mut snapshots, lease) = match task.await {
                Ok(subscription) => subscription,
                Err(error) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.watch_failed(epoch, watch_epoch, &id, error, cx)
                    });
                    return;
                }
            };
            let registered = this
                .update_in(cx, |this, _, _| {
                    if !this.host.accepts_watch(epoch, watch_epoch, &id) {
                        return false;
                    }
                    this.workspace.host_watch = Some(lease);
                    true
                })
                .unwrap_or(false);
            if !registered {
                cx.background_executor()
                    .spawn(async move {
                        drop(snapshots);
                    })
                    .detach();
                return;
            }
            while let Some(result) = snapshots.next().await {
                let keep = this
                    .update_in(cx, |this, window, cx| {
                        if !this.host.accepts_watch(epoch, watch_epoch, &id) {
                            return false;
                        }
                        match result {
                            Ok(snapshot) => {
                                if snapshot.id != id {
                                    this.watch_failed(
                                        epoch,
                                        watch_epoch,
                                        &id,
                                        host::Error::Protocol(
                                            "Host returned the wrong thread.".into(),
                                        ),
                                        cx,
                                    );
                                    return false;
                                }
                                let previous = this.host.thread.snapshot.clone();
                                if !this.host.apply_snapshot(epoch, watch_epoch, snapshot) {
                                    this.workspace.host_watch = None;
                                    this.sync_composer(cx);
                                    cx.notify();
                                    return false;
                                }
                                if let Some(live) = this.host.thread.snapshot.as_deref() {
                                    this.tabs.model.observe_active_snapshot(live);
                                }
                                let review_changed = previous.as_ref().is_none_or(|old| {
                                    this.host.thread.snapshot.as_ref().is_some_and(|new| {
                                        old.status != new.status
                                            || old.records_revision != new.records_revision
                                    })
                                });
                                this.turn_records_snapshot(window, cx);
                                if review_changed {
                                    this.refresh_cleanup(cx);
                                    this.refresh_turn_review(cx);
                                }
                                this.conversation_changed(window, cx);
                                this.record_snapshot(cx);
                                this.thread_files_snapshot(cx);
                                if previous
                                    .as_ref()
                                    .is_some_and(|snapshot| snapshot.status.busy())
                                    && this
                                        .host
                                        .thread
                                        .snapshot
                                        .as_ref()
                                        .is_some_and(|snapshot| !snapshot.status.busy())
                                {
                                    this.state.repository.refresh();
                                    this.pump(cx);
                                }
                                this.sync_composer(cx);
                                cx.notify();
                                true
                            }
                            Err(error) => {
                                this.watch_failed(epoch, watch_epoch, &id, error, cx);
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
            cx.background_executor()
                .spawn(async move {
                    drop(snapshots);
                })
                .detach();
        })
        .detach();
    }

    fn watch_failed(
        &mut self,
        epoch: u64,
        watch_epoch: u64,
        id: &str,
        error: host::Error,
        cx: &mut Context<Self>,
    ) {
        if !self.host.accepts_watch(epoch, watch_epoch, id) {
            return;
        }
        self.workspace.host_watch = None;
        self.host.connection.connected = false;
        self.tabs.model.disconnected(&self.tabs.host);
        self.refresh_tab_rows();
        self.host.connection.error = Some(error.to_string());
        self.refresh_turn_review(cx);
        self.sync_composer(cx);
        cx.notify();
    }

    fn build_live_layout(&self, snapshot: &Snapshot) -> plait_client::folding::Layout {
        let live_end = !self.workspace.history.cache.detached;
        let find_match_message = self
            .workspace
            .conversation_find
            .open
            .then(|| self.current_hit().map(|hit| hit.message))
            .flatten();
        let unknown = Default::default();
        let outcomes = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .map_or(&unknown, |tab| tab.session().outcomes());
        plait_client::folding::build_layout(
            snapshot,
            live_end,
            &self.workspace.turn_records.data,
            &self.workspace.opened_turns,
            find_match_message.as_deref(),
            self.preferences.value.fold_earlier_turns(),
            outcomes,
        )
    }

    /// Re-apply folding rules to the snapshot already on screen without
    /// rebuilding unrelated row measurements.
    pub(super) fn reconcile_live_folding(&mut self) {
        let Some(snapshot) = self.workspace.history.display.clone() else {
            return;
        };
        let next_layout = self.build_live_layout(&snapshot);
        if next_layout.rows == self.workspace.live_layout.rows {
            self.workspace.live_layout = next_layout;
            return;
        }
        match plait_client::folding::diff_layout(
            Some((
                snapshot.as_ref(),
                self.workspace.live_layout.rows.as_slice(),
            )),
            Some((snapshot.as_ref(), next_layout.rows.as_slice())),
        ) {
            Update::Reset(count) => self.workspace.live_scroll.reset(count),
            Update::Patch { splices, remeasure } => {
                for splice in splices {
                    self.workspace
                        .live_scroll
                        .splice(splice.range, splice.count);
                }
                for range in remeasure {
                    self.workspace.live_scroll.remeasure_items(range);
                }
            }
        }
        self.workspace.live_layout = next_layout;
        self.workspace.live_count = self.workspace.live_layout.len();
        debug_assert_eq!(
            self.workspace.live_scroll.item_count(),
            self.workspace.live_count
        );
    }

    /// Open a folded earlier turn for the lifetime of this tab while keeping
    /// the activated row where it sits on screen.
    pub(super) fn open_folded_turn(&mut self, turn_id: &str, cx: &mut Context<Self>) {
        self.workspace.opened_turns.insert(turn_id.to_owned());
        if let Some(tab_id) = self.tabs.model.active() {
            self.tabs.model.open_turn(tab_id, turn_id.to_owned());
        }
        let folded_ix = self.workspace.live_layout.folded_row_index(turn_id);
        // A bottom-aligned list pinned to its end would push an opened earlier
        // turn upward; pin the current viewport top first so the row stays put.
        if self.workspace.live_scroll.logical_scroll_top().item_ix >= self.workspace.live_count {
            let scroll = &self.workspace.live_scroll;
            // Resolve the end sentinel against the measured height, including
            // the blank space above a short, bottom-aligned transcript.
            scroll.scroll_to_end();
            let height = -scroll.scroll_px_offset_for_scrollbar().y;
            let top = height + px(super::live_conversation::LIST_TOP_PADDING)
                - scroll.viewport_bounds().size.height;
            scroll.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: top.min(px(0.)),
            });
            if top > px(0.) {
                scroll.scroll_by(top);
            }
        }
        let anchor = self.workspace.live_scroll.logical_scroll_top();
        self.reconcile_live_folding();
        if folded_ix == Some(anchor.item_ix) {
            self.workspace.live_scroll.scroll_to(anchor);
        }
        cx.notify();
    }

    pub(super) fn refresh_live_list(&mut self, previous: Option<&Snapshot>) {
        self.workspace.history.display = self.host.thread.snapshot.clone();
        self.reconcile_live_list(previous);
    }

    /// Bring the list's rows in line with the snapshot now shown. Returns
    /// whether rows were added or removed while the others kept their
    /// measured heights.
    pub(super) fn reconcile_live_list(&mut self, previous: Option<&Snapshot>) -> bool {
        let next = self.workspace.history.display.as_deref();
        let next_layout = next
            .map(|snapshot| self.build_live_layout(snapshot))
            .unwrap_or_default();
        let mut spliced = false;
        match plait_client::folding::diff_layout(
            previous.map(|snap| (snap, self.workspace.live_layout.rows.as_slice())),
            next.map(|snap| (snap, next_layout.rows.as_slice())),
        ) {
            Update::Reset(count) => {
                self.workspace.live_scroll.reset(count);
                self.workspace.expanded_live_tools.clear();
                self.workspace.history.expanded.clear();
            }
            Update::Patch { splices, remeasure } => {
                spliced = !splices.is_empty();
                for splice in splices {
                    self.workspace
                        .live_scroll
                        .splice(splice.range, splice.count);
                }
                for range in remeasure {
                    self.workspace.live_scroll.remeasure_items(range);
                }
                self.workspace.history.expanded.retain(|id| {
                    next.is_some_and(|snapshot| {
                        snapshot.messages.iter().any(|message| message.id == *id)
                    })
                });
                self.workspace.expanded_live_tools.retain(|id| {
                    next.is_some_and(|snapshot| snapshot.tools.iter().any(|tool| tool.id == *id))
                });
            }
        }
        // Cached once per snapshot or fold change; rendering resolves visible rows by position.
        self.workspace.live_layout = next_layout;
        self.workspace.live_count = self.workspace.live_layout.len();
        debug_assert_eq!(
            self.workspace.live_scroll.item_count(),
            self.workspace.live_count
        );
        spliced
    }

    /// Measure again the rows that took or gave up their turn's record. The
    /// row diff compares text, so it misses a row whose record moved: when a
    /// new request arrives, the answer before it draws the record that the
    /// state line drew until then. Only a change in the rows can move one.
    pub(super) fn remeasure_records(
        &mut self,
        previous: Option<&Snapshot>,
        before: &plait_client::markdown::Transcript,
    ) {
        let (Some(previous), Some(next)) = (previous, self.workspace.history.display.as_deref())
        else {
            return;
        };
        let moved = plait_client::history::moved_records(
            (previous, &before.turn_summaries),
            (next, &self.workspace.formatted.turn_summaries),
        );
        for message in moved {
            let slot = transcript::Slot::Message(message);
            if let Some(row) = self.workspace.live_layout.rows.iter().position(|shown| {
                matches!(
                    shown,
                    plait_client::folding::DisplayRow::Slot(shown) if *shown == slot
                ) || matches!(
                    shown,
                    plait_client::folding::DisplayRow::FoldedTurn {
                        closing_slot: Some(shown),
                        ..
                    } if *shown == slot
                )
            }) {
                self.workspace.live_scroll.remeasure_items(row..row + 1);
            }
        }
    }

    pub(super) fn create_host_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_host_thread_at(None, window, cx);
    }

    /// Create a thread that starts at `thinking`, or at Settings' default when
    /// `None`. Retrying an unconfirmed creation repeats the level it was made with.
    pub(super) fn create_host_thread_at(
        &mut self,
        thinking: Option<host::ThinkingLevel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.host.connection.connected || self.host.creation.creating {
            return;
        }
        if self.host.creation.pending_create.is_none() {
            let Some(choice) = self.host.creation.new_model.clone() else {
                return;
            };
            let harness = self.new_thread_harness(&choice);
            if !self.usable_in(harness, &choice) {
                return;
            }
            let mode = self.new_thread_mode(&choice);
            self.host.creation.pending_create = Some((request_id(), choice));
            self.host.creation.pending_thinking =
                Some(thinking.unwrap_or_else(|| self.preferences.value.default_thinking()));
            self.set_pending_harness(harness, mode);
        }
        let (id, choice) = self.host.creation.pending_create.clone().unwrap();
        let thinking = self.host.creation.pending_thinking;
        let harness = self.pending_harness();
        let protocol = self.host.protocol();
        self.host.creation.creating = true;
        self.host.connection.error = None;
        let selection_epoch = self.host.thread.watch_epoch;
        let checkout = self.state.checkout_key().map(|key| key.root.clone());
        let request = Request::create_thread(
            self.host.creation.checkout.clone(),
            &choice,
            id.clone(),
            thinking,
            harness,
            protocol,
        )
        .with_mode(self.pending_mode());
        self.create_with_admission(
            Created {
                draft_id: None,
                request_id: id,
                choice,
                harness,
                selection_epoch,
                checkout,
            },
            request,
            window,
            cx,
        );
        self.sync_composer(cx);
        cx.notify();
    }

    /// The host answered a thread creation. A draft's reply delivers its first
    /// message; any other opens the thread if the person is still waiting for it.
    pub(super) fn finish_create(
        &mut self,
        created: Created,
        result: Result<host::ThreadReply, host::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Created {
            draft_id,
            request_id,
            choice,
            harness,
            selection_epoch,
            checkout,
        } = created;
        // Only the creation still pending; the host answers each request once.
        if self.host.creation.pending_create.as_ref().map(|(id, _)| id) != Some(&request_id) {
            return;
        }
        self.host.creation.creating = false;
        let drafted = self.draft_sent(&request_id);
        match result {
            Ok(reply)
                if checkout.as_ref().is_none_or(|checkout| reply.thread.checkout == *checkout) && reply.thread.model == choice
                    && super::harnesses::harness_of(&reply.thread) == harness =>
            {
                self.host.creation.pending_create = None;
                let id = reply.thread.id.clone();
                if !self.host.connection.threads().iter().any(|thread| thread.id == id) {
                    self.host
                        .connection.threads_mut()
                        .insert(0, host::ThreadSummary::from(&reply.thread));
                    self.rebuild_projects(cx);
                }
                self.link_created_thread(draft_id.as_deref(), &id, cx);
                if self.new_thread_open && self.host.thread.watch_epoch == selection_epoch {
                    self.open_created_thread(id, window, cx);
                }
                self.record_recent_model(&choice, cx);
                self.load_threads(window, cx);
            }
            Ok(_) => {
                self.host.connection.error = Some("Host returned a different checkout, model, or harness. Creation is unconfirmed; reconnect before retrying.".into())
            }
            Err(error) => {
                if matches!(error, host::Error::Rejected(_)) {
                    self.host.creation.pending_create = None;
                    let message = if is_checkout_identity_changed(&error) {
                        let path = checkout.clone().unwrap_or_else(|| self.host.creation.checkout.clone());
                        self.tabs.identity.recovery = Some((path.clone(), false));
                        Self::identity_message(&path)
                    } else { error.to_string() };
                    if drafted {
                        self.draft_rejected(message);
                    } else {
                        self.host.connection.error = Some(message);
                    }
                } else {
                    self.host.connection.error = Some(format!(
                        "{error} Retry creation to reuse the same request ID."
                    ));
                }
            }
        }
        self.sync_composer(cx);
    }

    fn submit_message(
        &mut self,
        text: String,
        revision: u64,
        steer: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.host.thread.selected.clone() else {
            return;
        };
        if !self.can_submit() {
            return;
        }
        let mode = (self.host.protocol() >= 3).then_some(if steer {
            host::QueueMode::Steer
        } else {
            host::QueueMode::FollowUp
        });
        let images = self
            .workspace
            .composers
            .get(&id)
            .map(|c| c.read(cx).saved_images().to_vec())
            .unwrap_or_default();
        match self.host.submissions.begin_images(
            &id,
            text,
            self.next_approval_mode(),
            mode,
            Some(revision),
            images,
        ) {
            Ok(pending) => self.retry_submission_id(&id, &pending.request_id, window, cx),
            Err(error) => {
                self.host.connection.error = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn retry_submission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.host.thread.selected.clone() else {
            return;
        };
        let Some(pending) = self.host.submissions.pending(&id).cloned() else {
            return;
        };
        self.retry_submission_id(&id, &pending.request_id, window, cx);
    }

    pub(super) fn retry_submission_id(
        &mut self,
        id: &str,
        request_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.host.connection.connected
            || !self.session.ready()
            || !self.session_accepts_thread(id)
            || self.verified_host().is_none()
        {
            return;
        }
        let Some(pending) = self.host.submissions.get(id, request_id).cloned() else {
            return;
        };
        if (pending.checkout_wait && !self.admission.queue)
            || (!pending.images.is_empty() && self.host.protocol() < 6)
            || (pending.when_busy.is_some() && self.host.protocol() < 3)
            || (pending.approval.is_some() && self.host.protocol() < 2)
        {
            self.host.connection.error =
                Some("This message needs a newer host. Update before retrying.".into());
            return;
        }
        if !self.host.submissions.start_send(id, request_id) {
            return;
        }
        let id = id.to_owned();
        let composer = self.workspace.composers.get(&id).map(Entity::downgrade);
        let dispatch = self.dispatch(&id);
        self.host.thread.submitting = true;
        self.host.connection.error = None;
        self.sync_composer(cx);
        self.host_request::<host::SubmitReply>(
            pending.request(&id),
            window,
            cx,
            move |this, result, _, cx| {
                match result {
                    Ok(reply)
                        if reply.accepted
                            && reply.thread_id == id
                            && reply.request_id == pending.request_id =>
                    {
                        if this.admission.refused_busy.as_ref() == Some(&id) {
                            this.admission.refused_busy = None;
                        }
                        this.submission_accepted(&id, &pending, &dispatch, composer, cx);
                    }
                    Ok(_) => this.host.submissions.uncertain(
                        &pending.request_id,
                        "Host acceptance was not confirmed. Retry uses the same request ID.".into(),
                    ),
                    Err(error) => {
                        if matches!(error, host::Error::Rejected(_)) {
                            if matches!(&error, host::Error::Rejected(reason) if reason.code == "busy") { this.admission.refused_busy = Some(id.clone()); }
                            this.host.submissions.finish(&id, &pending.request_id);
                            let message = if is_checkout_identity_changed(&error) {
                                let path = this.host.connection.threads().iter().find(|thread| thread.id == id)
                                    .map(|thread| thread.checkout.clone())
                                    .unwrap_or_else(|| this.host.creation.checkout.clone());
                                this.tabs.identity.recovery = Some((path.clone(), false));
                                Self::identity_message(&path)
                            } else { error.to_string() };
                            this.set_thread_notice(id.clone(), message, true);
                            this.refresh_cleanup(cx);
                        } else {
                            this.host
                                .submissions
                                .uncertain(&pending.request_id, error.to_string());
                        }
                    }
                }
                this.host.thread.submitting =
                    this.host.thread.selected.as_ref().is_some_and(|selected| {
                        this.host
                            .submissions
                            .all(selected)
                            .iter()
                            .any(|p| this.host.submissions.is_sending(&p.request_id))
                    });
                this.capture_session(cx);
            },
        );
        cx.notify();
    }

    /// The machine, verified host and folder a send to thread `id` goes to.
    pub(super) fn dispatch(&self, id: &str) -> Dispatch {
        Dispatch {
            machine: self.machines.active.clone(),
            host: self.verified_host().cloned(),
            checkout: self
                .host
                .connection
                .threads()
                .iter()
                .find(|thread| thread.id == id)
                .map(|thread| thread.checkout.clone()),
        }
    }

    /// A host accepted request `pending` for thread `id`. Saved work changes
    /// only while the machine, verified host and folder it was sent from still
    /// hold, and this ledger holds the request: an acknowledgement from
    /// another machine's send clears nothing here (#132).
    pub(super) fn submission_accepted(
        &mut self,
        id: &str,
        pending: &plait_client::host_state::PendingSubmit,
        dispatch: &Dispatch,
        composer: Option<WeakEntity<Composer>>,
        cx: &mut Context<Self>,
    ) {
        if self.dispatch(id) != *dispatch
            || self.host.submissions.get(id, &pending.request_id).is_none()
        {
            return;
        }
        self.host.submissions.confirm(id, &pending.request_id);
        if let Some(host) = dispatch.host.clone() {
            self.saved_work
                .accepted_by
                .entry(pending.request_id.clone())
                .or_insert(host);
        }
        self.acknowledge_saved_draft(id, &pending.text, pending.draft_revision, cx);
        if let Some(composer) = composer {
            let _ = composer.update(cx, |composer, cx| {
                composer.acknowledge(&pending.text, pending.draft_revision, cx);
            });
        }
    }

    pub(super) fn control_thread(
        &mut self,
        approval: Option<(String, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.host.connection.connected || self.host.thread.controlling {
            return;
        }
        let Some(snapshot) = self.host.thread.snapshot.as_ref() else {
            return;
        };
        let id = snapshot.id.clone();
        let harness = super::harnesses::harness_of(snapshot);
        let request = if let Some((approval_id, allow)) = approval {
            if self.host.thread.decided_approvals.contains(&approval_id)
                || !snapshot.pending_approval.as_ref().is_some_and(|pending| {
                    pending.id == approval_id
                        && super::live_conversation::approvable(pending, harness)
                })
            {
                return;
            }
            self.host
                .thread
                .decided_approvals
                .insert(approval_id.clone());
            Request::Approve {
                thread_id: id.clone(),
                approval_id,
                allow,
            }
        } else {
            if !snapshot.status.busy() {
                return;
            }
            Request::Stop {
                thread_id: id.clone(),
            }
        };
        self.host.thread.controlling = true;
        let watch_epoch = self.host.thread.watch_epoch;
        self.host_request::<host::ThreadReply>(request, window, cx, move |this, result, _, _| {
            if !this
                .host
                .accepts_watch(this.host.connection.epoch, watch_epoch, &id)
            {
                return;
            }
            this.host.thread.controlling = false;
            match result {
                // Watch owns snapshot order; a command reply can arrive after a newer event.
                Ok(reply) if reply.thread.id == id => {}
                Ok(_) => {
                    this.host.connection.error =
                        Some("Host returned the wrong thread for this control request.".into())
                }
                Err(error) => this.host.connection.error = Some(error.to_string()),
            }
        });
        self.sync_composer(cx);
        cx.notify();
    }

    pub(super) fn save_default_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.connection.saving
            || !self
                .host
                .available(self.host.connection.settings_model.as_ref())
        {
            return;
        }
        let choice = self.host.connection.settings_model.clone().unwrap();
        self.host.connection.saving = true;
        self.host.connection.save_message = Some("Saving default model…".into());
        self.host_request::<host::DefaultReply>(
            Request::SetDefaultModel {
                provider: choice.provider.clone(),
                model_id: choice.model_id.clone(),
            },
            window,
            cx,
            move |this, result, _, _| {
                let result = match result {
                    Ok(reply) if reply.default_model == choice => Ok(reply.default_model),
                    Ok(_) => Err(
                        "Host returned a different model; reconnect to check the saved default."
                            .into(),
                    ),
                    Err(error) => Err(error.to_string()),
                };
                this.host.finish_save(result);
            },
        );
        cx.notify();
    }

    pub(super) fn can_submit(&self) -> bool {
        if self.checkout_mismatch() {
            return false;
        }
        self.session.ready()
            && self.navigation.available.get(&self.host.creation.checkout) != Some(&false)
            && self.host.connection.connected
            && self.tab_identity_current()
            && !self.host.thread.controlling
            && !self.thread_configuration_pending()
            && self.host.thread.snapshot.as_ref().is_some_and(|snapshot| {
                (self.host.protocol() >= 3
                    || (!snapshot.status.busy()
                        && self.host.submissions.pending(&snapshot.id).is_none()))
                    && self.session_accepts_thread(&snapshot.id)
                    && self.harness_reason().is_none()
                    && self.usable_in(super::harnesses::harness_of(snapshot), &snapshot.model)
                    && snapshot.queued.len() + self.host.submissions.all(&snapshot.id).len() < 16
            })
    }

    pub(super) fn sync_composer(&mut self, cx: &mut Context<Self>) {
        self.bind_selected_checkout();
        self.sync_thread_harness();
        self.sync_draft_composer(cx);
        let Some(id) = self.host.thread.selected.as_ref() else {
            return;
        };
        let Some(composer) = self.workspace.composers.get(id) else {
            return;
        };
        let busy = self
            .host
            .thread
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.status.busy());
        let reason = if self.navigation.available.get(&self.host.creation.checkout) == Some(&false)
        {
            Some("Checkout folder unavailable. Open its current location before sending.".into())
        } else if !self.session.ready() {
            Some("Restoring saved work…".into())
        } else if !self.session_accepts_thread(id) {
            Some("Thread checkout differs from the saved draft. Original work is preserved.".into())
        } else if self.workspace.thread_controls.editing.get(id)
            == Some(&self.host.connection.epoch)
        {
            Some("Restoring the queued message. Wait for cancellation to finish.".into())
        } else if !self.host.connection.connected {
            Some("Host disconnected. Reconnect to send.".into())
        } else if self.host.thread.snapshot.is_none() {
            Some("Loading saved thread…".into())
        } else if self.host.protocol() < 3
            && (self.host.thread.submitting || self.host.submissions.pending(id).is_some())
        {
            Some("Message unconfirmed · Retry safely above.".into())
        } else if let Some(reason) = self.harness_reason() {
            Some(reason)
        } else if !self.host.thread.snapshot.as_ref().is_some_and(|snapshot| {
            self.usable_in(super::harnesses::harness_of(snapshot), &snapshot.model)
        }) {
            Some(
                if self.host.protocol() >= 3 {
                    "Model unavailable · choose another model below."
                } else {
                    "Model unavailable · check Settings."
                }
                .into(),
            )
        } else {
            None
        };
        let send_blocked = if self.thread_configuration_pending() {
            Some("Saving model settings…".into())
        } else if self.verified_host().is_none() {
            // Typing is saved for this machine; sending waits for its host.
            Some("Checking which machine this is…".into())
        } else if self
            .host
            .thread
            .snapshot
            .as_ref()
            .is_some_and(|s| s.queued.len() + self.host.submissions.all(id).len() >= 16)
        {
            Some("Queue full · cancel a message to send another.".into())
        } else {
            None
        };
        let agent = self.agent_commands();
        composer.update(cx, |composer, cx| {
            let revision = composer.saved_draft().1;
            composer.set_images_uploading(
                self.host.submissions.all(id).iter().any(|pending| {
                    pending.draft_revision == Some(revision)
                        && !pending.images.is_empty()
                        && self.host.submissions.is_sending(&pending.request_id)
                }),
                cx,
            );
            composer.set_image_support(
                self.host.protocol(),
                self.host
                    .thread
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.image_input.as_deref()),
                cx,
            );
            composer.set_agent_commands(agent, cx);
            composer.set_send_blocked(send_blocked, cx);
            composer.set_enabled(
                reason.is_none() && !self.host.thread.controlling,
                reason,
                cx,
            );
            composer.set_busy(busy, cx);
            composer.set_queue_enabled(
                self.host.protocol() >= 3,
                self.host.connection.connected && !self.host.thread.controlling,
                cx,
            );
        });
        self.sync_prompt_history(cx);
    }
}

#[cfg(test)]
mod identity_error_tests {
    use super::is_checkout_identity_changed;
    use plait_client::host;

    #[test]
    fn classifies_only_identity_refusals() {
        let rejected = |code: &str, message: &str| {
            host::Error::Rejected(host::HostError {
                retry_after_seconds: None,
                code: code.into(),
                message: message.into(),
            })
        };
        assert!(is_checkout_identity_changed(&rejected(
            "checkout_identity_changed",
            "Checkout identity changed."
        )));
        assert!(is_checkout_identity_changed(&rejected(
            "invalid_request",
            "This folder changed since it was opened."
        )));
        assert!(!is_checkout_identity_changed(&rejected(
            "invalid_request",
            "Another error."
        )));
        assert!(!is_checkout_identity_changed(&host::Error::Protocol(
            "This folder changed since it was opened.".into()
        )));
    }
}
