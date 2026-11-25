//! The Issues tab's state, free of GPUI: one project's Linear team, its issue
//! list, and the selected issue's details.
//!
//! The host makes every Linear request. The list and the details each carry
//! their own generation, so a reply that lands after the filter, the search,
//! the selection, the project, or the connection changed is dropped instead
//! of replacing newer content. Text from Linear is shown, never followed:
//! nothing here opens a link from an issue.

use crate::host::linear::{
    Binding, Cache, Filter, IssueDetail, IssueReply, IssueSummary, Issues, ProjectBinding,
    Rejection, StateUpdate, Teams, WorkflowState, WriteFailure,
};
use crate::linear_settings::{Pending, message};
use std::path::{Path, PathBuf};

/// The filters offered, in the order they are drawn.
pub const FILTERS: [(Filter, &str); 5] = [
    (Filter::Active, "Active"),
    (Filter::Assigned, "Assigned to me"),
    (Filter::Backlog, "Backlog"),
    (Filter::Completed, "Completed"),
    (Filter::All, "All"),
];

/// Issues kept in memory across pages, so a long list stays bounded.
pub const LIST_LIMIT: usize = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Load {
    #[default]
    Idle,
    Loading,
    Failed,
}

/// The parameters of one list request, computed by the model so the UI never
/// assembles them from stale fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRequest {
    pub generation: u64,
    pub team_id: String,
    pub filter: Filter,
    pub search: Option<String>,
    pub cursor: Option<String>,
    pub refresh: bool,
}

#[derive(Debug)]
pub struct IssuesView {
    pub checkout: Option<PathBuf>,
    /// `None` until the binding is read; `Some(None)` when no team is linked.
    pub binding: Option<Option<Binding>>,
    pub binding_load: Load,
    pub filter: Filter,
    /// The search the shown list was asked for.
    pub search: String,
    pub issues: Vec<IssueSummary>,
    pub cache: Option<Cache>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub list: Load,
    pub list_note: Option<String>,
    pub selected: Option<String>,
    pub detail: Option<IssueReply<IssueDetail>>,
    pub detail_load: Load,
    pub detail_note: Option<String>,
    /// The linked team's workflow states, in Linear's order, for the status menu.
    pub states: Option<Vec<WorkflowState>>,
    pub states_load: Load,
    /// The status change in flight or the last one that did not succeed.
    pub state_change: Option<StateChange>,
    /// Whether the host records Linear writes durably; older hosts offer no writes.
    pub writes: Option<bool>,
    epoch: Option<u64>,
    binding_generation: u64,
    list_generation: u64,
    detail_generation: u64,
    states_generation: u64,
}

/// One person-chosen move of the selected issue to another workflow state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateChange {
    pub issue_id: String,
    pub state_id: String,
    pub state_name: String,
    /// Reused for a retry, so the host returns a saved outcome instead of
    /// sending the move twice.
    pub request_id: String,
    pub status: ChangeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeStatus {
    Sending,
    /// Not applied. `retry` is whether trying the same request again is safe.
    Failed {
        note: String,
        retry: bool,
    },
    /// Linear may have applied it; the host will not send it again.
    Unknown,
}

impl Default for IssuesView {
    fn default() -> Self {
        Self {
            checkout: None,
            binding: None,
            binding_load: Load::Idle,
            filter: Filter::Active,
            search: String::new(),
            issues: Vec::new(),
            cache: None,
            next_cursor: None,
            has_more: false,
            list: Load::Idle,
            list_note: None,
            selected: None,
            detail: None,
            detail_load: Load::Idle,
            detail_note: None,
            states: None,
            states_load: Load::Idle,
            state_change: None,
            writes: None,
            epoch: None,
            binding_generation: 0,
            list_generation: 0,
            detail_generation: 0,
            states_generation: 0,
        }
    }
}

fn next(generation: &mut u64) -> u64 {
    *generation = generation.wrapping_add(1);
    *generation
}

