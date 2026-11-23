//! Finding text in the open conversation. Blocking: call on a background
//! thread.
//!
//! The search runs over the blocks the view draws (`markdown::Transcript`),
//! not the Markdown source, so every match is a byte range in one block and
//! can be highlighted where it is drawn. Requests and replies are searched;
//! the reply still streaming, reasoning, and tool output are not.

use crate::{
    file_view::FIND_LIMIT,
    history::History,
    host::{HistoryField, Snapshot},
    markdown::{Block, Kind, Transcript},
    transcript::{self, Slot},
};
use std::{collections::HashMap, ops::Range};

/// One match: a range of one block of one message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The message's stable ID, so a match keeps its identity as rows change.
    pub message: String,
    /// The message's position in the conversation's rows.
    pub row: usize,
    pub block: usize,
    pub range: Range<usize>,
}

/// Every match in conversation order, at most `FIND_LIMIT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matches {
    pub hits: Vec<Hit>,
    /// Each message's matches, as a range of `hits`, so drawing a row looks
    /// up only its own.
    by_message: HashMap<String, Range<usize>>,
}

impl Matches {
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Whether the search stopped counting at the limit.
    pub fn capped(&self) -> bool {
        self.hits.len() >= FIND_LIMIT
    }

    /// The matches in message `id`, and the index of the first in `hits`.
    pub fn in_message(&self, id: &str) -> (usize, &[Hit]) {
        match self.by_message.get(id) {
            Some(range) => (range.start, &self.hits[range.clone()]),
            None => (0, &[]),
        }
    }

    /// Where the same match is now: same message, block, and range.
    pub fn position(&self, hit: &Hit) -> Option<usize> {
        let (first, hits) = self.in_message(&hit.message);
        hits.iter()
            .position(|found| found.block == hit.block && found.range == hit.range)
            .map(|index| first + index)
    }

    /// The first match in row `row` or after it, wrapping to the first.
    pub fn first_from(&self, row: usize) -> usize {
        let index = self.hits.partition_point(|hit| hit.row < row);
        if index == self.hits.len() { 0 } else { index }
    }
}

/// The next or previous of `count` matches after `current`, wrapping.
pub fn step(current: usize, count: usize, forward: bool) -> usize {
    match count {
        0 => 0,
        _ if forward => (current + 1) % count,
        _ => (current % count + count - 1) % count,
    }
}

/// Byte ranges of `query` in `text`, ignoring case, at most `limit`. Ranges
/// start and end on character boundaries and do not overlap.
pub fn find(text: &str, query: &str, limit: usize) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if query.is_empty() {
        return found;
    }
    if query.is_ascii() {
        // ASCII bytes never occur inside a multi-byte character, so a byte
        // comparison is both exact and fast on long conversations.
        let (haystack, needle) = (text.as_bytes(), query.as_bytes());
        let mut at = 0;
        while at + needle.len() <= haystack.len() && found.len() < limit {
            if haystack[at..at + needle.len()].eq_ignore_ascii_case(needle) {
                found.push(at..at + needle.len());
                at += needle.len();
            } else {
                at += 1;
            }
        }
        return found;
    }
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut at = 0;
    while at < text.len() && found.len() < limit {
        match folded_match(text, at, &needle) {
            Some(end) => {
                found.push(at..end);
                at = end;
            }
            None => at += text[at..].chars().next().map_or(1, char::len_utf8),
        }
    }
    found
}

/// Where a match of the lowercased `needle` starting at `start` ends. A
/// match must end on a character of `text`, never inside its lowercase form.
fn folded_match(text: &str, start: usize, needle: &[char]) -> Option<usize> {
    let mut matched = 0;
    for (offset, ch) in text[start..].char_indices() {
        for lower in ch.to_lowercase() {
            if needle.get(matched) != Some(&lower) {
                return None;
            }
            matched += 1;
        }
        if matched == needle.len() {
            return Some(start + offset + ch.len_utf8());
        }
    }
    None
}

/// The parts of a block's text that are drawn as text: a list item without
/// its marker, each table cell, each diff line. A match never spans two.
pub fn segments(block: &Block) -> Vec<Range<usize>> {
    let len = block.text.len();
    let whole = |start: usize| std::iter::once(start.min(len)..len).collect();
    match &block.kind {
        Kind::List { marker, .. } => whole(*marker),
        Kind::Table(table) => table.rows.iter().flatten().cloned().collect(),
        Kind::Diff(lines) => lines.iter().map(|line| line.range.clone()).collect(),
        Kind::Rule => Vec::new(),
        _ => whole(0),
    }
}

