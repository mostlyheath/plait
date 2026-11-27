//! GPUI-free automations view models, formatting, and state transitions.

use std::{collections::HashMap, time::Duration};

use crate::{
    host::automations::{
        Definition, DetailReply, LastResult, Occurrence, OccurrenceOutcome, Schedule, State,
        Summary,
    },
    navigation,
};

/// Combined sidebar and header view model for one host automation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationItem {
    pub summary: Summary,
    needs_deadline: bool,
    pub thread_id: Option<String>,
    pub pause_reason: Option<String>,
    pub error: Option<String>,
    pub definition: Option<Definition>,
    pub occurrences: Vec<Occurrence>,
}

impl AutomationItem {
    pub fn from_summary(summary: Summary) -> Self {
        let thread_id = summary.thread_id.clone();
        let pause_reason = summary.pause_reason.clone();
        let error = summary.error.clone();
        Self {
            summary,
            needs_deadline: false,
            thread_id,
            pause_reason,
            error,
            definition: None,
            occurrences: Vec::new(),
        }
    }

    /// Merges a `get_automation` reply into this item.
    pub fn apply_detail(&mut self, detail: DetailReply) {
        let record = detail.automation;
        self.summary.name = record.definition.name.clone();
        self.summary.schedule = record.definition.schedule.clone();
        self.summary.target_path = record.definition.target.path().to_path_buf();
        self.needs_deadline = self.summary.state != State::Active
            && record.state == State::Active
            && self.summary.next_run.is_none();
        self.summary.state = record.state;
        if record.state != State::Active {
            self.summary.next_run = None;
        }
        if let Some(latest) = detail.occurrences.first() {
            self.summary.last_result = Some(LastResult {
                due: latest.due,
                outcome: latest.outcome,
                detail: latest.detail.clone(),
            });
        }
        self.thread_id = record.thread_id.or(self.thread_id.take());
        self.pause_reason = record.pause_reason;
        self.error = record.error;
        self.definition = Some(record.definition);
        self.occurrences = detail.occurrences;
    }

    /// Merges a refreshed `Summary` (from `list_automations`, `pause_automation`,
    /// or `resume_automation`) while retaining previously loaded detail fields.
    pub fn apply_summary(&mut self, summary: Summary) {
        if let Some(thread_id) = summary.thread_id.clone() {
            self.thread_id = Some(thread_id);
        }
        if summary.state == State::Active {
            self.pause_reason = None;
            self.error = None;
        } else if let Some(reason) = summary.pause_reason.clone() {
            self.pause_reason = Some(reason);
        }
        if let Some(error) = summary.error.clone() {
            self.error = Some(error);
        }
        self.summary = summary;
    }

    fn has_receipt(&self, due: u64) -> bool {
        let decided = |time, outcome| time == due && outcome != OccurrenceOutcome::Pending;
        self.summary
            .last_result
            .as_ref()
            .is_some_and(|r| decided(r.due, r.outcome))
            || self.occurrences.iter().any(|r| decided(r.due, r.outcome))
    }

    pub fn effective_thread_id(&self) -> Option<&str> {
        self.thread_id
            .as_deref()
            .or(self.summary.thread_id.as_deref())
    }

    /// Reason why a paused automation stopped running, if the host reported one.
    pub fn effective_pause_reason(&self) -> Option<&str> {
        if self.summary.state != State::Paused {
            return None;
        }
        self.pause_reason
            .as_deref()
            .or(self.summary.pause_reason.as_deref())
            .or_else(|| {
                self.summary
                    .last_result
                    .as_ref()
                    .filter(|last| last.outcome == OccurrenceOutcome::Paused)
                    .and_then(|last| last.detail.as_deref())
            })
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
    }

    /// Placement error for a failed automation, if reported.
    pub fn effective_error(&self) -> Option<&str> {
        if self.summary.state != State::Failed {
            return None;
        }
        self.error
            .as_deref()
            .or(self.summary.error.as_deref())
            .map(str::trim)
            .filter(|message| !message.is_empty())
    }

