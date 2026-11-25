//! The message composer: a native multi-line input with IME, undo, prompt
//! recall, and completion for files (`@`) and local commands (`/`).
//!
//! The composer owns editable text and reports intent through `ComposerEvent`.
//! Its parent owns host requests, Git reads, and what a command does.

mod commands;
mod completion;
mod draft;
mod history;
mod images;
mod input;
mod mentions;
mod parent;
mod parking;
pub(super) use parking::ParkedEditor;
mod trigger;

pub use commands::{AgentCommand, CommandId};
pub use completion::{Completion, CompletionEvent, CompletionItem};

use super::theme::{self, AuditA11y, colors};
use commands::COMMANDS;
use draft::Draft;
use gpui::{prelude::*, *};
use input::{InputElement, LINE_HEIGHT, Layout, PLACEHOLDER};
use mentions::Latest;
use plait_client::{fuzzy::fuzzy_match, git::FileList};
use std::{
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};
use trigger::{Trigger, TriggerKind};
use unicode_segmentation::UnicodeSegmentation;

/// Suggestions kept for one query; typing narrows the list faster than scrolling.
const SUGGESTION_LIMIT: usize = 50;
/// The suggestion list is narrower than a wide composer, so paths stay scannable.
const SUGGESTION_WIDTH: f32 = 560.;
const SUGGESTION_GUTTER: f32 = 8.;
/// Two Escape presses this close together stop a run; one alone never does.
const STOP_WINDOW: Duration = Duration::from_millis(1500);

