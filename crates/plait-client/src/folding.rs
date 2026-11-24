//! Pure, GPUI-free model for folding earlier conversation turns.
//!
//! Decides which turns of a thread draw folded, segments a snapshot's
//! transcript slots into turns, builds the virtualized display row list, and
//! diffs two display layouts into incremental list splices and remeasures.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use crate::{
    display_text,
    history::records,
    host::{MessageRole, RecordKind, Snapshot, ThreadStatus, ToolStatus, TurnOutcome, TurnRecord},
    transcript::{self, Row, Slot, Splice, Update},
};

/// Execution state of a turn, used to decide whether an earlier turn may fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnState {
    Finished,
    Running,
    Failed,
    Stopped,
    WaitingForApproval,
}

/// Folding history owned by the open tab, independent of its saved bookmark.
#[derive(Default)]
pub struct ObservedOutcomes {
    terminal: HashMap<String, TurnState>,
    opened_work: RefCell<HashSet<String>>,
}

impl ObservedOutcomes {
    pub fn set_work_open(&self, request: &str, open: bool) -> bool {
        let mut opened = self.opened_work.borrow_mut();
        if open {
            opened.insert(request.to_owned())
        } else {
            opened.remove(request)
        }
    }

    pub fn observe(&mut self, live: &Snapshot) {
        let outcome = latest_turn_state(live);
        if matches!(outcome, TurnState::Failed | TurnState::Stopped)
            && let Some(request) = live
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::User)
        {
            self.terminal.insert(request.id.clone(), outcome);
        }
    }
}

/// Inputs for one turn when deciding whether it draws folded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnInput<'a> {
    pub id: &'a str,
    pub state: TurnState,
    pub is_latest: bool,
}

/// Whether `turn` should draw folded.
///
/// Never folds:
/// - when `fold_earlier_turns` is off;
/// - the latest turn;
/// - a turn that is running, failed, stopped, or waiting for approval;
/// - the turn holding the current Find match;
/// - a turn the person opened in this tab.
pub fn is_folded(
    turn: &TurnInput<'_>,
    opened: &HashSet<String>,
    find_match_turn: Option<&str>,
    fold_earlier_turns: bool,
) -> bool {
    if !fold_earlier_turns || turn.is_latest || turn.state != TurnState::Finished {
        return false;
    }
    if find_match_turn == Some(turn.id) || opened.contains(turn.id) {
        return false;
    }
    true
}

/// For each turn in `turns`, whether it should draw folded.
pub fn folded_turns(
    turns: &[TurnInput<'_>],
    opened: &HashSet<String>,
    find_match_turn: Option<&str>,
    fold_earlier_turns: bool,
) -> Vec<bool> {
    turns
        .iter()
        .map(|turn| is_folded(turn, opened, find_match_turn, fold_earlier_turns))
        .collect()
}

/// First non-empty line of a request for its folded disclosure row.
/// Falls back to the image count when a request carries only images.
pub fn first_request_line(text: &str, image_count: usize) -> String {
    if let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) {
        return display_text::escape(line);
    }
    match image_count {
        0 => String::new(),
        1 => "Image".into(),
        n => format!("{n} images"),
    }
}

/// One turn in the loaded transcript, starting at a user request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    /// Stable turn identity: the starting user message's ID.
    pub id: String,
    /// Index of the starting user message in `snapshot.messages`.
    pub request_message: usize,
    /// Range of indices in `transcript::layout(snapshot)` belonging to this turn.
    pub slots: Range<usize>,
    /// Last slot in `slots` that can carry a closing record line.
    pub closing_slot: Option<Slot>,
    /// Execution state of this turn.
    pub state: TurnState,
    /// This record's original request and the end of its turn are both loaded.
    complete_finished_run: bool,
    /// Whether this is the thread's latest turn.
    pub is_latest: bool,
}

