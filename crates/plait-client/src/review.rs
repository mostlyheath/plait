//! Quoting picked diff lines into a message, so a thread can be asked about
//! exactly those lines. Presentation text only; nothing here reads files.

use crate::model::{DiffKind, DiffLine};

/// Longer picks are quoted up to this many lines, then summarized; a message
/// should point at code, not carry a whole file.
pub const QUOTE_LIMIT: usize = 200;

/// A reference and fenced patch for `lines`, such as
/// `` `src/a.rs:12-14` (unstaged) `` followed by a ```` ```diff ```` block.
/// File and hunk headers are left out. None when nothing quotable remains.
pub fn quote(path: &str, side: &str, lines: &[DiffLine]) -> Option<String> {
    let picked: Vec<&DiffLine> = lines
        .iter()
        .filter(|line| {
            matches!(
                line.kind,
                DiffKind::Added | DiffKind::Removed | DiffKind::Context
            )
        })
        .collect();
    if picked.is_empty() {
        return None;
    }
    let shown = &picked[..picked.len().min(QUOTE_LIMIT)];
    let body: Vec<&str> = shown.iter().map(|line| &*line.text).collect();
    let fence = "`".repeat(longest_backtick_run(&body).max(2) + 1);
    let mut text = format!("`{}` ({side}):\n{fence}diff\n", location(path, &picked));
    for line in body {
        text.push_str(line);
        text.push('\n');
    }
    if picked.len() > shown.len() {
        text.push_str(&format!("… {} more lines\n", picked.len() - shown.len()));
    }
    text.push_str(&fence);
    Some(text)
}

/// `path:12-14` by new line numbers; a pick of only removed lines uses the
/// old numbers and says so.
fn location(path: &str, lines: &[&DiffLine]) -> String {
    let numbers = |side: fn(&DiffLine) -> Option<usize>| {
        let mut all = lines.iter().filter_map(|line| side(line));
        let first = all.next()?;
        let (low, high) = all.fold((first, first), |(low, high), n| (low.min(n), high.max(n)));
        Some(if low == high {
            low.to_string()
        } else {
            format!("{low}-{high}")
        })
    };
    if let Some(range) = numbers(|line| line.new_line) {
        format!("{path}:{range}")
    } else if let Some(range) = numbers(|line| line.old_line) {
        format!("{path}:{range}, removed")
    } else {
        path.to_owned()
    }
}

/// A fence must be longer than any backtick run inside it.
fn longest_backtick_run(lines: &[&str]) -> usize {
    let mut longest = 0;
    for line in lines {
        let mut run = 0;
        for c in line.chars() {
            run = if c == '`' { run + 1 } else { 0 };
            longest = longest.max(run);
        }
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn line(kind: DiffKind, text: &str, old: Option<usize>, new: Option<usize>) -> DiffLine {
        DiffLine {
            text: Arc::from(text),
            kind,
            old_line: old,
            new_line: new,
        }
    }

    #[test]
    fn a_pick_is_quoted_with_its_new_line_range_and_without_headers() {
        let lines = [
            line(DiffKind::Hunk, "@@ -10,3 +10,3 @@", None, None),
            line(DiffKind::Context, " fn main() {", Some(10), Some(10)),
            line(DiffKind::Removed, "-    old();", Some(11), None),
            line(DiffKind::Added, "+    new();", None, Some(11)),
            line(DiffKind::Context, " }", Some(12), Some(12)),
        ];
        assert_eq!(
            quote("src/main.rs", "unstaged", &lines).unwrap(),
            "`src/main.rs:10-12` (unstaged):\n```diff\n fn main() {\n-    old();\n+    new();\n }\n```"
        );
        assert_eq!(
            quote("src/main.rs", "staged", &lines[3..4]).unwrap(),
            "`src/main.rs:11` (staged):\n```diff\n+    new();\n```"
        );
        assert_eq!(quote("a", "staged", &lines[..1]), None);
    }

    #[test]
    fn removed_lines_use_old_numbers_and_fences_outgrow_backticks() {
        let lines = [
            line(DiffKind::Removed, "-let s = \"````\";", Some(4), None),
            line(DiffKind::Removed, "-done();", Some(5), None),
        ];
        let text = quote("a.rs", "unstaged", &lines).unwrap();
        assert!(
            text.starts_with("`a.rs:4-5, removed` (unstaged):\n`````diff\n"),
            "{text}"
        );
        assert!(text.ends_with("\n`````"));
    }

    #[test]
    fn long_picks_are_summarized() {
        let lines: Vec<_> = (1..=QUOTE_LIMIT + 5)
            .map(|n| line(DiffKind::Added, "+x", None, Some(n)))
            .collect();
        let text = quote("a", "unstaged", &lines).unwrap();
        assert!(text.starts_with(&format!("`a:1-{}`", QUOTE_LIMIT + 5)));
        assert!(text.contains("… 5 more lines\n```"));
        assert_eq!(text.matches("+x").count(), QUOTE_LIMIT);
    }
}

/// Immutable historical Changes selection.
pub mod recorded;
