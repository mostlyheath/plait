//! A bounded reading window over complete stored history and the current live tail.
//! All merging and text expansion belongs on a background worker.
use std::{collections::HashSet, sync::Arc};

use crate::{
    host::{
        Client, Error, HistoryField, HistoryItem, HistoryPage, HistoryText, Message, MessageRole,
        Request, Snapshot, ThreadStatus, Tool, ToolStatus,
    },
    transcript::{self, Row},
};

pub mod records;

pub const MAX_ITEMS: usize = 600;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
const PAGE_BYTES: usize = 2 * 1024 * 1024;
const PART_UNITS: u64 = 262_144;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Message(String),
    Tool(String),
}
impl Key {
    pub fn of(item: &HistoryItem) -> Self {
        match item {
            HistoryItem::Message { id, .. } => Self::Message(id.clone()),
            HistoryItem::Tool { id, .. } => Self::Tool(id.clone()),
        }
    }
    pub fn row(row: Row<'_>) -> Option<Self> {
        match row {
            Row::Message(m) | Row::Reasoning(m) => Some(Self::Message(m.id.clone())),
            Row::Tool(t) => Some(Self::Tool(t.id.clone())),
            _ => None,
        }
    }
}

/// Oversized fields are read in bounded, explicitly numbered parts. Start over
/// and Next part keep all text reachable without retaining an unbounded string.
#[derive(Clone, Debug)]
pub struct Part {
    pub field: HistoryField,
    pub reference: String,
    pub length: u64,
    pub start: u64,
    pub end: u64,
    pub number: usize,
}
impl Part {
    pub fn has_next(&self) -> bool {
        self.end < self.length
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub item: HistoryItem,
    pub parts: Vec<Part>,
    /// Re-read the containing page if a memory-bound eviction cuts through it.
    cursor: Option<String>,
}
impl Entry {
    /// Validate the inline pieces before reading any continuation.
    pub fn from_inline(mut item: HistoryItem, cursor: Option<String>) -> Result<Self, Error> {
        let mut parts = Vec::new();
        for more in item.more().to_vec() {
            let text = field_mut(&mut item, more.field)?;
            if text.encode_utf16().count() as u64 != more.offset
                || more.offset >= more.length
                || parts.iter().any(|part: &Part| part.field == more.field)
            {
                return Err(Error::Protocol(
                    "History continuation has an invalid offset or field.".into(),
                ));
            }
            parts.push(Part {
                field: more.field,
                reference: more.reference,
                length: more.length,
                start: 0,
                end: more.offset,
                number: 1,
            });
        }
        Ok(Self {
            item,
            parts,
            cursor,
        })
    }
    pub fn key(&self) -> Key {
        Key::of(&self.item)
    }
    fn bytes(&self) -> usize {
        match &self.item {
            HistoryItem::Message {
                text, reasoning, ..
            } => text.len() + reasoning.len(),
            HistoryItem::Tool { input, output, .. } => input.len() + output.len(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub entries: Vec<Arc<Entry>>,
    pub before: Option<String>,
}

#[derive(Clone, Default)]
pub struct History {
    pub entries: Vec<Arc<Entry>>,
    /// None means beginning; Some(None) means re-read the latest page.
    pub older: Option<Option<String>>,
    pub loaded: bool,
    /// Newer rows were evicted while reading backward. Jump to latest reopens them.
    pub detached: bool,
}
impl History {
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        self.entries.capacity() * size_of::<Arc<Entry>>()
            + self
                .entries
                .iter()
                .map(|e| {
                    let item = match &e.item {
                        HistoryItem::Message {
                            id,
                            text,
                            reasoning,
                            images,
                            more,
                            ..
                        } => {
                            id.capacity()
                                + text.capacity()
                                + reasoning.capacity()
                                + images.capacity() * size_of::<crate::images::ImageRef>()
                                + images
                                    .iter()
                                    .map(|i| i.id.capacity() + i.media_type.capacity())
                                    .sum::<usize>()
                                + more.capacity() * size_of::<crate::host::More>()
                        }
                        HistoryItem::Tool {
                            id,
                            name,
                            input,
                            output,
                            more,
                            ..
                        } => {
                            id.capacity()
                                + name.capacity()
                                + input.capacity()
                                + output.capacity()
                                + more.capacity() * size_of::<crate::host::More>()
                        }
                    };
                    size_of::<Entry>()
                        + item
                        + e.cursor.as_ref().map_or(0, String::capacity)
                        + e.parts.capacity() * size_of::<Part>()
                        + e.parts
                            .iter()
                            .map(|p| p.reference.capacity())
                            .sum::<usize>()
                        + e.item
                            .more()
                            .iter()
                            .map(|m| m.reference.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }

    /// Drop the farthest retained entry, keeping the visible semantic anchor.
    pub fn evict_for_budget(&mut self, anchor: Option<&Key>) -> bool {
        if self.entries.len() <= 1 {
            return false;
        }
        let at = anchor
            .and_then(|key| self.entries.iter().position(|e| e.key() == *key))
            .unwrap_or(self.entries.len() - 1);
        if at < self.entries.len() / 2 {
            self.entries.pop();
            self.detached = true;
        } else {
            self.entries.remove(0);
            self.older = self.entries.first().map(|e| e.cursor.clone());
        }
        self.entries.shrink_to_fit();
        true
    }
    /// Re-read the page containing the visible row, not the oldest retained page.
    pub fn restore_cursor(&self, anchor: Option<&Key>) -> Option<String> {
        anchor
            .and_then(|key| self.get(key))
            .and_then(|entry| entry.cursor.clone())
    }

    pub fn latest(&mut self, mut page: Page) {
        for entry in &mut page.entries {
            if let Some(previous) = self.get(&entry.key()) {
                let next = Arc::make_mut(entry);
                for part in &previous.parts {
                    if part.start == 0 {
                        continue;
                    }
                    if let Some(current) = next.parts.iter_mut().find(|current| {
                        current.field == part.field
                            && current.reference == part.reference
                            && current.length == part.length
                    }) {
                        let mut old = previous.item.clone();
                        if let (Ok(source), Ok(target)) = (
                            field_mut(&mut old, part.field),
                            field_mut(&mut next.item, part.field),
                        ) {
                            *target = source.clone();
                            *current = part.clone();
                        }
                    }
                }
            }
        }
        let incoming: HashSet<_> = page.entries.iter().map(|e| e.key()).collect();
        // If the live tail moved past our last page, do not imply a continuous seam.
        if self.detached
            || (!self.entries.is_empty()
                && !self.entries.iter().any(|e| incoming.contains(&e.key())))
        {
            self.entries.clear();
            self.loaded = false;
        }
        if !self.loaded {
            self.older = page.before.clone().map(Some);
        }
        self.entries
            .retain(|entry| !incoming.contains(&entry.key()));
        self.entries.extend(page.entries);
        self.loaded = true;
        self.detached = false;
        self.bound(false);
    }
    pub fn prepend(&mut self, page: Page) {
        let known: HashSet<_> = self.entries.iter().map(|entry| entry.key()).collect();
        let mut entries: Vec<_> = page
            .entries
            .into_iter()
            .take_while(|entry| !known.contains(&entry.key()))
            .collect();
        // A stored-entry boundary can make one page larger than the window.
        // Keep its nearest older portion, with room for the current scroll anchor.
        let excess = entries.len().saturating_sub(MAX_ITEMS / 2);
        self.older = if excess > 0 {
            entries.drain(..excess);
            entries.first().map(|entry| entry.cursor.clone())
        } else {
            page.before.map(Some)
        };
        entries.append(&mut self.entries);
        self.entries = entries;
        self.loaded = true;
        self.bound(true);
    }
    pub fn replace(&mut self, entry: Entry) {
        let key = entry.key();
        if let Some(existing) = self.entries.iter_mut().find(|e| e.key() == key) {
            *existing = Arc::new(entry);
        }
        // Preserve the part being read by trimming the farthest end first.
        let position = self
            .entries
            .iter()
            .position(|e| e.key() == key)
            .unwrap_or(0);
        self.bound(position < self.entries.len() / 2);
    }
    pub fn get(&self, key: &Key) -> Option<&Arc<Entry>> {
        self.entries.iter().find(|entry| entry.key() == *key)
    }
    pub fn bytes(&self) -> usize {
        self.entries.iter().map(|e| e.bytes()).sum()
    }
    fn bound(&mut self, older: bool) {
        let mut bytes = self.bytes();
        while self.entries.len() > MAX_ITEMS || bytes > MAX_BYTES {
            if self.entries.len() <= 1 {
                break;
            }
            let removed = if older {
                self.detached = true;
                self.entries.pop().unwrap()
            } else {
                self.entries.remove(0)
            };
            bytes -= removed.bytes();
            if !older {
                self.older = self.entries.first().map(|entry| entry.cursor.clone());
            }
        }
    }
    /// Whether another latest-page request can improve the stored tail. The UI
    /// only retries a changed signature automatically, so errors cannot busy-loop.
    pub fn needs_latest(&self, live: &Snapshot) -> bool {
        if self.detached {
            return false;
        }
        if !self.loaded {
            return true;
        }
        transcript::rows(live).iter().rev().take(32).any(|row| {
            let Some(key) = Key::row(*row) else {
                return false;
            };
            let Some(entry) = self.get(&key) else {
                return true;
            };
            match (&entry.item, row) {
                (HistoryItem::Tool { status, output, .. }, Row::Tool(tool)) => {
                    *status != tool.status
                        || (*status != ToolStatus::Running
                            && !output.ends_with(&tool.output)
                            && entry.parts.is_empty())
                }
                _ => false,
            }
        })
    }
    pub fn project(&self, live: &Snapshot) -> Snapshot {
        if !self.loaded {
            return live.clone();
        }
        let mut output = live.clone();
        output.messages.clear();
        output.tools.clear();
        if self.detached {
            output.live_text.clear();
            output.live_reasoning.clear();
        }
        let live_rows = transcript::rows(live);
        let mut entries: Vec<HistoryItem> = self
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect();
        if !self.detached {
            // Stored fields win over clipped tails; current tool state/output win
            // if the history read raced a completion or a new output chunk.
            for entry in &mut entries {
                if let HistoryItem::Tool {
                    id, status, output, ..
                } = entry
                    && let Some(tool) = live.tools.iter().find(|t| t.id == *id)
                {
                    if *status == ToolStatus::Running || *status != tool.status {
                        *output = tool.output.clone();
                    }
                    *status = tool.status;
                }
            }
            let keys: HashSet<_> = entries.iter().map(Key::of).collect();
            let seam = live_rows
                .iter()
                .rposition(|row| Key::row(*row).is_some_and(|key| keys.contains(&key)));
            // A first page can be empty while a new accepted input is arriving.
            let start = seam.map_or(
                if entries.is_empty() {
                    0
                } else {
                    live_rows.len()
                },
                |i| i + 1,
            );
            let mut added = keys;
            for row in &live_rows[start..] {
                if let Some(key) = Key::row(*row)
                    && !added.insert(key)
                {
                    continue;
                }
                match row {
                    Row::Message(m) | Row::Reasoning(m) => entries.push(HistoryItem::Message {
                        images: m.images.clone(),
                        id: m.id.clone(),
                        role: m.role,
                        text: m.text.clone(),
                        reasoning: m.reasoning.clone(),
                        more: vec![],
                    }),
                    Row::Tool(t) => entries.push(HistoryItem::Tool {
                        id: t.id.clone(),
                        name: t.name.clone(),
                        status: t.status,
                        input: t.input.clone(),
                        output: t.output.clone(),
                        more: vec![],
                    }),
                    _ => {}
                }
            }
        }
        let mut after = None;
        for entry in entries {
            match entry {
                HistoryItem::Message {
                    images,
                    id,
                    role,
                    text,
                    reasoning,
                    ..
                } => {
                    after = Some(id.clone());
                    output.messages.push(Message {
                        images,
                        id,
                        role,
                        text,
                        reasoning,
                    });
                }
                HistoryItem::Tool {
                    id,
                    name,
                    status,
                    input,
                    output: text,
                    ..
                } => output.tools.push(Tool {
                    id,
                    name,
                    status,
                    input,
                    output: text,
                    after: Some(after.clone()),
                }),
            }
        }
        output
    }
}

fn field_mut(item: &mut HistoryItem, field: HistoryField) -> Result<&mut String, Error> {
    match (item, field) {
        (HistoryItem::Message { text, .. }, HistoryField::Text) => Ok(text),
        (HistoryItem::Message { reasoning, .. }, HistoryField::Reasoning) => Ok(reasoning),
        (HistoryItem::Tool { input, .. }, HistoryField::Input) => Ok(input),
        (HistoryItem::Tool { output, .. }, HistoryField::Output) => Ok(output),
        _ => Err(Error::Protocol(
            "History continuation names the wrong field.".into(),
        )),
    }
}

pub fn read_page(client: &Client, id: &str, before: Option<String>) -> Result<Page, Error> {
    read_page_version(client, id, before, 3)
}
pub fn read_page_version(
    client: &Client,
    id: &str,
    before: Option<String>,
    protocol: u32,
) -> Result<Page, Error> {
    let mut request = Request::history(id.to_owned(), before.clone(), Some(100));
    if let Request::History {
        protocol_version, ..
    } = &mut request
    {
        *protocol_version = protocol;
    }
    let page: HistoryPage = client.request(&request)?;
    prepare_page(page, before, |reference, offset| {
        client.request(&Request::history_text(
            id.to_owned(),
            reference.to_owned(),
            offset,
        ))
    })
}
pub fn prepare_page(
    page: HistoryPage,
    cursor: Option<String>,
    mut read: impl FnMut(&str, u64) -> Result<HistoryText, Error>,
) -> Result<Page, Error> {
    let mut budget = PAGE_BYTES;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for item in page.items {
        if !seen.insert(Key::of(&item)) {
            return Err(Error::Protocol(
                "History page contains duplicate row IDs.".into(),
            ));
        }
        let mut entry = Entry::from_inline(item, cursor.clone())?;
        for part in &mut entry.parts {
            let text = field_mut(&mut entry.item, part.field)?;
            while part.has_next() && part.end < PART_UNITS && budget >= 262_144 {
                let next = read(&part.reference, part.end)?;
                budget = budget.saturating_sub(next.text.len());
                append(text, part, next)?;
            }
        }
        entry.parts.retain(Part::has_next);
        entries.push(Arc::new(entry));
    }
    Ok(Page {
        entries,
        before: page.before,
    })
}
fn append(text: &mut String, part: &mut Part, next: HistoryText) -> Result<(), Error> {
    let end = part
        .end
        .checked_add(next.text.encode_utf16().count() as u64)
        .ok_or_else(|| Error::Protocol("History offset overflow.".into()))?;
    if next.offset != part.end
        || next.length != part.length
        || end > part.length
        || (next.next_offset != (end < part.length).then_some(end))
        || end <= part.end
    {
        return Err(Error::Protocol(
            "Host returned inconsistent history text.".into(),
        ));
    }
    text.push_str(&next.text);
    part.end = end;
    Ok(())
}
pub fn read_part(
    client: &Client,
    id: &str,
    entry: &Entry,
    field: HistoryField,
    restart: bool,
) -> Result<Entry, Error> {
    let mut entry = entry.clone();
    let part = entry
        .parts
        .iter_mut()
        .find(|part| part.field == field)
        .ok_or_else(|| Error::Protocol("History field is no longer available.".into()))?;
    part.start = if restart { 0 } else { part.end };
    part.end = part.start;
    part.number = if restart { 1 } else { part.number + 1 };
    let text = field_mut(&mut entry.item, field)?;
    text.clear();
    loop {
        let next = client.request(&Request::history_text(
            id.to_owned(),
            part.reference.clone(),
            part.end,
        ))?;
        append(text, part, next)?;
        if !part.has_next() || part.end - part.start >= PART_UNITS {
            break;
        }
    }
    Ok(entry)
}

/// Uses only reported facts: snapshots have no per-turn duration or stop actor.
pub fn run_state(snapshot: &Snapshot, now: u64) -> Option<(String, bool)> {
    if let Some(retry) = &snapshot.retry {
        let seconds = retry.at.saturating_sub(now).div_ceil(1000);
        return Some((
            if seconds == 0 {
                format!("Retrying model request · attempt {}", retry.attempt)
            } else {
                format!("Model retry {} · in {seconds}s", retry.attempt)
            },
            false,
        ));
    }
    Some(match snapshot.status {
        ThreadStatus::Running if !snapshot.live_reasoning.is_empty() => ("Thinking…".into(), false),
        ThreadStatus::Running if !snapshot.live_text.is_empty() => ("Writing…".into(), false),
        ThreadStatus::Running
            if snapshot
                .tools
                .iter()
                .any(|t| t.status == ToolStatus::Running) =>
        {
            ("Running tools…".into(), false)
        }
        ThreadStatus::Running => ("Waiting for the model…".into(), false),
        ThreadStatus::AwaitingApproval => ("Waiting for your approval".into(), false),
        ThreadStatus::Stopped => ("Stopped · send a message to continue".into(), false),
        ThreadStatus::Interrupted => (
            "Interrupted when the host restarted · send a message to continue".into(),
            false,
        ),
        ThreadStatus::Error => (
            match snapshot.error.as_ref() {
                Some(error) if error.code == "turn_limit" => {
                    "Turn limit reached · send a message to continue".into()
                }
                Some(error) => format!("Run failed · {}", error.message),
                None => "Run failed · send a message to continue".into(),
            },
            true,
        ),
        ThreadStatus::Idle if !snapshot.messages.is_empty() => ("Finished".into(), false),
        ThreadStatus::Idle => return None,
    })
}

/// What a finished turn did, from the tool calls the host reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnSummary {
    /// Tool calls in the turn.
    pub steps: usize,
    /// Paths its finished edit and write calls changed, in the order first changed.
    pub files: Vec<String>,
}

impl TurnSummary {
    /// "5 steps", or "1 step".
    pub fn steps_label(&self) -> String {
        let steps = self.steps;
        format!("{steps} {}", if steps == 1 { "step" } else { "steps" })
    }

    /// The whole record in one line, for assistive technology.
    pub fn label(&self) -> String {
        if self.steps == 0 {
            "no tool calls".into()
        } else if self.files.is_empty() {
            self.steps_label()
        } else {
            format!("{} · edited {}", self.steps_label(), self.files.join(", "))
        }
    }
}

/// The record each message's own row draws: its turn's, unless the message
/// ends the rows. There the state line draws it, or, while older history is
/// being read, nothing does: the turn may run on past what is loaded.
pub fn row_record<'a>(
    summaries: &'a std::collections::HashMap<String, TurnSummary>,
    snapshot: &Snapshot,
    id: &str,
) -> Option<&'a TurnSummary> {
    summaries
        .get(id)
        .filter(|_| snapshot.messages.last().is_none_or(|last| last.id != id))
}

/// Positions in `after` of the messages whose rows draw another record than
/// they did in `before`. Those rows change height though their text has not,
/// as when a new request arrives and the answer before it takes its record.
pub fn moved_records(
    before: (&Snapshot, &std::collections::HashMap<String, TurnSummary>),
    after: (&Snapshot, &std::collections::HashMap<String, TurnSummary>),
) -> Vec<usize> {
    after
        .0
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            row_record(before.1, before.0, &message.id) != row_record(after.1, after.0, &message.id)
        })
        .map(|(index, _)| index)
        .collect()
}

/// Summaries describe observed steps, without inventing per-turn timing.
pub fn turn_summaries(snapshot: &Snapshot) -> std::collections::HashMap<String, TurnSummary> {
    let mut summaries = std::collections::HashMap::new();
    let mut answer: Option<String> = None;
    let mut steps = 0usize;
    let mut files = Vec::new();
    // A turn whose request is outside what is loaded has no record: the
    // steps before the first loaded row cannot be counted.
    let mut whole = false;
    let finish = |answer: &mut Option<String>,
                  steps: usize,
                  whole: bool,
                  files: &mut Vec<String>,
                  summaries: &mut std::collections::HashMap<String, TurnSummary>| {
        // Every turn that ended in an answer has a record, so each closes
        // the same way; one with no tool calls has nothing to count.
        if let Some(id) = answer.take()
            && whole
        {
            let files = std::mem::take(files);
            summaries.insert(id, TurnSummary { steps, files });
        }
        files.clear();
    };
    for row in transcript::rows(snapshot) {
        match row {
            Row::Message(m) if m.role == MessageRole::User => {
                finish(&mut answer, steps, whole, &mut files, &mut summaries);
                steps = 0;
                whole = true;
            }
            Row::Message(m) => answer = Some(m.id.clone()),
            Row::Tool(t) => {
                answer = None;
                steps += 1;
                if matches!(t.name.as_str(), "edit" | "write")
                    && t.status == ToolStatus::Done
                    && let Some(path) = transcript::tool_subject(t)
                    && !files.contains(&path)
                {
                    files.push(path);
                }
            }
            _ => {}
        }
    }
    if !snapshot.status.busy() {
        finish(&mut answer, steps, whole, &mut files, &mut summaries);
    }
    summaries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{HostError, ModelChoice, Retry};

    fn message(id: &str, text: &str) -> HistoryItem {
        HistoryItem::Message {
            images: Vec::new(),
            id: id.into(),
            role: MessageRole::Assistant,
            text: text.into(),
            reasoning: String::new(),
            more: vec![],
        }
    }
    fn page(ids: &[&str], before: Option<&str>) -> Page {
        prepare_page(
            HistoryPage {
                items: ids
                    .iter()
                    .map(|id| message(id, &format!("Full {id}")))
                    .collect(),
                before: before.map(str::to_owned),
            },
            None,
            |_, _| panic!("no text parts"),
        )
        .unwrap()
    }
    #[test]
    fn cold_restore_uses_the_anchor_page_and_budget_eviction_keeps_it() {
        let mut history = History::default();
        history.latest(page(&["old", "middle", "new"], None));
        Arc::make_mut(&mut history.entries[0]).cursor = Some("old-page".into());
        Arc::make_mut(&mut history.entries[1]).cursor = Some("middle-page".into());
        let anchor = Key::Message("middle".into());
        assert_eq!(
            history.restore_cursor(Some(&anchor)).as_deref(),
            Some("middle-page")
        );
        let before = history.retained_bytes();
        assert!(history.evict_for_budget(Some(&anchor)));
        assert!(history.get(&anchor).is_some());
        assert!(history.retained_bytes() < before);
        assert_eq!(history.restore_cursor(None), None);
    }

    fn live(ids: &[&str]) -> Snapshot {
        Snapshot {
            id: "thread".into(),
            checkout: "/checkout".into(),
            model: ModelChoice {
                provider: "provider".into(),
                model_id: "model".into(),
            },
            messages: ids
                .iter()
                .map(|id| Message {
                    images: Vec::new(),
                    id: (*id).into(),
                    role: MessageRole::Assistant,
                    text: (*id).into(),
                    reasoning: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }
    fn ids(snapshot: &Snapshot) -> Vec<&str> {
        snapshot.messages.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn full_items_replace_tail_and_prepend_without_duplicate_seams() {
        let mut cache = History::default();
        cache.latest(page(&["b", "c"], Some("older")));
        cache.prepend(page(&["a", "b"], None));
        let output = cache.project(&live(&["b", "c", "d"]));
        assert_eq!(ids(&output), ["a", "b", "c", "d"]);
        assert_eq!(output.messages[2].text, "Full c");
        assert_eq!(cache.older, None);
    }

    #[test]
    fn new_snapshot_after_a_page_request_keeps_new_rows_and_running_output() {
        let mut cache = History::default();
        let tool = HistoryItem::Tool {
            id: "tool".into(),
            name: "command".into(),
            status: ToolStatus::Running,
            input: "full arguments".into(),
            output: String::new(),
            more: vec![],
        };
        let page = prepare_page(
            HistoryPage {
                items: vec![message("a", "complete"), tool],
                before: None,
            },
            None,
            |_, _| unreachable!(),
        )
        .unwrap();
        cache.latest(page);
        let mut snapshot = live(&["a", "b"]);
        snapshot.tools.push(Tool {
            id: "tool".into(),
            name: "command".into(),
            status: ToolStatus::Done,
            input: "arguments".into(),
            output: "finished while reading".into(),
            after: Some(Some("a".into())),
        });
        let output = cache.project(&snapshot);
        assert_eq!(ids(&output), ["a", "b"]);
        assert_eq!(output.tools[0].status, ToolStatus::Done);
        assert_eq!(output.tools[0].output, "finished while reading");
        assert_eq!(output.tools[0].input, "full arguments");
        assert!(cache.needs_latest(&snapshot));
    }

    #[test]
    fn snapshot_tail_eviction_keeps_loaded_history_and_model_change_keeps_identity() {
        let mut cache = History::default();
        cache.latest(page(&["a", "b", "c"], None));
        let before = cache.project(&live(&["b", "c"]));
        let mut latest = live(&["c", "d"]);
        latest.model.model_id = "other".into();
        let after = cache.project(&latest);
        assert_eq!(ids(&after), ["a", "b", "c", "d"]);
        assert!(matches!(
            transcript::changes(Some(&before), Some(&after)),
            transcript::Update::Patch { .. }
        ));
    }

    #[test]
    fn latest_refresh_merges_overlap_and_resets_an_unbridgeable_gap() {
        let mut cache = History::default();
        cache.latest(page(&["a", "b"], Some("older")));
        cache.latest(page(&["b", "c"], Some("next")));
        assert_eq!(ids(&cache.project(&live(&["c"]))), ["a", "b", "c"]);
        cache.latest(page(&["x", "y"], Some("gap")));
        assert_eq!(ids(&cache.project(&live(&["y"]))), ["x", "y"]);
        assert_eq!(cache.older, Some(Some("gap".into())));
    }

    #[test]
    fn reasoning_only_messages_survive_the_seam_once() {
        let mut cache = History::default();
        cache.latest(page(&["a"], None));
        let mut snapshot = live(&["a", "think"]);
        snapshot.messages[1].text.clear();
        snapshot.messages[1].reasoning = "Plan".into();
        assert!(cache.needs_latest(&snapshot));
        let output = cache.project(&snapshot);
        assert_eq!(ids(&output), ["a", "think"]);
        assert!(matches!(transcript::rows(&output)[1], Row::Reasoning(_)));
    }

    #[test]
    fn prepending_retains_ordered_row_identity() {
        let previous = live(&["c", "d"]);
        let next = live(&["a", "b", "c", "d"]);
        assert_eq!(
            transcript::changes(Some(&previous), Some(&next)),
            transcript::Update::Patch {
                splices: vec![transcript::Splice {
                    range: 0..0,
                    count: 2
                }],
                remeasure: vec![]
            }
        );
    }

    #[test]
    fn memory_window_drops_the_far_end_and_keeps_a_reload_cursor() {
        let mut cache = History::default();
        let names: Vec<_> = (0..MAX_ITEMS).map(|i| i.to_string()).collect();
        cache.latest(page(
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
            Some("older"),
        ));
        cache.prepend(page(&["old-a", "old-b"], Some("oldest")));
        assert_eq!(cache.entries.len(), MAX_ITEMS);
        assert!(cache.detached);
        let display = cache.project(&live(&["new"]));
        assert_eq!(display.messages[0].id, "old-a");
        assert!(!display.messages.iter().any(|m| m.id == "new"));
        cache.latest(page(&["new"], Some("before-new")));
        assert!(!cache.detached);
        assert_eq!(cache.entries.len(), 1);
        let large = "a".repeat(MAX_BYTES / 4);
        cache.latest(Page {
            entries: (0..6)
                .map(|i| {
                    Arc::new(Entry {
                        item: message(&i.to_string(), &large),
                        parts: vec![],
                        cursor: Some("reload-page".into()),
                    })
                })
                .collect(),
            before: None,
        });
        assert!(cache.bytes() <= MAX_BYTES);
        assert_eq!(cache.older, Some(Some("reload-page".into())));
    }

    #[test]
    fn oversized_page_walks_back_in_contiguous_windows_without_skipping() {
        let names: Vec<_> = (0..MAX_ITEMS * 5 / 2).map(|i| i.to_string()).collect();
        let source = page(&names.iter().map(String::as_str).collect::<Vec<_>>(), None);
        let mut cache = History::default();
        let mut visited = HashSet::new();
        cache.latest(source.clone());
        for start in [900, 600, 300, 0] {
            let display = cache.project(&live(&[]));
            assert_eq!(display.messages.len(), MAX_ITEMS);
            for (offset, item) in display.messages.iter().enumerate() {
                assert_eq!(item.id, (start + offset).to_string());
                visited.insert(item.id.clone());
            }
            if start == 0 {
                assert_eq!(cache.older, None);
            } else {
                assert_eq!(cache.older, Some(None));
                cache.prepend(source.clone());
            }
        }
        assert_eq!(visited.len(), names.len());
    }

    #[test]
    fn history_text_is_hydrated_on_utf16_boundaries() {
        let item = HistoryItem::Message {
            images: Vec::new(),
            id: "a".into(),
            role: MessageRole::Assistant,
            text: "a🙂".into(),
            reasoning: String::new(),
            more: vec![crate::host::More {
                field: HistoryField::Text,
                reference: "opaque".into(),
                offset: 3,
                length: 6,
            }],
        };
        let full = prepare_page(
            HistoryPage {
                items: vec![item],
                before: None,
            },
            None,
            |reference, offset| {
                assert_eq!(reference, "opaque");
                assert_eq!(offset, 3);
                Ok(HistoryText {
                    text: "🙂b".into(),
                    offset,
                    next_offset: None,
                    length: 6,
                })
            },
        )
        .unwrap();
        assert!(full.entries[0].parts.is_empty());
        assert!(
            matches!(&full.entries[0].item, HistoryItem::Message { text, .. } if text == "a🙂🙂b")
        );
    }

    #[test]
    fn huge_fields_stop_at_a_part_and_bad_offsets_are_rejected() {
        let text = "x".repeat(65536);
        let item = HistoryItem::Message {
            images: Vec::new(),
            id: "a".into(),
            role: MessageRole::Assistant,
            text: text.clone(),
            reasoning: String::new(),
            more: vec![crate::host::More {
                field: HistoryField::Text,
                reference: "ref".into(),
                offset: 65536,
                length: 1_000_000,
            }],
        };
        let full = prepare_page(
            HistoryPage {
                items: vec![item.clone()],
                before: None,
            },
            None,
            |_, offset| {
                Ok(HistoryText {
                    text: text.clone(),
                    offset,
                    next_offset: Some(offset + 65536),
                    length: 1_000_000,
                })
            },
        )
        .unwrap();
        let part = &full.entries[0].parts[0];
        assert_eq!(part.end, PART_UNITS);
        assert!(part.has_next());
        assert!(
            prepare_page(
                HistoryPage {
                    items: vec![item],
                    before: None
                },
                None,
                |_, offset| Ok(HistoryText {
                    text: String::new(),
                    offset,
                    next_offset: Some(offset),
                    length: 1_000_000
                })
            )
            .is_err()
        );
    }

    #[test]
    fn duplicate_rows_are_rejected_but_tool_and_message_ids_are_distinct() {
        assert!(
            prepare_page(
                HistoryPage {
                    items: vec![message("a", "x"), message("a", "y")],
                    before: None
                },
                None,
                |_, _| unreachable!()
            )
            .is_err()
        );
        let tool = HistoryItem::Tool {
            id: "a".into(),
            name: "read".into(),
            status: ToolStatus::Done,
            input: "{}".into(),
            output: "x".into(),
            more: vec![],
        };
        assert!(
            prepare_page(
                HistoryPage {
                    items: vec![message("a", "x"), tool],
                    before: None
                },
                None,
                |_, _| unreachable!()
            )
            .is_ok()
        );
    }

    #[test]
    fn latest_refresh_preserves_the_large_text_part_being_read() {
        let make = || {
            Entry::from_inline(
                HistoryItem::Message {
                    images: Vec::new(),
                    id: "a".into(),
                    role: MessageRole::Assistant,
                    text: "first".into(),
                    reasoning: String::new(),
                    more: vec![crate::host::More {
                        field: HistoryField::Text,
                        reference: "ref".into(),
                        offset: 5,
                        length: 500_000,
                    }],
                },
                None,
            )
            .unwrap()
        };
        let mut cache = History::default();
        let mut old = make();
        *field_mut(&mut old.item, HistoryField::Text).unwrap() = "second".into();
        old.parts[0].start = 5;
        old.parts[0].end = 11;
        old.parts[0].number = 2;
        cache.latest(Page {
            entries: vec![Arc::new(old)],
            before: None,
        });
        cache.latest(Page {
            entries: vec![Arc::new(make())],
            before: None,
        });
        assert_eq!(cache.entries[0].parts[0].number, 2);
        assert!(
            matches!(&cache.entries[0].item, HistoryItem::Message { text, .. } if text == "second")
        );
    }

    #[test]
    fn summaries_attach_to_the_answer_after_tools_and_count_real_changed_paths() {
        let mut snapshot = live(&["user", "plan", "answer"]);
        snapshot.messages[0].role = MessageRole::User;
        snapshot.tools = vec![Tool {
            id: "edit".into(),
            name: "edit".into(),
            status: ToolStatus::Done,
            input: "{\"path\":\"src/file.rs\"}".into(),
            output: "ok".into(),
            after: Some(Some("plan".into())),
        }];
        let summaries = turn_summaries(&snapshot);
        let summary = summaries.get("answer").expect("the answer closes the turn");
        assert_eq!(
            (summary.steps, summary.files.as_slice()),
            (1, &["src/file.rs".to_owned()][..])
        );
        assert_eq!(summary.label(), "1 step · edited src/file.rs");
        // Without changed files the record is the step count alone.
        let reads = TurnSummary {
            steps: 3,
            files: Vec::new(),
        };
        assert_eq!(reads.label(), "3 steps");
        // A turn that ends in tool calls, with no answer after them, has no record.
        snapshot.messages.pop();
        assert!(turn_summaries(&snapshot).is_empty());
        // A turn that only answered still closes with a record, with nothing to count.
        let mut talk = live(&["user", "answer"]);
        talk.messages[0].role = MessageRole::User;
        let summaries = turn_summaries(&talk);
        let summary = summaries.get("answer").expect("the answer closes the turn");
        assert_eq!((summary.steps, summary.files.len()), (0, 0));
        assert_eq!(summary.label(), "no tool calls");
        // While the turn runs it has not closed.
        talk.status = ThreadStatus::Running;
        assert!(turn_summaries(&talk).is_empty());
        // Rows that begin after a turn's request cannot count its steps.
        let mut cut = live(&["answer", "user", "reply"]);
        cut.messages[1].role = MessageRole::User;
        let summaries = turn_summaries(&cut);
        assert_eq!(summaries.keys().collect::<Vec<_>>(), ["reply"]);
    }

    #[test]
    fn a_record_moves_into_its_row_when_another_message_ends_the_rows() {
        let turns = |ids: &[&str]| {
            let mut snapshot = live(ids);
            for message in snapshot.messages.iter_mut().step_by(2) {
                message.role = MessageRole::User;
            }
            let summaries = turn_summaries(&snapshot);
            (snapshot, summaries)
        };
        let (one, first) = turns(&["ask", "answer"]);
        // The answer that ends the rows leaves its record to the state line.
        assert!(first.contains_key("answer"));
        assert_eq!(row_record(&first, &one, "answer"), None);
        // A new request ends the rows instead, so the answer's row takes it.
        let (two, second) = turns(&["ask", "answer", "again"]);
        assert!(row_record(&second, &two, "answer").is_some());
        assert_eq!(moved_records((&one, &first), (&two, &second)), [1]);
        // Nothing moves while the same message ends the rows.
        assert!(moved_records((&two, &second), (&two, &second)).is_empty());
        let (three, third) = turns(&["ask", "answer", "again", "reply"]);
        assert!(moved_records((&two, &second), (&three, &third)).is_empty());
    }

    #[test]
    fn run_states_do_not_invent_timing_or_stop_actor() {
        let mut s = live(&["a"]);
        assert_eq!(run_state(&s, 0).unwrap().0, "Finished");
        s.status = ThreadStatus::Running;
        assert_eq!(run_state(&s, 0).unwrap().0, "Waiting for the model…");
        s.live_reasoning = "think".into();
        assert_eq!(run_state(&s, 0).unwrap().0, "Thinking…");
        s.retry = Some(Retry {
            attempt: 2,
            at: 4500,
        });
        assert_eq!(run_state(&s, 2001).unwrap().0, "Model retry 2 · in 3s");
        s.retry = None;
        for (state, prefix) in [
            (ThreadStatus::AwaitingApproval, "Waiting for your approval"),
            (ThreadStatus::Stopped, "Stopped"),
            (
                ThreadStatus::Interrupted,
                "Interrupted when the host restarted",
            ),
            (ThreadStatus::Error, "Run failed"),
        ] {
            s.status = state;
            assert!(run_state(&s, 0).unwrap().0.starts_with(prefix));
        }
        s.error = Some(HostError {
            retry_after_seconds: None,
            code: "turn_limit".into(),
            message: "raw".into(),
        });
        assert!(
            run_state(&s, 0)
                .unwrap()
                .0
                .starts_with("Turn limit reached")
        );
    }
}
