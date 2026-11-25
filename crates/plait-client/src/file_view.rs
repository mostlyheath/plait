//! Reading one checkout file for the read-only viewer, finding text in it, and
//! telling whether it changed on disk. Blocking: call on a background thread.
//!
//! The bounded read and its grew-while-reading guard follow Sidelit's
//! (same maintainer, MIT); opening uses the checkout-relative, no-symlink
//! `open_beneath` shared with Git previews.

use crate::highlight;
use std::{
    hash::{Hash, Hasher},
    ops::Range,
    path::Path,
};

/// A NUL byte in this much of the start marks a binary file, as Git decides.
const SNIFF: usize = 8000;
/// Find stops counting here, so a one-letter query stays instant.
pub const FIND_LIMIT: usize = 1000;
/// Lines longer than this are clipped when drawn; the text itself is kept.
pub const LINE_CLIP: usize = 2000;

pub use plait_protocol::file_view::{READ_LIMIT, Stamp};

/// What the viewer can show for a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Contents {
    Text(Text),
    Binary {
        len: u64,
    },
    TooLarge {
        len: u64,
    },
    /// Not valid UTF-8; shown as a note rather than guessed at.
    NotUtf8 {
        len: u64,
    },
}

/// A text file, split into lines and highlighted once, off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub text: String,
    /// Byte range of each line, without its line ending.
    pub lines: Vec<Range<usize>>,
    /// Highlight spans, sorted; empty when the language is unknown or the file
    /// is past the highlighter's limit.
    pub spans: Vec<highlight::Span>,
    /// Characters in the longest line, for horizontal scrolling.
    pub widest: usize,
    /// First line with the widest clipped width.
    pub widest_index: usize,
    pub hash: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub contents: Contents,
    pub stamp: Stamp,
}

/// What the viewer shows for bytes read by `read_bytes`.
pub fn classify(stamp: Stamp, bytes: Option<Vec<u8>>, relative: &Path) -> Opened {
    let Some(bytes) = bytes else {
        return Opened {
            contents: Contents::TooLarge { len: stamp.len },
            stamp,
        };
    };
    let len = bytes.len() as u64;
    let contents = if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        Contents::Binary { len }
    } else {
        match String::from_utf8(bytes) {
            Ok(text) => Contents::Text(prepare(text, &relative.to_string_lossy())),
            Err(_) => Contents::NotUtf8 { len },
        }
    };
    Opened { contents, stamp }
}

/// Split into lines and highlight by the path's language.
pub fn prepare(text: String, path: &str) -> Text {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            let end = if index > start && text.as_bytes()[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            lines.push(start..end);
            start = index + 1;
        }
    }
    // A final line without a newline; an empty file still has one line.
    if start < text.len() || lines.is_empty() {
        lines.push(start..text.len());
    }
    let (widest_index, widest) = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (index, text[line.clone()].chars().count().min(LINE_CLIP)))
        .fold(
            (0, 0),
            |best, item| if item.1 > best.1 { item } else { best },
        );
    let spans = highlight::highlight(highlight::language_for_path(path), &text);
    Text {
        hash: hash(&text),
        text,
        lines,
        spans,
        widest,
        widest_index,
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Byte ranges of `query` in `text`, at most `FIND_LIMIT`, ignoring letter
/// case as find in the conversation does, outside ASCII too.
pub fn find(text: &str, query: &str) -> Vec<Range<usize>> {
    crate::conversation_find::find(text, query, FIND_LIMIT)
}

/// The line holding byte `offset`.
pub fn line_of(lines: &[Range<usize>], offset: usize) -> usize {
    lines
        .partition_point(|line| line.end < offset)
        .min(lines.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_split_into_lines_and_highlighted() {
        let text = prepare("fn main() {\r\n    1\n}".into(), "src/main.rs");
        assert_eq!(text.lines, vec![0..11, 13..18, 19..20]);
        assert_eq!(&text.text[text.lines[1].clone()], "    1");
        assert!(
            text.spans
                .iter()
                .any(|(_, token)| *token == highlight::Token::Keyword)
        );
        assert_eq!(text.widest, 11);
        assert_eq!(text.widest_index, 0);
        assert_eq!(prepare("a\nlonger\nx".into(), "a").widest_index, 1);
        assert_eq!(prepare("longer\na".into(), "a").widest_index, 0);
        assert_eq!(prepare(String::new(), "a").lines, vec![0..0]);
        assert_eq!(prepare("a\n".into(), "a").lines, vec![0..1]);
    }

    #[test]
    fn find_ignores_case_and_stops_at_the_limit() {
        assert_eq!(find("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
        assert_eq!(
            find("aaaa", "aa"),
            vec![0..2, 2..4],
            "matches do not overlap"
        );
        assert_eq!(
            find("Ébé ébé ÉBÉ", "ébé"),
            vec![0..5, 6..11, 12..17],
            "case is ignored outside ASCII too"
        );
        assert_eq!(find("let café = CAFÉ;", "Café"), vec![4..9, 12..17]);
        assert!(find("abc", "").is_empty());
        assert_eq!(find(&"x".repeat(5000), "x").len(), FIND_LIMIT);
    }

    #[test]
    fn offsets_map_to_lines() {
        let lines = vec![0..3, 4..4, 5..9];
        assert_eq!(line_of(&lines, 0), 0);
        assert_eq!(line_of(&lines, 3), 0);
        assert_eq!(line_of(&lines, 4), 1);
        assert_eq!(line_of(&lines, 6), 2);
        assert_eq!(line_of(&lines, 99), 2);
    }
}