    pub fn supported(&self) -> bool {
        self.summary.state != State::Unsupported && self.summary.schedule != Schedule::Unsupported
    }
}

/// Window-owned state for the selected host's automations.
#[derive(Default)]
pub struct AutomationsState {
    /// True only once the current host answered `list_automations` without `invalid_request`.
    pub supported: bool,
    pub items: Vec<AutomationItem>,
    pub revision: u64,
    pub collapsed: bool,
    pub loading: bool,
    pub refresh_queued: bool,
    preparing_polls: u32,
    // Last advertised deadline and retry exponent for each active card.
    deadlines: HashMap<String, (u64, u32)>,
    // Keep stamps for deleted IDs so outstanding reads cannot restore them.
    card_revisions: HashMap<String, u64>,
    refresh_revision: u64,
    pub busy: Option<String>,
    pub confirm_delete: Option<String>,
    /// Inline status or error note for one automation: `(id, message, is_error)`.
    pub note: Option<(String, String, bool)>,
    pub error: Option<String>,
    pub generation: u64,
    pub action_generation: u64,
    pub epoch: u64,
    /// Thread ID to open once the thread list catches up with a newly placed automation.
    pub pending_open_thread: Option<String>,
}

impl AutomationsState {
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn reset_for_machine(&mut self) {
        self.reset_for_reconnect(self.epoch);
    }

    pub fn reset_for_reconnect(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.supported = false;
        self.items.clear();
        self.deadlines.clear();
        self.card_revisions.clear();
        self.refresh_queued = false;
        self.preparing_polls = 0;
        self.loading = false;
        self.busy = None;
        self.confirm_delete = None;
        self.note = None;
        self.error = None;
        self.pending_open_thread = None;
        self.generation = self.generation.wrapping_add(1);
        self.action_generation = self.action_generation.wrapping_add(1);
        self.bump();
    }

    pub fn begin_refresh(&mut self, epoch: u64) -> Option<u64> {
        if self.loading {
            self.refresh_queued = true;
            return None;
        }
        self.refresh_queued = false;
        self.epoch = epoch;
        self.generation = self.generation.wrapping_add(1);
        self.refresh_revision = self.revision;
        self.loading = true;
        self.error = None;
        Some(self.generation)
    }

    pub fn next_preparing_delay(&mut self) -> Option<std::time::Duration> {
        if !self.has_preparing() {
            self.preparing_polls = 0;
            return None;
        }
        if self.preparing_polls >= 6 {
            return None;
        }
        let seconds = 1 << self.preparing_polls;
        self.preparing_polls += 1;
        Some(std::time::Duration::from_secs(seconds))
    }

    pub fn card_revision(&self, id: &str) -> u64 {
        self.card_revisions.get(id).copied().unwrap_or_default()
    }

    pub fn record_successful_action(&mut self, id: &str) {
        self.bump();
        self.card_revisions.insert(id.to_owned(), self.revision);
    }

    pub fn next_refresh_delay(&mut self, now_ms: u64) -> Option<Duration> {
        let previous = std::mem::take(&mut self.deadlines);
        let scheduled = self
            .items
            .iter()
            .filter(|item| item.supported() && item.summary.state == State::Active)
            .filter_map(|item| {
                let summary = &item.summary;
                // A claim can advance the schedule before the host records its decision.
                let pending = previous.get(&summary.id).copied().filter(|(due, _)| {
                    (*due <= now_ms || summary.next_run.is_none()) && !item.has_receipt(*due)
                });
                let (due, attempt) = pending.or_else(|| summary.next_run.map(|due| (due, 0)))?;
                let (delay, next_attempt) = if due > now_ms {
                    (Duration::from_millis((due - now_ms).max(1000)), 0)
                } else {
                    (Duration::from_secs(1 << attempt), (attempt + 1).min(5))
                };
                self.deadlines
                    .insert(summary.id.clone(), (due, next_attempt));
                Some(delay)
            })
            .min();
        scheduled
            .into_iter()
            .chain(self.next_preparing_delay())
            .min()
    }

