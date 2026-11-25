//! Single-line native text input: IME, selection, clipboard, and undo.
//!
//! The field owns its text. A parent subscribes to [`TextFieldEvent`] and binds
//! Enter, Escape, and the vertical arrows in its own key context; the field
//! leaves those keys unbound so they reach the parent.

use super::theme::{AuditA11y, ROW_HEIGHT, colors, tag};
use gpui::{prelude::*, *};
use std::{borrow::Cow, ops::Range};
use unicode_segmentation::UnicodeSegmentation;

const LINE_HEIGHT: f32 = 20.;
const HISTORY_LIMIT: usize = 100;

actions!(
    text_field,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToStart,
        DeleteToEnd,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Start,
        End,
        SelectStart,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
    ]
);

pub(super) fn bind_keys(cx: &mut App) {
    let context = Some("PlaitTextField");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("alt-backspace", DeleteWordLeft, context),
        KeyBinding::new("alt-delete", DeleteWordRight, context),
        KeyBinding::new("cmd-backspace", DeleteToStart, context),
        KeyBinding::new("cmd-delete", DeleteToEnd, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("alt-left", WordLeft, context),
        KeyBinding::new("alt-right", WordRight, context),
        KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SelectWordRight, context),
        KeyBinding::new("cmd-left", Start, context),
        KeyBinding::new("cmd-right", End, context),
        KeyBinding::new("home", Start, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("cmd-shift-left", SelectStart, context),
        KeyBinding::new("cmd-shift-right", SelectEnd, context),
        KeyBinding::new("shift-home", SelectStart, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
    ]);
}

/// Pasted or dictated text can span lines; the field holds one.
pub(super) fn single_line(text: &str) -> Cow<'_, str> {
    let breaks = |ch: char| matches!(ch, '\n' | '\r' | '\t' | '\u{2028}' | '\u{2029}');
    if !text.contains(breaks) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", " ").replace(breaks, " "))
}

// Native text services count UTF-16 units; the line stores UTF-8.
fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units + ch.len_utf16() > utf16 {
            return offset;
        }
        units += ch.len_utf16();
    }
    text.len()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Snapshot {
    text: String,
    anchor: usize,
    head: usize,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        plait_client::host::providers::wipe(&mut self.text);
    }
}

/// Editing state of one line. Offsets are UTF-8 bytes on grapheme boundaries.
/// Plain data, so the editing rules are tested without a window.
#[derive(Default)]
pub(super) struct Line {
    sensitive: bool,
    value: Snapshot,
    /// Text an input method is still composing.
    marked: Option<Range<usize>>,
    composition_start: Option<Snapshot>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Counts text changes, so layout and listeners can tell text from selection.
    revision: u64,
    /// The last edit was typing at the cursor; a run of typing undoes as one step.
    typing: bool,
}

impl Line {
    pub fn text(&self) -> &str {
        &self.value.text
    }

    pub fn selection(&self) -> Range<usize> {
        self.value.anchor.min(self.value.head)..self.value.anchor.max(self.value.head)
    }

    pub fn selected_text(&self) -> &str {
        &self.value.text[self.selection()]
    }

    pub fn is_composing(&self) -> bool {
        self.marked.is_some()
    }