actions!(
    composer,
    [
        Submit,
        SubmitNow,
        Steer,
        Newline,
        Complete,
        Cancel,
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocumentStart,
        DocumentEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let context = Some("PlaitComposer");
    cx.bind_keys([
        KeyBinding::new("enter", Submit, context),
        KeyBinding::new("cmd-enter", SubmitNow, context),
        KeyBinding::new("alt-enter", Steer, context),
        KeyBinding::new("shift-enter", Newline, context),
        // Tab accepts a suggestion; with none open it moves focus as usual.
        KeyBinding::new("tab", Complete, context),
        KeyBinding::new("escape", Cancel, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("alt-backspace", DeleteWordLeft, context),
        KeyBinding::new("alt-delete", DeleteWordRight, context),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, context),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("cmd-left", Home, context),
        KeyBinding::new("cmd-right", End, context),
        KeyBinding::new("cmd-shift-left", SelectHome, context),
        KeyBinding::new("cmd-shift-right", SelectEnd, context),
        KeyBinding::new("home", DocumentStart, context),
        KeyBinding::new("end", DocumentEnd, context),
        KeyBinding::new("cmd-up", DocumentStart, context),
        KeyBinding::new("cmd-down", DocumentEnd, context),
        KeyBinding::new("alt-left", WordLeft, context),
        KeyBinding::new("alt-right", WordRight, context),
        KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SelectWordRight, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
    ]);
}

#[derive(Clone, Debug)]
pub enum ComposerEvent {
    Submit {
        text: String,
        revision: u64,
        steer: bool,
    },
    Stop,
    /// A local command was chosen from the `/` list. The parent performs it.
    Command(CommandId),
    /// A file mention opened. The parent answers with `Composer::set_files`.
    FilesNeeded,
}

/// The checkout's file list as the parent last reported it.
#[derive(Clone)]
pub enum MentionFiles {
    /// Nothing reported yet, or a read is in progress.
    Loading,
    Ready(Arc<FileList>),
    /// The list cannot be read; the reason is shown in the suggestion list.
    Unavailable(String),
}

/// What accepting a suggestion does.
#[derive(Clone)]
enum Pick {
    File(String),
    Command(CommandId),
    /// An agent command: its text replaces the draft, ready for arguments.
    Insert(SharedString),
}

struct Popup {
    trigger: Trigger,
    completion: Completion<Pick>,
}

impl Popup {
    fn new(trigger: Trigger) -> Self {
        let name = match trigger.kind {
            TriggerKind::Mention => "File suggestions",
            TriggerKind::Command => "Command suggestions",
        };
        Self {
            trigger,
            completion: Completion::new(name),
        }
    }

    fn key(&self) -> (TriggerKind, usize) {
        (self.trigger.kind, self.trigger.start)
    }
}

/// Escape stops a run only as a deliberate double press.
#[derive(Default)]
struct StopGesture {
    armed_at: Option<Instant>,
}

impl StopGesture {
    /// Record a press. True when it is the second one inside the window.
    fn press(&mut self, now: Instant) -> bool {
        let confirmed = self.armed(now);
        self.armed_at = (!confirmed).then_some(now);
        confirmed
    }

    fn armed(&self, now: Instant) -> bool {
        self.armed_at
            .is_some_and(|at| now.saturating_duration_since(at) < STOP_WINDOW)
    }

    fn disarm(&mut self) {
        self.armed_at = None;
    }
}

pub struct Composer {
    pub(super) box_focus: FocusHandle,
    focus: FocusHandle,
    draft: Draft,
    images: images::Attachments,
    enabled: bool,
    send_blocked: Option<String>,
    reason: Option<String>,
    busy: bool,
    queue_enabled: bool,
    stop_enabled: bool,
    selecting: bool,
    layout: Layout,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    reveal_cursor: bool,
    input_changed: bool,
    layout_revision: u64,
    paint_error: Option<String>,
    popup: Option<Popup>,
    /// A trigger the user dismissed with Escape; it stays closed until it changes.
    dismissed: Option<(TriggerKind, usize)>,
    files: MentionFiles,
    ranking: Latest,
    rank_task: Option<Task<()>>,
    stop: StopGesture,
    /// Skills and templates of the person's Pi setup, offered after local commands.
    agent: Arc<[AgentCommand]>,
}

impl Composer {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            // Tracked focus uses the handle's tab settings, not the div's.
            box_focus: cx.focus_handle(),
            focus: cx.focus_handle().tab_stop(true),
            draft: Draft::default(),
            images: Default::default(),
            enabled: false,
            send_blocked: None,
            reason: Some("Connect a local thread to send messages.".into()),
            busy: false,
            queue_enabled: false,
            stop_enabled: false,
            selecting: false,
            layout: Layout::default(),
            bounds: None,
            scroll: px(0.),
            reveal_cursor: true,
            input_changed: false,
            layout_revision: 0,
            paint_error: None,
            popup: None,
            dismissed: None,
            files: MentionFiles::Loading,
            ranking: Latest::default(),
            rank_task: None,
            stop: StopGesture::default(),
            agent: Arc::from([]),
        }
    }

    /// The draft to keep across a restart, its edit revision, and the pending
    /// submission. A recalled prompt on screen is not the draft; the text
    /// being written underneath it is.
    pub fn saved_draft(&self) -> (&str, u64, &Option<(String, u64)>) {
        (
            self.draft.durable_text(),
            self.draft.revision,
            &self.draft.submitted,
        )
    }

    pub fn restore_draft(
        &mut self,
        text: String,
        revision: u64,
        submitted: Option<(String, u64)>,
        cx: &mut Context<Self>,
    ) {
        let history = std::mem::take(&mut self.draft.history);
        self.draft = Draft::restored(text, revision, submitted, history);
        // The restored revision can repeat one the layout has already seen.
        self.layout.invalidate();
        self.close_popup();
        self.dismissed = None;
        self.input_changed = true;
        self.reveal_cursor = true;
        cx.notify();
    }

    pub fn set_enabled(&mut self, enabled: bool, reason: Option<String>, cx: &mut Context<Self>) {
        if self.enabled == enabled && self.reason == reason {
            return;
        }
        self.enabled = enabled;
        self.reason = reason;
        if !enabled {
            self.draft.finish_composition();
            self.close_popup();
            self.stop.disarm();
        }
        self.input_changed = true;
        cx.notify();
    }

    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            self.stop.disarm();
            // Some commands exist only while the agent works.
            self.fill_commands();
            cx.notify();
        }
    }

    /// Supply the person's skills and templates. Cheap when the list is the same one.
    pub fn set_agent_commands(&mut self, commands: Arc<[AgentCommand]>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&self.agent, &commands) {
            return;
        }
        self.agent = commands;
        self.fill_commands();
        cx.notify();
    }

    /// Supply the prompts already sent in this thread, oldest first, for
    /// recall with Up and Down. Cheap when nothing changed.
    pub fn set_history(&mut self, prompts: impl IntoIterator<Item = impl AsRef<str>>) {
        self.draft.history.set_host(prompts);
    }

    /// Answer `ComposerEvent::FilesNeeded` with the checkout's files. Matching
    /// against them runs on the background executor.
    pub fn set_files(&mut self, files: MentionFiles, cx: &mut Context<Self>) {
        match (&self.files, &files) {
            (MentionFiles::Ready(current), MentionFiles::Ready(next))
                if Arc::ptr_eq(current, next) =>
            {
                return;
            }
            // While a newer list loads, the previous one is still the best answer.
            (MentionFiles::Ready(_), MentionFiles::Loading) => return,
            _ => {}
        }
        self.files = files;
        self.rank_files(cx);
        cx.notify();
    }

    pub fn acknowledge(
        &mut self,
        submitted: &str,
        revision: Option<u64>,
        cx: &mut Context<Self>,
    ) -> bool {
        let cleared = match revision {
            Some(revision) => self.draft.clear_revision(submitted, revision),
            None => self.draft.clear_if_unchanged(submitted),
        };
        if cleared {
            self.images.references.clear();
            self.images.pins.clear();
            self.images.pixels.clear();
            self.images.generation += 1;
            self.images.loading = false;
            self.images.error = None;
            self.close_popup();
            self.reveal_cursor = true;
            self.input_changed = true;
            cx.notify();
        }
        cleared
    }

    pub fn set_queue_enabled(&mut self, enabled: bool, can_stop: bool, cx: &mut Context<Self>) {
        if self.queue_enabled != enabled || self.stop_enabled != can_stop {
            self.queue_enabled = enabled;
            self.stop_enabled = can_stop;
            cx.notify();
        }
    }

    pub fn set_send_blocked(&mut self, reason: Option<String>, cx: &mut Context<Self>) {
        if self.send_blocked != reason {
            self.send_blocked = reason;
            cx.notify();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.draft.value.text.is_empty() && self.images.references.is_empty()
    }

    /// True when the draft has neither non-whitespace text nor images to send.
    pub fn is_blank(&self) -> bool {
        self.draft.value.text.trim().is_empty() && self.images.references.is_empty()
    }

    /// Nothing typed, attached, or still being attached.
    pub fn holds_nothing(&self) -> bool {
        self.is_empty() && !self.images.loading
    }

    pub fn restore_cancelled(&mut self, text: &str, revision: u64, cx: &mut Context<Self>) -> bool {
        if !self.is_empty() || self.draft.revision != revision || self.draft.marked.is_some() {
            return false;
        }
        self.draft.insert(text);
        self.reveal_cursor = true;
        self.input_changed = true;
        cx.notify();
        true
    }

    fn changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.images.error = None;
        self.reveal_cursor = true;
        self.stop.disarm();
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn submit(&mut self, steer: bool, cx: &mut Context<Self>) {
        if self.send_blocked.is_some() || self.image_send_blocked() {
            return;
        }
        if steer && !(self.busy && self.queue_enabled) {
            return;
        }
        let submitted = if !self.images.references.is_empty()
            && self.enabled
            && !(self.busy && !self.queue_enabled)
            && self.draft.marked.is_none()
        {
            let text = self.draft.value.text.clone();
            if !text.trim().is_empty() {
                self.draft.history.record(&text);
            }
            self.draft.submitted = Some((text.clone(), self.draft.revision));
            Some(text)
        } else {
            self.draft
                .submit(self.enabled, self.busy && !self.queue_enabled)
        };
        if let Some(text) = submitted {
            self.close_popup();
            cx.emit(ComposerEvent::Submit {
                text,
                revision: self.draft.revision,
                steer,
            });
            cx.notify();
        }
    }

    /// Add a block, such as quoted diff lines, to the end of the draft as its
    /// own paragraph, with the cursor after it. False when the composer is off.
    pub fn append_block(
        &mut self,
        block: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        window.focus(&self.focus, cx);
        self.edit(
            |draft| {
                draft.finish_composition();
                let text = &draft.value.text;
                let gap = match () {
                    _ if text.is_empty() || text.ends_with("\n\n") => "",
                    _ if text.ends_with('\n') => "\n",
                    _ => "\n\n",
                };
                let block = format!("{gap}{block}\n\n");
                draft.select(text.len(), false);
                draft.insert(&block);
            },
            window,
            cx,
        );
        self.reveal_cursor = true;
        true
    }

    fn edit(
        &mut self,
        operation: impl FnOnce(&mut Draft),
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.enabled {
            operation(&mut self.draft);
            self.refresh_popup(true, cx);
            self.changed(window, cx);
        }
    }

    fn move_to(&mut self, offset: usize, extend: bool, window: &Window, cx: &mut Context<Self>) {
        self.draft.finish_composition();
        self.draft.select(offset, extend);
        self.refresh_popup(false, cx);
        self.reveal_cursor = true;
        self.stop.disarm();
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn horizontal(&mut self, forward: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let selection = self.draft.selection();
        let offset = if !extend && !selection.is_empty() {
            if forward {
                selection.end
            } else {
                selection.start
            }
        } else {
            self.draft.boundary(self.draft.value.head, forward)
        };
        self.move_to(offset, extend, window, cx);
    }

    fn word(&mut self, forward: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let offset = self.draft.word_boundary(self.draft.value.head, forward);
        self.move_to(offset, extend, window, cx);
    }

    fn delete_word(&mut self, forward: bool, window: &Window, cx: &mut Context<Self>) {
        self.edit(
            |draft| draft.delete_to(|d| d.word_boundary(d.value.head, forward), forward),
            window,
            cx,
        );
    }

    fn layout_is_current(&self) -> bool {
        self.layout_revision == self.draft.revision
    }

    /// Visual-row edge, matching Cmd-Left/Right; rows are the wrapped layout.
    fn row_edge(&self, end: bool) -> usize {
        self.layout
            .rows
            .get(self.layout.row_at(self.draft.value.head))
            .map(|row| if end { row.range.end } else { row.range.start })
            .unwrap_or(if end { self.draft.value.text.len() } else { 0 })
    }

    fn delete_to_line_edge(&mut self, end: bool, window: &Window, cx: &mut Context<Self>) {
        // A stale layout could point inside other text; fall back to one grapheme.
        let target = if self.layout_is_current() {
            self.row_edge(end)
        } else {
            self.draft.value.head
        };
        self.edit(|draft| draft.delete_to(|_| target, end), window, cx);
    }

    fn cursor_position(&self) -> Point<Pixels> {
        self.position_for_offset(self.draft.value.head)
    }

    fn position_for_offset(&self, offset: usize) -> Point<Pixels> {
        let row = self.layout.row_at(offset);
        let x = self
            .layout
            .rows
            .get(row)
            .map(|row| {
                row.line
                    .x_for_index(offset.min(row.range.end) - row.range.start)
            })
            .unwrap_or(px(0.));
        point(x, px(row as f32 * LINE_HEIGHT))
    }

    fn index_at(&self, position: Point<Pixels>) -> usize {
        let row = (position.y / px(LINE_HEIGHT)).floor().max(0.) as usize;
        self.layout
            .rows
            .get(row)
            .map(|row| {
                self.draft
                    .snap(row.range.start + row.line.closest_index_for_x(position.x))
            })
            .unwrap_or(self.draft.value.text.len())
    }

    fn mouse_index(&self, position: Point<Pixels>) -> usize {
        self.bounds
            .map(|bounds| self.index_at(position - bounds.origin + point(px(0.), self.scroll)))
            .unwrap_or(0)
    }

    fn vertical(&mut self, down: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let mut position = self.cursor_position();
        position.y += px(if down { LINE_HEIGHT } else { -LINE_HEIGHT });
        let offset = if position.y < px(0.) {
            0
        } else {
            self.index_at(position)
        };
        self.move_to(offset, extend, window, cx);
    }

    /// Whether the cursor is on the first (or last) line the user sees.
    fn on_edge_row(&self, first: bool) -> bool {
        let head = self.draft.value.head;
        if !self.layout_is_current() {
            // Before the next frame only logical lines are known.
            let text = &self.draft.value.text;
            return if first {
                !text[..head].contains('\n')
            } else {
                !text[head..].contains('\n')
            };
        }
        let row = self.layout.row_at(head);
        if first {
            row == 0
        } else {
            row + 1 >= self.layout.rows.len()
        }
    }

    /// Up and Down: choose a suggestion, else walk sent prompts, else move the cursor.
    fn arrow(&mut self, down: bool, window: &Window, cx: &mut Context<Self>) {
        if let Some(popup) = self.popup.as_mut().filter(|p| p.completion.has_items()) {
            popup.completion.step(down);
            cx.notify();
            return;
        }
        if self.enabled && self.draft.recall(!down, self.on_edge_row(!down)) {
            self.close_popup();
            self.changed(window, cx);
            return;
        }
        self.vertical(down, false, window, cx);
    }

    fn line_edge(&mut self, end: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        self.move_to(self.row_edge(end), extend, window, cx);
    }

    fn copy(&self, cx: &mut App) {
        let selection = self.draft.selection();
        if !selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.draft.value.text[selection].into(),
            ));
        }
    }

    fn paste_clipboard(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        // Preserve native text paste when GPUI returns text alongside a file or image.
        let Some(text) = item.text() else {
            for entry in item.entries {
                if let ClipboardEntry::Image(image) = entry {
                    self.paste_images(image.bytes, cx);
                    return;
                }
            }
            return;
        };
        // Most clipboards need no conversion; skip the copies when they do not.
        let text = if text.contains('\r') {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text
        };
        self.edit(|draft| draft.insert(&text), window, cx);
    }

    fn rectangles(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let Some(bounds) = self.bounds else {
            return Vec::new();
        };
        if !self.layout_is_current() {
            return Vec::new();
        }
        if range.is_empty() {
            return vec![Bounds::new(
                bounds.origin + self.position_for_offset(range.start) - point(px(0.), self.scroll),
                size(px(1.), px(LINE_HEIGHT)),
            )];
        }
        let rows = &self.layout.rows;
        let first = self.layout.row_at(range.start);
        rows[first.min(rows.len())..]
            .iter()
            .enumerate()
            .take_while(|(_, row)| row.range.start < range.end)
            .filter_map(|(index, row)| {
                if range.start > row.range.end {
                    return None;
                }
                let start = range.start.max(row.range.start) - row.range.start;
                let end = range.end.min(row.range.end) - row.range.start;
                let x = row.line.x_for_index(start);
                let width = (row.line.x_for_index(end) - x).max(px(2.));
                Some(Bounds::new(
                    bounds.origin
                        + point(x, px((first + index) as f32 * LINE_HEIGHT) - self.scroll),
                    size(width, px(LINE_HEIGHT)),
                ))
            })
            .collect()
    }
}