    /// Installs a background refresh result if its epoch and generation still match.
    pub fn apply_list(
        &mut self,
        epoch: u64,
        generation: u64,
        result: Result<Option<Vec<AutomationItem>>, String>,
    ) -> bool {
        if self.epoch != epoch || self.generation != generation {
            return false;
        }
        self.loading = false;
        match result {
            Ok(Some(items)) => {
                self.supported = true;
                let mut merged = Vec::with_capacity(items.len());
                for item in items {
                    if self.card_revision(&item.summary.id) <= self.refresh_revision {
                        self.refresh_queued |= item.needs_deadline;
                        merged.push(item);
                    } else if let Some(current) = self.by_id(&item.summary.id) {
                        merged.push(current.clone());
                    }
                }
                for current in &self.items {
                    if self.card_revision(&current.summary.id) > self.refresh_revision
                        && !merged
                            .iter()
                            .any(|item| item.summary.id == current.summary.id)
                    {
                        merged.push(current.clone());
                    }
                }
                self.items = merged;
                self.error = None;
                if let Some(confirm) = &self.confirm_delete
                    && !self.items.iter().any(|item| item.summary.id == *confirm)
                {
                    self.confirm_delete = None;
                }
            }
            Ok(None) => {
                self.items.retain(|item| {
                    self.card_revisions
                        .get(&item.summary.id)
                        .is_some_and(|revision| *revision > self.refresh_revision)
                });
                self.supported = !self.items.is_empty();
                self.confirm_delete = None;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
            }
        }
        self.bump();
        true
    }

    pub fn begin_action(&mut self, epoch: u64, id: String) -> Option<u64> {
        if self.busy.is_some() || !self.by_id(&id).is_some_and(AutomationItem::supported) {
            return None;
        }
        self.epoch = epoch;
        self.action_generation = self.action_generation.wrapping_add(1);
        self.busy = Some(id);
        Some(self.action_generation)
    }

    pub fn finish_action(&mut self, epoch: u64, generation: u64) -> bool {
        if self.epoch != epoch || self.action_generation != generation {
            return false;
        }
        self.busy = None;
        true
    }

    pub fn apply_summary(&mut self, epoch: u64, summary: Summary) -> bool {
        if self.epoch != epoch {
            return false;
        }
        if let Some(existing) = self
            .items
            .iter_mut()
            .find(|item| item.summary.id == summary.id)
        {
            existing.apply_summary(summary);
        } else {
            self.items.push(AutomationItem::from_summary(summary));
        }
        self.bump();
        true
    }

    pub fn apply_detail(&mut self, epoch: u64, detail: DetailReply) -> bool {
        if self.epoch != epoch {
            return false;
        }
        let id = detail.automation.id.clone();
        if let Some(existing) = self.items.iter_mut().find(|item| item.summary.id == id) {
            existing.apply_detail(detail);
            self.refresh_queued |= existing.needs_deadline;
            self.bump();
            return true;
        }
        false
    }

    pub fn apply_deleted(&mut self, epoch: u64, id: &str) -> bool {
        if self.epoch != epoch {
            return false;
        }
        let before = self.items.len();
        self.items.retain(|item| item.summary.id != id);
        if self.confirm_delete.as_deref() == Some(id) {
            self.confirm_delete = None;
        }
        if self
            .note
            .as_ref()
            .is_some_and(|(note_id, _, _)| note_id == id)
        {
            self.note = None;
        }
        if self.items.len() != before {
            self.bump();
        }
        true
    }

    pub fn set_confirm_delete(&mut self, id: Option<String>) {
        if self.confirm_delete != id {
            self.confirm_delete = id;
            self.bump();
        }
    }

    pub fn set_note(&mut self, id: String, message: String, is_error: bool) {
        self.note = Some((id, message, is_error));
        self.bump();
    }

    pub fn clear_note(&mut self, id: &str) {
        if self
            .note
            .as_ref()
            .is_some_and(|(note_id, _, _)| note_id == id)
        {
            self.note = None;
            self.bump();
        }
    }

