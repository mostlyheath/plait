pub mod write;
pub use write::{Backup, WriteOp, WriteOutcome};
pub mod branch;
pub mod counts;
pub mod log;
pub mod record;

pub mod worktree;

pub use branch::{BRANCH_LIMIT, Branch, Branches};
pub use counts::LineCount;
pub use log::{Commit, History};
/// More paths than this are clipped; a picker cannot usefully show them anyway.
pub const FILE_LIST_LIMIT: usize = 200_000;

/// Files a prompt can name: tracked, plus untracked files that are not ignored.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileList {
    /// Checkout-relative paths with `/` separators, sorted and unique.
    pub paths: Vec<String>,
    /// The checkout has more files than the output or count limit allows.
    pub truncated: bool,
    /// Paths left out because they cannot be written into a prompt:
    /// non-UTF-8 names and names with control characters.
    pub skipped: usize,
    pub elapsed: std::time::Duration,
}

use crate::{
    model::{Diff, DiffKind, DiffLine},
    patch::{self, FileHeaders},
};
use std::{sync::Arc, time::Duration};
pub fn display_line(line: &str) -> String {
    let mut text: String = line.chars().take(2000).collect();
    if line.chars().count() > 2000 {
        text.push_str(" … [line clipped]");
    }
    text.replace('\t', "    ")
}

pub fn format_diff(
    bytes: &[u8],
    mut truncated: bool,
    elapsed: Duration,
    headers: FileHeaders,
) -> Diff {
    let mut lines = Vec::new();
    let mut hunks = Vec::new();
    let (mut old, mut new) = (None, None);
    for line in String::from_utf8_lossy(bytes).lines().take(10_001) {
        if lines.len() == 10_000 {
            truncated = true;
            break;
        }
        if line.starts_with("diff ") {
            old = None;
            new = None;
        }
        if let Some(hunk) = line.strip_prefix("@@ ") {
            let mut ranges = hunk.split_whitespace();
            old = ranges
                .next()
                .and_then(|v| v.strip_prefix('-'))
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.parse::<usize>().ok());
            new = ranges
                .next()
                .and_then(|v| v.strip_prefix('+'))
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.parse::<usize>().ok());
        }
        let kind = if line.starts_with("@@") {
            DiffKind::Hunk
        } else if old.is_none() && new.is_none() {
            DiffKind::Header
        } else if line.starts_with('+') {
            DiffKind::Added
        } else if line.starts_with('-') {
            DiffKind::Removed
        } else {
            DiffKind::Context
        };
        let old_line = if matches!(kind, DiffKind::Removed) || line.starts_with(' ') {
            old
        } else {
            None
        };
        let new_line = if matches!(kind, DiffKind::Added) || line.starts_with(' ') {
            new
        } else {
            None
        };
        if old_line.is_some() {
            old = old.map(|n| n.saturating_add(1));
        }
        if new_line.is_some() {
            new = new.map(|n| n.saturating_add(1));
        }
        if matches!(kind, DiffKind::Hunk) {
            hunks.push(lines.len());
        }
        lines.push(DiffLine {
            text: Arc::from(display_line(line)),
            kind,
            old_line,
            new_line,
        });
    }
    if lines.is_empty() {
        return Diff::message(
            "No text diff in this layer. The file may have changed; Refresh to update status.",
            elapsed,
        );
    }
    let patch::Rows { rows, widest } = patch::rows(&lines, headers);
    Diff {
        lines,
        rows,
        widest_row: widest,
        hunks,
        truncated,
        elapsed,
    }
}

/// Prepare a recorded patch for display.
pub fn recorded_diff(bytes: &[u8], truncated: bool, elapsed: Duration) -> Diff {
    let display = crate::display_text::escape(&String::from_utf8_lossy(bytes));
    format_diff(display.as_bytes(), truncated, elapsed, FileHeaders::Omit)
}