/// Completion: `@` files and `/` commands.
impl Composer {
    fn close_popup(&mut self) {
        self.popup = None;
        self.ranking.cancel();
        self.rank_task = None;
    }

    /// Re-read the trigger under the cursor. Only an edit opens a list; moving
    /// the cursor into an old `@word` does not.
    fn refresh_popup(&mut self, edited: bool, cx: &mut Context<Self>) {
        let draft = &self.draft;
        let quiet = draft.marked.is_none()
            && draft.selection().is_empty()
            && draft.recall_position().is_none();
        let trigger = (self.enabled && quiet)
            .then(|| trigger::detect(&draft.value.text, draft.value.head))
            .flatten();
        let Some(trigger) = trigger else {
            self.close_popup();
            self.dismissed = None;
            return;
        };
        let key = (trigger.kind, trigger.start);
        if self.dismissed == Some(key) {
            return self.close_popup();
        }
        self.dismissed = None;
        match &mut self.popup {
            Some(popup) if popup.key() == key => {
                if popup.trigger.query == trigger.query {
                    return;
                }
                popup.trigger = trigger;
            }
            _ if edited => {
                self.close_popup();
                if trigger.kind == TriggerKind::Mention {
                    cx.emit(ComposerEvent::FilesNeeded);
                }
                self.popup = Some(Popup::new(trigger));
            }
            _ => return self.close_popup(),
        }
        self.fill_commands();
        self.rank_files(cx);
    }