    pub fn by_id(&self, id: &str) -> Option<&AutomationItem> {
        self.items.iter().find(|item| item.summary.id == id)
    }

    pub fn by_thread(&self, thread_id: &str) -> Option<&AutomationItem> {
        self.items
            .iter()
            .find(|item| item.effective_thread_id() == Some(thread_id))
    }

    pub fn has_preparing(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.supported() && item.summary.state == State::Preparing)
    }

    /// Injects state for headless UI scene tests. Never call from production code.
    pub fn set_for_test(&mut self, supported: bool, items: Vec<AutomationItem>) {
        self.supported = supported;
        self.items = items;
        self.loading = false;
        self.error = None;
        self.bump();
    }
}

pub fn state_label(state: State) -> &'static str {
    match state {
        State::Active => "Active",
        State::Paused => "Paused",
        State::Preparing => "Preparing",
        State::Failed => "Failed",
        State::Unsupported => "Unsupported",
    }
}

pub fn outcome_label(outcome: OccurrenceOutcome) -> &'static str {
    match outcome {
        OccurrenceOutcome::Pending => "Pending",
        OccurrenceOutcome::Queued => "Queued",
        OccurrenceOutcome::Skipped => "Skipped",
        OccurrenceOutcome::Paused => "Paused",
        OccurrenceOutcome::Failed => "Failed",
        OccurrenceOutcome::Missed => "Missed",
        OccurrenceOutcome::Other => "Recorded",
    }
}

fn short_weekday(day: u8) -> Option<&'static str> {
    match day {
        0 => Some("Sun"),
        1 => Some("Mon"),
        2 => Some("Tue"),
        3 => Some("Wed"),
        4 => Some("Thu"),
        5 => Some("Fri"),
        6 => Some("Sat"),
        _ => None,
    }
}

fn plural_weekday(day: u8) -> Option<&'static str> {
    match day {
        0 => Some("Sundays"),
        1 => Some("Mondays"),
        2 => Some("Tuesdays"),
        3 => Some("Wednesdays"),
        4 => Some("Thursdays"),
        5 => Some("Fridays"),
        6 => Some("Saturdays"),
        _ => None,
    }
}

/// Compact human-readable schedule phrase for sidebar cards.
pub fn schedule_words(schedule: &Schedule) -> String {
    match schedule {
        Schedule::Unsupported => "Unsupported".into(),
        Schedule::Once { at, .. } => match at.split_once('T') {
            Some((date, time)) => format!("Once, {date} at {time}"),
            None => format!("Once at {at}"),
        },
        Schedule::Hourly { minute: 0, .. } => "Hourly".into(),
        Schedule::Hourly { minute, .. } => format!("Hourly at :{minute:02}"),
        Schedule::Daily { time, .. } => format!("Daily at {time}"),
        Schedule::Weekdays { time, .. } => format!("Weekdays at {time}"),
        Schedule::Weekly { days, time, .. } => {
            let mut sorted = days.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.len() == 1
                && let Some(day) = plural_weekday(sorted[0])
            {
                return format!("{day} at {time}");
            }
            let names: Vec<&str> = sorted.into_iter().filter_map(short_weekday).collect();
            if names.is_empty() {
                format!("Weekly at {time}")
            } else {
                format!("{} at {time}", names.join(", "))
            }
        }
    }
}

/// Full schedule phrase including its IANA time zone for headers and hover text.
pub fn schedule_with_zone(schedule: &Schedule) -> String {
    let words = schedule_words(schedule);
    let zone = schedule.time_zone().trim();
    if zone.is_empty() {
        words
    } else {
        format!("{words} ({zone})")
    }
}