impl IssuesView {
    /// Points the view at `checkout` on connection `epoch`, starting over when
    /// either changed. The selection survives a reconnect to the same project,
    /// so a restored tab keeps its issue. Returns whether it started over.
    pub fn target(&mut self, epoch: u64, checkout: &Path, selected: Option<String>) -> bool {
        if self.epoch == Some(epoch) && self.checkout.as_deref() == Some(checkout) {
            return false;
        }
        let keep = if self.checkout.as_deref() == Some(checkout) {
            self.selected.take().or(selected)
        } else {
            selected
        };
        let (filter, search) = (self.filter, std::mem::take(&mut self.search));
        let generations = (
            self.binding_generation,
            self.list_generation,
            self.detail_generation,
            self.states_generation,
        );
        *self = Self {
            checkout: Some(checkout.to_owned()),
            epoch: Some(epoch),
            selected: keep,
            filter,
            search,
            binding_generation: generations.0.wrapping_add(1),
            list_generation: generations.1.wrapping_add(1),
            detail_generation: generations.2.wrapping_add(1),
            states_generation: generations.3.wrapping_add(1),
            ..Self::default()
        };
        true
    }

    pub fn team(&self) -> Option<&Binding> {
        self.binding.as_ref().and_then(Option::as_ref)
    }

    pub fn needs_binding(&self) -> bool {
        self.binding.is_none() && self.binding_load == Load::Idle
    }

    pub fn start_binding(&mut self) -> u64 {
        self.binding_load = Load::Loading;
        next(&mut self.binding_generation)
    }

    pub fn binding_reply(
        &mut self,
        generation: u64,
        result: Result<ProjectBinding, Rejection>,
    ) -> bool {
        if generation != self.binding_generation || self.binding_load != Load::Loading {
            return false;
        }
        match result {
            Ok(project) => {
                self.binding_load = Load::Idle;
                let changed =
                    self.team().map(|b| &b.team_id) != project.binding.as_ref().map(|b| &b.team_id);
                self.binding = Some(project.binding);
                if changed {
                    self.clear_list();
                }
            }
            Err(error) => {
                self.binding_load = Load::Failed;
                self.list_note = Some(message(Pending::Loading, error));
            }
        }
        true
    }

    fn clear_list(&mut self) {
        next(&mut self.list_generation);
        self.issues.clear();
        self.cache = None;
        self.next_cursor = None;
        self.has_more = false;
        self.list = Load::Idle;
        self.list_note = None;
    }

    /// Whether the list should be read: a team is linked and nothing is shown.
    pub fn needs_list(&self) -> bool {
        self.team().is_some()
            && self.issues.is_empty()
            && self.cache.is_none()
            && self.list == Load::Idle
    }

    fn request(&mut self, cursor: Option<String>, refresh: bool) -> Option<ListRequest> {
        let team_id = self.team()?.team_id.clone();
        self.list = Load::Loading;
        self.list_note = None;
        let search = self.search.trim();
        Some(ListRequest {
            generation: next(&mut self.list_generation),
            team_id,
            filter: self.filter,
            search: (!search.is_empty()).then(|| search.chars().take(200).collect()),
            cursor,
            refresh,
        })
    }

    /// The first page, replacing what is shown once it arrives.
    pub fn start_list(&mut self, refresh: bool) -> Option<ListRequest> {
        self.request(None, refresh)
    }

    /// The next page, appended to what is shown.
    pub fn start_more(&mut self) -> Option<ListRequest> {
        if self.list == Load::Loading || !self.has_more || self.issues.len() >= LIST_LIMIT {
            return None;
        }
        let cursor = self.next_cursor.clone()?;
        self.request(Some(cursor), false)
    }

    pub fn list_reply(&mut self, request: &ListRequest, result: Result<Issues, Rejection>) -> bool {
        if request.generation != self.list_generation || self.list != Load::Loading {
            return false;
        }
        match result {
            Ok(page) => {
                self.list = Load::Idle;
                if request.cursor.is_none() {
                    self.issues.clear();
                }
                let room = LIST_LIMIT.saturating_sub(self.issues.len());
                let clipped = page.issues.len() > room;
                self.issues.extend(page.issues.into_iter().take(room));
                self.has_more = page.has_more && !clipped && self.issues.len() < LIST_LIMIT;
                self.next_cursor = page.next_cursor.filter(|_| self.has_more);
                self.cache = Some(page.cache);
            }
            Err(error) => {
                self.list = Load::Failed;
                self.list_note = Some(message(Pending::Loading, error));
            }
        }
        true
    }

    /// Changes the filter; the list starts over. Returns whether it changed.
    pub fn set_filter(&mut self, filter: Filter) -> bool {
        if self.filter == filter {
            return false;
        }
        self.filter = filter;
        self.clear_list();
        true
    }