/// Segment `slots` (from `transcript::layout(snapshot)`) into turns.
///
/// Each user request starts a turn, except steered follow-ups that belong to
/// the same `TurnRecord` as the turn already in progress. `live_end` is true
/// when the loaded window reaches the thread's latest turn (not detached
/// reading older history pages).
pub fn segment_turns(
    snapshot: &Snapshot,
    slots: &[Slot],
    live_end: bool,
    records: &[TurnRecord],
    outcomes: &ObservedOutcomes,
) -> Vec<Turn> {
    let mut run_of_request = HashMap::new();
    for record in records {
        if record.kind == RecordKind::Turn {
            let outcome = record
                .outcome
                .map(|value| match value {
                    TurnOutcome::Finished => TurnState::Finished,
                    TurnOutcome::Failed => TurnState::Failed,
                    TurnOutcome::Stopped | TurnOutcome::Interrupted => TurnState::Stopped,
                })
                .or_else(|| {
                    record
                        .request_ids
                        .iter()
                        .find_map(|id| outcomes.terminal.get(&format!("request-{id}")))
                        .copied()
                });
            for request_id in &record.request_ids {
                run_of_request.insert(format!("request-{request_id}"), (record, outcome));
            }
        }
    }

    struct Draft<'a> {
        id: String,
        request_message: usize,
        start: usize,
        closing_slot: Option<Slot>,
        run: Option<&'a TurnRecord>,
        outcome: Option<TurnState>,
        loaded_requests: usize,
        has_running_tool: bool,
        has_interrupted_tool: bool,
    }

    let mut drafts: Vec<Draft<'_>> = Vec::new();
    for (slot_ix, &slot) in slots.iter().enumerate() {
        if let Slot::Message(msg_ix) = slot
            && let Some(message) = snapshot.messages.get(msg_ix)
            && message.role == MessageRole::User
        {
            let record = run_of_request.get(&message.id);
            let run = record.map(|(run, _)| *run);
            if let Some(previous) = drafts.last_mut()
                && let Some(run) = run
                && previous.run.is_some_and(|old| old.id == run.id)
            {
                previous.loaded_requests += 1;
            } else {
                drafts.push(Draft {
                    id: message.id.clone(),
                    request_message: msg_ix,
                    start: slot_ix,
                    closing_slot: None,
                    run,
                    outcome: record
                        .and_then(|(_, outcome)| *outcome)
                        .or_else(|| outcomes.terminal.get(&message.id).copied()),
                    loaded_requests: 1,
                    has_running_tool: false,
                    has_interrupted_tool: false,
                });
            }
        }
        let Some(current) = drafts.last_mut() else {
            continue;
        };
        if let Some(row) = transcript::resolve(snapshot, slot) {
            if let Row::Tool(tool) = row {
                match tool.status {
                    ToolStatus::Running => current.has_running_tool = true,
                    ToolStatus::Interrupted => current.has_interrupted_tool = true,
                    ToolStatus::Done | ToolStatus::Error => {}
                }
            }
            if records::closing_key(row).is_some() {
                current.closing_slot = Some(slot);
            }
        }
    }

    let count = drafts.len();
    let mut turns = Vec::with_capacity(count);
    for (index, draft) in drafts.iter().enumerate() {
        let end = drafts.get(index + 1).map_or(slots.len(), |next| next.start);
        let is_latest = live_end && index + 1 == count;
        let state = if is_latest {
            latest_turn_state(snapshot)
        } else if let Some(outcome) = draft.outcome {
            outcome
        } else if draft.has_running_tool {
            TurnState::Running
        } else if draft.has_interrupted_tool {
            TurnState::Stopped
        } else {
            TurnState::Finished
        };
        turns.push(Turn {
            id: draft.id.clone(),
            request_message: draft.request_message,
            slots: draft.start..end,
            closing_slot: draft.closing_slot,
            state,
            is_latest,
            complete_finished_run: draft.run.is_some_and(|run| {
                run.outcome == Some(TurnOutcome::Finished)
                    && run.request_ids_total == draft.loaded_requests
                    && run.request_ids.first().map(String::as_str)
                        == draft.id.strip_prefix("request-")
            }) && state == TurnState::Finished
                && !draft.has_running_tool
                && (end < slots.len() || live_end),
        });
    }
    turns
}

fn latest_turn_state(snapshot: &Snapshot) -> TurnState {
    if snapshot.pending_approval.is_some() || snapshot.status == ThreadStatus::AwaitingApproval {
        return TurnState::WaitingForApproval;
    }
    match snapshot.status {
        ThreadStatus::Running => TurnState::Running,
        ThreadStatus::AwaitingApproval => TurnState::WaitingForApproval,
        ThreadStatus::Error => TurnState::Failed,
        ThreadStatus::Stopped | ThreadStatus::Interrupted => TurnState::Stopped,
        ThreadStatus::Idle => TurnState::Finished,
    }
}

/// One row in the virtualized conversation list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayRow {
    /// An unfolded transcript slot.
    Slot(Slot),
    /// A folded earlier turn: draws as its request disclosure row and its closing line.
    FoldedTurn {
        /// Stable turn identity (the starting user message's ID).
        id: String,
        /// Index of the starting user message in `snapshot.messages`.
        request_message: usize,
        /// Slot carrying this turn's closing record line, if any.
        closing_slot: Option<Slot>,
    },
}

