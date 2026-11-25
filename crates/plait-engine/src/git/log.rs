//! Commit history, read-only, with the same hardening as status and diff.

use super::{DIFF_LIMIT, Result, STATUS_LIMIT, command, format_diff, head_exists};
use crate::{model::Diff, patch::FileHeaders};
use std::{ffi::OsStr, path::Path, time::Instant};

/// A page larger than this is a UI bug, and bounds the output Git must produce.
const MAX_PAGE: usize = 500;
const FIELDS: usize = 5;

pub use plait_protocol::git::log::{Commit, History};

/// Up to `limit` commits reachable from `tip` (HEAD when `None`), skipping the
/// first `skip`.
/// An unborn branch has an empty history, not an error.
pub fn log(root: &Path, tip: Option<&str>, skip: usize, limit: usize) -> Result<History> {
    let limit = limit.min(MAX_PAGE);
    if tip.is_some_and(|tip| !is_object_id(tip)) {
        return Err("Not a full commit ID".into());
    }
    if limit == 0 || (tip.is_none() && !head_exists(root)?) {
        return Ok(History::default());
    }
    // Later pages start from the first page's newest commit, so a commit made
    // meanwhile cannot shift the pages and duplicate or skip entries.
    let start = tip.unwrap_or("HEAD");
    let skip = format!("--skip={skip}");
    // One more than asked, to learn whether another page exists.
    let count = format!("-n{}", limit + 1);
    let args = [
        "log",
        "--no-show-signature",
        "-z",
        // Fields are NUL-separated; -z makes NUL end each record too.
        "--format=%H%x00%h%x00%an%x00%at%x00%s",
        &count,
        &skip,
        "--end-of-options",
        start,
        "--",
    ]
    .map(OsStr::new);
    let output = command(root, &args, STATUS_LIMIT)?;
    if output.truncated {
        return Err("History page exceeds the safe read limit. Ask for fewer commits.".into());
    }
    let mut commits = parse_log(&output.bytes)?;
    let more = commits.len() > limit;
    commits.truncate(limit);
    Ok(History { commits, more })
}

pub(super) fn parse_log(bytes: &[u8]) -> Result<Vec<Commit>> {
    let mut fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    // The final piece is empty after the last record's terminator.
    if fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    if !fields.len().is_multiple_of(FIELDS) {
        return Err("Malformed Git history record".into());
    }
    fields
        .chunks(FIELDS)
        .map(|record| {
            let id = text(record[0]);
            if !is_object_id(&id) {
                return Err("Malformed Git history record".to_owned());
            }
            Ok(Commit {
                id,
                short: text(record[1]),
                author: display(record[2]),
                time: text(record[3])
                    .parse()
                    .map_err(|_| "Malformed Git commit time".to_owned())?,
                subject: display(record[4]),
            })
        })
        .collect()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Names and subjects are untrusted; control characters could disturb the UI.
fn display(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.trim().to_owned()
}

pub(super) fn is_object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One commit's message and patch, against its first parent, as a `Diff`.
/// `id` must be a full object ID from `log`.
pub fn show(root: &Path, id: &str) -> Result<Diff> {
    let start = Instant::now();
    if !is_object_id(id) {
        return Err("Not a full commit ID.".into());
    }
    let revision = format!("{id}^{{commit}}");
    let args = [
        "show",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-notes",
        "--no-show-signature",
        "--unified=3",
        "--diff-merges=first-parent",
        "--submodule=short",
        "--ignore-submodules=dirty",
        "--date=iso",
        // Message lines are indented so none can pass for a diff header or hunk.
        "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n%w(0,4,4)%B%w(0,0,0)",
        "--end-of-options",
        &revision,
        "--",
    ]
    .map(OsStr::new);
    let output = command(root, &args, DIFF_LIMIT)?;
    // A commit can touch several files; each gets a heading.
    let mut diff = format_diff(
        &output.bytes,
        output.truncated,
        start.elapsed(),
        FileHeaders::Heading,
    );
    diff.elapsed = start.elapsed();
    Ok(diff)
}

pub use plait_protocol::git::log::{GRAPH_LIMIT, GraphCommit};
pub fn graph(root: &Path, heads: &[String]) -> Result<(Vec<GraphCommit>, bool)> {
    if heads.is_empty() {
        return Ok((Vec::new(), false));
    }
    if heads.len() > 256 || heads.iter().any(|id| !is_object_id(id)) {
        return Err("Invalid worktree heads.".into());
    }
    let mut args = vec![
        "log",
        "--topo-order",
        "--no-ext-diff",
        "--no-textconv",
        "--no-notes",
        "--no-color",
        "--no-show-signature",
        "-z",
        "--format=%H%x00%h%x00%an%x00%at%x00%s%x00%P",
        "-n301",
        "--end-of-options",
    ];
    args.extend(heads.iter().map(String::as_str));
    args.push("--");
    let args: Vec<_> = args.iter().map(OsStr::new).collect();
    let raw = command(root, &args, STATUS_LIMIT)?;
    if raw.truncated {
        return Err("Worktree history exceeds the safe read limit.".into());
    }
    let mut fields: Vec<_> = raw.bytes.split(|b| *b == 0).collect();
    if fields.last().is_some_and(|s| s.is_empty()) {
        fields.pop();
    }
    if !fields.len().is_multiple_of(6) {
        return Err("Malformed worktree history.".into());
    }
    let mut commits = Vec::new();
    for fields in fields.chunks(6) {
        let mut record = fields[..5].join(&0);
        record.push(0);
        let commit = parse_log(&record)?.remove(0);
        let parents = text(fields[5])
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if parents.iter().any(|id| !is_object_id(id)) {
            return Err("Malformed worktree parent.".into());
        }
        commits.push(GraphCommit { commit, parents });
    }
    let more = commits.len() > GRAPH_LIMIT;
    commits.truncate(GRAPH_LIMIT);
    Ok((commits, more))
}
