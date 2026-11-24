//! Editable draft: text, selection, undo, IME composition, the acceptance
//! guard, recall of sent prompts, and appending quotes from a reply.

use super::history::PromptHistory;
use gpui::{Context, Window};
use std::{ops::Range, sync::Arc};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

// shortcut: keep 100 full drafts; use edit deltas if large drafts become common.
const UNDO_LIMIT: usize = 100;
/// Full-draft undo steps for a large paste would otherwise hold 100 copies of it.
const UNDO_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Snapshot {
    pub text: String,
    pub anchor: usize,
    pub head: usize,
}

/// A sent prompt shown in place of the draft while the user walks history.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Recall {
    /// History as it was when recall began, oldest first; later sends cannot shift it.
    #[serde(with = "super::parking::strings")]
    entries: Arc<[String]>,
    index: usize,
    /// The draft being written, returned unchanged when recall ends.
    stash: Snapshot,
    /// The cursor has not moved since this prompt was shown, so Up and Down
    /// keep walking even inside a multi-line prompt.
    untouched: bool,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Draft {
    pub value: Snapshot,
    pub marked: Option<Range<usize>>,
    composition_start: Option<Snapshot>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    pub revision: u64,
    pub submitted: Option<(String, u64)>,
    pub history: PromptHistory,
    recall: Option<Recall>,
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

impl Draft {
    pub(super) fn retained_bytes(&self) -> usize {
        let snapshot = |s: &Snapshot| s.text.capacity();
        std::mem::size_of::<Self>()
            + snapshot(&self.value)
            + self.undo.capacity() * std::mem::size_of::<Snapshot>()
            + self.redo.capacity() * std::mem::size_of::<Snapshot>()
            + self
                .undo
                .iter()
                .chain(&self.redo)
                .map(snapshot)
                .sum::<usize>()
            + self.composition_start.as_ref().map_or(0, snapshot)
            + self.submitted.as_ref().map_or(0, |(s, _)| s.capacity())
            + self.history.retained_bytes()
            + self.recall.as_ref().map_or(0, |r| {
                snapshot(&r.stash)
                    + r.entries.len() * std::mem::size_of::<String>()
                    + r.entries.iter().map(String::capacity).sum::<usize>()
            })
    }
    pub fn restored(
        text: String,
        revision: u64,
        submitted: Option<(String, u64)>,
        history: PromptHistory,
    ) -> Self {
        Self {
            value: Snapshot {
                anchor: text.len(),
                head: text.len(),
                text,
            },
            revision,
            submitted,
            history,
            ..Self::default()
        }
    }

    pub fn selection(&self) -> Range<usize> {
        self.value.anchor.min(self.value.head)..self.value.anchor.max(self.value.head)
    }

    /// Whether `offset` starts a grapheme. Looks only at nearby text, so
    /// cursor movement stays fast in a large draft.
    fn is_boundary(&self, offset: usize) -> bool {
        let text = &self.value.text;
        if !text.is_char_boundary(offset) {
            return false;
        }
        GraphemeCursor::new(offset, text.len(), true)
            .is_boundary(text, 0)
            .unwrap_or_else(|_| text.grapheme_indices(true).any(|(i, _)| i == offset))
    }

    /// The nearest grapheme boundary strictly after or before `offset`.
    pub fn boundary(&self, offset: usize, forward: bool) -> usize {
        let text = &self.value.text;
        let requested = offset.min(text.len());
        let offset = floor_char_boundary(text, requested);
        // An offset inside a character still counts the character's own start as before it.
        if !forward && offset < requested && self.is_boundary(offset) {
            return offset;
        }
        let mut cursor = GraphemeCursor::new(offset, text.len(), true);
        let found = if forward {
            cursor.next_boundary(text, 0)
        } else {
            cursor.prev_boundary(text, 0)
        };
        match found {
            Ok(Some(boundary)) => boundary,
            Ok(None) if forward => text.len(),
            Ok(None) => 0,
            // The whole draft is one chunk, so this is unreachable; scan to stay correct.
            Err(_) => {
                let mut indices = text.grapheme_indices(true).map(|(i, _)| i);
                if forward {
                    indices.find(|i| *i > offset).unwrap_or(text.len())
                } else {
                    indices.take_while(|i| *i < offset).last().unwrap_or(0)
                }
            }
        }
    }

    pub fn snap(&self, offset: usize) -> usize {
        if offset >= self.value.text.len() {
            self.value.text.len()
        } else if self.is_boundary(offset) {
            offset
        } else {
            self.boundary(offset, false)
        }
    }

    pub fn snap_end(&self, offset: usize) -> usize {
        if offset >= self.value.text.len() {
            self.value.text.len()
        } else if self.is_boundary(offset) {
            offset
        } else {
            self.boundary(offset, true)
        }
    }

    pub fn normalize_selection(&mut self) {
        let range = self.selection();
        let start = if range.is_empty() {
            self.snap_end(range.start)
        } else {
            self.snap(range.start)
        };
        let end = self.snap_end(range.end);
        if self.value.head < self.value.anchor {
            self.value.head = start;
            self.value.anchor = end;
        } else {
            self.value.anchor = start;
            self.value.head = end;
        }
    }

    /// The cursor or selection moved by hand: arrow keys now move inside a recalled prompt.
    pub fn touch(&mut self) {
        if let Some(recall) = &mut self.recall {
            recall.untouched = false;
        }
    }

    pub fn select(&mut self, offset: usize, extend: bool) {
        self.value.head = self.snap(offset);
        if !extend {
            self.value.anchor = self.value.head;
        }
        self.touch();
    }

    fn remember(&mut self, before: Snapshot) {
        if before.text == self.value.text {
            return;
        }
        self.undo.push(before);
        self.redo.clear();
        let mut bytes: usize = self.undo.iter().map(|step| step.text.capacity()).sum();
        while self.undo.len() > UNDO_LIMIT || (bytes > UNDO_BYTES && self.undo.len() > 1) {
            bytes -= self.undo.remove(0).text.capacity();
        }
    }

    pub fn finish_composition(&mut self) {
        self.marked = None;
        self.normalize_selection();
        if let Some(before) = self.composition_start.take() {
            self.remember(before);
        }
    }

    pub fn replace(&mut self, range: Range<usize>, text: &str, composing: bool) {
        self.adopt_recalled();
        let before = self.value.clone();
        if composing && self.composition_start.is_none() {
            self.composition_start = Some(before.clone());
        }
        self.value.text.replace_range(range.clone(), text);
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
            self.normalize_selection();
            self.remember(before);
        }
    }

    pub fn mark(&mut self, range: Range<usize>, text: &str, selected: Option<Range<usize>>) {
        let start = range.start;
        self.replace(range, text, true);
        if let Some(selected) = selected {
            self.value.anchor = start + byte_offset(text, selected.start);
            self.value.head = start + byte_offset(text, selected.end);
        }
    }

    pub fn set_external_value(&mut self, text: &str, enabled: bool) -> bool {
        if !enabled {
            return false;
        }
        // End the IME transaction before a whole-field accessibility edit.
        self.finish_composition();
        self.replace(0..self.value.text.len(), text, false);
        true
    }

    pub fn insert(&mut self, text: &str) {
        let range = self.marked.clone().unwrap_or_else(|| self.selection());
        self.replace(range, text, false);
    }

    fn append_quote(&mut self, quote: &str) {
        self.cancel_recall();
        let (anchor, head) = (self.value.anchor, self.value.head);
        let text = &self.value.text;
        let end = text.len();
        let gap = if text.is_empty() || text.ends_with("\n\n") {
            ""
        } else if text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        self.replace(end..end, &format!("{gap}{quote}"), false);
        if end != 0 {
            self.value.anchor = anchor;
            self.value.head = head;
        }
    }

    pub fn delete(&mut self, forward: bool) {
        self.finish_composition();
        let mut range = self.selection();
        if range.is_empty() {
            if forward {
                range.end = self.boundary(range.end, true);
            } else {
                range.start = self.boundary(range.start, false);
            }
        }
        self.replace(range, "", false);
    }

    /// Start of the previous word or end of the next one, skipping spaces and punctuation.
    pub fn word_boundary(&self, offset: usize, forward: bool) -> usize {
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

    /// Delete the selection, or from the cursor to `target` when nothing is selected.
    /// A target at the cursor (line edge already reached) deletes one grapheme, as in
    /// native macOS fields.
    pub fn delete_to(&mut self, target: impl FnOnce(&Self) -> usize, forward: bool) {
        self.finish_composition();
        let range = self.selection();
        if !range.is_empty() {
            return self.replace(range, "", false);
        }
        let head = self.value.head;
        let target = target(self);
        let range = match target.cmp(&head) {
            std::cmp::Ordering::Less => self.snap(target)..head,
            std::cmp::Ordering::Greater => head..self.snap_end(target),
            std::cmp::Ordering::Equal if forward => head..self.boundary(head, true),
            std::cmp::Ordering::Equal => self.boundary(head, false)..head,
        };
        self.replace(range, "", false);
    }

    pub fn undo(&mut self, redo: bool) {
        self.finish_composition();
        // Undo while walking history returns to the draft; nothing was edited yet.
        if self.cancel_recall() {
            return;
        }
        let history = if redo { &mut self.redo } else { &mut self.undo };
        if let Some(value) = history.pop() {
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
    }

    pub fn can_submit(&self, enabled: bool, busy: bool) -> bool {
        enabled && !busy && self.marked.is_none() && !self.value.text.trim().is_empty()
    }

    pub fn submit(&mut self, enabled: bool, busy: bool) -> Option<String> {
        if !self.can_submit(enabled, busy) {
            return None;
        }
        let text = self.value.text.clone();
        self.submitted = Some((text.clone(), self.revision));
        // The host keeps only the tail of a long prompt; this copy is exact.
        self.history.record(&text);
        Some(text)
    }

    pub fn clear_if_unchanged(&mut self, submitted: &str) -> bool {
        let Some((text, revision)) = self.submitted.take() else {
            return false;
        };
        text == submitted && self.clear_revision(submitted, revision)
    }

    /// An acknowledgement belongs to one edit revision, even with many requests pending.
    pub fn clear_revision(&mut self, submitted: &str, revision: u64) -> bool {
        if revision != self.revision || self.value.text != submitted || self.marked.is_some() {
            return false;
        }
        self.submitted = None;
        if self.cancel_recall() {
            return true;
        }
        self.replace(0..self.value.text.len(), "", false);
        true
    }

    /// The text to keep across a restart: while a sent prompt is on screen,
    /// that is still the draft underneath it.
    pub fn durable_text(&self) -> &str {
        self.recall
            .as_ref()
            .map_or(&self.value.text, |recall| &recall.stash.text)
    }

    /// One-based position among the sent prompts, newest first, while recalling.
    pub fn recall_position(&self) -> Option<(usize, usize)> {
        self.recall
            .as_ref()
            .map(|recall| (recall.entries.len() - recall.index, recall.entries.len()))
    }

    /// Replace the visible text without an undo step. Each step advances the
    /// revision, even to identical text, so an acknowledgement for an earlier
    /// submission can never clear what recall put on screen.
    fn show(&mut self, value: Snapshot) {
        self.revision += 1;
        self.value = value;
        self.marked = None;
    }

    /// Step to an older or newer sent prompt. `edge` says the cursor is on the
    /// first visual line (for older) or the last (for newer). Stepping past the
    /// newest prompt returns the draft exactly as it was. Returns false when
    /// the key should move the cursor instead.
    pub fn recall(&mut self, older: bool, edge: bool) -> bool {
        if self.marked.is_some() || !self.selection().is_empty() {
            return false;
        }
        let index = match &self.recall {
            Some(recall) if !recall.untouched && !edge => return false,
            Some(recall) if older => match recall.index.checked_sub(1) {
                Some(index) => index,
                None => return false,
            },
            Some(recall) if recall.index + 1 == recall.entries.len() => {
                return self.cancel_recall();
            }
            Some(recall) => recall.index + 1,
            None => {
                let entries = self.history.entries();
                if !older || !edge || entries.is_empty() {
                    return false;
                }
                self.recall = Some(Recall {
                    index: entries.len() - 1,
                    entries,
                    stash: self.value.clone(),
                    untouched: true,
                });
                self.recall.as_ref().map_or(0, |recall| recall.index)
            }
        };
        let Some(recall) = &mut self.recall else {
            return false;
        };
        recall.index = index;
        recall.untouched = true;
        let text = recall.entries[index].clone();
        self.show(Snapshot {
            anchor: text.len(),
            head: text.len(),
            text,
        });
        true
    }

    /// Leave history and return the draft, its selection, and its undo stack untouched.
    pub fn cancel_recall(&mut self) -> bool {
        let Some(recall) = self.recall.take() else {
            return false;
        };
        self.show(recall.stash);
        true
    }

    /// An edit turns the recalled prompt into the draft. The text being written
    /// before becomes one undo step, so it is never lost.
    fn adopt_recalled(&mut self) {
        if let Some(recall) = self.recall.take() {
            self.remember(recall.stash);
        }
    }

    pub fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.value.text[..range.start].encode_utf16().count()
            ..self.value.text[..range.end].encode_utf16().count()
    }

    pub fn byte_range(&self, range: Range<usize>) -> Range<usize> {
        let start = byte_offset(&self.value.text, range.start);
        start..byte_offset(&self.value.text, range.end).max(start)
    }
}

impl super::Composer {
    pub(in crate::ui) fn accepts_quote(&self) -> bool {
        self.enabled && self.draft.marked.is_none()
    }

    pub(in crate::ui) fn quote_selection(
        &mut self,
        selection: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts_quote() || selection.is_empty() {
            return false;
        }
        let quote = plait_client::selection::quote(selection);
        window.focus(&self.focus, cx);
        self.edit(|draft| draft.append_quote(&quote), window, cx);
        true
    }
}

// Native text services use UTF-16; editor state uses UTF-8 byte boundaries.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn with_history(prompts: &[&str]) -> Draft {
        let mut draft = Draft::default();
        draft.history.set_host(prompts.iter().copied());
        draft
    }

    #[test]
    fn multiline_editing_selection_and_history() {
        let mut draft = Draft::default();
        draft.insert("one\ntwo");
        draft.select(0, false);
        draft.select(3, true);
        draft.insert("first");
        assert_eq!(draft.value.text, "first\ntwo");
        draft.undo(false);
        assert_eq!(draft.value.text, "one\ntwo");
        assert_eq!(draft.selection(), 0..3);
        draft.undo(true);
        assert_eq!(draft.value.text, "first\ntwo");
        draft.insert("\n");
        assert_eq!(draft.value.text, "first\n\ntwo");
        draft.undo(false);
        draft.insert("!");
        draft.undo(true);
        assert_eq!(draft.value.text, "first!\ntwo");
    }

    #[test]
    fn typing_after_quoting_into_an_empty_draft_starts_on_the_blank_line() {
        let mut draft = Draft::default();
        draft.append_quote("> a reply\n\n");
        draft.insert("My response");
        assert_eq!(draft.value.text, "> a reply\n\nMy response");
    }

    #[test]
    fn appending_a_quote_preserves_the_caret_and_outlives_an_old_ack() {
        let mut draft = Draft::default();
        draft.insert("existing draft");
        draft.select(4, false);
        let submitted = draft.submit(true, false).unwrap();
        let revision = draft.revision;
        draft.append_quote("> a reply\n\n");
        assert_eq!(draft.value.text, "existing draft\n\n> a reply\n\n");
        assert_eq!(draft.selection(), 4..4);
        assert_eq!(draft.revision, revision + 1);
        assert!(!draft.clear_revision(&submitted, revision));
        draft.undo(false);
        assert_eq!(draft.value.text, submitted);
        assert_eq!(draft.selection(), 4..4);
    }

    #[test]
    fn deletion_and_movement_preserve_graphemes() {
        for grapheme in ["e\u{301}", "👩🏽‍💻", "🇺🇦", "क्‍ष"] {
            let mut draft = Draft::default();
            draft.insert(&format!("A{grapheme}Z"));
            assert_eq!(draft.boundary(1, true), 1 + grapheme.len());
            draft.select(1 + grapheme.len(), false);
            draft.delete(false);
            assert_eq!(draft.value.text, "AZ");
            draft.undo(false);
            draft.select(1, false);
            draft.delete(true);
            assert_eq!(draft.value.text, "AZ");
        }
    }

    #[test]
    fn local_boundary_search_agrees_with_a_full_scan_at_every_offset() {
        let text = "a🇺🇦🇺🇦🇺b e\u{301}\u{301}\r\n👩🏽‍💻👨‍👩‍👧‍👦 क्‍ष 한글 x\u{200d}y";
        let mut draft = Draft::default();
        draft.insert(text);
        let starts: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        for offset in 0..=text.len() + 2 {
            let after = starts
                .iter()
                .copied()
                .find(|i| *i > offset)
                .unwrap_or(text.len());
            let before = starts
                .iter()
                .copied()
                .take_while(|i| *i < offset)
                .last()
                .unwrap_or(0);
            let at_or_before = starts
                .iter()
                .copied()
                .take_while(|i| *i <= offset)
                .last()
                .unwrap_or(0);
            let at_or_after = starts
                .iter()
                .copied()
                .find(|i| *i >= offset)
                .unwrap_or(text.len());
            if offset <= text.len() {
                assert_eq!(draft.boundary(offset, true), after, "after {offset}");
                assert_eq!(draft.boundary(offset, false), before, "before {offset}");
            }
            let end = offset >= text.len();
            assert_eq!(
                draft.snap(offset),
                if end { text.len() } else { at_or_before },
                "snap {offset}"
            );
            assert_eq!(
                draft.snap_end(offset),
                if end { text.len() } else { at_or_after },
                "snap_end {offset}"
            );
        }
    }

    #[test]
    fn insertion_that_joins_a_grapheme_keeps_cursor_at_its_end() {
        let mut draft = Draft::default();
        draft.insert("\u{301}Z");
        draft.select(0, false);
        draft.insert("e");
        assert_eq!(draft.value.head, "e\u{301}".len());
        draft.delete(false);
        assert_eq!(draft.value.text, "Z");
    }

    #[test]
    fn utf16_offsets_clamp_without_splitting_scalars() {
        let mut draft = Draft::default();
        draft.insert("A🙂é\n漢");
        assert_eq!(draft.byte_range(1..3), 1..5);
        assert_eq!(draft.byte_range(2..2), 1..1);
        assert_eq!(draft.byte_range(99..usize::MAX), 11..11);
        assert_eq!(draft.byte_range(std::ops::Range { start: 3, end: 1 }), 5..5);
        assert_eq!(draft.utf16_range(1..5), 1..3);
    }

    #[test]
    fn composition_is_one_undo_step_and_blocks_submit() {
        let mut draft = Draft::default();
        draft.insert("A");
        draft.replace(1..1, "n", true);
        assert!(draft.submit(true, false).is_none());
        draft.replace(1..2, "に", true);
        draft.replace(1..4, "日本", false);
        assert_eq!(draft.submit(true, false).as_deref(), Some("A日本"));
        draft.undo(false);
        assert_eq!(draft.value.text, "A");
        draft.undo(true);
        assert_eq!(draft.value.text, "A日本");
    }

    #[test]
    fn marked_selection_is_relative_utf16_and_updates_replace_the_mark() {
        let mut draft = Draft::default();
        draft.insert("A🙂Z");
        draft.mark(draft.byte_range(1..3), "é🙂", Some(1..3));
        assert_eq!(draft.value.text, "Aé🙂Z");
        assert_eq!(draft.marked, Some(1..7));
        assert_eq!(draft.selection(), 3..7);
        draft.mark(draft.marked.clone().unwrap(), "漢", Some(1..99));
        assert_eq!(draft.value.text, "A漢Z");
        assert_eq!(draft.selection(), 4..4);
        draft.finish_composition();
        draft.undo(false);
        assert_eq!(draft.value.text, "A🙂Z");
    }

    #[test]
    fn cancelled_composition_does_not_add_an_undo_step() {
        let mut draft = Draft::default();
        draft.insert("A");
        draft.submit(true, false);
        draft.mark(1..1, "に", None);
        draft.mark(1..4, "", None);
        draft.finish_composition();
        assert!(!draft.clear_if_unchanged("A"));
        draft.undo(false);
        assert!(draft.value.text.is_empty());
    }

    #[test]
    fn empty_composition_still_blocks_enter() {
        let mut draft = Draft::default();
        draft.insert("draft");
        draft.replace(5..5, "", true);
        assert!(draft.submit(true, false).is_none());
        draft.finish_composition();
        assert!(draft.submit(true, false).is_some());
    }

    #[test]
    fn submission_never_clears_and_rejects_blank_disabled_busy() {
        let mut draft = Draft::default();
        for text in ["", " \n\t", "\u{2003}"] {
            draft.replace(0..draft.value.text.len(), text, false);
            assert!(draft.submit(true, false).is_none());
        }
        draft.insert("hello");
        assert!(draft.submit(false, false).is_none());
        assert!(draft.submit(true, true).is_none());
        let submitted = draft.submit(true, false).unwrap();
        assert_eq!(draft.value.text, submitted);
        assert!(draft.clear_if_unchanged(&submitted));
        assert!(draft.value.text.is_empty());
        assert!(!draft.clear_if_unchanged(&submitted));
    }

    #[test]
    fn acceptance_checks_revision_not_just_text() {
        let mut draft = Draft::default();
        draft.insert("hello");
        let submitted = draft.submit(true, false).unwrap();
        draft.insert("!");
        draft.undo(false);
        assert_eq!(draft.value.text, submitted);
        assert!(!draft.clear_if_unchanged(&submitted));
        assert_eq!(draft.value.text, "hello");
        draft.submit(true, false);
        draft.select(0, false);
        assert!(draft.clear_if_unchanged("hello"));
    }

    #[test]
    fn restored_submission_keeps_the_edit_revision_guard() {
        let restore = |revision| {
            Draft::restored(
                "hello".into(),
                revision,
                Some(("hello".into(), 3)),
                PromptHistory::default(),
            )
        };
        let mut unchanged = restore(3);
        assert_eq!(unchanged.selection(), 5..5);
        assert!(unchanged.clear_if_unchanged("hello"));
        let mut edited_then_restored = restore(5);
        assert!(!edited_then_restored.clear_if_unchanged("hello"));
        assert_eq!(edited_then_restored.value.text, "hello");
    }

    #[test]
    fn external_replacement_preserves_history_and_acceptance_guard() {
        let mut draft = Draft::default();
        draft.insert("submitted");
        draft.submit(true, false);
        assert!(draft.set_external_value("next\n🙂", true));
        assert_eq!(draft.value.text, "next\n🙂");
        draft.undo(false);
        assert_eq!(draft.value.text, "submitted");
        assert!(!draft.clear_if_unchanged("submitted"));
        draft.undo(true);
        assert_eq!(draft.value.text, "next\n🙂");
    }

    #[test]
    fn external_replacement_finishes_composition_as_a_separate_undo_step() {
        let mut draft = Draft::default();
        draft.insert("A");
        draft.mark(1..1, "日本", Some(0..2));
        draft.set_external_value("replacement", true);
        assert!(draft.marked.is_none());
        draft.undo(false);
        assert_eq!(draft.value.text, "A日本");
        draft.undo(false);
        assert_eq!(draft.value.text, "A");
    }

    #[test]
    fn disabled_external_replacement_preserves_draft_and_marked_text() {
        let mut draft = Draft::default();
        draft.insert("draft");
        draft.mark(5..5, "に", None);
        let revision = draft.revision;
        assert!(!draft.set_external_value("replacement", false));
        assert_eq!(draft.value.text, "draftに");
        assert_eq!(draft.marked, Some(5..8));
        assert_eq!(draft.revision, revision);
    }

    #[test]
    fn word_deletion_skips_separators_and_respects_selection() {
        let mut draft = Draft::default();
        draft.insert("fix the bug.  ");
        draft.delete_to(|d| d.word_boundary(d.value.head, false), false);
        assert_eq!(draft.value.text, "fix the ");
        draft.select(5, false);
        draft.delete_to(|d| d.word_boundary(d.value.head, false), false);
        assert_eq!(draft.value.text, "fix he ");
        draft.select(0, false);
        draft.delete_to(|d| d.word_boundary(d.value.head, true), true);
        assert_eq!(draft.value.text, " he ");
        draft.select(1, false);
        draft.select(3, true);
        draft.delete_to(|_| 0, false);
        assert_eq!(draft.value.text, "  ");
        draft.undo(false);
        assert_eq!(draft.value.text, " he ");
    }

    #[test]
    fn line_deletion_stops_at_the_target_and_joins_lines_at_an_edge() {
        let mut draft = Draft::default();
        draft.insert("one\ntwo three");
        draft.delete_to(|_| 4, false);
        assert_eq!(draft.value.text, "one\n");
        draft.delete_to(|_| 4, false);
        assert_eq!(draft.value.text, "one");
        draft.select(1, false);
        draft.delete_to(|_| 3, true);
        assert_eq!(draft.value.text, "o");
        draft.undo(false);
        draft.undo(false);
        assert_eq!(draft.value.text, "one\n");
    }

    #[test]
    fn word_deletion_keeps_graphemes_and_finishes_composition() {
        let mut draft = Draft::default();
        draft.insert("go 👩🏽‍💻");
        draft.delete_to(|d| d.value.head, false);
        assert_eq!(draft.value.text, "go ");
        draft.mark(3..3, "に", None);
        draft.delete_to(|d| d.word_boundary(d.value.head, false), false);
        assert!(draft.marked.is_none());
        assert_eq!(draft.value.text, "go ");
    }

    #[test]
    fn unrelated_acceptance_cannot_clear_draft() {
        let mut draft = Draft::default();
        draft.insert("hello");
        assert!(!draft.clear_if_unchanged("hello"));
        draft.submit(true, false);
        assert!(!draft.clear_if_unchanged("another message"));
        assert_eq!(draft.value.text, "hello");
    }

    #[test]
    fn undo_steps_are_bounded_by_count_and_by_size() {
        let mut draft = Draft::default();
        for _ in 0..UNDO_LIMIT + 20 {
            draft.insert("x");
        }
        assert_eq!(draft.undo.len(), UNDO_LIMIT);
        let mut draft = Draft::default();
        draft.insert(&"y".repeat(UNDO_BYTES / 2));
        for _ in 0..6 {
            draft.insert("z");
        }
        assert!(draft.undo.len() <= 3);
        // The most recent step always survives, however large.
        draft.undo(false);
        assert_eq!(draft.value.text.len(), UNDO_BYTES / 2 + 5);
    }

    #[test]
    fn recall_walks_newest_first_and_returns_the_draft_unchanged() {
        let mut draft = with_history(&["first", "second", "third"]);
        draft.insert("in pro");
        draft.insert("gress");
        draft.select(3, false);
        let undo_steps = draft.undo.len();
        // Down never starts recall, and Up needs the first line.
        assert!(!draft.recall(false, true));
        assert!(!draft.recall(true, false));
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "third");
        assert_eq!(draft.selection(), 5..5);
        assert_eq!(draft.recall_position(), Some((1, 3)));
        assert!(draft.recall(true, true));
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "first");
        assert_eq!(draft.recall_position(), Some((3, 3)));
        // The oldest prompt is the end; the key falls back to moving the cursor.
        assert!(!draft.recall(true, true));
        assert_eq!(draft.value.text, "first");
        assert!(draft.recall(false, true));
        assert!(draft.recall(false, true));
        assert_eq!(draft.value.text, "third");
        assert!(draft.recall(false, true));
        assert_eq!(draft.value.text, "in progress");
        assert_eq!(draft.selection(), 3..3);
        assert_eq!(draft.recall_position(), None);
        assert_eq!(draft.undo.len(), undo_steps);
        draft.undo(false);
        assert_eq!(draft.value.text, "in pro");
        draft.undo(true);
        assert_eq!(draft.value.text, "in progress");
    }

    #[test]
    fn recall_needs_history_a_collapsed_cursor_and_no_composition() {
        let mut empty = Draft::default();
        assert!(!empty.recall(true, true));
        let mut draft = with_history(&["sent"]);
        draft.insert("draft");
        draft.select(0, false);
        draft.select(2, true);
        assert!(!draft.recall(true, true));
        draft.select(5, false);
        draft.mark(5..5, "に", None);
        assert!(!draft.recall(true, true));
        assert_eq!(draft.value.text, "draftに");
        draft.finish_composition();
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "sent");
    }

    #[test]
    fn recall_keeps_walking_a_multiline_prompt_until_the_cursor_moves() {
        let mut draft = with_history(&["old", "two\nlines"]);
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "two\nlines");
        // The cursor is on the last line, yet Up continues to the older prompt.
        assert!(draft.recall(true, false));
        assert_eq!(draft.value.text, "old");
        assert!(draft.recall(false, false));
        assert_eq!(draft.value.text, "two\nlines");
        draft.select(5, false);
        assert!(!draft.recall(true, false));
        assert!(!draft.recall(false, false));
        assert_eq!(draft.value.text, "two\nlines");
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "old");
    }

    #[test]
    fn editing_a_recalled_prompt_keeps_the_previous_draft_in_undo() {
        let mut draft = with_history(&["sent prompt"]);
        draft.insert("unfinished");
        assert!(draft.recall(true, true));
        draft.insert(" again");
        assert_eq!(draft.value.text, "sent prompt again");
        assert_eq!(draft.recall_position(), None);
        draft.undo(false);
        assert_eq!(draft.value.text, "sent prompt");
        draft.undo(false);
        assert_eq!(draft.value.text, "unfinished");
        draft.undo(true);
        draft.undo(true);
        assert_eq!(draft.value.text, "sent prompt again");
        // Recall is over, so Down no longer returns anything.
        assert!(!draft.recall(false, true));
    }

    #[test]
    fn undo_and_cancel_leave_recall_without_touching_history() {
        let mut draft = with_history(&["sent"]);
        draft.insert("a");
        draft.insert("b");
        draft.undo(false);
        let (undo, redo) = (draft.undo.len(), draft.redo.len());
        assert!(draft.recall(true, true));
        draft.undo(false);
        assert_eq!(draft.value.text, "a");
        assert_eq!((draft.undo.len(), draft.redo.len()), (undo, redo));
        assert!(draft.recall(true, true));
        draft.undo(true);
        assert_eq!(draft.value.text, "a");
        assert!(draft.recall(true, true));
        assert!(draft.cancel_recall());
        assert!(!draft.cancel_recall());
        assert_eq!(draft.value.text, "a");
        draft.undo(true);
        assert_eq!(draft.value.text, "ab");
    }

    #[test]
    fn recall_advances_the_revision_so_an_old_acceptance_cannot_clear_it() {
        let mut draft = with_history(&["hello"]);
        draft.insert("hello");
        let submitted = draft.submit(true, false).unwrap();
        // The recalled prompt has the same text as the pending submission.
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, submitted);
        assert!(draft.recall(false, true));
        assert_eq!(draft.value.text, submitted);
        assert!(!draft.clear_if_unchanged(&submitted));
        assert_eq!(draft.value.text, "hello");

        let mut draft = with_history(&["earlier"]);
        draft.insert("pending");
        let submitted = draft.submit(true, false).unwrap();
        // The newest sent prompt is the pending one itself.
        assert!(draft.recall(true, true));
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "earlier");
        assert!(!draft.clear_if_unchanged(&submitted));
        assert_eq!(draft.value.text, "earlier");
        assert!(draft.recall(false, true));
        assert!(draft.recall(false, true));
        assert_eq!(draft.value.text, "pending");
        assert_eq!(draft.recall_position(), None);
    }

    #[test]
    fn sending_a_recalled_prompt_returns_to_the_draft_being_written() {
        let mut draft = with_history(&["run the tests"]);
        draft.insert("half an idea");
        let undo_steps = draft.undo.len();
        assert!(draft.recall(true, true));
        let submitted = draft.submit(true, false).unwrap();
        assert_eq!(submitted, "run the tests");
        assert_eq!(draft.durable_text(), "half an idea");
        assert!(draft.clear_if_unchanged(&submitted));
        assert_eq!(draft.value.text, "half an idea");
        assert_eq!(draft.undo.len(), undo_steps);
        assert_eq!(draft.recall_position(), None);
        assert!(!draft.clear_if_unchanged(&submitted));
    }

    #[test]
    fn recall_is_unaffected_by_prompts_sent_or_loaded_meanwhile() {
        let mut draft = with_history(&["one", "two"]);
        assert!(draft.recall(true, true));
        assert_eq!(draft.submit(true, false).as_deref(), Some("two"));
        draft.history.set_host(["one", "two", "two", "three"]);
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "one");
        assert!(draft.recall(false, true));
        assert!(draft.recall(false, true));
        assert_eq!(draft.value.text, "");
        // A new walk sees the new history.
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "three");
    }

    #[test]
    fn durable_text_is_the_draft_under_a_recalled_prompt() {
        let mut draft = with_history(&["sent"]);
        draft.insert("keep me");
        assert_eq!(draft.durable_text(), "keep me");
        assert!(draft.recall(true, true));
        assert_eq!(draft.value.text, "sent");
        assert_eq!(draft.durable_text(), "keep me");
        draft.insert("!");
        assert_eq!(draft.durable_text(), "sent!");
    }
    #[test]
    fn acknowledgements_for_multiple_requests_clear_only_their_revision() {
        let mut draft = Draft::default();
        draft.insert("first");
        let first_revision = draft.revision;
        draft.submit(true, false).unwrap();
        draft.insert(" and second");
        let second_revision = draft.revision;
        let second = draft.submit(true, false).unwrap();
        assert!(!draft.clear_revision("first", first_revision));
        assert_eq!(draft.value.text, second);
        assert!(draft.clear_revision(&second, second_revision));
        draft.insert(&second);
        assert!(!draft.clear_revision(&second, second_revision));
    }
}
