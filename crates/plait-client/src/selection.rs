//! Text selection over conversation blocks, independent of layout.
//!
//! A position names a row of the conversation, a block within that row, and a
//! byte offset into the block's text. The view maps pointer positions to these
//! and paints the result; ordering, ranges, and copied text are decided here.

use crate::markdown::{Block, Kind};
use std::ops::Range;

/// Ordered by row, then block, then offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub row: usize,
    pub block: usize,
    pub offset: usize,
}

/// A non-empty selection from `start` up to `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

impl Span {
    /// The span between two positions in either order; `None` when they coincide.
    pub fn between(a: Position, b: Position) -> Option<Self> {
        (a != b).then(|| Self {
            start: a.min(b),
            end: a.max(b),
        })
    }

    /// The selected bytes of one block of `len` bytes, if any.
    pub fn within(&self, row: usize, block: usize, len: usize) -> Option<Range<usize>> {
        let at = |offset| Position { row, block, offset };
        let start = if self.start <= at(0) {
            0
        } else if self.start <= at(len) {
            self.start.offset
        } else {
            return None;
        };
        let end = if self.end >= at(len) {
            len
        } else if self.end >= at(0) {
            self.end.offset
        } else {
            return None;
        };
        (start < end).then_some(start..end)
    }
}

/// Move `offset` down to a character boundary inside `text`.
pub fn clamp(text: &str, offset: usize) -> usize {
    text.floor_char_boundary(offset.min(text.len()))
}

#[derive(PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(ch: char) -> Class {
    if ch.is_alphanumeric() || ch == '_' {
        Class::Word
    } else if ch.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// The word, run of spaces, or single symbol at `offset`, for a double click.
pub fn word(text: &str, offset: usize) -> Range<usize> {
    let offset = clamp(text, offset);
    let Some(ch) = text[offset..]
        .chars()
        .next()
        .or_else(|| text[..offset].chars().next_back())
    else {
        return 0..0;
    };
    // At the very end, the click belongs to the last character.
    let at = if offset == text.len() {
        offset - ch.len_utf8()
    } else {
        offset
    };
    let kind = class(ch);
    if kind == Class::Other {
        return at..at + ch.len_utf8();
    }
    let before: usize = text[..at]
        .chars()
        .rev()
        .take_while(|ch| class(*ch) == kind && *ch != '\n')
        .map(char::len_utf8)
        .sum();
    let after: usize = text[at..]
        .chars()
        .take_while(|ch| class(*ch) == kind && *ch != '\n')
        .map(char::len_utf8)
        .sum();
    at - before..at + after
}

/// The line at `offset`, without its terminator, for a triple click in code.
pub fn line(text: &str, offset: usize) -> Range<usize> {
    let offset = clamp(text, offset);
    let start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |at| offset + at);
    start..end
}