/// Relative description of an upcoming run time, matching sidebar age style.
pub fn format_next_run(next_run: Option<u64>, now_ms: u64) -> Option<String> {
    let next = next_run?;
    if next <= now_ms {
        return Some("Due now".into());
    }
    let remaining_ms = next - now_ms;
    let minutes = remaining_ms / 60_000;
    let phrase = match minutes {
        0 => "Next in <1m".into(),
        1..60 => format!("Next in {minutes}m"),
        60..1440 => {
            let hours = minutes / 60;
            let rem = minutes % 60;
            if rem == 0 {
                format!("Next in {hours}h")
            } else {
                format!("Next in {hours}h {rem}m")
            }
        }
        _ => format!("Next in {}d", minutes / 1440),
    };
    Some(phrase)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastResultText {
    /// Short card summary, e.g. `Queued · 6m` or `Skipped · 2h`.
    pub compact: String,
    /// Full description including host detail when present.
    pub full: String,
    pub outcome: OccurrenceOutcome,
}

/// Formats an automation's latest occurrence receipt without inventing missing detail.
pub fn format_last_result(last: Option<&LastResult>, now_ms: u64) -> Option<LastResultText> {
    let last = last?;
    let label = outcome_label(last.outcome);
    let age = navigation::age(Some(last.due), now_ms);
    let detail = last
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let compact = if age.is_empty() {
        label.to_owned()
    } else {
        format!("{label} · {age}")
    };
    let full = match (detail, age.is_empty()) {
        (Some(detail), false) => format!("{label}: {detail} ({age})"),
        (Some(detail), true) => format!("{label}: {detail}"),
        (None, false) => format!("{label} ({age})"),
        (None, true) => label.to_owned(),
    };
    Some(LastResultText {
        compact,
        full,
        outcome: last.outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{
        ApprovalMode,
        automations::{Automation, CommandPolicy, Target},
        harness::HarnessId,
    };

    fn sample_summary(id: &str, state: State) -> Summary {
        Summary {
            id: id.into(),
            name: "Daily audit".into(),
            schedule: Schedule::Weekdays {
                time: "09:00".into(),
                time_zone: "America/New_York".into(),
            },
            target_path: "/code/forecast".into(),
            state,
            next_run: (state == State::Active).then_some(1_000_000 + 3 * 3_600_000),
            last_result: None,
            pause_reason: None,
            error: None,
            thread_id: None,
        }
    }

    #[test]
    fn schedule_words_formats_every_kind() {
        assert_eq!(
            schedule_words(&Schedule::Once {
                at: "2026-06-01T09:30".into(),
                time_zone: "UTC".into(),
            }),
            "Once, 2026-06-01 at 09:30"
        );
        assert_eq!(
            schedule_words(&Schedule::Hourly {
                minute: 0,
                time_zone: "UTC".into(),
            }),
            "Hourly"
        );
        assert_eq!(
            schedule_words(&Schedule::Hourly {
                minute: 15,
                time_zone: "UTC".into(),
            }),
            "Hourly at :15"
        );
        assert_eq!(
            schedule_words(&Schedule::Daily {
                time: "08:00".into(),
                time_zone: "UTC".into(),
            }),
            "Daily at 08:00"
        );
        assert_eq!(
            schedule_with_zone(&Schedule::Weekdays {
                time: "09:00".into(),
                time_zone: "America/New_York".into(),
            }),
            "Weekdays at 09:00 (America/New_York)"
        );
        assert_eq!(
            schedule_words(&Schedule::Weekly {
                days: vec![1],
                time: "10:00".into(),
                time_zone: "UTC".into(),
            }),
            "Mondays at 10:00"
        );
        assert_eq!(
            schedule_words(&Schedule::Weekly {
                days: vec![5, 1, 3, 1],
                time: "17:30".into(),
                time_zone: "UTC".into(),
            }),
            "Mon, Wed, Fri at 17:30"
        );
    }

    #[test]
    fn next_run_and_last_result_format_honestly() {
        let now = 10_000_000;
        assert_eq!(format_next_run(None, now), None);
        assert_eq!(format_next_run(Some(now), now).as_deref(), Some("Due now"));
        assert_eq!(
            format_next_run(Some(now + 20_000), now).as_deref(),
            Some("Next in <1m")
        );
        assert_eq!(
            format_next_run(Some(now + 15 * 60_000), now).as_deref(),
            Some("Next in 15m")
        );
        assert_eq!(
            format_next_run(Some(now + 3 * 3_600_000), now).as_deref(),
            Some("Next in 3h")
        );
        assert_eq!(
            format_next_run(Some(now + 3 * 3_600_000 + 20 * 60_000), now).as_deref(),
            Some("Next in 3h 20m")
        );
        assert_eq!(
            format_next_run(Some(now + 2 * 86_400_000), now).as_deref(),
            Some("Next in 2d")
        );

        let skipped = LastResult {
            due: now - 10 * 60_000,
            outcome: OccurrenceOutcome::Skipped,
            detail: Some("Checkout is busy.".into()),
        };
        let formatted = format_last_result(Some(&skipped), now).unwrap();
        assert_eq!(formatted.compact, "Skipped · 10m");
        assert_eq!(formatted.full, "Skipped: Checkout is busy. (10m)");
    }

    #[test]
    fn refresh_finishing_after_resume_keeps_active_card_and_updates_its_neighbor() {
        let mut state = AutomationsState::default();
        let paused = sample_summary("a", State::Paused);
        state.apply_summary(0, paused.clone());
        let read = state.begin_refresh(0).unwrap();
        let action = state.begin_action(0, "a".into()).unwrap();
        assert!(state.finish_action(0, action));
        state.record_successful_action("a");
        state.apply_summary(0, sample_summary("a", State::Active));
        let neighbor = sample_summary("b", State::Failed);
        state.apply_list(
            0,
            read,
            Ok(Some(vec![
                AutomationItem::from_summary(paused.clone()),
                AutomationItem::from_summary(neighbor.clone()),
            ])),
        );
        assert_eq!(state.by_id("a").unwrap().summary.state, State::Active);
        assert_eq!(state.by_id("b").unwrap().summary, neighbor);

        let fresh = state.begin_refresh(0).unwrap();
        state.apply_list(
            0,
            fresh,
            Ok(Some(vec![AutomationItem::from_summary(paused)])),
        );
        assert_eq!(state.by_id("a").unwrap().summary.state, State::Paused);
    }

    #[test]
    fn refresh_finishing_after_delete_keeps_card_deleted() {
        let mut state = AutomationsState::default();
        let old = sample_summary("a", State::Paused);
        state.apply_summary(0, old.clone());
        let read = state.begin_refresh(0).unwrap();
        let action = state.begin_action(0, "a".into()).unwrap();
        assert!(state.finish_action(0, action));
        state.record_successful_action("a");
        state.apply_deleted(0, "a");
        state.apply_list(0, read, Ok(Some(vec![AutomationItem::from_summary(old)])));
        assert!(state.by_id("a").is_none());
    }

    #[test]
    fn idle_refresh_follows_earliest_active_run_and_retries_overdue_runs() {
        let mut state = AutomationsState::default();
        for (id, due) in [("later", 12_000), ("first", 8_000)] {
            let mut active = sample_summary(id, State::Active);
            active.next_run = Some(due);
            state.apply_summary(0, active);
        }
        assert_eq!(
            state.next_refresh_delay(2_000),
            Some(Duration::from_secs(6))
        );
        assert_eq!(
            state.next_refresh_delay(9_000),
            Some(Duration::from_secs(1))
        );
        state.apply_summary(0, sample_summary("first", State::Paused));
        state.apply_deleted(0, "later");
        assert_eq!(state.next_refresh_delay(9_000), None);
    }

    #[test]
    fn claimed_run_is_watched_until_its_receipt() {
        let mut state = AutomationsState::default();
        let mut summary = sample_summary("once", State::Active);
        summary.next_run = Some(5000);
        state.apply_summary(0, summary.clone());
        assert_eq!(state.next_refresh_delay(0), Some(Duration::from_secs(5)));
        summary.next_run = None;
        state.apply_summary(0, summary.clone());
        for seconds in [1, 2, 4, 8, 16, 32, 32] {
            assert_eq!(
                state.next_refresh_delay(6000),
                Some(Duration::from_secs(seconds))
            );
        }
        summary.last_result = Some(LastResult {
            due: 5000,
            outcome: OccurrenceOutcome::Queued,
            detail: None,
        });
        state.apply_summary(0, summary);
        assert_eq!(state.next_refresh_delay(6000), None);
    }

    #[test]
    fn preparing_polls_back_off_and_stop() {
        let mut state = AutomationsState::default();
        state.apply_summary(0, sample_summary("a", State::Preparing));
        for seconds in [1, 2, 4, 8, 16, 32] {
            assert_eq!(state.next_preparing_delay().unwrap().as_secs(), seconds);
        }
        assert!(state.next_preparing_delay().is_none());
        let mut summary = sample_summary("a", State::Preparing);
        summary.schedule = Schedule::Unsupported;
        state.apply_summary(0, summary);
        assert!(!state.has_preparing());
        assert!(state.begin_action(0, "a".into()).is_none());
    }

    fn sample_detail(state: State) -> DetailReply {
        DetailReply {
            automation: Automation {
                id: "a1".into(),
                thread_id: Some("thr-1".into()),
                definition: Definition {
                    name: "Daily audit".into(),
                    instructions: "Check dependencies".into(),
                    target: Target::Project {
                        checkout: "/code/forecast".into(),
                    },
                    harness: HarnessId::Pi,
                    provider: "faux".into(),
                    model_id: "faux-1".into(),
                    mode: None,
                    thinking: None,
                    approval: ApprovalMode::Auto,
                    command_policy: CommandPolicy::Sandboxed,
                    schedule: Schedule::Weekdays {
                        time: "09:00".into(),
                        time_zone: "America/New_York".into(),
                    },
                    catch_up_ms: Some(43_200_000),
                },
                revision: Some(2),
                state,
                pause_reason: Some("target missing".into()),
                cursor: 1_000_000,
                missed_count: 0,
                admission_request_id: None,
                error: None,
            },
            occurrences: Vec::new(),
        }
    }

    #[test]
    fn placement_activating_during_list_read_queues_a_deadline_read() {
        let mut placed = AutomationItem::from_summary(sample_summary("a1", State::Preparing));
        placed.apply_detail(sample_detail(State::Active));
        let mut placement = AutomationsState::default();
        let read = placement.begin_refresh(1).unwrap();
        placement.apply_list(1, read, Ok(Some(vec![placed])));
        assert!(placement.refresh_queued);
    }

    #[test]
    fn state_guards_reject_stale_replies_and_preserve_details() {
        let mut state = AutomationsState::default();
        let gen1 = state.begin_refresh(1).unwrap();
        assert_eq!(state.begin_refresh(1), None);
        assert!(state.refresh_queued);
        state.reset_for_reconnect(1);
        let gen2 = state.begin_refresh(1).unwrap();
        assert!(!state.apply_list(1, gen1, Ok(Some(vec![]))));
        assert!(state.loading);

        let item = AutomationItem::from_summary(sample_summary("a1", State::Paused));
        assert!(state.apply_list(1, gen2, Ok(Some(vec![item]))));
        assert!(state.supported);
        assert_eq!(state.items.len(), 1);

        assert!(state.apply_detail(1, sample_detail(State::Paused)));
        assert_eq!(
            state.by_thread("thr-1").unwrap().effective_pause_reason(),
            Some("target missing")
        );

        // Resuming clears pause_reason while preserving thread_id.
        assert!(state.apply_summary(1, sample_summary("a1", State::Active)));
        let resumed = state.by_thread("thr-1").unwrap();
        assert_eq!(resumed.summary.state, State::Active);
        assert_eq!(resumed.effective_pause_reason(), None);

        // Stale epoch replies are ignored.
        assert!(!state.apply_deleted(0, "a1"));
        assert_eq!(state.items.len(), 1);

        state.set_confirm_delete(Some("a1".into()));
        assert!(state.apply_deleted(1, "a1"));
        assert!(state.items.is_empty());
        assert!(state.confirm_delete.is_none());

        // Unsupported host hides the section.
        let gen3 = state.begin_refresh(2).unwrap();
        assert!(state.apply_list(2, gen3, Ok(None)));
        assert!(!state.supported);
    }
}
