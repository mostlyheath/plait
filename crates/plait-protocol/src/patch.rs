//! Which rows a Git patch draws, worked out once in the background.
//!
//! A diff should start at the change. Above one file's diff the view already
//! names the file, so the header lines that only repeat its path
//! (`diff --git`, `index`, `---`, `+++`) are left out. A commit's patch can
//! hold several files, so there each file's `diff` line becomes one heading
//! row instead. Header lines that say something else — modes, renames,
//! similarity, binary notices — are kept. Every row points at the patch line
//! it stands for, so picking lines, staging hunks, and the line count still
//! use the real patch.

use crate::model::{DiffKind, DiffLine};
use std::sync::Arc;

/// What happens to the header lines that only name a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileHeaders {
    /// One file, named above the diff: its path-only lines are not drawn.
    Omit,
    /// Possibly several files: each file's lines become one heading row.
    Heading,
}

/// One drawn row of a patch.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PatchRow {
    /// The patch line at this index into `Diff::lines`, drawn as itself.
    Line(usize),
    /// A file's heading, standing for its `diff` line at `line`.
    File {
        line: usize,
        name: Arc<str>,
        directory: Arc<str>,
    },
}

impl PatchRow {
    /// The patch line this row stands for.
    pub fn line(&self) -> usize {
        match self {
            Self::Line(line) | Self::File { line, .. } => *line,
        }
    }
}

/// The drawn rows of a patch and the widest one, as an index into `rows`.
#[derive(Debug, PartialEq, Eq)]
pub struct Rows {
    pub rows: Vec<PatchRow>,
    pub widest: usize,
}

/// The rows drawn for `lines`. Lines before the first `diff` line, such as a
/// commit's message, are never left out. If nothing would remain, every line
/// is drawn: an empty view would hide that Git reported something.
pub fn rows(lines: &[DiffLine], headers: FileHeaders) -> Rows {
    let mut rows = Vec::with_capacity(lines.len());
    let mut in_file = false;
    for (index, line) in lines.iter().enumerate() {
        // Only header lines are candidates: a removed line reading `--- a`
        // is part of a hunk and kind-checked out of here.
        if line.kind != DiffKind::Header {
            rows.push(PatchRow::Line(index));
        } else if line.text.starts_with("diff ") {
            in_file = true;
            if headers == FileHeaders::Heading {
                rows.push(heading(lines, index));
            }
        } else if !(in_file && repeats_path(&line.text)) {
            rows.push(PatchRow::Line(index));
        }
    }
    if rows.is_empty() {
        rows = (0..lines.len()).map(PatchRow::Line).collect();
    }
    let widest = widest(lines, &rows);
    Rows { rows, widest }
}

/// A file header line whose only content is the file's path or blob IDs.
fn repeats_path(text: &str) -> bool {
    ["index ", "--- ", "+++ "]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

/// The drawn line with the most text; headings truncate, so they never set the width.
fn widest(lines: &[DiffLine], rows: &[PatchRow]) -> usize {
    rows.iter()
        .enumerate()
        .filter_map(|(row, patch)| match patch {
            PatchRow::Line(line) => Some((row, lines[*line].text.len())),
            PatchRow::File { .. } => None,
        })
        .max_by_key(|(_, width)| *width)
        .map_or(0, |(row, _)| row)
}

/// The heading for the file whose `diff` line is `lines[at]`, named from the
/// header lines that follow it, up to its first hunk or the next file.
fn heading(lines: &[DiffLine], at: usize) -> PatchRow {
    let header = lines[at + 1..]
        .iter()
        .take_while(|line| line.kind == DiffKind::Header && !line.text.starts_with("diff "))
        .map(|line| &*line.text);
    let path = file_path(&lines[at].text, header);
    let (directory, name) = path.rsplit_once('/').unwrap_or(("", &path));
    PatchRow::File {
        line: at,
        name: Arc::from(name),
        directory: Arc::from(directory),
    }
}

/// A file's path for display: the new side, else a rename or copy target,
/// else the old side (a deleted file), else read from the `diff` line itself
/// (binary files and mode-only changes have no `---`/`+++` lines).
fn file_path<'a>(diff_line: &str, header: impl Iterator<Item = &'a str>) -> String {
    let (mut new, mut target, mut old) = (None, None, None);
    for text in header {
        if let Some(path) = text.strip_prefix("+++ ") {
            new = side_path(path, "b/");
        } else if let Some(path) = text.strip_prefix("--- ") {
            old = side_path(path, "a/");
        } else if let Some(path) = text
            .strip_prefix("rename to ")
            .or_else(|| text.strip_prefix("copy to "))
        {
            target = Some(unquote(path));
        }
    }
    new.or(target)
        .or(old)
        .unwrap_or_else(|| diff_line_path(diff_line))
}