    fn fill_commands(&mut self) {
        let Some(popup) = self
            .popup
            .as_mut()
            .filter(|popup| popup.trigger.kind == TriggerKind::Command)
        else {
            return;
        };
        let query = &popup.trigger.query;
        let local = commands::matching(COMMANDS, query, self.busy)
            .into_iter()
            .map(|found| CompletionItem {
                label: format!("/{}", found.command.name).into(),
                detail: found.command.detail.into(),
                // The drawn label has a leading slash the match does not.
                label_matches: found.name_matches.iter().map(|at| at + 1).collect(),
                detail_matches: found.detail_matches,
                name: format!("/{}: {}", found.command.name, found.command.detail).into(),
                value: Pick::Command(found.command.id),
            });
        // The person's own commands come after the app's, and only the best few.
        let agent = commands::matching_agent(&self.agent, query)
            .into_iter()
            .take(SUGGESTION_LIMIT)
            .map(|found| CompletionItem {
                label: found.command.label.clone(),
                detail: found.command.detail.clone(),
                label_matches: found.name_matches.iter().map(|at| at + 1).collect(),
                detail_matches: found.detail_matches,
                name: found.command.spoken.clone(),
                value: Pick::Insert(found.command.label.clone()),
            });
        popup.completion.set_items(local.chain(agent).collect());
    }

