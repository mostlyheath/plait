//! Background history reads and projection; render only consumes prepared data.
use super::{Plait, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    history::{self, History, Key},
    host::{HistoryField, Snapshot},
    markdown, transcript,
};
use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub(super) enum Load {
    Latest,
    Restore(Option<String>),
    Older,
    Part(Key, HistoryField, bool),
}
#[derive(Default)]
pub(super) struct HistoryView {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    resume: bool,
    pub retained_bytes: usize,
    pub cache: Arc<History>,
    pub display: Option<Arc<Snapshot>>,
    pub enabled: bool,
    pub loading: Option<Load>,
    pub error: Option<String>,
    failed: Option<Load>,
    tried: Option<u64>,
    revision: u64,
    projecting: bool,
    pub expanded: HashSet<String>,
    pub now: u64,
    timer: bool,
}
impl HistoryView {
    fn pause(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.cancelled = Default::default();
        self.resume = true;
        self.loading = None;
        self.projecting = false;
        self.timer = false;
        self.tried = None;
    }
}
fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn tail_stamp(snapshot: &Snapshot) -> u64 {
    let mut hash = DefaultHasher::new();
    for message in &snapshot.messages {
        message.id.hash(&mut hash);
        message.text.len().hash(&mut hash);
        message.reasoning.len().hash(&mut hash);
    }
    for tool in &snapshot.tools {
        tool.id.hash(&mut hash);
        (tool.status as u8).hash(&mut hash);
    }
    (snapshot.status as u8).hash(&mut hash);
    hash.finish()
}
impl Plait {
    pub(super) fn pause_conversation(&mut self, window: Option<&Window>, cx: &App) {
        if self.workspace.tab != super::Workspace::Conversation {
            return;
        }
        let bookmark = self.capture_tab_bookmark(window, cx);
        let records_current = self.records_current();
        if let Some(saved) = self.workspace.restore.as_mut() {
            saved.conversation = bookmark.conversation;
            saved.panels.enlarged_image = bookmark.panels.enlarged_image;
        } else {
            self.workspace.restore = Some(bookmark);
        }
        self.workspace.restore_pending[0] = true;
        self.workspace.host_watch = None;
        self.host.thread.watch_epoch += 1;
        // Loaded immutable records still describe the same observed snapshot.
        if records_current && let Some(stamp) = self.workspace.turn_records.stamp.as_mut() {
            stamp.watch = self.host.thread.watch_epoch;
        }
        self.workspace
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.workspace.cancelled = Default::default();
        self.workspace.file_viewer.retire_read();
        self.workspace.images = Default::default();
        self.workspace.history.pause();
    }