/// The display rows strictly between the request and the final assistant reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkFold {
    pub request: String,
    manually_open: bool,
    pub rows: Range<usize>,
    pub summary: String,
    find_inside: bool,
}

impl WorkFold {
    fn new(
        snapshot: &Snapshot,
        turn: &Turn,
        slots: &[Slot],
        first_row: usize,
        find: Option<&str>,
        opened: &HashSet<String>,
    ) -> Option<Self> {
        if !turn.complete_finished_run || slots.len() < 3 {
            return None;
        }
        let Slot::Message(last) = *slots.last()? else {
            return None;
        };
        if snapshot.messages[last].role != MessageRole::Assistant {
            return None;
        }
        let work = &slots[1..slots.len() - 1];
        // The live snapshot can clip tools independently of its messages.
        let summary = if snapshot.truncated {
            "Work".into()
        } else {
            let tools: Vec<_> = work
                .iter()
                .filter_map(|slot| match slot {
                    Slot::Tool(ix) => snapshot.tools.get(*ix),
                    _ => None,
                })
                .collect();
            crate::tool_view::summary(&tools, false)
        };
        let find_inside = find.is_some_and(|id| {
            work.iter()
                .any(|slot| matches!(slot, Slot::Message(ix) if snapshot.messages[*ix].id == id))
        });
        Some(Self {
            request: turn.id.clone(),
            manually_open: opened.contains(&turn.id),
            rows: first_row + 1..first_row + slots.len() - 1,
            summary,
            find_inside,
        })
    }

    /// None draws the original rows without a disclosure or indentation.
    pub fn open(&self, enabled: bool) -> Option<bool> {
        enabled.then_some(self.find_inside || self.manually_open)
    }
}

/// Cached display layout for one snapshot and folding state.
///
/// Built when the snapshot, opened turns, current Find match, or preference
/// changes — never per frame during rendering or scrolling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub rows: Vec<DisplayRow>,
    pub work: Arc<[WorkFold]>,
    pub slots: Vec<Slot>,
    pub raw_index_by_key: HashMap<String, usize>,
    /// Maps every loaded message ID to the ID of the turn that owns it.
    pub turn_of_message: HashMap<String, String>,
    /// Maps every currently unfolded message ID to its display row index.
    pub row_of_message: HashMap<String, usize>,
    /// Maps every currently unfolded tool ID to its display row index.
    pub row_of_tool: HashMap<String, usize>,
    /// For each display row, its starting index in `transcript::layout(snapshot)`.
    pub raw_index_of_row: Vec<usize>,
}

impl Layout {
    pub fn work_at(&self, row: usize) -> Option<&WorkFold> {
        let index = self.work.partition_point(|fold| fold.rows.end <= row);
        self.work.get(index).filter(|fold| fold.rows.contains(&row))
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Map a display row index to the starting unfolded slot index, for Find's
    /// `first_from` lookup.
    pub fn raw_index(&self, display_index: usize) -> usize {
        self.raw_index_of_row
            .get(display_index)
            .copied()
            .unwrap_or_else(|| {
                self.raw_index_of_row
                    .last()
                    .map_or(0, |last| last.saturating_add(1))
            })
    }

    /// Display row index of the folded turn with `turn_id`, if it is currently folded.
    pub fn folded_row_index(&self, turn_id: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, DisplayRow::FoldedTurn { id, .. } if id == turn_id))
    }
}