    /// Start ranking files for the open mention. Results arrive through
    /// `show_ranked`; a newer query or a closed list discards them.
    fn rank_files(&mut self, cx: &mut Context<Self>) {
        let Some(popup) = self
            .popup
            .as_mut()
            .filter(|popup| popup.trigger.kind == TriggerKind::Mention)
        else {
            return;
        };
        let list = match &self.files {
            MentionFiles::Ready(list) => list.clone(),
            MentionFiles::Loading => {
                return popup.completion.set_note(Some("Loading files…".into()));
            }
            MentionFiles::Unavailable(error) => {
                popup.completion.set_items(Vec::new());
                return popup
                    .completion
                    .set_note(Some(format!("Files unavailable: {error}").into()));
            }
        };
        let ticket = self.ranking.begin();
        let query = popup.trigger.query.clone();
        let files = list.clone();
        let ranked = cx
            .background_executor()
            .spawn(async move { mentions::rank(&files.paths, &query, SUGGESTION_LIMIT) });
        self.rank_task = Some(cx.spawn(async move |this, cx| {
            let ranked = ranked.await;
            let _ = this.update(cx, |this, cx| this.show_ranked(ticket, &list, ranked, cx));
        }));
    }

    fn show_ranked(
        &mut self,
        ticket: u64,
        list: &FileList,
        ranked: Vec<mentions::RankedFile>,
        cx: &mut Context<Self>,
    ) {
        if !self.ranking.accepts(ticket) {
            return;
        }
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let items: Vec<_> = ranked
            .into_iter()
            .filter_map(|file| {
                let path = list.paths.get(file.index)?;
                let (name, directory) = mentions::split(path);
                let (label_matches, detail_matches) =
                    mentions::split_positions(path, &file.positions);
                Some(CompletionItem {
                    label: name.to_owned().into(),
                    detail: directory.to_owned().into(),
                    label_matches,
                    detail_matches,
                    name: path.clone().into(),
                    value: Pick::File(path.clone()),
                })
            })
            .collect();
        popup.completion.set_note(if items.is_empty() {
            Some("No matching files".into())
        } else if list.truncated {
            Some(
                format!(
                    "Large checkout: only {} files are searched",
                    list.paths.len()
                )
                .into(),
            )
        } else {
            None
        });
        popup.completion.set_items(items);
        cx.notify();
    }

    /// Accept the selected suggestion, or the one at `index`. False when no
    /// list with suggestions is open, so the key keeps its usual meaning.
    fn accept(&mut self, index: Option<usize>, window: &Window, cx: &mut Context<Self>) -> bool {
        let Some(popup) = self.popup.as_ref() else {
            return false;
        };
        let item = match index {
            Some(index) => popup.completion.item(index),
            None => popup.completion.selected_item(),
        };
        let Some(pick) = item.map(|item| item.value.clone()) else {
            return false;
        };
        let trigger = popup.trigger.clone();
        match pick {
            Pick::File(path) => {
                // Suggestions for the previous keystroke may still be on screen.
                if fuzzy_match(&trigger.query, &path).is_none() {
                    return true;
                }
                let range = trigger.start..self.draft.value.head;
                let text = trigger::mention_text(&path);
                self.close_popup();
                self.edit(|draft| draft.replace(range, &text, false), window, cx);
            }
            Pick::Command(id) => {
                self.close_popup();
                self.edit(
                    |draft| draft.replace(0..draft.value.text.len(), "", false),
                    window,
                    cx,
                );
                cx.emit(ComposerEvent::Command(id));
            }
            Pick::Insert(label) => {
                // Nothing runs: the text waits for arguments, and the runtime expands it on send.
                let text = format!("{label} ");
                self.close_popup();
                self.edit(
                    |draft| draft.replace(0..draft.value.text.len(), &text, false),
                    window,
                    cx,
                );
            }
        }
        true
    }