/// Every match of `query` in the loaded messages of `snapshot`, as
/// `transcript` draws them, in conversation order. Messages without a
/// projection yet are skipped. Past `FIND_LIMIT` the newest matches are
/// kept: the end of a conversation is where it is usually read.
pub fn search(snapshot: &Snapshot, transcript: &Transcript, query: &str) -> Matches {
    search_cancellable(snapshot, transcript, query, &|| false)
}

pub fn search_cancellable(
    snapshot: &Snapshot,
    transcript: &Transcript,
    query: &str,
    cancelled: &impl Fn() -> bool,
) -> Matches {
    let mut matches = Matches::default();
    if query.is_empty() {
        return matches;
    }
    let mut rows = vec![0; snapshot.messages.len()];
    for (row, slot) in transcript::layout(snapshot).into_iter().enumerate() {
        if let Slot::Message(index) = slot {
            rows[index] = row;
        }
    }
    let mut newest_first = Vec::new();
    let mut total = 0;
    for (index, message) in snapshot.messages.iter().enumerate().rev() {
        if cancelled() {
            return Matches::default();
        }
        if total >= FIND_LIMIT {
            break;
        }
        let Some(parsed) = transcript.messages.get(&message.id) else {
            continue;
        };
        let hits = message_hits(
            &message.id,
            rows[index],
            &parsed.blocks,
            query,
            FIND_LIMIT - total,
        );
        total += hits.len();
        if !hits.is_empty() {
            newest_first.push(hits);
        }
    }
    for hits in newest_first.into_iter().rev() {
        let start = matches.hits.len();
        let id = hits[0].message.clone();
        matches.hits.extend(hits);
        matches.by_message.insert(id, start..matches.hits.len());
    }
    matches
}

/// The matches in one message's blocks, at most `limit`.
fn message_hits(id: &str, row: usize, blocks: &[Block], query: &str, limit: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        for segment in segments(block) {
            let found = find(&block.text[segment.clone()], query, limit - hits.len());
            hits.extend(found.into_iter().map(|range| Hit {
                message: id.to_owned(),
                row,
                block: index,
                range: segment.start + range.start..segment.start + range.end,
            }));
            if hits.len() >= limit {
                return hits;
            }
        }
    }
    hits
}