/// A `---`/`+++` path without its side prefix; None for `/dev/null`. Git
/// follows a path containing a space with a tab, shown as trailing spaces.
fn side_path(text: &str, prefix: &str) -> Option<String> {
    let path = unquote(text.trim_end());
    if path == "/dev/null" {
        return None;
    }
    Some(
        path.strip_prefix(prefix)
            .map_or(path.clone(), str::to_owned),
    )
}

/// The path in `diff --git a/P b/P`, `diff --cc P`, or `diff --combined P`.
/// Both sides of `--git` name the same file here (a rename has `rename to`),
/// which is what splits a path with spaces in it. Anything else is shown as written.
fn diff_line_path(text: &str) -> String {
    let Some(sides) = text.strip_prefix("diff --git ") else {
        let path = ["diff --cc ", "diff --combined "]
            .iter()
            .find_map(|prefix| text.strip_prefix(prefix));
        return path.map_or_else(|| text.to_owned(), unquote);
    };
    if sides.starts_with('"') {
        // Quoted sides: the second starts after the first's closing quote.
        return closing_quote(sides)
            .and_then(|end| sides.get(end + 2..))
            .map(|second| {
                let path = unquote(second);
                path.strip_prefix("b/").map_or(path.clone(), str::to_owned)
            })
            .unwrap_or_else(|| sides.to_owned());
    }
    let middle = sides.len() / 2;
    let (Some(first), Some(second)) = (sides.get(..middle), sides.get(middle + 1..)) else {
        return sides.to_owned();
    };
    if sides.len() % 2 == 0 || sides.as_bytes()[middle] != b' ' {
        return sides.to_owned();
    }
    if first == second {
        // `diff.noprefix`: no side prefixes at all.
        return second.to_owned();
    }
    match (first.get(2..), second.get(2..)) {
        (Some(a), Some(b)) if a == b => b.to_owned(),
        _ => sides.to_owned(),
    }
}

/// The byte index of the quote that closes a quoted path starting at 0.
fn closing_quote(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate().skip(1) {
        match byte {
            b'\\' if !escaped => escaped = true,
            b'"' if !escaped => return Some(index),
            _ => escaped = false,
        }
    }
    None
}