    /// Changes the search; the list starts over. Returns whether it changed.
    pub fn set_search(&mut self, search: &str) -> bool {
        let search = search.trim();
        if self.search == search {
            return false;
        }
        self.search = search.to_owned();
        self.clear_list();
        true
    }

    /// The listed issue with `id`, if it is shown.
    pub fn summary(&self, id: &str) -> Option<&IssueSummary> {
        self.issues.iter().find(|issue| issue.id == id)
    }

    /// Selects an issue and returns the generation its details load under.
    pub fn select(&mut self, id: &str) -> Option<u64> {
        if self.selected.as_deref() == Some(id) && self.detail_load != Load::Failed {
            return None;
        }
        self.selected = Some(id.to_owned());
        self.detail = None;
        self.detail_note = None;
        self.detail_load = Load::Loading;
        Some(next(&mut self.detail_generation))
    }

    /// Reloads the selected issue's details, for Refresh or after a restore.
    pub fn reload_detail(&mut self) -> Option<(String, u64)> {
        let id = self.selected.clone()?;
        if self.detail_load == Load::Loading {
            return None;
        }
        self.detail_load = Load::Loading;
        self.detail_note = None;
        Some((id, next(&mut self.detail_generation)))
    }

    pub fn needs_detail(&self) -> bool {
        self.selected.is_some() && self.detail.is_none() && self.detail_load == Load::Idle
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.detail = None;
        self.detail_note = None;
        self.detail_load = Load::Idle;
        next(&mut self.detail_generation);
    }

    pub fn detail_reply(
        &mut self,
        generation: u64,
        id: &str,
        result: Result<IssueReply<IssueDetail>, Rejection>,
    ) -> bool {
        if generation != self.detail_generation
            || self.detail_load != Load::Loading
            || self.selected.as_deref() != Some(id)
        {
            return false;
        }
        match result {
            Ok(reply) => {
                self.detail_load = Load::Idle;
                self.detail = Some(reply);
            }
            Err(error) => {
                self.detail_load = Load::Failed;
                self.detail_note = Some(message(Pending::Loading, error));
            }
        }
        true
    }

    /// Whether the linked team's workflow states should be read.
    pub fn needs_states(&self) -> bool {
        self.writes == Some(true)
            && self.team().is_some()
            && self.states.is_none()
            && self.states_load == Load::Idle
    }

    pub fn start_states(&mut self) -> u64 {
        self.states_load = Load::Loading;
        next(&mut self.states_generation)
    }

    /// Keeps the linked team's states, ordered as Linear orders them.
    pub fn states_reply(&mut self, generation: u64, result: Result<Teams, Rejection>) -> bool {
        if generation != self.states_generation || self.states_load != Load::Loading {
            return false;
        }
        let team = self.team().map(|b| b.team_id.clone());
        match result.map(|teams| {
            teams
                .teams
                .into_iter()
                .find(|t| Some(&t.id) == team.as_ref())
        }) {
            Ok(Some(team)) => {
                let mut states = team.states;
                states.sort_by(|a, b| a.position.total_cmp(&b.position));
                self.states = Some(states);
                self.states_load = Load::Idle;
            }
            // The team is gone from the list, or the read failed: no menu.
            Ok(None) | Err(_) => self.states_load = Load::Failed,
        }
        true
    }

    /// Starts moving the selected issue to `state_id`, unless a change is in
    /// flight, the state is the current one, or it is not one of the team's.
    pub fn start_state_change(
        &mut self,
        state_id: &str,
        request_id: String,
    ) -> Option<StateChange> {
        if self.writes != Some(true)
            || matches!(&self.state_change, Some(c) if c.status == ChangeStatus::Sending)
        {
            return None;
        }
        let issue = &self.detail.as_ref()?.issue.summary;
        if issue.state.id == state_id {
            return None;
        }
        let state = self.states.as_ref()?.iter().find(|s| s.id == state_id)?;
        let change = StateChange {
            issue_id: issue.id.clone(),
            state_id: state.id.clone(),
            state_name: state.name.clone(),
            request_id,
            status: ChangeStatus::Sending,
        };
        self.state_change = Some(change.clone());
        Some(change)
    }

    /// The same request again, after a refusal or a lost reply.
    pub fn retry_state_change(&mut self) -> Option<StateChange> {
        let change = self.state_change.as_mut()?;
        if !matches!(change.status, ChangeStatus::Failed { retry: true, .. }) {
            return None;
        }
        change.status = ChangeStatus::Sending;
        Some(change.clone())
    }

