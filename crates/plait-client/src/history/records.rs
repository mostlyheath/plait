//! Complete record pages and run-to-row projection. Call off the UI thread.
use super::Key;
use crate::{
    git::record::{Reason, Uncovered},
    host::{self, Client, MessageRole, RecordKind, RecordState, Request, Snapshot, TurnRecord},
    transcript::{self, Row},
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Visible previews stay small even when the retained record lists every path.
pub const PREVIEW_LIMIT: usize = 12;
pub fn preview_name(name: &str) -> String {
    let name = crate::display_text::escape(name);
    let mut text: String = name
        .chars()
        .take(240)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if name.chars().nth(240).is_some() {
        text.push('…');
    }
    text
}

/// Read the entire retained list. A cursor can continue any of its three lists.
pub fn read(client: &Client, thread: &str) -> Result<Vec<TurnRecord>, host::Error> {
    let mut records = Vec::new();
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let page = client.request::<host::TurnRecordsReply>(&Request::turn_records(
            thread.into(),
            cursor,
            None,
        ))?;
        append(&mut records, page.records)?;
        cursor = page.before;
        match &cursor {
            None => break,
            Some(cursor) if seen.insert(cursor.clone()) => {}
            Some(_) => return Err(invalid()),
        }
    }
    if records.iter().any(|r| !complete(r)) {
        return Err(invalid());
    }
    Ok(records)
}
fn invalid() -> host::Error {
    host::Error::Protocol(
        "The host returned inconsistent turn-record pages. Refresh to try again.".into(),
    )
}
fn complete(r: &TurnRecord) -> bool {
    r.paths.len() == r.paths_total
        && r.request_ids.len() == r.request_ids_total
        && r.not_covered.len() == r.not_covered_total
}
fn append(records: &mut Vec<TurnRecord>, fragments: Vec<TurnRecord>) -> Result<(), host::Error> {
    for mut part in fragments {
        if let Some(last) = records.last_mut().filter(|last| last.id == part.id) {
            if last.paths.len() != part.paths_offset
                || last.request_ids.len() != part.request_ids_offset
                || last.not_covered.len() != part.not_covered_offset
                || last.paths_total != part.paths_total
                || last.request_ids_total != part.request_ids_total
                || last.not_covered_total != part.not_covered_total
                || last.kind != part.kind
                || last.state != part.state
                || last.undoes != part.undoes
                || last.created_at != part.created_at
                || last.head_moved != part.head_moved
                || last.after_restart != part.after_restart
            {
                return Err(invalid());
            }
            last.paths.append(&mut part.paths);
            last.request_ids.append(&mut part.request_ids);
            last.not_covered.append(&mut part.not_covered);
        } else {
            if records.last().is_some_and(|r| !complete(r))
                || records.iter().any(|r| r.id == part.id)
                || part.paths_offset != 0
                || part.request_ids_offset != 0
                || part.not_covered_offset != 0
            {
                return Err(invalid());
            }
            records.push(part);
        }
        let last = records.last().unwrap();
        if last.paths.len() > last.paths_total
            || last.request_ids.len() > last.request_ids_total
            || last.not_covered.len() > last.not_covered_total
        {
            return Err(invalid());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Recording,
    Ready,
    Failed,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: PathBuf,
    pub name: String,
    pub counts: String,
    pub review_detail: String,
    pub current_detail: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Original run identity; stable across undo and redo.
    pub run: String,
    /// Exact record whose paths are displayed; independent of the undo target.
    pub displayed: String,
    /// The usable leaf of the undo chain, never the provisional before ID.
    pub target: Option<String>,
    pub availability: Availability,
    pub undone: bool,
    pub steps: Option<usize>,
    pub files: Vec<File>,
    pub not_covered: Vec<Uncovered>,
    pub detail: String,
    pub review_detail: String,
    pub more_label: String,
    pub more_detail: String,
    pub after_restart: bool,
    pub head_moved: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    pub rows: HashMap<Key, Line>,
    pub latest: Option<Line>,
    /// Suppresses message-based summaries inside a run joined by steering.
    pub covered: HashSet<Key>,
    pub runs: HashMap<String, Line>,
}

pub fn coverage_reason(reason: Reason) -> &'static str {
    match reason {
        Reason::IgnoredDirectory => "ignored folder",
        Reason::IgnoredLarge => "ignored file over 1 MiB",
        Reason::IgnoredLimit => "ignored-file count or size limit",
        Reason::TooLarge => "file over 100 MiB",
        Reason::NestedRepository => "nested repository",
        Reason::Submodule => "submodule",
        Reason::Special => "unsupported file type",
        Reason::Name => "unsupported file name",
        Reason::Changing => "changed while recording",
        Reason::Unreadable => "could not be read",
    }
}

fn line(
    original: &TurnRecord,
    children: &HashMap<&str, &TurnRecord>,
    steps: Option<usize>,
) -> Line {
    let mut current = original;
    let mut depth = 0;
    let mut seen = HashSet::from([original.id.as_str()]);
    while let Some(child) = children.get(current.id.as_str()) {
        if !seen.insert(child.id.as_str()) {
            break;
        }
        current = child;
        depth += 1;
    }
    let availability = match current.state {
        RecordState::Recording => Availability::Recording,
        RecordState::Failed | RecordState::NoEffect => Availability::Failed,
        RecordState::Ready => Availability::Ready,
        RecordState::Undone => Availability::Unavailable,
    };
    let files: Vec<_> = current
        .paths
        .iter()
        .map(|file| {
            let path = crate::display_text::escape(&file.path.to_string_lossy());
            File {
                path: file.path.clone(),
                name: crate::display_text::escape(
                    &file
                        .path
                        .file_name()
                        .unwrap_or(file.path.as_os_str())
                        .to_string_lossy(),
                )
                .replace(['\n', '\r', '\t'], " "),
                current_detail: format!(
                    "Open {} in current Changes. Turn review needs a newer host.",
                    path
                ),
                review_detail: if current.kind == RecordKind::Undo {
                    format!("Review {} restored by this undo.", path)
                } else {
                    match (file.added, file.removed) {
                        (Some(a), Some(r)) => {
                            format!("Review {} recorded during this turn (+{a} −{r}).", path)
                        }
                        _ => format!("Review {} recorded during this turn.", path),
                    }
                },
                counts: match (file.added, file.removed) {
                    (Some(a), Some(r)) => format!("+{a} −{r}"),
                    _ => String::new(),
                },
            }
        })
        .collect();
    let not_covered: Vec<_> = original
        .not_covered
        .iter()
        .chain(&current.not_covered)
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let after_restart = original.after_restart || current.after_restart;
    let head_moved = original.head_moved || current.head_moved;
    let mut detail = match availability {
        Availability::Recording => "The host is still taking this run's record.".into(),
        Availability::Failed => {
            "The record could not be made, so this run cannot be undone.".into()
        }
        Availability::Unavailable => "The undo record is no longer available.".into(),
        Availability::Ready => String::from(
            "Restores recorded file content, executable bits and symlinks. Other permissions, extended attributes, hard links, Git history and outside-checkout effects are not restored.",
        ),
    };
    if after_restart {
        detail.push_str(
            "\nAfter restart: this record may include changes made while the host was down.",
        );
    }
    if head_moved {
        detail.push_str("\nHEAD moved during this run. Undo does not restore HEAD or the branch.");
    }
    if steps.is_none() {
        detail.push_str("\nEarlier messages are not loaded; the step count is unavailable.");
    }
    for file in files.iter().take(PREVIEW_LIMIT) {
        detail.push_str(&format!(
            "\n{} {}",
            preview_name(&file.path.to_string_lossy()),
            file.counts
        ));
    }
    if files.len() > PREVIEW_LIMIT {
        detail.push_str(&format!(
            "\n{} more changed paths",
            files.len() - PREVIEW_LIMIT
        ));
    }
    if !not_covered.is_empty() {
        detail.push_str("\nNot covered:");
        for item in not_covered.iter().take(PREVIEW_LIMIT) {
            detail.push_str(&format!(
                "\n{} — {}",
                preview_name(&item.path),
                coverage_reason(item.reason)
            ));
        }
        if not_covered.len() > PREVIEW_LIMIT {
            detail.push_str(&format!(
                "\n{} more not-covered paths",
                not_covered.len() - PREVIEW_LIMIT
            ));
        }
    }
    let review_detail = match availability {
        Availability::Ready => "Review this turn's recorded changes.",
        Availability::Recording => "The host is still recording this turn.",
        Availability::Failed => "This turn's after record could not be made.",
        Availability::Unavailable => {
            "This record is no longer retained. Turn records are kept for 30 days."
        }
    }
    .into();
    let more_label = format!("{} more", files.len().saturating_sub(3));
    let more_detail = format!("Review all {} files in this record.", files.len());
    Line {
        review_detail,
        more_label,
        more_detail,
        run: original.id.clone(),
        displayed: current.id.clone(),
        target: (availability == Availability::Ready).then(|| current.id.clone()),
        availability,
        undone: depth % 2 == 1,
        steps,
        files,
        not_covered,
        detail,
        after_restart,
        head_moved,
    }
}

/// A message with both reasoning and text has two UI rows. Its closing line
/// belongs to the text row; reasoning alone can still close a stopped run.
pub fn closing_key(row: Row<'_>) -> Option<Key> {
    match row {
        Row::Reasoning(message) if !message.text.is_empty() => None,
        _ => Key::row(row),
    }
}

/// A run closes only after its last joined request. A detached history page
/// cannot close its final run: the answer may continue beyond the loaded page.
pub fn project(snapshot: &Snapshot, records: &[TurnRecord], live_end: bool) -> Lines {
    let mut members = HashMap::new();
    let mut children = HashMap::new();
    for record in records {
        if record.kind == RecordKind::Turn {
            for request in &record.request_ids {
                members.insert(format!("request-{request}"), record);
            }
        } else if record.state != RecordState::NoEffect
            && let Some(target) = &record.undoes
        {
            // Pages are newest first; keep the newest child if a host lists two.
            children.entry(target.as_str()).or_insert(record);
        }
    }
    let mut result = Lines::default();
    let mut active: Option<&TurnRecord> = None;
    let mut closing = None;
    let mut steps = None;
    let mut last_member = false;
    let finish = |result: &mut Lines,
                  active: Option<&TurnRecord>,
                  closing: &Option<Key>,
                  steps,
                  last_member,
                  latest| {
        if let (Some(record), Some(key), true) = (active, closing, last_member) {
            let line = line(record, &children, steps);
            result.runs.insert(line.run.clone(), line.clone());
            if latest {
                result.latest = Some(line);
            } else {
                result.rows.insert(key.clone(), line);
            }
        }
    };
    for row in transcript::rows(snapshot) {
        if let Row::Message(message) = row
            && message.role == MessageRole::User
        {
            let next = members.get(&message.id).copied();
            if active.map(|r| &r.id) != next.map(|r| &r.id) {
                finish(&mut result, active, &closing, steps, last_member, false);
                active = next;
                steps = next
                    .and_then(|r| r.request_ids.first())
                    .filter(|id| message.id == format!("request-{id}"))
                    .map(|_| 0);
                closing = None;
            }
            last_member = next
                .and_then(|r| r.request_ids.last())
                .is_some_and(|id| message.id == format!("request-{id}"));
        }
        if active.is_some() {
            if matches!(row, Row::Tool(_)) {
                steps = steps.map(|n| n + 1);
            }
            if let Some(key) = Key::row(row) {
                result.covered.insert(key.clone());
                if closing_key(row).is_some() {
                    closing = Some(key);
                }
            }
        }
    }
    if live_end && !snapshot.status.busy() {
        finish(&mut result, active, &closing, steps, last_member, true);
    }
    result
}

#[cfg(test)]
mod tests;