/// The text a selection copies. `blocks` yields every visible block of the
/// selected rows in order, as (row, block index, block).
pub fn text<'a>(
    blocks: impl IntoIterator<Item = (usize, usize, &'a Block)>,
    span: &Span,
) -> String {
    let mut out = String::new();
    let mut previous: Option<(usize, bool)> = None;
    for (row, index, block) in blocks {
        let Some(range) = span.within(row, index, block.text.len()) else {
            continue;
        };
        let range = clamp(&block.text, range.start)..clamp(&block.text, range.end);
        let item = matches!(block.kind, Kind::List { .. });
        if let Some((last_row, last_item)) = previous {
            // Items of one list stay on consecutive lines; other blocks are paragraphs.
            let tight = last_row == row && last_item && item;
            out.push_str(if tight { "\n" } else { "\n\n" });
        }
        out.push_str(&block.text[range]);
        previous = Some((row, item));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Message;

    fn at(row: usize, block: usize, offset: usize) -> Position {
        Position { row, block, offset }
    }

    #[test]
    fn spans_order_their_ends_and_reject_empty_selections() {
        let span = Span::between(at(2, 0, 4), at(1, 3, 1)).unwrap();
        assert_eq!((span.start, span.end), (at(1, 3, 1), at(2, 0, 4)));
        assert_eq!(Span::between(at(1, 1, 1), at(1, 1, 1)), None);
    }

    #[test]
    fn a_span_covers_part_of_its_end_blocks_and_all_of_those_between() {
        let span = Span::between(at(1, 1, 3), at(2, 0, 2)).unwrap();
        assert_eq!(span.within(1, 0, 10), None);
        assert_eq!(span.within(1, 1, 10), Some(3..10));
        assert_eq!(span.within(1, 2, 10), Some(0..10));
        assert_eq!(span.within(2, 0, 10), Some(0..2));
        assert_eq!(span.within(2, 1, 10), None);
        assert_eq!(span.within(0, 5, 10), None);
        assert_eq!(span.within(3, 0, 10), None);
        // An end that sits at the start of a block selects nothing in it.
        let span = Span::between(at(1, 0, 3), at(1, 1, 0)).unwrap();
        assert_eq!(span.within(1, 1, 10), None);
        // A stale offset past the end of a shorter block is clamped.
        let span = Span::between(at(1, 0, 3), at(1, 0, 99)).unwrap();
        assert_eq!(span.within(1, 0, 10), Some(3..10));
        assert_eq!(span.within(1, 0, 2), None);
    }

    #[test]
    fn double_click_selects_words_spaces_or_one_symbol() {
        let text = "let naïve_name = call(🙂);";
        assert_eq!(&text[word(text, 5)], "naïve_name");
        assert_eq!(&text[word(text, 0)], "let");
        assert_eq!(&text[word(text, 3)], " ");
        assert_eq!(&text[word(text, text.find('(').unwrap())], "(");
        assert_eq!(&text[word(text, text.find('🙂').unwrap())], "🙂");
        // Inside a multi-byte character, and at the very end.
        assert_eq!(&text[word(text, text.find('ï').unwrap() + 1)], "naïve_name");
        assert_eq!(&text[word(text, text.len())], ";");
        assert_eq!(word("", 0), 0..0);
        assert_eq!(&"a  \n  b"[word("a  \n  b", 1)], "  ");
    }

    #[test]
    fn triple_click_in_code_selects_one_line() {
        let text = "first\nsecond\n\nlast";
        assert_eq!(&text[line(text, 0)], "first");
        assert_eq!(&text[line(text, 8)], "second");
        assert_eq!(&text[line(text, 12)], "second");
        assert_eq!(line(text, 13), 13..13);
        assert_eq!(&text[line(text, 99)], "last");
    }

    #[test]
    fn copied_text_joins_blocks_and_rows_and_respects_partial_ends() {
        let first = Message::parse("Intro **bold**.\n\n- one\n- two\n\n```rust\nfn a() {}\n```");
        let second = Message::plain("Next message");
        let rows = [(4, &first), (6, &second)];
        let blocks = || {
            rows.iter().flat_map(|(row, message)| {
                message
                    .blocks
                    .iter()
                    .enumerate()
                    .map(move |(index, block)| (*row, index, block))
            })
        };
        let all = Span::between(at(0, 0, 0), at(9, 0, 0)).unwrap();
        assert_eq!(
            text(blocks(), &all),
            "Intro bold.\n\n• one\n• two\n\nfn a() {}\n\nNext message"
        );
        let part = Span::between(at(4, 0, 6), at(4, 1, "• one".len())).unwrap();
        assert_eq!(text(blocks(), &part), "bold.\n\n• one");
        let across = Span::between(at(4, 3, 3), at(6, 0, 4)).unwrap();
        assert_eq!(text(blocks(), &across), "a() {}\n\nNext");
        // A stale end inside a multi-byte character cannot split it.
        let unicode = Message::plain("née");
        let span = Span::between(at(0, 0, 0), at(0, 0, 2)).unwrap();
        assert_eq!(text([(0, 0, &unicode.blocks[0])], &span), "n");
    }
}