/// Build the virtualized display layout for `snapshot`.
///
/// `find_match_message` is the ID of the message holding the current Find
/// match, if any; its owning turn is kept open.
pub fn build_layout(
    snapshot: &Snapshot,
    live_end: bool,
    records: &[TurnRecord],
    opened: &HashSet<String>,
    find_match_message: Option<&str>,
    fold_earlier_turns: bool,
    outcomes: &ObservedOutcomes,
) -> Layout {
    let slots = transcript::layout(snapshot);
    let turns = segment_turns(snapshot, &slots, live_end, records, outcomes);
    let mut layout = Layout {
        slots: Vec::new(),
        work: Default::default(),
        raw_index_by_key: slots
            .iter()
            .enumerate()
            .filter_map(|(i, &slot)| {
                Some((transcript::row_key(transcript::resolve(snapshot, slot)?), i))
            })
            .collect(),
        rows: Vec::with_capacity(slots.len()),
        turn_of_message: HashMap::with_capacity(snapshot.messages.len()),
        row_of_message: HashMap::with_capacity(snapshot.messages.len()),
        row_of_tool: HashMap::with_capacity(snapshot.tools.len()),
        raw_index_of_row: Vec::with_capacity(slots.len()),
    };

    for turn in &turns {
        for &slot in &slots[turn.slots.clone()] {
            if let Slot::Message(msg_ix) = slot
                && let Some(message) = snapshot.messages.get(msg_ix)
            {
                layout
                    .turn_of_message
                    .insert(message.id.clone(), turn.id.clone());
            }
        }
    }
    let find_match_turn = find_match_message
        .and_then(|message| layout.turn_of_message.get(message))
        .cloned();

    // Leading rows whose request was clipped or paged out cannot fold.
    let first_turn_start = turns.first().map_or(slots.len(), |turn| turn.slots.start);
    for (raw_ix, &slot) in slots[..first_turn_start].iter().enumerate() {
        push_slot(&mut layout, snapshot, raw_ix, slot);
    }

    let mut work = Vec::new();
    let opened_work = outcomes.opened_work.borrow();
    for turn in &turns {
        let input = TurnInput {
            id: &turn.id,
            state: turn.state,
            is_latest: turn.is_latest,
        };
        if is_folded(
            &input,
            opened,
            find_match_turn.as_deref(),
            fold_earlier_turns,
        ) {
            layout.rows.push(DisplayRow::FoldedTurn {
                id: turn.id.clone(),
                request_message: turn.request_message,
                closing_slot: turn.closing_slot,
            });
            layout.raw_index_of_row.push(turn.slots.start);
        } else {
            let members = &slots[turn.slots.clone()];
            if let Some(fold) = WorkFold::new(
                snapshot,
                turn,
                members,
                layout.rows.len(),
                find_match_message,
                &opened_work,
            ) {
                work.push(fold);
            }
            for (offset, &slot) in members.iter().enumerate() {
                push_slot(&mut layout, snapshot, turn.slots.start + offset, slot);
            }
        }
    }

    layout.work = work.into();
    layout.slots = slots;
    layout
}