    pub fn dismiss_state_change(&mut self) {
        if !matches!(&self.state_change, Some(c) if c.status == ChangeStatus::Sending) {
            self.state_change = None;
        }
    }

    /// Applies the host's answer. Returns whether the details should be read
    /// again, because Linear accepted the move but the refreshed issue is missing.
    pub fn state_reply(
        &mut self,
        change: &StateChange,
        result: Result<StateUpdate, WriteFailure>,
    ) -> bool {
        let Some(current) = self.state_change.as_mut() else {
            return false;
        };
        if current.request_id != change.request_id || current.status != ChangeStatus::Sending {
            return false;
        }
        match result {
            Ok(update) => {
                self.state_change = None;
                let shown = self
                    .detail
                    .as_mut()
                    .filter(|d| d.issue.summary.id == change.issue_id);
                match (update.issue, shown) {
                    (Some(issue), Some(shown)) => {
                        shown.issue = issue;
                        false
                    }
                    (None, Some(shown)) => {
                        // Accepted: show the new state until the details are read again.
                        shown.issue.summary.state.id = change.state_id.clone();
                        shown.issue.summary.state.name = change.state_name.clone();
                        true
                    }
                    (_, None) => false,
                }
            }
            Err(WriteFailure::Unknown) => {
                current.status = ChangeStatus::Unknown;
                false
            }
            Err(WriteFailure::Lost) => {
                current.status = ChangeStatus::Failed {
                    note: "The host's answer was lost. Trying again will not move it twice.".into(),
                    retry: true,
                };
                false
            }
            Err(WriteFailure::Refused(error)) => {
                current.status = ChangeStatus::Failed {
                    note: state_message(error),
                    retry: !matches!(
                        error,
                        Rejection::SignedOut | Rejection::KeyRejected | Rejection::NeedsNewerHost
                    ),
                };
                false
            }
        }
    }
}

/// Fixed text for a refused status change. Host messages are never shown here.
fn state_message(error: Rejection) -> String {
    match error {
        Rejection::SignedOut | Rejection::KeyRejected => {
            "Linear did not accept the key. Sign in again in Settings → Linear.".into()
        }
        Rejection::NotFound => "Linear no longer has this issue or state.".into(),
        Rejection::Conflict => {
            "This request was already used for another change. Choose the state again.".into()
        }
        error => format!(
            "The status was not changed. {}",
            message(Pending::Loading, error)
        ),
    }
}

/// The issue a thread starts from or quotes: its stable identity and the
/// words a person recognises. Saved with the thread only once the host has
/// created it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueRef {
    pub workspace_id: Option<String>,
    pub issue_id: String,
    pub identifier: String,
    pub url: String,
    pub title: String,
}

impl IssueRef {
    pub fn of(summary: &IssueSummary, workspace_id: Option<String>) -> Self {
        Self {
            workspace_id,
            issue_id: summary.id.clone(),
            identifier: summary.identifier.clone(),
            url: summary.url.clone(),
            title: summary.title.clone(),
        }
    }
}

/// Description text carried into a draft, at most; the rest stays in Linear.
pub const REFERENCE_DESCRIPTION_LIMIT: usize = 4000;

/// The block a draft receives: the issue's identifier, title, and address,
/// then its description when asked for, clipped on a character boundary.
/// It is ordinary draft text the person reads and edits before sending.
pub fn reference(issue: &IssueRef, description: Option<&str>) -> String {
    let mut text = format!(
        "Linear issue {}: {}\n{}",
        issue.identifier, issue.title, issue.url
    );
    if let Some(description) = description.map(str::trim).filter(|d| !d.is_empty()) {
        let clipped: String = description
            .chars()
            .take(REFERENCE_DESCRIPTION_LIMIT)
            .collect();
        text.push_str("\n\n");
        text.push_str(&clipped);
        if clipped.len() < description.len() {
            text.push_str("\n\n(Description shortened; the rest is in Linear.)");
        }
    }
    text
}

/// How fresh a reply is, in words, or `None` when it came straight from Linear.
pub fn freshness(cache: &Cache) -> Option<String> {
    if cache.stale {
        Some(format!(
            "Saved {} ago; Linear could not be reached.",
            age(cache.cache_age_seconds)
        ))
    } else if cache.partial {
        Some(
            cache
                .warning
                .clone()
                .unwrap_or_else(|| "Some fields could not be loaded.".into()),
        )
    } else {
        None
    }
}

