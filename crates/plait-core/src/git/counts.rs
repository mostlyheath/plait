//! Uncommitted added and removed lines for a few named paths, such as the
//! files one thread wrote. A hardened read: no filters, textconv, or external
//! diff. Blocking: call on a background thread.

use super::{
    Output, Result, STATUS_LIMIT, TIMEOUT, execute, filter_overrides, open_beneath,
    path_from_bytes, read_command_with, run, validate_relative,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// Paths beyond this are left out; the host sends at most 500.
const PATH_LIMIT: usize = 500;
/// Untracked files larger than this are listed without a count.
const READ_LIMIT: u64 = 4 * 1024 * 1024;

/// One path that differs from HEAD. Counts are `None` for binary files and
/// untracked files too large to count.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LineCount {
    #[serde(with = "crate::wire::path")]
    pub path: PathBuf,
    pub added: Option<usize>,
    pub removed: Option<usize>,
    /// Not tracked by Git yet.
    pub untracked: bool,
}

/// Staged and unstaged changes together, against HEAD, for `paths` relative to
/// `root`. Paths without changes are left out; the order of `paths` is kept.
pub fn line_counts(root: &Path, paths: &[PathBuf]) -> Result<Vec<LineCount>> {
    // A plain folder has no HEAD to count against.
    if super::plain_folder(root) {
        return Ok(Vec::new());
    }
    let paths = &paths[..paths.len().min(PATH_LIMIT)];
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    for path in paths {
        validate_relative(path)?;
    }
    // Three processes in all: the configuration read, the diff, and the listing.
    let overrides = filter_overrides(root)?;
    let mut found = tracked(root, &overrides, paths)?;
    for path in untracked(root, &overrides, paths)? {
        let added = count_lines(root, &path);
        found.push(LineCount {
            path,
            added,
            removed: added.map(|_| 0),
            untracked: true,
        });
    }
    found.sort_by_key(|count| paths.iter().position(|path| *path == count.path));
    Ok(found)
}

fn tracked(root: &Path, overrides: &[String], paths: &[PathBuf]) -> Result<Vec<LineCount>> {
    let numstat = |base: &str| {
        let mut command = read_command_with(root, overrides);
        command
            .args([
                "diff",
                base,
                "--numstat",
                "-z",
                "--no-renames",
                "--no-ext-diff",
            ])
            .args(["--no-textconv", "--"])
            .args(paths);
        run(&mut command, None, STATUS_LIMIT, TIMEOUT)
    };
    let mut raw = numstat("HEAD")?;
    // Before the first commit there is no HEAD; everything staged is new.
    if !raw.status.success() && String::from_utf8_lossy(&raw.err.bytes).contains("HEAD") {
        raw = numstat("--cached")?;
    }
    if !raw.status.success() {
        return Err(raw.failure());
    }
    if raw.out.truncated {
        return Err("Line counts exceed the safe read limit.".into());
    }
    parse_numstat(&raw.out.bytes)
}

/// `added\tremoved\tpath` records, NUL-terminated; `-` counts mean binary.
pub(super) fn parse_numstat(bytes: &[u8]) -> Result<Vec<LineCount>> {
    let mut counts = Vec::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let mut fields = record.splitn(3, |b| *b == b'\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err("Git returned an unreadable line count.".into());
        };
        let number = |field: &[u8]| std::str::from_utf8(field).ok()?.parse().ok();
        let path = path_from_bytes(path);
        validate_relative(&path)?;
        counts.push(LineCount {
            path,
            added: number(added),
            removed: number(removed),
            untracked: false,
        });
    }
    Ok(counts)
}

fn untracked(root: &Path, overrides: &[String], paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut command = read_command_with(root, overrides);
    command
        .args(["ls-files", "-z", "--others", "--exclude-standard", "--"])
        .args(paths);
    let output: Output = execute(&mut command, STATUS_LIMIT, TIMEOUT)?;
    let mut found = Vec::new();
    for name in output.bytes.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let path = path_from_bytes(name);
        validate_relative(&path)?;
        found.push(path);
    }
    Ok(found)
}

/// Lines in a new text file, read without following symlinks. `None` for
/// binary, oversized, or unreadable files.
fn count_lines(root: &Path, path: &Path) -> Option<usize> {
    let file = open_beneath(root, path, libc::O_RDONLY, 0).ok()?;
    if file.metadata().ok()?.len() > READ_LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(READ_LIMIT).read_to_end(&mut bytes).ok()?;
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return None;
    }
    let lines = bytes.iter().filter(|b| **b == b'\n').count();
    Some(lines + usize::from(bytes.last().is_some_and(|b| *b != b'\n')))
}