fn push_slot(layout: &mut Layout, snapshot: &Snapshot, raw_ix: usize, slot: Slot) {
    let display_ix = layout.rows.len();
    match slot {
        Slot::Message(msg_ix) => {
            if let Some(message) = snapshot.messages.get(msg_ix) {
                layout.row_of_message.insert(message.id.clone(), display_ix);
            }
        }
        Slot::Tool(tool_ix) => {
            if let Some(tool) = snapshot.tools.get(tool_ix) {
                layout.row_of_tool.insert(tool.id.clone(), display_ix);
            }
        }
        Slot::Reasoning(_) | Slot::Streaming | Slot::LiveReasoning => {}
    }
    layout.rows.push(DisplayRow::Slot(slot));
    layout.raw_index_of_row.push(raw_ix);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RowKey<'a> {
    Message(&'a str),
    Streaming,
    Reasoning(&'a str),
    LiveReasoning,
    Tool(&'a str),
    FoldedTurn(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resolved<'a> {
    Slot(Row<'a>),
    FoldedTurn {
        id: &'a str,
        request_text: &'a str,
        request_images: usize,
        closing: Option<RowKey<'a>>,
    },
}

impl<'a> Resolved<'a> {
    fn key(self) -> RowKey<'a> {
        match self {
            Self::Slot(row) => slot_key(row),
            Self::FoldedTurn { id, .. } => RowKey::FoldedTurn(id),
        }
    }
}

fn slot_key(row: Row<'_>) -> RowKey<'_> {
    match row {
        Row::Message(message) => RowKey::Message(&message.id),
        Row::Streaming(_) => RowKey::Streaming,
        Row::Reasoning(message) => RowKey::Reasoning(&message.id),
        Row::LiveReasoning(_) => RowKey::LiveReasoning,
        Row::Tool(tool) => RowKey::Tool(&tool.id),
    }
}

fn resolve_display<'a>(snapshot: &'a Snapshot, row: &'a DisplayRow) -> Option<Resolved<'a>> {
    match row {
        DisplayRow::Slot(slot) => transcript::resolve(snapshot, *slot).map(Resolved::Slot),
        DisplayRow::FoldedTurn {
            id,
            request_message,
            closing_slot,
        } => {
            let message = snapshot.messages.get(*request_message)?;
            let closing = closing_slot
                .and_then(|slot| transcript::resolve(snapshot, slot))
                .map(slot_key);
            Some(Resolved::FoldedTurn {
                id: id.as_str(),
                request_text: message.text.as_str(),
                request_images: message.images.len(),
                closing,
            })
        }
    }
}

/// Diff two display layouts into incremental `ListState` splices and remeasures.
///
/// Mirrors `transcript::changes`: retains ordered stable row keys in linear
/// time so measured heights and scroll anchors stay intact across streaming,
/// history paging, opening a folded turn, and Find jumps.
pub fn diff_layout(
    previous: Option<(&Snapshot, &[DisplayRow])>,
    next: Option<(&Snapshot, &[DisplayRow])>,
) -> Update {
    let count = next.map_or(0, |(_, rows)| rows.len());
    let (Some((prev_snap, prev_rows)), Some((next_snap, next_rows))) = (previous, next) else {
        return Update::Reset(count);
    };
    if prev_snap.id != next_snap.id || prev_snap.checkout != next_snap.checkout {
        return Update::Reset(count);
    }
    let old: Vec<_> = prev_rows
        .iter()
        .filter_map(|row| resolve_display(prev_snap, row))
        .collect();
    let new: Vec<_> = next_rows
        .iter()
        .filter_map(|row| resolve_display(next_snap, row))
        .collect();
    if old.len() != prev_rows.len() || new.len() != next_rows.len() {
        return Update::Reset(count);
    }
    let positions: HashMap<_, _> = old
        .iter()
        .enumerate()
        .map(|(index, row)| (row.key(), index))
        .collect();
    let keys: HashSet<_> = new.iter().map(|row| row.key()).collect();
    // Ambiguous identity must not preserve measurements or disclosure on the wrong row.
    if positions.len() != old.len() || keys.len() != new.len() {
        return Update::Reset(count);
    }

    let mut splices = Vec::new();
    let mut remeasure: Vec<Range<usize>> = Vec::new();
    let (mut old_start, mut new_start) = (0, 0);
    for (new_index, row) in new.iter().enumerate() {
        let Some(&old_index) = positions
            .get(&row.key())
            .filter(|&&index| index >= old_start)
        else {
            continue;
        };
        push_splice(
            &mut splices,
            new_start,
            old_index - old_start,
            new_index - new_start,
        );
        if old[old_index] != *row {
            if let Some(range) = remeasure.last_mut().filter(|range| range.end == new_index) {
                range.end += 1;
            } else {
                remeasure.push(new_index..new_index + 1);
            }
        }
        old_start = old_index + 1;
        new_start = new_index + 1;
    }
    push_splice(
        &mut splices,
        new_start,
        old.len() - old_start,
        new.len() - new_start,
    );
    Update::Patch { splices, remeasure }
}

fn push_splice(splices: &mut Vec<Splice>, start: usize, removed: usize, inserted: usize) {
    if removed != 0 || inserted != 0 {
        splices.push(Splice {
            range: start..start + removed,
            count: inserted,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Approval, ApprovalKind, Message, RecordState, Tool};

    fn turn(id: &str, state: TurnState, is_latest: bool) -> TurnInput<'_> {
        TurnInput {
            id,
            state,
            is_latest,
        }
    }

    fn message(id: &str, role: MessageRole, text: &str) -> Message {
        Message {
            id: id.into(),
            role,
            text: text.into(),
            ..Default::default()
        }
    }

    fn tool(id: &str, after: &str, status: ToolStatus) -> Tool {
        Tool {
            id: id.into(),
            name: "read".into(),
            status,
            input: r#"{"path":"src/lib.rs"}"#.into(),
            output: "ok".into(),
            after: Some(Some(after.into())),
        }
    }

    fn snapshot_with(messages: Vec<Message>, tools: Vec<Tool>, status: ThreadStatus) -> Snapshot {
        Snapshot {
            id: "thr_1".into(),
            checkout: "/repo".into(),
            status,
            messages,
            tools,
            ..Default::default()
        }
    }

    #[test]
    fn never_fold_rules_hold_individually() {
        let empty = HashSet::new();
        // Baseline: finished earlier turn folds when setting is on.
        assert!(is_folded(
            &turn("t1", TurnState::Finished, false),
            &empty,
            None,
            true
        ));
        // Latest turn never folds.
        assert!(!is_folded(
            &turn("t1", TurnState::Finished, true),
            &empty,
            None,
            true
        ));
        // Running, failed, stopped, and waiting-for-approval turns never fold.
        for state in [
            TurnState::Running,
            TurnState::Failed,
            TurnState::Stopped,
            TurnState::WaitingForApproval,
        ] {
            assert!(
                !is_folded(&turn("t1", state, false), &empty, None, true),
                "{state:?} must never fold"
            );
        }
        // Turn holding current Find match never folds.
        assert!(!is_folded(
            &turn("t1", TurnState::Finished, false),
            &empty,
            Some("t1"),
            true
        ));
        // Turn opened by the person never folds while in the tab's opened set.
        let opened = HashSet::from(["t1".to_owned()]);
        assert!(!is_folded(
            &turn("t1", TurnState::Finished, false),
            &opened,
            None,
            true
        ));
    }

    #[test]
    fn segmentation_keeps_observed_failures_and_stops_open_in_history() {
        let mut snap = snapshot_with(
            vec![message("failed", MessageRole::User, "first")],
            vec![],
            ThreadStatus::Error,
        );
        let mut outcomes = ObservedOutcomes::default();
        outcomes.observe(&snap);
        snap.messages
            .push(message("stopped", MessageRole::User, "second"));
        snap.status = ThreadStatus::Stopped;
        outcomes.observe(&snap);
        snap.messages
            .push(message("next", MessageRole::User, "third"));
        snap.status = ThreadStatus::Idle;
        for live_end in [true, false] {
            let slots = transcript::layout(&snap);
            let turns = segment_turns(&snap, &slots, live_end, &[], &outcomes);
            assert_eq!(turns[0].state, TurnState::Failed);
            assert_eq!(turns[1].state, TurnState::Stopped);
            let layout = build_layout(&snap, live_end, &[], &HashSet::new(), None, true, &outcomes);
            assert_eq!(
                layout.rows[..2],
                [DisplayRow::Slot(slots[0]), DisplayRow::Slot(slots[1])]
            );
        }
    }

    #[test]
    fn setting_off_folds_no_turn() {
        let turns = [
            turn("t1", TurnState::Finished, false),
            turn("t2", TurnState::Finished, false),
            turn("t3", TurnState::Finished, true),
        ];
        assert_eq!(
            folded_turns(&turns, &HashSet::new(), None, false),
            vec![false, false, false]
        );
    }

    #[test]
    fn opened_turn_stays_open_when_a_new_turn_becomes_latest() {
        let mut opened = HashSet::new();
        // Before opening: t1 is folded, t2 is latest (open).
        let before = [
            turn("t1", TurnState::Finished, false),
            turn("t2", TurnState::Finished, true),
        ];
        assert_eq!(
            folded_turns(&before, &opened, None, true),
            vec![true, false]
        );

        // Person opens t1.
        opened.insert("t1".into());
        assert_eq!(
            folded_turns(&before, &opened, None, true),
            vec![false, false]
        );

        // t3 arrives and becomes latest; t2 becomes an earlier turn and folds,
        // while t1 remains open for as long as the tab is open.
        let after = [
            turn("t1", TurnState::Finished, false),
            turn("t2", TurnState::Finished, false),
            turn("t3", TurnState::Running, true),
        ];
        assert_eq!(
            folded_turns(&after, &opened, None, true),
            vec![false, true, false]
        );
    }

    #[test]
    fn find_match_moving_between_turns_opens_only_the_current_match_turn() {
        let turns = [
            turn("t1", TurnState::Finished, false),
            turn("t2", TurnState::Finished, false),
            turn("t3", TurnState::Finished, true),
        ];
        let opened = HashSet::from(["t1".to_owned()]);

        // Match in t2 opens t2; t1 stays open because the person opened it.
        assert_eq!(
            folded_turns(&turns, &opened, Some("t2"), true),
            vec![false, false, false]
        );
        // Match moves to t3: t2 folds back, t1 stays open.
        assert_eq!(
            folded_turns(&turns, &opened, Some("t3"), true),
            vec![false, true, false]
        );
        // Match moves to t1, then Find closes: t1 stays open, t2 stays folded.
        assert_eq!(
            folded_turns(&turns, &opened, Some("t1"), true),
            vec![false, true, false]
        );
        assert_eq!(
            folded_turns(&turns, &opened, None, true),
            vec![false, true, false]
        );
    }

    #[test]
    fn first_request_line_picks_first_nonempty_line_or_image_fallback() {
        assert_eq!(
            first_request_line("\n  First line  \nSecond line", 0),
            "First line"
        );
        assert_eq!(first_request_line("   ", 1), "Image");
        assert_eq!(first_request_line("", 3), "3 images");
        assert_eq!(first_request_line("", 0), "");
    }

    #[test]
    fn build_layout_folds_earlier_turns_and_keeps_latest_and_non_finished_open() {
        let mut snap = snapshot_with(
            vec![
                message("u1", MessageRole::User, "First request\nsecond line"),
                message("a1", MessageRole::Assistant, "First answer"),
                message("u2", MessageRole::User, "Interrupted request"),
                message("a2", MessageRole::Assistant, "Partial answer"),
                message("u3", MessageRole::User, "Latest request"),
                message("a3", MessageRole::Assistant, "Latest answer"),
            ],
            vec![
                tool("t1", "u1", ToolStatus::Done),
                tool("t2", "u2", ToolStatus::Interrupted),
            ],
            ThreadStatus::Idle,
        );
        let layout = build_layout(
            &snap,
            true,
            &[],
            &HashSet::new(),
            None,
            true,
            &Default::default(),
        );
        // Turn 1 (u1, t1, a1) folds into 1 row; Turn 2 has an interrupted tool so stays open (3 rows);
        // Turn 3 is latest so stays open (2 rows).
        assert_eq!(layout.len(), 1 + 3 + 2);
        assert_eq!(
            layout.rows[0],
            DisplayRow::FoldedTurn {
                id: "u1".into(),
                request_message: 0,
                closing_slot: Some(Slot::Message(1)),
            }
        );
        assert_eq!(
            layout.turn_of_message.get("a1").map(String::as_str),
            Some("u1")
        );
        assert!(!layout.row_of_message.contains_key("a1"));
        assert_eq!(layout.row_of_message.get("u2"), Some(&1));

        // Waiting for approval keeps latest open even when detached is false.
        snap.pending_approval = Some(Approval {
            id: "ap1".into(),
            tool_id: "t3".into(),
            kind: ApprovalKind::Command,
            summary: "cargo test".into(),
            details: "{}".into(),
            dirty_paths: Vec::new(),
            risk: String::new(),
        });
        let turns = segment_turns(
            &snap,
            &transcript::layout(&snap),
            true,
            &[],
            &Default::default(),
        );
        assert_eq!(turns.last().unwrap().state, TurnState::WaitingForApproval);
    }

    #[test]
    fn steered_requests_sharing_a_turn_record_fold_as_one_turn() {
        let snap = snapshot_with(
            vec![
                message("request-r1", MessageRole::User, "Start the refactor"),
                message("a1", MessageRole::Assistant, "Working on it"),
                message("request-r2", MessageRole::User, "Also rename foo"),
                message("a2", MessageRole::Assistant, "Done"),
                message("request-r3", MessageRole::User, "Latest"),
                message("a3", MessageRole::Assistant, "Latest answer"),
            ],
            vec![],
            ThreadStatus::Idle,
        );
        let mut record = TurnRecord {
            id: "rec-1".into(),
            request_ids: vec!["r1".into(), "r2".into()],
            request_ids_offset: 0,
            request_ids_total: 2,
            kind: RecordKind::Turn,
            undoes: None,
            state: RecordState::Ready,
            head_moved: false,
            outcome: None,
            after_restart: false,
            paths: Vec::new(),
            paths_offset: 0,
            paths_total: 0,
            not_covered: Vec::new(),
            not_covered_offset: 0,
            not_covered_total: 0,
            created_at: 1000,
        };
        let mut observed = ObservedOutcomes::default();
        observed
            .terminal
            .insert("request-r2".into(), TurnState::Stopped);
        for (host, expected) in [
            (None, TurnState::Stopped),
            (Some(TurnOutcome::Finished), TurnState::Finished),
            (Some(TurnOutcome::Failed), TurnState::Failed),
            (Some(TurnOutcome::Interrupted), TurnState::Stopped),
            (Some(TurnOutcome::Stopped), TurnState::Stopped),
        ] {
            record.outcome = host;
            let turns = segment_turns(
                &snap,
                &transcript::layout(&snap),
                true,
                std::slice::from_ref(&record),
                &observed,
            );
            assert_eq!(turns[0].state, expected);
        }
        record.outcome = None;
        let layout = build_layout(
            &snap,
            true,
            &[record],
            &HashSet::new(),
            None,
            true,
            &Default::default(),
        );
        // r1 + a1 + r2 + a2 fold into ONE FoldedTurn closing at a2 (index 3); r3 + a3 stay open.
        assert_eq!(layout.len(), 3);
        assert_eq!(
            layout.rows[0],
            DisplayRow::FoldedTurn {
                id: "request-r1".into(),
                request_message: 0,
                closing_slot: Some(Slot::Message(3)),
            }
        );
        assert_eq!(
            layout.turn_of_message.get("a2").map(String::as_str),
            Some("request-r1")
        );
    }

    #[test]
    fn work_requires_a_recorded_finish_and_both_ends_of_the_turn() {
        let mut snapshot = snapshot_with(
            vec![
                message("request-start", MessageRole::User, "Start."),
                message("progress", MessageRole::Assistant, "Checking."),
                message("request-steer", MessageRole::User, "Include the guide."),
                message("answer", MessageRole::Assistant, "Done."),
            ],
            vec![tool("t", "request-start", ToolStatus::Done)],
            ThreadStatus::Idle,
        );
        snapshot.messages[3].reasoning = "Final check".into();
        let mut record: TurnRecord = serde_json::from_value(serde_json::json!({
            "id":"turn", "request_ids":["start","steer"], "request_ids_offset":0,
            "request_ids_total":2, "kind":"turn", "state":"ready", "head_moved":false,
            "after_restart":false, "paths":[], "paths_offset":0, "paths_total":0,
            "not_covered":[], "not_covered_offset":0, "not_covered_total":0, "created_at":1,
        }))
        .unwrap();
        let layout = |s: &Snapshot, r: &TurnRecord, live_end, find| {
            build_layout(
                s,
                live_end,
                std::slice::from_ref(r),
                &HashSet::new(),
                find,
                false,
                &Default::default(),
            )
        };
        for outcome in [
            None,
            Some(TurnOutcome::Failed),
            Some(TurnOutcome::Stopped),
            Some(TurnOutcome::Interrupted),
        ] {
            record.outcome = outcome;
            assert!(layout(&snapshot, &record, true, None).work.is_empty());
        }
        record.outcome = Some(TurnOutcome::Finished);
        for find in [
            None,
            Some("progress"),
            Some("request-steer"),
            Some("answer"),
        ] {
            let display = layout(&snapshot, &record, true, find);
            let fold = display.work_at(1).unwrap();
            assert_eq!(display.len(), 6);
            assert_eq!(fold.rows, 1..5);
            assert!(display.work_at(0).is_none() && display.work_at(5).is_none());
            assert_eq!(
                fold.open(true),
                Some(matches!(find, Some("progress" | "request-steer")))
            );
            assert_eq!(fold.open(false), None);
            assert_eq!(fold.summary, "1 step · 1 read");
        }
        let tab = ObservedOutcomes::default();
        tab.set_work_open("request-start", true);
        let reopened = build_layout(
            &snapshot,
            true,
            std::slice::from_ref(&record),
            &HashSet::new(),
            None,
            false,
            &tab,
        );
        assert_eq!(reopened.work[0].open(true), Some(true));
        assert_eq!(reopened.work[0].open(false), None);
        assert_eq!(
            layout(&snapshot, &record, true, None).work[0].open(true),
            Some(false)
        );
        for status in [
            ThreadStatus::Running,
            ThreadStatus::AwaitingApproval,
            ThreadStatus::Error,
            ThreadStatus::Stopped,
            ThreadStatus::Interrupted,
        ] {
            snapshot.status = status;
            assert!(layout(&snapshot, &record, true, None).work.is_empty());
        }
        snapshot.status = ThreadStatus::Idle;
        assert!(layout(&snapshot, &record, false, None).work.is_empty());
        snapshot.messages.pop();
        assert!(layout(&snapshot, &record, true, None).work.is_empty());
        snapshot.messages.truncate(1);
        snapshot.tools.clear();
        snapshot
            .messages
            .push(message("answer", MessageRole::Assistant, "Direct reply."));
        assert!(layout(&snapshot, &record, true, None).work.is_empty());
        snapshot.messages[1].reasoning = "Check.".into();
        assert!(layout(&snapshot, &record, true, None).work.is_empty());
        record.request_ids.pop();
        record.request_ids_total = 1;
        assert_eq!(
            layout(&snapshot, &record, true, None).work[0].summary,
            "0 steps"
        );
        snapshot.messages[0].id = "request-steer".into();
        assert!(layout(&snapshot, &record, true, None).work.is_empty());
    }

    #[test]
    fn opening_a_folded_turn_diffs_as_a_single_splice() {
        let snap = snapshot_with(
            vec![
                message("u1", MessageRole::User, "First"),
                message("a1", MessageRole::Assistant, "Answer 1"),
                message("u2", MessageRole::User, "Second"),
                message("a2", MessageRole::Assistant, "Answer 2"),
                message("u3", MessageRole::User, "Latest"),
                message("a3", MessageRole::Assistant, "Answer 3"),
            ],
            vec![tool("t2", "u2", ToolStatus::Done)],
            ThreadStatus::Idle,
        );
        let closed = build_layout(
            &snap,
            true,
            &[],
            &HashSet::new(),
            None,
            true,
            &Default::default(),
        );
        let opened_set = HashSet::from(["u2".to_owned()]);
        let opened = build_layout(
            &snap,
            true,
            &[],
            &opened_set,
            None,
            true,
            &Default::default(),
        );
        assert_eq!(
            diff_layout(Some((&snap, &closed.rows)), Some((&snap, &opened.rows))),
            Update::Patch {
                splices: vec![Splice {
                    range: 1..2,
                    count: 3,
                }],
                remeasure: vec![],
            }
        );
    }
}