    pub(super) fn conversation_snapshot(&self) -> Option<&Snapshot> {
        self.workspace
            .history
            .display
            .as_deref()
            .or(self.host.thread.snapshot.as_deref())
    }
    pub(super) fn conversation_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tab != super::Workspace::Conversation {
            return;
        }
        self.workspace.history.enabled = self.host.protocol() >= 3;
        if std::mem::take(&mut self.workspace.history.resume) {
            self.workspace.history.cache = Default::default();
            let before = self
                .workspace
                .restore
                .as_ref()
                .and_then(|b| b.conversation.before.clone());
            self.load_history(Load::Restore(before), window, cx);
        }
        if self.workspace.restore_pending[0]
            && !self.workspace.history.cache.loaded
            && let Some(before) = self
                .workspace
                .restore
                .as_ref()
                .and_then(|b| b.conversation.before.clone())
        {
            self.load_history(Load::Restore(Some(before)), window, cx);
        }
        self.workspace.history.revision += 1;
        self.workspace.history.now = millis();
        self.prepare_conversation(window, cx);
        self.tick_retry(window, cx);
    }
    fn tick_retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.history.timer
            || self
                .host
                .thread
                .snapshot
                .as_ref()
                .is_none_or(|s| s.retry.is_none())
        {
            return;
        }
        self.workspace.history.timer = true;
        let epoch = self.host.connection.epoch;
        let watch_epoch = self.host.thread.watch_epoch;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update_in(cx, |this, _, cx| {
                        if this.host.connection.epoch != epoch
                            || this.host.thread.watch_epoch != watch_epoch
                        {
                            return false;
                        }
                        this.workspace.history.now = millis();
                        let keep = this
                            .host
                            .thread
                            .snapshot
                            .as_ref()
                            .is_some_and(|s| s.retry.is_some());
                        if !keep {
                            this.workspace.history.timer = false;
                        }
                        cx.notify();
                        keep
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }
    /// One projection at a time. A newer snapshot marks the result stale and is
    /// coalesced into the next job, rather than spawning a task per token.
    fn prepare_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.history.projecting {
            return;
        }
        let Some(snapshot) = self.host.thread.snapshot.clone() else {
            return;
        };
        self.workspace.history.projecting = true;
        let id = snapshot.id.clone();
        let epoch = self.host.connection.epoch;
        let watch_epoch = self.host.thread.watch_epoch;
        let revision = self.workspace.history.revision;
        let cache = self.workspace.history.cache.clone();
        let enabled = self.workspace.history.enabled;
        let previous = self.workspace.formatted.clone();
        let records = self.workspace.turn_records.data.clone();
        let record_generation = self.workspace.turn_records.data_generation;
        let cancelled = self.workspace.cancelled.clone();
        let view_cancelled = self.workspace.history.cancelled.clone();
        let anchor = self.conversation_snapshot().and_then(|s| {
            match self
                .workspace
                .live_layout
                .rows
                .get(self.workspace.live_scroll.logical_scroll_top().item_ix)?
            {
                plait_client::folding::DisplayRow::Slot(slot) => {
                    transcript::resolve(s, *slot).and_then(Key::row)
                }
                plait_client::folding::DisplayRow::FoldedTurn { id, .. } => {
                    Some(Key::Message(id.clone()))
                }
            }
        });
        let task = cx.background_executor().spawn(async move {
            let mut cache = (*cache).clone();
            loop {
                if cancelled.load(std::sync::atomic::Ordering::Acquire)
                    || view_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return None;
                }
                let display = if enabled {
                    cache.project(&snapshot)
                } else {
                    (*snapshot).clone()
                };
                let stored = cache
                    .entries
                    .iter()
                    .filter_map(|entry| match &entry.item {
                        plait_client::host::HistoryItem::Tool { id, status, .. }
                            if *status != plait_client::host::ToolStatus::Running
                                && display
                                    .tools
                                    .iter()
                                    .any(|tool| tool.id == *id && tool.status == *status) =>
                        {
                            Some(id.clone())
                        }
                        _ => None,
                    })
                    .collect();
                let formatted = markdown::Transcript::update_cancellable(
                    &previous,
                    &display,
                    &stored,
                    &|| cancelled.load(std::sync::atomic::Ordering::Acquire)
                        || view_cancelled.load(std::sync::atomic::Ordering::Acquire),
                )?;
                let retained = cache.retained_bytes()
                    + display.retained_bytes()
                    + snapshot.retained_bytes()
                    + formatted.retained_bytes();
                if retained > 96 * 1024 * 1024 {
                    if cache.evict_for_budget(anchor.as_ref()) { continue; }
                    return Some(Err("This conversation part exceeds the reading memory limit. Its stored history is unchanged."));
                }
                let lines = if cache.detached { history::records::project_detached(&display, &snapshot, &records) } else { history::records::project(&display, &records, true) };
                return Some(Ok((
                    Arc::new(cache),
                    Arc::new(display),
                    Arc::new(formatted),
                    Arc::new(lines),
                    retained,
                )));
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(result) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.host.accepts_watch(epoch, watch_epoch, &id) {
                    return;
                }
                this.workspace.history.projecting = false;
                if this.workspace.history.revision != revision {
                    this.prepare_conversation(window, cx);
                    return;
                }
                let (cache, display, formatted, lines, retained) = match result {
                    Ok(value) => value,
                    Err(error) => {
                        this.workspace.history.error = Some(error.into());
                        this.notice = Some(error.into());
                        cx.notify();
                        return;
                    }
                };
                this.workspace.history.cache = cache;
                this.workspace.history.retained_bytes = retained;
                let previous = this.workspace.history.display.replace(display);
                let before = std::mem::replace(&mut this.workspace.formatted, formatted);
                if this.reconcile_live_list(previous.as_deref()) {
                    this.remeasure_records(previous.as_deref(), &before);
                }
                this.install_record_lines(lines, record_generation);
                if this.records_current()
                    && let Some(work) = this.tabs.model.work_mut()
                {
                    work.records_ready(!this.workspace.turn_records.lines.runs.is_empty());
                }
                if this.panel_recorded()
                    && this
                        .tabs
                        .model
                        .panel()
                        .is_some_and(|p| p.recorded.is_none())
                    || this.tabs.model.work().is_some_and(|w| w.open && w.recorded)
                {
                    this.sync_work_scope(true, window, cx);
                }
                this.conversation_rows_changed(cx);
                this.restore_ready_bookmarks(cx);
                this.restore_tab_image(window, cx);
                if this.workspace.tab == super::Workspace::Conversation
                    && !this.settings_open
                    && window.is_window_active()
                {
                    this.mark_thread_read(&id, cx);
                }
                if let Some(stamp) = this
                    .tabs
                    .model
                    .active()
                    .and_then(|id| this.tabs.model.read_stamp(id))
                {
                    this.tabs.model.finish_activation(stamp);
                }
                if this.workspace.history.enabled
                    && this.workspace.history.loading.is_none()
                    && let Some(live) = this.host.thread.snapshot.as_ref()
                    && this.workspace.history.cache.needs_latest(live)
                    && this.workspace.history.tried != Some(tail_stamp(live))
                {
                    this.load_history(Load::Latest, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn load_history(&mut self, load: Load, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace.history.enabled
            || self.workspace.tab != super::Workspace::Conversation
            || !self.host.connection.connected
            || self.workspace.history.loading.is_some()
        {
            return;
        }
        let Some(snapshot) = self.host.thread.snapshot.as_ref() else {
            return;
        };
        let id = snapshot.id.clone();
        let epoch = self.host.connection.epoch;
        let watch_epoch = self.host.thread.watch_epoch;
        let before = match &load {
            Load::Older => match self.workspace.history.cache.older.clone() {
                Some(cursor) => cursor,
                None => return,
            },
            _ => None,
        };
        self.workspace.history.tried = Some(tail_stamp(snapshot));
        self.workspace.history.loading = Some(load.clone());
        self.workspace.history.error = None;
        self.workspace.history.failed = None;
        let cache = self.workspace.history.cache.clone();
        let operation = load.clone();
        let thread = id.clone();
        let protocol = self.host.protocol();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            let mut cache = (*cache).clone();
            match operation {
                Load::Restore(before) => {
                    cache.latest(history::read_page_version(
                        &client,
                        &thread,
                        before.clone(),
                        protocol,
                    )?);
                    cache.detached = before.is_some();
                }
                Load::Latest => cache.latest(history::read_page_version(
                    &client, &thread, None, protocol,
                )?),
                Load::Older => cache.prepend(history::read_page_version(
                    &client, &thread, before, protocol,
                )?),
                Load::Part(key, field, restart) => {
                    let entry = cache.get(&key).ok_or_else(|| {
                        plait_client::host::Error::Protocol(
                            "This text is outside the reading window. Load its page again.".into(),
                        )
                    })?;
                    let entry = history::read_part(&client, &thread, entry, field, restart)?;
                    cache.replace(entry);
                }
            }
            Ok::<_, plait_client::host::Error>(Arc::new(cache))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.host.accepts_watch(epoch, watch_epoch, &id) {
                    return;
                }
                this.workspace.history.loading = None;
                match result {
                    Ok(cache) => {
                        if matches!(load, Load::Part(..)) {
                            this.clear_reading_selection();
                        }
                        this.workspace.history.cache = cache;
                        this.workspace.history.revision += 1;
                        this.prepare_conversation(window, cx);
                    }
                    Err(error) => {
                        this.workspace.history.error =
                            Some(format!("Could not load conversation text: {error}"));
                        this.workspace.history.failed = Some(load);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn jump_latest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.history.cache.detached {
            self.load_history(Load::Latest, window, cx);
        }
        self.workspace.live_scroll.scroll_to_end();
        cx.notify();
    }
    pub(super) fn scroll_history(
        &mut self,
        event: &ListScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_open
            && event.is_scrolled
            && event.visible_range.start <= 3
            && self.workspace.history.error.is_none()
        {
            self.load_history(Load::Older, window, cx);
        }
    }
    pub(super) fn history_heading(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.workspace.history.enabled {
            return div().into_any_element();
        }
        let loading = self.workspace.history.loading.is_some();
        let failed = self.workspace.history.failed.clone();
        let label = if let Some(error) = &self.workspace.history.error {
            error.clone()
        } else if loading {
            "Loading conversation text…".into()
        } else if self.workspace.history.cache.detached {
            "Reading older history · newer pages are outside this window".into()
        } else if self.workspace.history.cache.loaded
            && self.workspace.history.cache.older.is_none()
        {
            if self
                .workspace
                .live_scroll
                .scroll_px_offset_for_scrollbar()
                .y
                >= px(-1.)
            {
                "Beginning of conversation"
            } else {
                "History loaded · scroll up for the beginning"
            }
            .into()
        } else if self.workspace.history.cache.loaded {
            "Earlier messages available".into()
        } else {
            "Loading complete history · live preview below".into()
        };
        div()
            .flex_shrink_0()
            .px_6()
            .py_1()
            .child(
                row()
                    .w_full()
                    .max_w(px(CONTENT_WIDTH))
                    .mx_auto()
                    .min_h(px(26.))
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(if failed.is_some() {
                                colors().amber
                            } else {
                                colors().muted
                            }))
                            .child(label),
                    )
                    .when_some(failed, |row, load| {
                        row.child(
                            button("retry-history", "Retry")
                                .on_activate(cx, move |this, window, cx| {
                                    this.load_history(load.clone(), window, cx)
                                }),
                        )
                    })
                    .when(
                        !loading
                            && self.workspace.history.error.is_none()
                            && self.workspace.history.cache.older.is_some(),
                        |row| {
                            row.child(
                                button("older-history", "Load earlier")
                                    .on_activate(cx, |this, window, cx| {
                                        this.load_history(Load::Older, window, cx)
                                    }),
                            )
                        },
                    )
                    .when(self.workspace.history.cache.detached, |row| {
                        row.child(
                            button("latest-history", "Latest")
                                .on_activate(cx, |this, window, cx| this.jump_latest(window, cx)),
                        )
                    }),
            )
            .into_any_element()
    }
    pub(super) fn history_parts(
        &self,
        key: Key,
        field: Option<HistoryField>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let entry = self.workspace.history.cache.get(&key)?;
        if !entry
            .parts
            .iter()
            .any(|part| field.is_none_or(|field| field == part.field))
        {
            return None;
        }
        let mut view = column().gap_1();
        for part in entry
            .parts
            .iter()
            .filter(|part| field.is_none_or(|field| field == part.field))
        {
            let name = match part.field {
                HistoryField::Text => "Message",
                HistoryField::Reasoning => "Reasoning",
                HistoryField::Input => "Arguments",
                HistoryField::Output => "Output",
            };
            let id = format!("part-{:?}-{name}", key);
            let field = part.field;
            let next_key = key.clone();
            let start_key = key.clone();
            view = view.child(
                row()
                    .gap_2()
                    .flex_wrap()
                    .text_size(px(11.))
                    .text_color(rgb(colors().amber))
                    .child(format!(
                        "{name} · part {}{}",
                        part.number,
                        if part.has_next() {
                            " · more text available"
                        } else {
                            " · last part"
                        }
                    ))
                    .when(part.has_next(), |row| {
                        row.child(
                            button(SharedString::from(format!("{id}-next")), "Next part")
                                .disabled(self.workspace.history.loading.is_some())
                                .on_activate(cx, move |this, window, cx| {
                                    this.load_history(
                                        Load::Part(next_key.clone(), field, false),
                                        window,
                                        cx,
                                    )
                                }),
                        )
                    })
                    .when(part.start > 0, |row| {
                        row.child(
                            button(SharedString::from(format!("{id}-start")), "Start over")
                                .disabled(self.workspace.history.loading.is_some())
                                .on_activate(cx, move |this, window, cx| {
                                    this.load_history(
                                        Load::Part(start_key.clone(), field, true),
                                        window,
                                        cx,
                                    )
                                }),
                        )
                    }),
            );
        }
        Some(view.into_any_element())
    }
    pub(super) fn toggle_reasoning(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.workspace.history.expanded.remove(id) {
            self.workspace.history.expanded.insert(id.to_owned());
        }
        if let Some(snapshot) = self.conversation_snapshot()
            && let Some(index) = self
                .workspace
                .live_layout
                .rows
                .iter()
                .position(|row| match row {
                    plait_client::folding::DisplayRow::Slot(slot) => {
                        matches!(
                            transcript::resolve(snapshot, *slot),
                            Some(transcript::Row::Reasoning(m)) if m.id == id
                        )
                    }
                    plait_client::folding::DisplayRow::FoldedTurn { .. } => false,
                })
        {
            self.workspace.live_scroll.remeasure_items(index..index + 1);
        }
        cx.notify();
    }
}