/// Undo Git's C-style quoting of a path (`"caf\303\251"` → `café`). Control
/// characters stay escaped, as in the changed-file list.
fn unquote(text: &str) -> String {
    let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else {
        return text.to_owned();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut rest = inner.bytes().peekable();
    while let Some(byte) = rest.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        let Some(code) = rest.next() else {
            bytes.push(b'\\');
            break;
        };
        bytes.push(match code {
            b'0'..=b'7' => {
                let mut value = u32::from(code - b'0');
                for _ in 0..2 {
                    match rest.peek() {
                        Some(digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            rest.next();
                        }
                        _ => break,
                    }
                }
                value as u8
            }
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            other => other,
        });
    }
    String::from_utf8_lossy(&bytes)
        .chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

/// Where a hunk header's ranges (`@@ -1,5 +1,6 @@`) end and the context Git
/// adds after them, often the enclosing function, begins. Combined diffs mark
/// hunks with `@@@`, so the closing marker matches the opening one. A header
/// without a closing marker is all ranges.
pub fn hunk_ranges_end(text: &str) -> usize {
    let marker = text.bytes().take_while(|b| *b == b'@').count();
    if marker == 0 {
        return text.len();
    }
    let close = &text[..marker];
    text[marker..]
        .find(close)
        .map_or(text.len(), |at| marker + at + marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as `format_diff` classifies them: headers until the first hunk
    /// of each file, then hunk content.
    fn patch(text: &str) -> Vec<DiffLine> {
        let mut in_hunk = false;
        text.lines()
            .map(|line| {
                if line.starts_with("diff ") {
                    in_hunk = false;
                }
                let kind = if line.starts_with("@@") {
                    in_hunk = true;
                    DiffKind::Hunk
                } else if !in_hunk {
                    DiffKind::Header
                } else if line.starts_with('+') {
                    DiffKind::Added
                } else if line.starts_with('-') {
                    DiffKind::Removed
                } else {
                    DiffKind::Context
                };
                DiffLine {
                    text: Arc::from(line),
                    kind,
                    old_line: None,
                    new_line: None,
                }
            })
            .collect()
    }

    /// Each drawn row as text: a heading as `# directory | name`.
    fn drawn(text: &str, headers: FileHeaders) -> Vec<String> {
        let lines = patch(text);
        let Rows { rows, widest } = rows(&lines, headers);
        assert!(widest < rows.len().max(1));
        // Every row stands for a real patch line, in patch order.
        assert!(rows.windows(2).all(|w| w[0].line() < w[1].line()));
        rows.iter()
            .map(|row| match row {
                PatchRow::Line(line) => lines[*line].text.to_string(),
                PatchRow::File {
                    line,
                    name,
                    directory,
                } => {
                    assert!(lines[*line].text.starts_with("diff "));
                    format!("# {directory} | {name}")
                }
            })
            .collect()
    }

    const MODIFIED: &str = "diff --git a/src/app.rs b/src/app.rs\n\
        index 1111111..2222222 100644\n\
        --- a/src/app.rs\n\
        +++ b/src/app.rs\n\
        @@ -1,2 +1,2 @@ fn main() {\n\
        -old\n\
        +new\n\
        --- a/src/app.rs\n";

    #[test]
    fn a_modified_file_starts_at_its_first_hunk() {
        assert_eq!(
            drawn(MODIFIED, FileHeaders::Omit),
            [
                "@@ -1,2 +1,2 @@ fn main() {",
                "-old",
                "+new",
                // A removed line that reads like a header is still content.
                "--- a/src/app.rs"
            ]
        );
        assert_eq!(
            drawn(MODIFIED, FileHeaders::Heading)[..2],
            ["# src | app.rs", "@@ -1,2 +1,2 @@ fn main() {"]
        );
    }

    #[test]
    fn rows_point_at_patch_lines_for_picking_and_hunks() {
        let lines = patch(MODIFIED);
        let Rows { rows, widest } = rows(&lines, FileHeaders::Omit);
        let at: Vec<usize> = rows.iter().map(PatchRow::line).collect();
        assert_eq!(at, [4, 5, 6, 7]);
        // The widest drawn line, not the hidden `diff --git` line.
        assert_eq!(rows[widest], PatchRow::Line(4));
    }

    #[test]
    fn a_new_file_keeps_its_mode_but_not_dev_null() {
        let new = "diff --git a/notes.md b/notes.md\n\
            new file mode 100644\n\
            index 0000000..3333333\n\
            --- /dev/null\n\
            +++ b/notes.md\n\
            @@ -0,0 +1 @@\n\
            +# Notes\n";
        assert_eq!(
            drawn(new, FileHeaders::Omit),
            ["new file mode 100644", "@@ -0,0 +1 @@", "+# Notes"]
        );
        assert_eq!(
            drawn(new, FileHeaders::Heading)[..2],
            ["#  | notes.md", "new file mode 100644"]
        );
    }

    #[test]
    fn a_deleted_file_is_named_from_its_old_side() {
        let deleted = "diff --git a/docs/old.md b/docs/old.md\n\
            deleted file mode 100644\n\
            index 3333333..0000000\n\
            --- a/docs/old.md\n\
            +++ /dev/null\n\
            @@ -1 +0,0 @@\n\
            -# Old\n";
        assert_eq!(
            drawn(deleted, FileHeaders::Omit),
            ["deleted file mode 100644", "@@ -1 +0,0 @@", "-# Old"]
        );
        assert_eq!(drawn(deleted, FileHeaders::Heading)[0], "# docs | old.md");
    }

    #[test]
    fn a_rename_keeps_its_similarity_and_both_names() {
        let rename = "diff --git a/src/a.rs b/lib/b.rs\n\
            similarity index 90%\n\
            rename from src/a.rs\n\
            rename to lib/b.rs\n\
            index 1111111..2222222 100644\n\
            --- a/src/a.rs\n\
            +++ b/lib/b.rs\n\
            @@ -1 +1 @@\n\
            -a\n\
            +b\n";
        let kept = [
            "similarity index 90%",
            "rename from src/a.rs",
            "rename to lib/b.rs",
        ];
        assert_eq!(drawn(rename, FileHeaders::Omit)[..3], kept);
        let headed = drawn(rename, FileHeaders::Heading);
        assert_eq!(headed[0], "# lib | b.rs");
        assert_eq!(headed[1..4], kept);
        // A pure rename has no `+++` line; the target names it.
        let pure = "diff --git a/a.rs b/b.rs\n\
            similarity index 100%\n\
            rename from a.rs\n\
            rename to b.rs\n";
        assert_eq!(drawn(pure, FileHeaders::Heading)[0], "#  | b.rs");
    }

    #[test]
    fn a_mode_change_keeps_both_modes() {
        let mode = "diff --git a/run me.sh b/run me.sh\n\
            old mode 100644\n\
            new mode 100755\n";
        assert_eq!(
            drawn(mode, FileHeaders::Omit),
            ["old mode 100644", "new mode 100755"]
        );
        // No `+++` line: the path, spaces and all, comes from the `diff` line.
        assert_eq!(
            drawn(mode, FileHeaders::Heading),
            ["#  | run me.sh", "old mode 100644", "new mode 100755"]
        );
    }

    #[test]
    fn a_binary_file_keeps_its_notice() {
        let binary = "diff --git \"a/img/caf\\303\\251.png\" \"b/img/caf\\303\\251.png\"\n\
            index 1111111..2222222 100644\n\
            Binary files \"a/img/caf\\303\\251.png\" and \"b/img/caf\\303\\251.png\" differ\n";
        let notice =
            "Binary files \"a/img/caf\\303\\251.png\" and \"b/img/caf\\303\\251.png\" differ";
        assert_eq!(drawn(binary, FileHeaders::Omit), [notice]);
        assert_eq!(
            drawn(binary, FileHeaders::Heading),
            ["# img | café.png", notice]
        );
    }

    #[test]
    fn a_commit_patch_heads_each_file_and_keeps_its_message() {
        let commit = format!(
            "commit 0123\nAuthor: A <a@example.invalid>\n\n    Index the files\n\
             {MODIFIED}diff --git a/README.md b/README.md\n\
             index 1111111..2222222 100644\n\
             --- a/README.md\n\
             +++ b/README.md\n\
             @@ -1 +1 @@\n\
             -a\n\
             +b\n"
        );
        assert_eq!(
            drawn(&commit, FileHeaders::Heading),
            [
                "commit 0123",
                "Author: A <a@example.invalid>",
                "",
                "    Index the files",
                "# src | app.rs",
                "@@ -1,2 +1,2 @@ fn main() {",
                "-old",
                "+new",
                "--- a/src/app.rs",
                "#  | README.md",
                "@@ -1 +1 @@",
                "-a",
                "+b",
            ]
        );
    }

    #[test]
    fn a_file_with_no_hunks_still_draws_something() {
        // An empty new file: only its mode says anything.
        let empty = "diff --git a/empty b/empty\n\
            new file mode 100644\n\
            index 0000000..e69de29\n";
        assert_eq!(drawn(empty, FileHeaders::Omit), ["new file mode 100644"]);
        assert_eq!(
            drawn(empty, FileHeaders::Heading),
            ["#  | empty", "new file mode 100644"]
        );
        // Nothing but path lines: draw them rather than an empty view.
        let bare = "diff --git a/x b/x\nindex 1111111..2222222 100644\n";
        assert_eq!(
            drawn(bare, FileHeaders::Omit),
            bare.lines().collect::<Vec<_>>()
        );
        // A message line is never mistaken for a file header.
        assert_eq!(
            drawn("Untracked file — preview only\n", FileHeaders::Omit).len(),
            1
        );
    }

    #[test]
    fn quoted_and_combined_paths_read_as_written() {
        assert_eq!(unquote("\"a\\tb\\\"c\\\\d\""), "a\\tb\"c\\d");
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(diff_line_path("diff --cc src/a.rs"), "src/a.rs");
        assert_eq!(diff_line_path("diff --git a/x b/x"), "x");
        assert_eq!(diff_line_path("diff --git x x"), "x");
        // Sides that do not name one file are shown as Git wrote them.
        assert_eq!(diff_line_path("diff --git a/x b/y"), "a/x b/y");
    }

    #[test]
    fn hunk_ranges_end_at_the_closing_marker() {
        let header = "@@ -1,5 +1,6 @@ fn name() {";
        assert_eq!(&header[..hunk_ranges_end(header)], "@@ -1,5 +1,6 @@");
        let combined = "@@@ -1,2 -1,2 +1,3 @@@ impl A";
        assert_eq!(
            &combined[..hunk_ranges_end(combined)],
            "@@@ -1,2 -1,2 +1,3 @@@"
        );
        assert_eq!(hunk_ranges_end("@@ -0,0 +1 @@"), 13);
        assert_eq!(hunk_ranges_end("@@ broken"), 9);
    }
}