/// Whether a loaded message shows only part of its text, so the rest of it
/// cannot be searched.
pub fn partial(history: &History) -> bool {
    history.entries.iter().any(|entry| {
        entry
            .parts
            .iter()
            .any(|part| part.field == HistoryField::Text && (part.start > 0 || part.has_next()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Message, MessageRole, Tool, ToolStatus};

    fn message(id: &str, role: MessageRole, text: &str) -> Message {
        Message {
            id: id.into(),
            role,
            text: text.into(),
            ..Default::default()
        }
    }

    fn conversation(messages: Vec<Message>) -> (Snapshot, Transcript) {
        let snapshot = Snapshot {
            id: "t".into(),
            messages,
            ..Default::default()
        };
        let transcript = Transcript::update(&Transcript::default(), &snapshot);
        (snapshot, transcript)
    }

    #[test]
    fn find_ignores_case_and_finds_every_match_in_a_block() {
        assert_eq!(
            find("Queue, queue, QUEUE", "queue", 10),
            vec![0..5, 7..12, 14..19]
        );
        assert_eq!(find("aaaa", "aa", 10), vec![0..2, 2..4], "no overlap");
        assert_eq!(find("Queue queue", "queue", 1), vec![0..5], "limit");
        assert!(find("anything", "", 10).is_empty());
        assert!(find("", "a", 10).is_empty());
    }

    #[test]
    fn find_ignores_case_outside_ascii_on_character_boundaries() {
        // "É" and "é" are both two bytes.
        assert_eq!(find("Ébé ébé", "ébé", 10), vec![0..5, 6..11]);
        assert_eq!(find("ÉBÉ", "ébé", 10), vec![0..5]);
        assert_eq!(find("Straße", "STRASSE", 10), Vec::<Range<usize>>::new());
        assert_eq!(find("ünïcode café", "CAFÉ", 10), vec![10..15]);
        // An ASCII query beside multi-byte text keeps its byte offsets.
        assert_eq!(find("日本 Rust", "rust", 10), vec![7..11]);
        for range in find("ÅÄÖ åäö", "äö", 10) {
            assert!("ÅÄÖ åäö".is_char_boundary(range.start));
            assert!("ÅÄÖ åäö".is_char_boundary(range.end));
        }
    }

    #[test]
    fn matches_come_in_conversation_order_across_blocks_and_messages() {
        let (snapshot, transcript) = conversation(vec![
            message("q", MessageRole::User, "Fix the queue."),
            message(
                "a",
                MessageRole::Assistant,
                "The queue is fine.\n\n- one queue\n- two\n\n```rust\nlet queue = 1;\n```",
            ),
            message("b", MessageRole::Assistant, "No match here."),
            message("c", MessageRole::User, "Queue again"),
        ]);
        let matches = search(&snapshot, &transcript, "QUEUE");
        let places: Vec<_> = matches
            .hits
            .iter()
            .map(|hit| (hit.message.as_str(), hit.row, hit.block, hit.range.clone()))
            .collect();
        assert_eq!(
            places,
            vec![
                ("q", 0, 0, 8..13),
                ("a", 1, 0, 4..9),
                // A list item's text starts after its "• " marker.
                ("a", 1, 1, 8..13),
                ("a", 1, 3, 4..9),
                ("c", 3, 0, 0..5),
            ]
        );
        for hit in &matches.hits {
            let block = &transcript.messages[&hit.message].blocks[hit.block];
            assert!(block.text[hit.range.clone()].eq_ignore_ascii_case("queue"));
        }
        let (first, in_a) = matches.in_message("a");
        assert_eq!((first, in_a.len()), (1, 3));
        assert_eq!(matches.in_message("b"), (0, &[][..]));
        assert_eq!(matches.position(&matches.hits[3]), Some(3));
        assert!(search(&snapshot, &transcript, "").is_empty());
    }

    #[test]
    fn list_markers_are_not_searched() {
        let (snapshot, transcript) = conversation(vec![message(
            "a",
            MessageRole::Assistant,
            "1. first\n2. second",
        )]);
        assert!(search(&snapshot, &transcript, "1.").is_empty());
        assert_eq!(search(&snapshot, &transcript, "second").len(), 1);
    }

    #[test]
    fn rows_count_tools_and_reasoning_and_streaming_text_is_not_searched() {
        let mut answer = message("a", MessageRole::Assistant, "found it");
        answer.reasoning = "found in reasoning".into();
        let (mut snapshot, _) = conversation(vec![
            message("q", MessageRole::User, "ask"),
            answer,
            message("q2", MessageRole::User, "found again"),
        ]);
        snapshot.tools.push(Tool {
            id: "t1".into(),
            name: "read".into(),
            status: ToolStatus::Done,
            input: "{\"path\":\"found.rs\"}".into(),
            output: "found in output".into(),
            after: Some(Some("q".into())),
        });
        snapshot.live_text = "found while streaming".into();
        let transcript = Transcript::update(&Transcript::default(), &snapshot);
        let rows: Vec<_> = search(&snapshot, &transcript, "found")
            .hits
            .iter()
            .map(|hit| (hit.message.clone(), hit.row))
            .collect();
        // Rows: q, tool, reasoning, a, q2, streaming.
        assert_eq!(rows, vec![("a".into(), 3), ("q2".into(), 4)]);
    }

    #[test]
    fn counting_stops_at_the_limit_and_keeps_the_newest() {
        let text = "x ".repeat(FIND_LIMIT);
        let (snapshot, transcript) = conversation(vec![
            message("a", MessageRole::User, &text),
            message("b", MessageRole::User, "x"),
        ]);
        let matches = search(&snapshot, &transcript, "x");
        assert_eq!(matches.len(), FIND_LIMIT);
        assert!(matches.capped());
        assert_eq!(
            matches.in_message("a"),
            (0, &matches.hits[..FIND_LIMIT - 1])
        );
        assert_eq!(matches.in_message("b").0, FIND_LIMIT - 1);
        assert_eq!(matches.hits.last().unwrap().message, "b");
    }

    #[test]
    fn stepping_wraps_both_ways() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 3, false), 2);
        assert_eq!(step(1, 3, false), 0);
        assert_eq!(step(0, 0, true), 0);
        assert_eq!(step(0, 0, false), 0);
        assert_eq!(
            step(5, 3, false),
            1,
            "a current past the end steps from its place"
        );
    }

    #[test]
    fn the_first_match_from_a_row_wraps_to_the_start() {
        let (snapshot, transcript) = conversation(vec![
            message("a", MessageRole::User, "hit"),
            message("b", MessageRole::User, "miss"),
            message("c", MessageRole::User, "hit"),
        ]);
        let matches = search(&snapshot, &transcript, "hit");
        assert_eq!(matches.first_from(0), 0);
        assert_eq!(matches.first_from(1), 1);
        assert_eq!(matches.first_from(2), 1);
        assert_eq!(matches.first_from(3), 0);
    }
}