/// A duration in its largest whole unit: "40 seconds", "3 minutes", "2 days".
pub fn age(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..60 => (seconds, "second"),
        60..3600 => (seconds / 60, "minute"),
        3600..86400 => (seconds / 3600, "hour"),
        _ => (seconds / 86400, "day"),
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// Linear's priority number in words; 0 is "No priority".
pub fn priority(value: f64) -> &'static str {
    match value as i64 {
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "No priority",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(id: &str) -> serde_json::Value {
        json!({"id": id, "identifier": format!("PLA-{}", id.len()), "title": format!("Issue {id}"),
               "url": "https://linear.app/acme/issue/PLA-1", "priority": 2, "updated_at": "2026-11-18T12:00:00.000Z",
               "team_id": "T", "state": {"id": "S", "name": "Todo", "type": "unstarted"},
               "assignee": null, "labels": []})
    }
    fn cache() -> serde_json::Value {
        json!({"cached": false, "stale": false, "cached_at": 0, "cache_age_seconds": 0, "partial": false, "warning": null})
    }
    fn page(ids: &[&str], more: bool) -> Issues {
        let mut v = json!({"issues": ids.iter().map(|id| summary(id)).collect::<Vec<_>>(),
                           "has_more": more, "next_cursor": more.then_some("C")});
        v.as_object_mut()
            .unwrap()
            .extend(cache().as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }
    fn bound(team: Option<&str>) -> ProjectBinding {
        serde_json::from_value(json!({"checkout": "/repo", "binding": team.map(|id| json!({
            "workspace_id": "ORG", "team_id": id, "team_key": "PLA", "team_name": "Plait", "updated_at": 1}))}))
        .unwrap()
    }
    fn detail(id: &str) -> IssueReply<IssueDetail> {
        let mut issue = summary(id);
        issue.as_object_mut().unwrap().extend(
            json!({"description": "Body", "attachments": [], "comments": [],
            "comments_has_more": false, "comments_next_cursor": null})
            .as_object()
            .unwrap()
            .clone(),
        );
        let mut v = json!({"issue": issue, "workspace_id": "ORG"});
        v.as_object_mut()
            .unwrap()
            .extend(cache().as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }
    fn linked() -> IssuesView {
        let mut view = IssuesView::default();
        view.target(1, Path::new("/repo"), None);
        let g = view.start_binding();
        view.binding_reply(g, Ok(bound(Some("T"))));
        view
    }

    #[test]
    fn an_unlinked_project_lists_nothing() {
        let mut view = IssuesView::default();
        view.target(1, Path::new("/repo"), None);
        assert!(view.needs_binding());
        let g = view.start_binding();
        view.binding_reply(g, Ok(bound(None)));
        assert!(matches!(view.binding, Some(None)));
        assert!(!view.needs_list());
        assert_eq!(view.start_list(false), None);
    }

    #[test]
    fn pages_append_and_a_new_filter_starts_over() {
        let mut view = linked();
        assert!(view.needs_list());
        let first = view.start_list(false).unwrap();
        assert_eq!(first.team_id, "T");
        assert!(view.list_reply(&first, Ok(page(&["a", "b"], true))));
        let more = view.start_more().unwrap();
        assert_eq!(more.cursor.as_deref(), Some("C"));
        view.list_reply(&more, Ok(page(&["c"], false)));
        assert_eq!(view.issues.len(), 3);
        assert_eq!(view.start_more(), None, "no more pages");
        assert!(view.set_filter(Filter::Completed));
        assert!(view.issues.is_empty() && view.needs_list());
        assert!(!view.set_filter(Filter::Completed));
    }

    #[test]
    fn a_reply_for_an_older_filter_or_search_is_dropped() {
        let mut view = linked();
        let old = view.start_list(false).unwrap();
        view.set_search("retry");
        let current = view.start_list(false).unwrap();
        assert_eq!(current.search.as_deref(), Some("retry"));
        assert!(!view.list_reply(&old, Ok(page(&["stale"], false))));
        assert!(view.list_reply(&current, Ok(page(&["fresh"], false))));
        assert_eq!(view.issues[0].id, "fresh");
    }

    #[test]
    fn the_list_is_bounded() {
        let mut view = linked();
        let ids: Vec<String> = (0..60).map(|i| format!("i{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut request = view.start_list(false).unwrap();
        loop {
            view.list_reply(&request, Ok(page(&refs, true)));
            match view.start_more() {
                Some(next) => request = next,
                None => break,
            }
        }
        assert_eq!(view.issues.len(), LIST_LIMIT);
        assert!(!view.has_more);
    }

    #[test]
    fn details_follow_the_selection_only() {
        let mut view = linked();
        let a = view.select("a").unwrap();
        let b = view.select("b").unwrap();
        assert!(
            !view.detail_reply(a, "a", Ok(detail("a"))),
            "an older selection"
        );
        assert!(view.detail_reply(b, "b", Ok(detail("b"))));
        assert_eq!(view.detail.as_ref().unwrap().issue.summary.id, "b");
        assert_eq!(view.select("b"), None, "already shown");
        view.clear_selection();
        assert!(view.selected.is_none() && view.detail.is_none());
    }

    #[test]
    fn a_reconnect_keeps_the_selection_but_another_project_does_not() {
        let mut view = linked();
        view.select("a");
        assert!(view.target(2, Path::new("/repo"), None));
        assert_eq!(view.selected.as_deref(), Some("a"));
        assert!(view.needs_binding());
        assert!(view.target(2, Path::new("/other"), Some("z".into())));
        assert_eq!(
            view.selected.as_deref(),
            Some("z"),
            "the restored tab's choice"
        );
    }

    #[test]
    fn a_failed_list_explains_without_host_text() {
        let mut view = linked();
        let request = view.start_list(false).unwrap();
        view.list_reply(&request, Err(Rejection::Offline));
        assert_eq!(view.list, Load::Failed);
        assert!(
            view.list_note
                .as_deref()
                .unwrap()
                .contains("could not be reached")
        );
        assert!(!view.needs_list(), "a failure waits for Refresh");
    }

    #[test]
    fn a_reference_names_the_issue_and_clips_its_description() {
        let summary: IssueSummary = serde_json::from_value(summary("a")).unwrap();
        let issue = IssueRef::of(&summary, Some("ORG".into()));
        assert_eq!(
            reference(&issue, None),
            "Linear issue PLA-1: Issue a\nhttps://linear.app/acme/issue/PLA-1"
        );
        let with = reference(&issue, Some("  Fix it.  "));
        assert!(with.ends_with("\n\nFix it."));
        let long = "é".repeat(REFERENCE_DESCRIPTION_LIMIT + 10);
        let clipped = reference(&issue, Some(&long));
        assert!(clipped.contains("Description shortened"));
        assert_eq!(clipped.matches('é').count(), REFERENCE_DESCRIPTION_LIMIT);
    }

    #[test]
    fn words_for_freshness_priority_and_age() {
        let mut stale: Cache = serde_json::from_value(cache()).unwrap();
        assert_eq!(freshness(&stale), None);
        stale.stale = true;
        stale.cache_age_seconds = 7200;
        assert_eq!(
            freshness(&stale).as_deref(),
            Some("Saved 2 hours ago; Linear could not be reached.")
        );
        assert_eq!(priority(1.0), "Urgent");
        assert_eq!(priority(0.0), "No priority");
        assert_eq!(age(1), "1 second");
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use serde_json::json;

    fn teams() -> Teams {
        serde_json::from_value(json!({"teams": [
            {"id": "OTHER", "key": "X", "name": "Other", "states": []},
            {"id": "T", "key": "PLA", "name": "Plait", "states": [
                {"id": "D", "name": "Done", "type": "completed", "position": 3},
                {"id": "S", "name": "Todo", "type": "unstarted", "position": 1}]}],
            "cached": false, "stale": false, "cached_at": 0, "cache_age_seconds": 0,
            "partial": false, "warning": null}))
        .unwrap()
    }

    fn detail() -> IssueReply<IssueDetail> {
        serde_json::from_value(
            json!({"issue": {"id": "I", "identifier": "PLA-1", "title": "T",
            "url": "https://linear.app/a/issue/PLA-1", "priority": 2, "updated_at": "1",
            "team_id": "T", "state": {"id": "S", "name": "Todo", "type": "unstarted"},
            "assignee": null, "labels": [], "description": null, "attachments": [],
            "comments": [], "comments_has_more": false, "comments_next_cursor": null},
            "workspace_id": "ORG", "cached": false, "stale": false, "cached_at": 0,
            "cache_age_seconds": 0, "partial": false, "warning": null}),
        )
        .unwrap()
    }

    /// A linked view showing issue "I" in state "S", with writes supported.
    fn shown() -> IssuesView {
        let mut view = IssuesView::default();
        view.target(1, Path::new("/repo"), None);
        let g = view.start_binding();
        view.binding_reply(
            g,
            Ok(
                serde_json::from_value(json!({"checkout": "/repo", "binding": {
                "workspace_id": "ORG", "team_id": "T", "team_key": "PLA",
                "team_name": "Plait", "updated_at": 1}}))
                .unwrap(),
            ),
        );
        view.writes = Some(true);
        assert!(view.needs_states());
        let g = view.start_states();
        assert!(view.states_reply(g, Ok(teams())));
        let g = view.select("I").unwrap();
        view.detail_reply(g, "I", Ok(detail()));
        view
    }

    fn accepted(issue: bool) -> StateUpdate {
        let mut d = detail().issue;
        d.summary.state.id = "D".into();
        d.summary.state.name = "Done".into();
        StateUpdate {
            issue: issue.then_some(d),
            partial: false,
            warning: None,
            outcome: "accepted".into(),
        }
    }

    #[test]
    fn states_keep_only_the_linked_team_in_linear_order() {
        let view = shown();
        let names: Vec<_> = view
            .states
            .as_ref()
            .unwrap()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["Todo", "Done"]);
    }

    #[test]
    fn a_status_change_needs_writes_a_new_state_and_one_at_a_time() {
        let mut view = shown();
        assert!(
            view.start_state_change("S", "r0".into()).is_none(),
            "already there"
        );
        assert!(
            view.start_state_change("NOPE", "r0".into()).is_none(),
            "not the team's"
        );
        let change = view.start_state_change("D", "r1".into()).unwrap();
        assert_eq!(change.state_name, "Done");
        assert!(
            view.start_state_change("D", "r2".into()).is_none(),
            "one in flight"
        );
        view.writes = Some(false);
        view.state_change = None;
        assert!(
            view.start_state_change("D", "r3".into()).is_none(),
            "older host"
        );
    }

    #[test]
    fn an_accepted_change_shows_the_new_state() {
        let mut view = shown();
        let change = view.start_state_change("D", "r1".into()).unwrap();
        assert!(!view.state_reply(&change, Ok(accepted(true))));
        assert!(view.state_change.is_none());
        assert_eq!(
            view.detail.as_ref().unwrap().issue.summary.state.name,
            "Done"
        );

        // Accepted without a refreshed issue: shown at once, then read again.
        let mut view = shown();
        let change = view.start_state_change("D", "r1".into()).unwrap();
        assert!(view.state_reply(&change, Ok(accepted(false))));
        assert_eq!(view.detail.as_ref().unwrap().issue.summary.state.id, "D");
    }

    #[test]
    fn a_lost_reply_retries_the_same_request_and_an_unknown_one_never_does() {
        let mut view = shown();
        let change = view.start_state_change("D", "r1".into()).unwrap();
        view.state_reply(&change, Err(WriteFailure::Lost));
        let retry = view.retry_state_change().unwrap();
        assert_eq!(retry.request_id, "r1", "the host returns its saved outcome");
        view.state_reply(&retry, Err(WriteFailure::Unknown));
        assert_eq!(
            view.state_change.as_ref().unwrap().status,
            ChangeStatus::Unknown
        );
        assert!(
            view.retry_state_change().is_none(),
            "an unknown move is never resent"
        );
        assert_eq!(view.detail.as_ref().unwrap().issue.summary.state.id, "S");
        view.dismiss_state_change();
        assert!(view.state_change.is_none());
    }

    #[test]
    fn a_refusal_explains_itself_and_a_stale_reply_is_dropped() {
        let mut view = shown();
        let change = view.start_state_change("D", "r1".into()).unwrap();
        let stale = StateChange {
            request_id: "old".into(),
            ..change.clone()
        };
        assert!(!view.state_reply(&stale, Ok(accepted(true))));
        assert_eq!(
            view.state_change.as_ref().unwrap().status,
            ChangeStatus::Sending
        );
        view.state_reply(&change, Err(WriteFailure::Refused(Rejection::KeyRejected)));
        match &view.state_change.as_ref().unwrap().status {
            ChangeStatus::Failed { note, retry } => {
                assert!(note.contains("Sign in again"), "{note}");
                assert!(!retry, "a rejected key needs Settings, not a retry");
            }
            other => panic!("{other:?}"),
        }
    }
}