    fn boundaries(&self) -> impl Iterator<Item = usize> + '_ {
        self.value.text.grapheme_indices(true).map(|(i, _)| i)
    }

    /// The grapheme boundary after `offset`, or before it.
    fn boundary(&self, offset: usize, forward: bool) -> usize {
        if forward {
            self.boundaries()
                .find(|i| *i > offset)
                .unwrap_or(self.value.text.len())
        } else {
            self.boundaries()
                .take_while(|i| *i < offset)
                .last()
                .unwrap_or(0)
        }
    }

    /// The grapheme boundary at or before `offset`.
    fn snap(&self, offset: usize) -> usize {
        if offset >= self.value.text.len() {
            return self.value.text.len();
        }
        self.boundaries()
            .take_while(|i| *i <= offset)
            .last()
            .unwrap_or(0)
    }

    /// The grapheme boundary at or after `offset`.
    fn snap_up(&self, offset: usize) -> usize {
        self.boundaries()
            .find(|i| *i >= offset)
            .unwrap_or(self.value.text.len())
    }

    /// Start of the previous word or end of the next, skipping spaces and punctuation.
    fn word_boundary(&self, offset: usize, forward: bool) -> usize {
        let words = self.value.text.unicode_word_indices();
        if forward {
            words
                .map(|(i, word)| i + word.len())
                .find(|i| *i > offset)
                .unwrap_or(self.value.text.len())
        } else {
            words
                .map(|(i, _)| i)
                .take_while(|i| *i < offset)
                .last()
                .unwrap_or(0)
        }
    }

    /// An edit can leave an end of the selection inside a grapheme.
    fn normalize(&mut self) {
        let range = self.selection();
        let start = if range.is_empty() {
            self.snap_up(range.start)
        } else {
            self.snap(range.start)
        };
        let end = self.snap_up(range.end);
        if self.value.head < self.value.anchor {
            (self.value.head, self.value.anchor) = (start, end);
        } else {
            (self.value.anchor, self.value.head) = (start, end);
        }
    }

    pub fn move_to(&mut self, offset: usize, extend: bool) {
        self.finish_composition();
        self.typing = false;
        self.value.head = self.snap(offset);
        if !extend {
            self.value.anchor = self.value.head;
        }
    }

    /// One grapheme left or right. Without Shift, a selection collapses to its edge.
    pub fn step(&mut self, forward: bool, extend: bool) {
        let selection = self.selection();
        let target = if extend || selection.is_empty() {
            self.boundary(self.value.head, forward)
        } else if forward {
            selection.end
        } else {
            selection.start
        };
        self.move_to(target, extend);
    }

    pub fn word_step(&mut self, forward: bool, extend: bool) {
        let target = self.word_boundary(self.value.head, forward);
        self.move_to(target, extend);
    }

    pub fn select_all(&mut self) {
        self.move_to(0, false);
        self.move_to(self.value.text.len(), true);
    }

    pub fn select_word_at(&mut self, offset: usize) {
        let word = self
            .value
            .text
            .split_word_bound_indices()
            .find(|(i, word)| *i <= offset && offset < *i + word.len())
            .map(|(i, word)| i..i + word.len())
            .unwrap_or(offset..offset);
        self.move_to(word.start, false);
        self.move_to(word.end, true);
    }

    fn remember(&mut self, before: Snapshot, typed: bool, starts_word: bool) {
        if self.sensitive || before.text == self.value.text {
            return;
        }
        let continues = typed && self.typing && !starts_word;
        self.typing = typed;
        self.redo.clear();
        // The snapshot taken when the run began already restores the text before it.
        if continues {
            return;
        }
        if self.undo.len() == HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(before);
    }

    pub fn finish_composition(&mut self) {
        self.marked = None;
        self.normalize();
        if let Some(before) = self.composition_start.take() {
            self.remember(before, false, false);
        }
    }

    fn replace(&mut self, range: Range<usize>, text: &str, composing: bool) {
        let text = single_line(text);
        let before = self.value.clone();
        if composing && self.composition_start.is_none() {
            self.composition_start = Some(before.clone());
        }
        self.value.text.replace_range(range.clone(), &text);
        self.value.head = range.start + text.len();
        self.value.anchor = self.value.head;
        if before.text != self.value.text {
            self.revision += 1;
        }
        if composing {
            self.marked = Some(range.start..self.value.head);
        } else if self.composition_start.is_some() {
            self.finish_composition();
        } else {
            self.marked = None;
            self.normalize();
            let typed = range.is_empty() && !text.is_empty();
            self.remember(before, typed, text.starts_with(char::is_whitespace));
        }
    }

    /// Replace the composition or selection, as typing and pasting do.
    pub fn insert(&mut self, text: &str) {
        let range = self.marked.clone().unwrap_or_else(|| self.selection());
        self.replace(range, text, false);
    }

    /// Composition text from an input method. `selected` counts UTF-16 units in `text`.
    fn mark(&mut self, range: Range<usize>, text: &str, selected: Option<Range<usize>>) {
        let start = range.start;
        self.replace(range, text, true);
        if let Some(selected) = selected {
            let marked = &self.value.text[start..self.value.head];
            self.value.anchor = start + byte_offset(marked, selected.start);
            self.value.head = start + byte_offset(marked, selected.end);
        }
    }

    /// Delete the selection, or from the cursor to `target`. A target at the
    /// cursor deletes one grapheme, as native fields do at a line edge.
    fn delete_to(&mut self, target: usize, forward: bool) {
        self.finish_composition();
        let selection = self.selection();
        if !selection.is_empty() {
            return self.replace(selection, "", false);
        }
        let head = self.value.head;
        let range = match target.cmp(&head) {
            std::cmp::Ordering::Less => self.snap(target)..head,
            std::cmp::Ordering::Greater => head..self.snap_up(target),
            std::cmp::Ordering::Equal if forward => head..self.boundary(head, true),
            std::cmp::Ordering::Equal => self.boundary(head, false)..head,
        };
        self.replace(range, "", false);
    }

    pub fn delete(&mut self, forward: bool) {
        self.delete_to(self.value.head, forward);
    }

    pub fn delete_word(&mut self, forward: bool) {
        self.delete_to(self.word_boundary(self.value.head, forward), forward);
    }

    pub fn delete_to_edge(&mut self, end: bool) {
        self.delete_to(if end { self.value.text.len() } else { 0 }, end);
    }

    pub fn undo(&mut self, redo: bool) {
        self.finish_composition();
        self.typing = false;
        let history = if redo { &mut self.redo } else { &mut self.undo };
        let Some(value) = history.pop() else {
            return;
        };
        let previous = std::mem::replace(&mut self.value, value);
        if previous.text != self.value.text {
            self.revision += 1;
        }
        if redo {
            self.undo.push(previous);
        } else {
            self.redo.push(previous);
        }
    }

    /// Replace everything, as one undoable edit. Used by callers and assistive technology.
    pub fn set_text(&mut self, text: &str) {
        self.finish_composition();
        self.typing = false;
        self.replace(0..self.value.text.len(), text, false);
        self.typing = false;
    }

    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.value.text[..range.start].encode_utf16().count()
            ..self.value.text[..range.end].encode_utf16().count()
    }

    fn byte_range(&self, range: Range<usize>) -> Range<usize> {
        let start = byte_offset(&self.value.text, range.start);
        start..byte_offset(&self.value.text, range.end).max(start)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextFieldEvent {
    /// The text changed. Selection changes alone do not emit.
    Changed,
}

pub(super) struct TextField {
    focus: FocusHandle,
    line: Line,
    name: SharedString,
    placeholder: SharedString,
    role: Role,
    secret: bool,
    revealed: bool,
    shaped: Option<ShapedLine>,
    /// The line revision `shaped` was built from; older geometry is not trusted.
    shaped_revision: u64,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    selecting: bool,
    /// Text changed outside an input event; tell the input method at the next paint.
    input_changed: bool,
    paint_error: Option<SharedString>,
}

impl TextField {
    /// `name` is the accessible name; `placeholder` shows while the field is empty.
    pub fn new(
        name: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            // GPUI uses a tracked handle's tab settings instead of the div's.
            focus: cx.focus_handle().tab_stop(true),
            line: Line::default(),
            name: name.into(),
            placeholder: placeholder.into(),
            role: Role::TextInput,
            secret: false,
            revealed: false,
            shaped: None,
            shaped_revision: 0,
            bounds: None,
            scroll: px(0.),
            selecting: false,
            input_changed: false,
            paint_error: None,
        }
    }

    /// Secret fields never expose a value to accessibility or clipboard, and have no undo.
    pub fn secret(mut self) -> Self {
        self.secret = true;
        self.line.sensitive = true;
        self
    }

    pub fn reveal(&mut self, cx: &mut Context<Self>) {
        self.revealed = !self.revealed;
        self.shaped = None;
        cx.notify();
    }

    pub fn revealed(&self) -> bool {
        self.revealed
    }

    pub fn take_secret(&mut self, cx: &mut Context<Self>) -> plait_client::host::providers::Secret {
        let text = std::mem::take(&mut self.line.value.text);
        self.line = Line {
            sensitive: self.secret,
            ..Default::default()
        };
        self.shaped = None;
        self.scroll = px(0.);
        self.input_changed = true;
        cx.notify();
        plait_client::host::providers::Secret::new(text)
    }

    fn display_offset(&self, offset: usize) -> usize {
        if self.secret && !self.revealed {
            self.line.text()[..offset].graphemes(true).count() * "•".len()
        } else {
            offset
        }
    }

    /// Announce the field as a search box.
    pub fn search(mut self) -> Self {
        self.role = Role::SearchInput;
        self
    }

    pub fn text(&self) -> &str {
        self.line.text()
    }

    pub(super) fn revision(&self) -> u64 {
        self.line.revision
    }

    /// An input method is mid-composition; Enter belongs to it, not to the parent.
    pub fn is_composing(&self) -> bool {
        self.line.is_composing()
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let before = self.line.revision;
        self.line.set_text(text);
        self.input_changed = true;
        self.report(before, cx);
    }

    fn report(&mut self, before: u64, cx: &mut Context<Self>) {
        if self.line.revision != before {
            cx.emit(TextFieldEvent::Changed);
        }
        cx.notify();
    }

    fn edit(&mut self, operation: impl FnOnce(&mut Line), window: &Window, cx: &mut Context<Self>) {
        let before = self.line.revision;
        operation(&mut self.line);
        window.notify_text_input_changed(&self.focus);
        self.report(before, cx);
    }

    fn select(
        &mut self,
        operation: impl FnOnce(&mut Line),
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        operation(&mut self.line);
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn copy(&self, cx: &mut App) {
        if self.secret {
            return;
        }
        let selected = self.line.selected_text();
        if !selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.into()));
        }
    }

    fn paste(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(|line| line.insert(&text), window, cx);
        }
    }

    /// Geometry is valid only for the text it was shaped from.
    fn layout(&self) -> Option<(&ShapedLine, Bounds<Pixels>)> {
        if self.shaped_revision != self.line.revision {
            return None;
        }
        Some((self.shaped.as_ref()?, self.bounds?))
    }

    fn x_for(&self, line: &ShapedLine, offset: usize) -> Pixels {
        // While empty, the shaped line is the placeholder.
        if self.line.text().is_empty() {
            px(0.)
        } else {
            line.x_for_index(self.display_offset(offset))
        }
    }

    fn rectangle(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        let (line, bounds) = self.layout()?;
        let left = self.x_for(line, range.start);
        let width = (self.x_for(line, range.end) - left).max(px(1.));
        Some(Bounds::new(
            point(bounds.left() + left - self.scroll, bounds.top()),
            size(width, px(LINE_HEIGHT)),
        ))
    }

    fn index_at(&self, position: Point<Pixels>) -> usize {
        let Some((line, bounds)) = self.layout() else {
            return self.line.text().len();
        };
        if self.line.text().is_empty() {
            return 0;
        }
        let index = line.closest_index_for_x(position.x - bounds.left() + self.scroll);
        if self.secret && !self.revealed {
            self.line
                .text()
                .grapheme_indices(true)
                .nth(index / "•".len())
                .map_or(self.line.text().len(), |(offset, _)| offset)
        } else {
            self.line.snap(index)
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        window.request_virtual_keyboard();
        self.selecting = true;
        let offset = self.index_at(event.position);
        let extend = event.modifiers.shift;
        self.select(
            |line| match event.click_count {
                1 => line.move_to(offset, extend),
                2 => line.select_word_at(offset),
                _ => line.select_all(),
            },
            window,
            cx,
        );
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.line.byte_range(range);
        *actual = Some(self.line.utf16_range(range.clone()));
        Some(if self.secret {
            "•".repeat(self.line.text()[range].encode_utf16().count())
        } else {
            self.line.text()[range].into()
        })
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.line.utf16_range(self.line.selection()),
            reversed: self.line.value.head < self.line.value.anchor,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.line.byte_range(range);
        self.select(
            |line| {
                line.typing = false;
                (line.value.anchor, line.value.head) = (range.start, range.end);
                if line.marked.is_none() {
                    line.normalize();
                }
            },
            window,
            cx,
        );
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.line.text().encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.line
            .marked
            .clone()
            .map(|range| self.line.utf16_range(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(Line::finish_composition, window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(
            |line| {
                let range = range
                    .map(|range| line.byte_range(range))
                    .or_else(|| line.marked.clone())
                    .unwrap_or_else(|| line.selection());
                line.replace(range, text, false);
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
            |line| {
                let range = range
                    .map(|range| line.byte_range(range))
                    .or_else(|| line.marked.clone())
                    .unwrap_or_else(|| line.selection());
                line.mark(range, text, selected);
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
        self.rectangle(self.line.byte_range(range))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.layout()?;
        Some(self.line.utf16_range(0..self.index_at(point)).end)
    }
}

/// Shapes and paints the line, and registers the field with the input method.
struct LineElement(Entity<TextField>);

impl Element for LineElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(LINE_HEIGHT).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        self.0.update(cx, |field, _| {
            let empty = field.line.text().is_empty();
            let text: SharedString = if empty {
                field.placeholder.clone()
            } else if field.secret && !field.revealed {
                "•".repeat(field.line.text().graphemes(true).count()).into()
            } else {
                field.line.text().to_owned().into()
            };
            let run = TextRun {
                len: text.len(),
                font: style.font(),
                color: if empty {
                    rgb(colors().subtle).into()
                } else {
                    style.color
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window
                .text_system()
                .shape_line(text, font_size, &[run], None);
            // Keep the cursor in view; long text scrolls sideways.
            let cursor = if empty {
                px(0.)
            } else {
                field.x_for(&line, field.line.value.head)
            };
            let width = bounds.size.width;
            if cursor < field.scroll {
                field.scroll = cursor;
            }
            if cursor > field.scroll + width - px(1.) {
                field.scroll = cursor - width + px(1.);
            }
            let overflow = if empty {
                px(0.)
            } else {
                (line.width + px(1.) - width).max(px(0.))
            };
            field.scroll = field.scroll.clamp(px(0.), overflow);
            field.shaped = Some(line);
            field.shaped_revision = field.line.revision;
            field.bounds = Some(bounds);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.0.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.0.clone()), cx);
        self.0.update(cx, |field, cx| {
            if std::mem::take(&mut field.input_changed) {
                window.notify_text_input_changed(&field.focus);
            }
            let selection = field.line.selection();
            let mut failure = None;
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                if !selection.is_empty()
                    && let Some(rectangle) = field.rectangle(selection.clone())
                {
                    window.paint_quad(fill(rectangle, rgba(colors().text_selection)));
                }
                if let Some(line) = &field.shaped {
                    let origin = point(bounds.left() - field.scroll, bounds.top());
                    if let Err(error) =
                        line.paint(origin, px(LINE_HEIGHT), TextAlign::Left, None, window, cx)
                    {
                        failure = Some(format!("Cannot display text: {error}").into());
                    }
                }
                if let Some(marked) = field.line.marked.clone()
                    && let Some(rectangle) = field.rectangle(marked)
                {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(rectangle.left(), rectangle.bottom() - px(2.)),
                            size(rectangle.size.width, px(1.)),
                        ),
                        rgb(colors().text),
                    ));
                }
                if selection.is_empty()
                    && field.focus.is_focused(window)
                    && let Some(cursor) = field.rectangle(selection)
                {
                    window.paint_quad(fill(cursor, rgb(colors().blue)));
                }
            });
            if field.paint_error != failure {
                field.paint_error = failure;
                cx.notify();
            }
        });
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let field = cx.entity().downgrade();
        let own_focus = self.focus.clone();
        div()
            .id(("text-field", cx.entity_id()))
            .role(self.role)
            .aria_label(self.name.clone())
            .audited()
            .aria_value(if self.secret {
                String::new()
            } else {
                self.line.text().to_owned()
            })
            .aria_placeholder(self.placeholder.clone())
            .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                if super::theme::beneath_modal(Some(&own_focus), window, cx) {
                    return;
                }
                if let Some(gpui::accesskit::ActionData::Value(text)) = data {
                    let _ = field.update(cx, |this, cx| this.set_text(text, cx));
                }
            })
            .track_focus(&self.focus)
            .key_context("PlaitTextField")
            .cursor(CursorStyle::IBeam)
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .min_w_0()
            .h(px(ROW_HEIGHT))
            .px_2()
            .rounded_md()
            .border_1()
            // A field shows focus however it was reached; the cursor alone is too quiet.
            .border_color(rgb(if focused {
                colors().focus_edge
            } else {
                colors().edge
            }))
            .bg(rgb(colors().bg))
            .text_size(px(13.))
            .text_color(rgb(colors().text))
            .on_action(
                cx.listener(|this, _: &Backspace, w, cx| {
                    this.edit(|line| line.delete(false), w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &Delete, w, cx| this.edit(|line| line.delete(true), w, cx)),
            )
            .on_action(cx.listener(|this, _: &DeleteWordLeft, w, cx| {
                this.edit(|line| line.delete_word(false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteWordRight, w, cx| {
                this.edit(|line| line.delete_word(true), w, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteToStart, w, cx| {
                this.edit(|line| line.delete_to_edge(false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteToEnd, w, cx| {
                this.edit(|line| line.delete_to_edge(true), w, cx)
            }))
            .on_action(cx.listener(|this, _: &Left, w, cx| {
                this.select(|line| line.step(false, false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &Right, w, cx| {
                this.select(|line| line.step(true, false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, w, cx| {
                this.select(|line| line.step(false, true), w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectRight, w, cx| {
                this.select(|line| line.step(true, true), w, cx)
            }))
            .on_action(cx.listener(|this, _: &WordLeft, w, cx| {
                this.select(|line| line.word_step(false, false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &WordRight, w, cx| {
                this.select(|line| line.word_step(true, false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWordLeft, w, cx| {
                this.select(|line| line.word_step(false, true), w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWordRight, w, cx| {
                this.select(|line| line.word_step(true, true), w, cx)
            }))
            .on_action(cx.listener(|this, _: &Start, w, cx| {
                this.select(|line| line.move_to(0, false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &End, w, cx| {
                this.select(|line| line.move_to(usize::MAX, false), w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectStart, w, cx| {
                this.select(|line| line.move_to(0, true), w, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectEnd, w, cx| {
                this.select(|line| line.move_to(usize::MAX, true), w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectAll, w, cx| this.select(Line::select_all, w, cx)),
            )
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Cut, w, cx| {
                this.copy(cx);
                this.edit(|line| line.insert(""), w, cx);
            }))
            .on_action(cx.listener(|this, _: &Paste, w, cx| this.paste(w, cx)))
            .on_action(
                cx.listener(|this, _: &Undo, w, cx| this.edit(|line| line.undo(false), w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Redo, w, cx| this.edit(|line| line.undo(true), w, cx)),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.selecting {
                    let offset = this.index_at(event.position);
                    this.select(|line| line.move_to(offset, true), window, cx);
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
            .child(div().flex_1().min_w_0().child(LineElement(cx.entity())))
            .when_some(self.paint_error.clone(), |el, error| {
                el.child(tag("text-field-error", error, colors().red).flex_shrink_0())
            })
    }
}

#[cfg(test)]
mod tests {
    // Named imports: a glob would bring in GPUI's own `test` attribute.
    use super::{HISTORY_LIMIT, Line, single_line};
    use std::borrow::Cow;

    fn typed(text: &str) -> Line {
        let mut line = Line::default();
        for ch in text.chars() {
            line.insert(&ch.to_string());
        }
        line
    }

    fn state(line: &Line) -> (&str, usize, usize) {
        (line.text(), line.value.anchor, line.value.head)
    }

    #[test]
    fn secret_edits_never_build_undo_history() {
        let mut line = Line {
            sensitive: true,
            ..Default::default()
        };
        line.insert("sample secret");
        line.select_all();
        line.insert("replacement");
        line.undo(false);
        assert_eq!(line.text(), "replacement");
        assert!(line.undo.is_empty() && line.redo.is_empty());
    }

    #[test]
    fn typing_inserts_at_the_cursor_and_replaces_the_selection() {
        let mut line = typed("helo");
        line.move_to(3, false);
        line.insert("l");
        assert_eq!(state(&line), ("hello", 4, 4));
        line.select_all();
        assert_eq!(line.selected_text(), "hello");
        line.insert("x");
        assert_eq!(state(&line), ("x", 1, 1));
        assert_eq!(line.revision, 6);
    }

    #[test]
    fn pasted_line_breaks_become_spaces() {
        let mut line = Line::default();
        line.insert("one\r\ntwo\nthree\tfour\u{2028}five");
        assert_eq!(line.text(), "one two three four five");
        assert_eq!(single_line("plain"), Cow::Borrowed("plain"));
    }

    #[test]
    fn arrows_move_by_grapheme_and_collapse_a_selection_to_its_edge() {
        // A family emoji and a combining accent are one step each.
        let mut line = typed("a");
        line.insert("👨‍👩‍👧");
        line.insert("e\u{301}");
        line.insert("z");
        let end = line.text().len();
        line.step(false, false);
        assert_eq!(line.value.head, end - 1);
        line.step(false, false);
        assert_eq!(line.value.head, end - 4);
        line.step(false, false);
        assert_eq!(line.value.head, 1);
        line.step(false, false);
        line.step(false, false);
        assert_eq!(line.value.head, 0);
        // Shift extends; a plain arrow then collapses to the edge it points at.
        line.step(true, true);
        line.step(true, true);
        assert_eq!(line.selection(), 0..end - 4);
        line.step(false, false);
        assert_eq!(state(&line).1, 0);
        line.select_all();
        line.step(true, false);
        assert_eq!((line.value.anchor, line.value.head), (end, end));
        // Offsets past the end, or inside a grapheme, snap to a boundary.
        line.move_to(usize::MAX, false);
        assert_eq!(line.value.head, end);
        line.move_to(3, false);
        assert_eq!(line.value.head, 1);
    }

    #[test]
    fn deleting_removes_the_selection_a_grapheme_a_word_or_to_an_edge() {
        let mut line = typed("alpha beta gamma");
        line.delete(false);
        assert_eq!(line.text(), "alpha beta gamm");
        line.delete_word(false);
        assert_eq!(line.text(), "alpha beta ");
        line.move_to(0, false);
        line.delete(true);
        assert_eq!(line.text(), "lpha beta ");
        line.delete_word(true);
        assert_eq!(line.text(), " beta ");
        // Nothing to delete at an edge.
        line.delete(false);
        assert_eq!(line.text(), " beta ");
        line.move_to(3, false);
        line.delete_to_edge(true);
        assert_eq!(line.text(), " be");
        line.delete_to_edge(false);
        assert_eq!(line.text(), "");
        let mut line = typed("keep this");
        line.move_to(4, false);
        line.move_to(9, true);
        line.delete_word(false);
        assert_eq!(state(&line), ("keep", 4, 4));
        let mut line = typed("e\u{301}");
        line.delete(false);
        assert_eq!(line.text(), "");
    }

    #[test]
    fn word_movement_and_double_click_selection() {
        let mut line = typed("open the palette");
        line.word_step(false, false);
        assert_eq!(line.value.head, 9);
        line.word_step(false, true);
        assert_eq!(line.selected_text(), "the ");
        line.word_step(true, false);
        assert_eq!(line.value.head, 8);
        line.select_word_at(6);
        assert_eq!(line.selected_text(), "the");
        line.select_word_at(line.text().len());
        assert_eq!(line.selected_text(), "");
    }

    #[test]
    fn a_run_of_typing_undoes_as_one_step_per_word() {
        let mut line = typed("new thread");
        line.undo(false);
        assert_eq!(line.text(), "new");
        line.undo(false);
        assert_eq!(line.text(), "");
        line.undo(false);
        assert_eq!(line.text(), "");
        line.undo(true);
        line.undo(true);
        assert_eq!(state(&line), ("new thread", 10, 10));
        // Moving the cursor ends the run.
        line.move_to(3, false);
        line.insert("s");
        line.insert("y");
        line.undo(false);
        assert_eq!(line.text(), "new thread");
        // A new edit discards the redo history.
        line.insert("!");
        line.undo(true);
        assert_eq!(line.text(), "new! thread");
        line.delete(false);
        line.undo(false);
        assert_eq!(state(&line), ("new! thread", 4, 4));
    }

    #[test]
    fn history_is_bounded() {
        let mut line = Line::default();
        for index in 0..HISTORY_LIMIT + 20 {
            line.set_text(&index.to_string());
        }
        assert_eq!(line.undo.len(), HISTORY_LIMIT);
        for _ in 0..HISTORY_LIMIT + 20 {
            line.undo(false);
        }
        assert_eq!(line.text(), "19");
    }

    #[test]
    fn composition_is_one_undo_step_and_reports_utf16_ranges() {
        let mut line = typed("a😀");
        // An input method marks text, revises it, then commits.
        line.mark(line.selection(), "ni", Some(2..2));
        assert!(line.is_composing());
        assert_eq!(line.marked, Some(5..7));
        assert_eq!(line.utf16_range(5..7), 3..5);
        let marked = line.marked.clone().unwrap();
        line.mark(marked, "你", Some(0..1));
        assert_eq!(line.selected_text(), "你");
        line.insert("你好");
        assert!(!line.is_composing());
        assert_eq!(state(&line), ("a😀你好", 11, 11));
        line.undo(false);
        assert_eq!(line.text(), "a😀");
        // Moving the cursor commits the composition where it stands.
        line.mark(line.selection(), "x", None);
        line.move_to(0, false);
        assert!(!line.is_composing());
        assert_eq!(line.text(), "a😀x");
        // UTF-16 offsets from text services map back to byte boundaries.
        assert_eq!(line.byte_range(1..3), 1..5);
        assert_eq!(line.byte_range(2..2), 1..1);
        assert_eq!(line.byte_range(9..99), 6..6);
        assert_eq!(line.utf16_range(0..line.text().len()), 0..4);
    }

    #[test]
    fn set_text_replaces_everything_and_can_be_undone() {
        let mut line = typed("draft");
        line.mark(line.selection(), "x", None);
        line.set_text("two\nlines");
        assert_eq!(state(&line), ("two lines", 9, 9));
        assert!(!line.is_composing());
        line.undo(false);
        assert_eq!(line.text(), "draftx");
        let revision = line.revision;
        line.set_text("draftx");
        assert_eq!(line.revision, revision);
    }
}