    fn completion_event(
        &mut self,
        event: CompletionEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            CompletionEvent::Pick(index) => {
                self.accept(Some(index), window, cx);
            }
            CompletionEvent::Scroll(distance) => {
                if let Some(popup) = &mut self.popup {
                    popup.completion.scroll_by(distance);
                    cx.notify();
                }
            }
        }
    }

    /// Escape closes the innermost thing first. Stopping a run takes a second
    /// press, so one stray key cannot cancel work.
    fn cancel(&mut self, window: &Window, cx: &mut Context<Self>) {
        // An IME owns Escape while it is composing.
        if self.draft.marked.is_some() {
            return;
        }
        if let Some(popup) = self.popup.as_ref().filter(|p| p.completion.is_visible()) {
            self.dismissed = Some(popup.key());
            self.close_popup();
            return cx.notify();
        }
        if self.enabled && self.draft.cancel_recall() {
            return self.changed(window, cx);
        }
        if !(self.stop_enabled && self.busy) {
            return cx.propagate();
        }
        if self.stop.press(Instant::now()) {
            cx.emit(ComposerEvent::Stop);
        } else {
            // Redraw when the window lapses so the hint stops offering it.
            let lapse = cx.background_executor().timer(STOP_WINDOW);
            cx.spawn(async move |this, cx| {
                lapse.await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        cx.notify();
    }

    fn suggestions(&self) -> Option<&Completion<Pick>> {
        self.popup
            .as_ref()
            .map(|popup| &popup.completion)
            .filter(|completion| completion.is_visible())
    }

    /// What the keys do while nothing is running. It is not drawn: the
    /// placeholder names the triggers, and Enter needs no label. Assistive
    /// technology still reads it with the draft.
    const IDLE_KEYS: &str = "Enter sends. Shift-Enter starts a new line. \
        @ names a file, and / runs a command.";

    /// One line inside the message box, when there is something to say: what the
    /// keys do during a run, a choice among suggestions, or why sending is
    /// off. Empty while idle.
    pub fn hint(&self) -> String {
        if let Some(error) = &self.paint_error {
            return error.clone();
        }
        if !self.enabled {
            return self
                .reason
                .clone()
                .unwrap_or_else(|| "Composer unavailable".into());
        }
        if let Some(reason) = &self.send_blocked {
            return reason.clone();
        }
        if self.suggestions().is_some_and(Completion::has_items) {
            return "↑↓ choose · Enter accepts · Esc closes".into();
        }
        if let Some((position, total)) = self.draft.recall_position() {
            return format!(
                "Sent message {position} of {total} · Down for newer · Esc returns to your draft"
            );
        }
        if !self.busy {
            return String::new();
        }
        if self.stop.armed(Instant::now()) {
            "Press Esc again to stop the agent".into()
        } else {
            if self.queue_enabled {
                "Enter queues · Option-Enter steers · Esc twice stops".into()
            } else {
                "Working · Draft your next message · Esc twice stops".into()
            }
        }
    }

    pub fn can_send(&self) -> bool {
        self.send_blocked.is_none()
            && !self.image_send_blocked()
            && (self
                .draft
                .can_submit(self.enabled, self.busy && !self.queue_enabled)
                || (self.enabled
                    && !(self.busy && !self.queue_enabled)
                    && self.draft.marked.is_none()
                    && !self.images.references.is_empty()))
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn queue_enabled(&self) -> bool {
        self.queue_enabled
    }

    pub fn can_stop(&self) -> bool {
        self.stop_enabled && self.busy
    }

    /// The box-chrome state rendered by the parent message box. Comparing this
    /// lets the parent redraw only when send/stop availability or the hint changes.
    pub fn box_state(&self) -> (bool, bool, bool, bool, bool, String) {
        (
            self.can_send(),
            self.busy,
            self.queue_enabled,
            self.can_stop(),
            self.is_blank(),
            self.hint(),
        )
    }

    pub fn submit_from_button(&mut self, cx: &mut Context<Self>) {
        self.submit(false, cx);
    }

    pub fn stop_from_button(&mut self, cx: &mut Context<Self>) {
        if self.stop_enabled {
            cx.emit(ComposerEvent::Stop);
        }
    }
}

/// Hooks for scene and layout tests, which have no keyboard.
#[cfg(test)]
impl Composer {
    /// Focus the input and type `text` through the same path as a key press.
    pub fn type_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.edit(|draft| draft.insert(text), window, cx);
    }

    /// Accessible names of the open suggestions, best first.
    pub fn suggestion_names(&self) -> Vec<String> {
        let Some(list) = self.suggestions() else {
            return Vec::new();
        };
        (0..)
            .map_while(|index| list.item(index))
            .map(|item| item.name.to_string())
            .collect()
    }

    /// Enter with a suggestion list open. True when a suggestion took the key.
    pub fn accept_suggestion(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.accept(None, window, cx)
    }

    /// The Up or Down key.
    pub fn press_arrow(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.arrow(down, window, cx);
    }

    pub fn text(&self) -> &str {
        &self.draft.value.text
    }

    /// The caret, or the selection when there is one.
    pub fn caret(&self) -> std::ops::Range<usize> {
        self.draft.selection()
    }
}

impl Focusable for Composer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EventEmitter<ComposerEvent> for Composer {}

impl EntityInputHandler for Composer {
    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        self.enabled
    }

    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.draft.byte_range(range);
        *actual = Some(self.draft.utf16_range(range.clone()));
        Some(self.draft.value.text[range].into())
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        (self.enabled || ignore_disabled).then(|| UTF16Selection {
            range: self.draft.utf16_range(self.draft.selection()),
            reversed: self.draft.value.head < self.draft.value.anchor,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.draft.byte_range(range);
        self.draft.value.anchor = range.start;
        self.draft.value.head = range.end;
        self.draft.touch();
        if self.draft.marked.is_none() {
            self.draft.normalize_selection();
        }
        self.refresh_popup(false, cx);
        self.reveal_cursor = true;
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.draft.value.text.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.draft
            .marked
            .clone()
            .map(|range| self.draft.utf16_range(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.finish_composition();
        self.refresh_popup(true, cx);
        self.changed(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(
            |draft| {
                let range = range
                    .map(|range| draft.byte_range(range))
                    .or_else(|| draft.marked.clone())
                    .unwrap_or_else(|| draft.selection());
                draft.replace(range, text, false);
            },
            window,
            cx,
        );
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(
            |draft| {
                let range = range
                    .map(|range| draft.byte_range(range))
                    .or_else(|| draft.marked.clone())
                    .unwrap_or_else(|| draft.selection());
                draft.mark(range, text, selected);
            },
            window,
            cx,
        );
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.rectangles(self.draft.byte_range(range))
            .into_iter()
            .next()
    }

    fn selection_bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.rectangles(self.draft.byte_range(range))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        if !self.layout_is_current() {
            return None;
        }
        self.bounds?;
        let offset = self.mouse_index(point);
        Some(self.draft.utf16_range(0..offset).end)
    }
}

impl Composer {
    /// The suggestion list, floating just above the composer box.
    fn suggestion_list(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let composer = cx.entity().downgrade();
        let list =
            self.suggestions()?
                .render("composer-suggestions", move |event, window, cx| {
                    let _ =
                        composer.update(cx, |this, cx| this.completion_event(event, window, cx));
                })?;
        // Conversation rows hang a few points left of the text edge. A strip of
        // the surface color keeps their ends from showing beside the list.
        Some(
            div()
                .absolute()
                .bottom_full()
                .left(px(-SUGGESTION_GUTTER))
                .w_full()
                .max_w(px(SUGGESTION_WIDTH + SUGGESTION_GUTTER))
                .pl(px(SUGGESTION_GUTTER))
                .pb(px(6.))
                .bg(rgb(colors().bg))
                .child(list),
        )
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = self.hint();
        // Focus stays in the text, so the current suggestion is part of its description.
        let description = match self
            .suggestions()
            .and_then(|list| Some((list.position()?, list.selected_item()?)))
        {
            Some(((position, total), item)) => {
                format!("Suggestion {position} of {total}: {}. {hint}", item.name)
            }
            None if hint.is_empty() => Self::IDLE_KEYS.into(),
            None => hint.clone(),
        };
        let input = cx.entity().downgrade();
        let enabled = self.enabled;
        let focused = self.focus.is_focused(window);
        // Suggestions belong to typing; they do not outlive keyboard focus.
        let suggestions = focused.then(|| self.suggestion_list(cx)).flatten();
        // The parent message box draws the raised outline and settings row;
        // this entity owns attachments, editable text, and floating suggestions.
        theme::column()
            .relative()
            .w_full()
            .min_w_0()
            .when(
                !self.images.references.is_empty()
                    || self.images.loading
                    || self.images.error.is_some(),
                |el| el.child(div().px_3().pt_2().child(self.image_strip(cx))),
            )
            .child(
                div()
                    .id("composer-input")
                    .role(Role::MultilineTextInput)
                    .aria_label("Message to agent")
                    .audited()
                    .aria_description(description)
                    .aria_value(self.draft.value.text.clone())
                    .aria_placeholder(PLACEHOLDER)
                    .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                        // A composer is never in a window modal.
                        if crate::ui::theme::beneath_modal(None, window, cx) {
                            return;
                        }
                        if let Some(gpui::accesskit::ActionData::Value(text)) = data {
                            let _ = input.update(cx, |this, cx| {
                                if this.draft.set_external_value(text, this.enabled) {
                                    this.refresh_popup(false, cx);
                                    this.changed(window, cx);
                                }
                            });
                        }
                    })
                    .a11y_synthetic_children(move |builder| {
                        if !enabled {
                            builder.parent_node().set_disabled();
                            builder.parent_node().set_read_only();
                        }
                    })
                    .track_focus(&self.focus)
                    .key_context("PlaitComposer")
                    .cursor(CursorStyle::IBeam)
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(6.))
                    .min_h(px(42.))
                    .text_color(rgb(if self.enabled {
                        colors().text
                    } else {
                        colors().muted
                    }))
                    .text_size(px(14.5))
                    .line_height(px(LINE_HEIGHT))
                    .on_action(cx.listener(|this, _: &Submit, window, cx| {
                        if !this.accept(None, window, cx) {
                            this.submit(false, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &SubmitNow, _, cx| this.submit(false, cx)))
                    .on_action(cx.listener(|this, _: &Steer, _, cx| this.submit(true, cx)))
                    .on_action(cx.listener(|this, _: &Complete, window, cx| {
                        if !this.accept(None, window, cx) {
                            window.focus_next(cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Cancel, window, cx| this.cancel(window, cx)))
                    .on_action(cx.listener(|this, _: &Newline, window, cx| {
                        if this.draft.marked.is_none() {
                            this.edit(|draft| draft.insert("\n"), window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Backspace, w, cx| {
                        this.edit(|d| d.delete(false), w, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &Delete, w, cx| this.edit(|d| d.delete(true), w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &DeleteWordLeft, w, cx| {
                            this.delete_word(false, w, cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &DeleteWordRight, w, cx| {
                            this.delete_word(true, w, cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &DeleteToLineStart, w, cx| {
                        this.delete_to_line_edge(false, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &DeleteToLineEnd, w, cx| {
                        this.delete_to_line_edge(true, w, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &Left, w, cx| this.horizontal(false, false, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &Right, w, cx| this.horizontal(true, false, w, cx)),
                    )
                    .on_action(cx.listener(|this, _: &SelectLeft, w, cx| {
                        this.horizontal(false, true, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &SelectRight, w, cx| {
                        this.horizontal(true, true, w, cx)
                    }))
                    .on_action(cx.listener(|this, _: &Up, w, cx| this.arrow(false, w, cx)))
                    .on_action(cx.listener(|this, _: &Down, w, cx| this.arrow(true, w, cx)))
                    .on_action(
                        cx.listener(|this, _: &SelectUp, w, cx| this.vertical(false, true, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectDown, w, cx| this.vertical(true, true, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &Home, w, cx| this.line_edge(false, false, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &End, w, cx| this.line_edge(true, false, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectHome, w, cx| {
                            this.line_edge(false, true, w, cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectEnd, w, cx| this.line_edge(true, true, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &DocumentStart, w, cx| this.move_to(0, false, w, cx)),
                    )
                    .on_action(cx.listener(|this, _: &DocumentEnd, w, cx| {
                        this.move_to(this.draft.value.text.len(), false, w, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &WordLeft, w, cx| this.word(false, false, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &WordRight, w, cx| this.word(true, false, w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectWordLeft, w, cx| {
                            this.word(false, true, w, cx)
                        }),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectWordRight, w, cx| {
                            this.word(true, true, w, cx)
                        }),
                    )
                    .on_action(cx.listener(|this, _: &SelectAll, w, cx| {
                        this.move_to(0, false, w, cx);
                        this.move_to(this.draft.value.text.len(), true, w, cx);
                    }))
                    .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
                    .on_action(cx.listener(|this, _: &Cut, w, cx| {
                        if this.enabled {
                            this.copy(cx);
                            this.edit(|d| d.insert(""), w, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &Paste, w, cx| this.paste_clipboard(w, cx)))
                    .on_action(
                        cx.listener(|this, _: &Undo, w, cx| this.edit(|d| d.undo(false), w, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &Redo, w, cx| this.edit(|d| d.undo(true), w, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            window.request_virtual_keyboard();
                            this.selecting = true;
                            let offset = this.mouse_index(event.position);
                            if event.click_count >= 3 {
                                this.move_to(0, false, window, cx);
                                this.move_to(this.draft.value.text.len(), true, window, cx);
                            } else if event.click_count == 2 {
                                let word = this
                                    .draft
                                    .value
                                    .text
                                    .split_word_bound_indices()
                                    .find(|(i, word)| *i <= offset && offset < *i + word.len())
                                    .map(|(i, word)| i..i + word.len())
                                    .unwrap_or(offset..offset);
                                this.move_to(word.start, false, window, cx);
                                this.move_to(word.end, true, window, cx);
                            } else {
                                this.move_to(offset, event.modifiers.shift, window, cx);
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        if this.selecting {
                            this.move_to(this.mouse_index(event.position), true, window, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.selecting = false),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.selecting = false),
                    )
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                        this.scroll -= event.delta.pixel_delta(px(LINE_HEIGHT)).y;
                        this.reveal_cursor = false;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(InputElement(cx.entity())),
            )
            .children(suggestions)
    }
}

#[cfg(test)]
mod tests {
    // Not a glob: GPUI exports its own `test` attribute.
    use super::{STOP_WINDOW, StopGesture};
    use std::time::Instant;

    #[test]
    fn one_escape_arms_and_only_a_prompt_second_one_stops() {
        let start = Instant::now();
        let mut gesture = StopGesture::default();
        assert!(!gesture.armed(start));
        assert!(!gesture.press(start));
        assert!(gesture.armed(start + STOP_WINDOW / 2));
        assert!(gesture.press(start + STOP_WINDOW / 2));
        // A confirmed stop starts over; a third press alone does nothing.
        assert!(!gesture.armed(start + STOP_WINDOW / 2));
        assert!(!gesture.press(start + STOP_WINDOW / 2));
    }

    #[test]
    fn a_late_or_interrupted_second_escape_only_arms_again() {
        let start = Instant::now();
        let mut gesture = StopGesture::default();
        assert!(!gesture.press(start));
        assert!(!gesture.armed(start + STOP_WINDOW));
        assert!(!gesture.press(start + STOP_WINDOW));
        assert!(gesture.armed(start + STOP_WINDOW + STOP_WINDOW / 2));
        // Typing or moving the cursor in between cancels the gesture.
        gesture.disarm();
        assert!(!gesture.press(start + STOP_WINDOW + STOP_WINDOW / 2));
        assert!(gesture.press(start + STOP_WINDOW + STOP_WINDOW / 2));
    }
}
