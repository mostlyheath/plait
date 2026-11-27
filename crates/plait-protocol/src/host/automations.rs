//! Wire types for host-owned automations (`runtime/src/automations/`).
//!
//! Request and reply shapes match `runtime/src/automations/service.ts`,
//! `schedule.ts`, and `schema.ts`. Operations are additive and carry no
//! `protocol_version` field; hosts without automations enabled reject
//! `list_automations` with `invalid_request`.

use super::{ApprovalMode, ThinkingLevel, harness::HarnessId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Additive automation operations handled by `runtime/src/automations/service.ts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    ListAutomations,
    GetAutomation { id: String },
    PauseAutomation { id: String },
    ResumeAutomation { id: String },
    RunAutomationNow { id: String, request_id: String },
    DeleteAutomation { id: String },
}

/// Schedule cadence and IANA time zone (`runtime/src/automations/schedule.ts`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Local wall-clock timestamp `YYYY-MM-DDTHH:mm`.
    Once {
        at: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Runs at local minute `0..=59` of every hour.
    Hourly {
        minute: u8,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Runs every day at `HH:mm`.
    Daily {
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Runs Monday through Friday at `HH:mm`.
    Weekdays {
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    /// Runs on selected weekdays (`0` = Sunday .. `6` = Saturday) at `HH:mm`.
    Weekly {
        days: Vec<u8>,
        time: String,
        #[serde(rename = "timeZone")]
        time_zone: String,
    },
    #[serde(other, skip_serializing)]
    Unsupported,
}

impl Schedule {
    pub fn time_zone(&self) -> &str {
        match self {
            Self::Unsupported => "",
            Self::Once { time_zone, .. }
            | Self::Hourly { time_zone, .. }
            | Self::Daily { time_zone, .. }
            | Self::Weekdays { time_zone, .. }
            | Self::Weekly { time_zone, .. } => time_zone,
        }
    }
}

/// Current lifecycle state of an automation on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Active,
    Paused,
    Preparing,
    Failed,
    #[serde(other, skip_serializing)]
    Unsupported,
}

/// Recorded outcome of an automation occurrence (`automation_occurrences` or `run_automation_now`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceOutcome {
    Pending,
    Queued,
    Skipped,
    Paused,
    Failed,
    Missed,
    #[serde(other, skip_serializing)]
    Other,
}

/// Most recent occurrence summary returned on `list_automations` cards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastResult {
    /// Scheduled or manual due timestamp in Unix milliseconds.
    pub due: u64,
    pub outcome: OccurrenceOutcome,
    #[serde(default)]
    pub detail: Option<String>,
}

/// Compact automation summary returned by `list_automations`, `pause_automation`,
/// and `resume_automation`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub schedule: Schedule,
    pub target_path: PathBuf,
    pub state: State,
    /// Next scheduled occurrence in Unix milliseconds when active.
    #[serde(default)]
    pub next_run: Option<u64>,
    #[serde(default)]
    pub last_result: Option<LastResult>,
    /// Optional fields a host may include alongside summary fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// Where an automation works (`service.ts` `Definition['target']`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Project { checkout: PathBuf },
    Worktree { checkout: PathBuf },
    Directory { path: PathBuf },
}

impl Target {
    pub fn path(&self) -> &Path {
        match self {
            Self::Project { checkout } | Self::Worktree { checkout } => checkout.as_path(),
            Self::Directory { path } => path.as_path(),
        }
    }
}

/// Command policy recorded in an automation definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPolicy {
    Sandboxed,
    Trusted,
}

/// Saved automation definition returned inside `get_automation`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub name: String,
    pub instructions: String,
    pub target: Target,
    pub harness: HarnessId,
    pub provider: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingLevel>,
    pub approval: ApprovalMode,
    pub command_policy: CommandPolicy,
    pub schedule: Schedule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up_ms: Option<u64>,
}

/// Full automation record returned by `get_automation` for both placed
/// automations (`active`/`paused`) and pending placements (`preparing`/`failed`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub definition: Definition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<String>,
    pub cursor: u64,
    #[serde(default)]
    pub missed_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One occurrence receipt from `get_automation` or `run_automation_now`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Occurrence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub due: u64,
    pub outcome: OccurrenceOutcome,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListReply {
    pub automations: Vec<Summary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailReply {
    pub automation: Automation,
    pub occurrences: Vec<Occurrence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryReply {
    pub automation: Summary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunNowReply {
    pub occurrence: Occurrence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteReply {
    pub deleted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_serialize_without_extra_fields() {
        assert_eq!(
            serde_json::to_value(Request::ListAutomations).unwrap(),
            json!({"op": "list_automations"})
        );
        assert_eq!(
            serde_json::to_value(Request::GetAutomation { id: "a1".into() }).unwrap(),
            json!({"op": "get_automation", "id": "a1"})
        );
        assert_eq!(
            serde_json::to_value(Request::PauseAutomation { id: "a1".into() }).unwrap(),
            json!({"op": "pause_automation", "id": "a1"})
        );
        assert_eq!(
            serde_json::to_value(Request::ResumeAutomation { id: "a1".into() }).unwrap(),
            json!({"op": "resume_automation", "id": "a1"})
        );
        assert_eq!(
            serde_json::to_value(Request::RunAutomationNow {
                id: "a1".into(),
                request_id: "click-1".into(),
            })
            .unwrap(),
            json!({"op": "run_automation_now", "id": "a1", "request_id": "click-1"})
        );
        assert_eq!(
            serde_json::to_value(Request::DeleteAutomation { id: "a1".into() }).unwrap(),
            json!({"op": "delete_automation", "id": "a1"})
        );
    }

    #[test]
    fn schedules_use_runtime_camel_case_timezone() {
        let cases = [
            json!({"kind": "once", "at": "2026-06-01T09:30", "timeZone": "UTC"}),
            json!({"kind": "hourly", "minute": 15, "timeZone": "America/New_York"}),
            json!({"kind": "daily", "time": "09:00", "timeZone": "Europe/London"}),
            json!({"kind": "weekdays", "time": "08:30", "timeZone": "America/Chicago"}),
            json!({"kind": "weekly", "days": [1, 3, 5], "time": "17:00", "timeZone": "Asia/Tokyo"}),
        ];
        for case in cases {
            let decoded: Schedule = serde_json::from_value(case.clone()).unwrap();
            assert_eq!(serde_json::to_value(&decoded).unwrap(), case);
        }
    }

    #[test]
    fn list_and_summary_replies_match_runtime_shapes() {
        let list: ListReply = serde_json::from_value(json!({
            "automations": [
                {
                    "id": "auto-1",
                    "name": "Review",
                    "schedule": {"kind": "hourly", "minute": 0, "timeZone": "UTC"},
                    "target_path": "/repo",
                    "state": "active",
                    "next_run": 1735725600000u64,
                    "last_result": {"due": 1735722000000u64, "outcome": "queued", "detail": null}
                },
                {
                    "id": "future",
                    "name": "Future",
                    "schedule": {"kind": "monthly", "day": 1},
                    "target_path": "/future",
                    "state": "blocked"
                },
                {
                    "id": "auto-2",
                    "name": "Preparing",
                    "schedule": {"kind": "weekdays", "time": "09:00", "timeZone": "UTC"},
                    "target_path": "/other",
                    "state": "preparing",
                    "next_run": null,
                    "last_result": null
                }
            ]
        }))
        .unwrap();
        assert_eq!(list.automations.len(), 3);
        assert_eq!(list.automations[0].state, State::Active);
        assert_eq!(
            list.automations[0].last_result.as_ref().unwrap().outcome,
            OccurrenceOutcome::Queued
        );
        assert_eq!(list.automations[1].state, State::Unsupported);
        assert_eq!(list.automations[1].schedule, Schedule::Unsupported);
        assert_eq!(list.automations[2].state, State::Preparing);
        assert!(list.automations[2].next_run.is_none());
    }

    #[test]
    fn detail_reply_decodes_placed_and_pending_automations() {
        let placed: DetailReply = serde_json::from_value(json!({
            "automation": {
                "id": "auto-1",
                "thread_id": "thr-1",
                "definition": {
                    "name": "Review",
                    "instructions": "Review recent changes",
                    "target": {"kind": "project", "checkout": "/repo"},
                    "harness": "pi",
                    "provider": "faux",
                    "model_id": "faux-1",
                    "approval": "auto",
                    "command_policy": "sandboxed",
                    "schedule": {"kind": "daily", "time": "09:00", "timeZone": "UTC"},
                    "catch_up_ms": 43200000
                },
                "revision": 2,
                "state": "paused",
                "pause_reason": "target missing",
                "cursor": 1735722000000u64,
                "missed_count": 1
            },
            "occurrences": [
                {
                    "automation_id": "auto-1",
                    "request_id": "auto-1-1735722000000",
                    "revision": 1,
                    "due": 1735722000000u64,
                    "outcome": "paused",
                    "detail": "target missing"
                }
            ]
        }))
        .unwrap();
        assert_eq!(placed.automation.thread_id.as_deref(), Some("thr-1"));
        assert_eq!(
            placed.automation.pause_reason.as_deref(),
            Some("target missing")
        );
        assert_eq!(placed.occurrences.len(), 1);
        assert_eq!(placed.occurrences[0].outcome, OccurrenceOutcome::Paused);

        let failed: DetailReply = serde_json::from_value(json!({
            "automation": {
                "id": "auto-2",
                "state": "failed",
                "admission_request_id": "automation-auto-2",
                "cursor": 1735722000000u64,
                "error": "This folder is inside a Git project. Choose the project instead.",
                "definition": {
                    "name": "Nested",
                    "instructions": "Check docs",
                    "target": {"kind": "directory", "path": "/repo/docs"},
                    "harness": "pi",
                    "provider": "faux",
                    "model_id": "faux-1",
                    "approval": "ask",
                    "command_policy": "trusted",
                    "schedule": {"kind": "hourly", "minute": 0, "timeZone": "UTC"},
                    "catch_up_ms": 1800000
                }
            },
            "occurrences": []
        }))
        .unwrap();
        assert_eq!(failed.automation.state, State::Failed);
        assert!(failed.automation.thread_id.is_none());
        assert!(
            failed
                .automation
                .error
                .as_deref()
                .unwrap()
                .contains("inside a Git project")
        );
    }

    #[test]
    fn run_now_and_delete_replies_decode_pending_and_final_receipts() {
        let pending: RunNowReply = serde_json::from_value(json!({
            "occurrence": {
                "request_id": "manual-abc",
                "due": 1735722000000u64,
                "outcome": "pending"
            }
        }))
        .unwrap();
        assert_eq!(pending.occurrence.outcome, OccurrenceOutcome::Pending);
        assert!(pending.occurrence.automation_id.is_none());

        let receipt: RunNowReply = serde_json::from_value(json!({
            "occurrence": {
                "automation_id": "auto-1",
                "request_id": "manual-abc",
                "revision": 1,
                "due": 1735722000000u64,
                "outcome": "skipped",
                "detail": "Checkout is busy."
            }
        }))
        .unwrap();
        assert_eq!(receipt.occurrence.outcome, OccurrenceOutcome::Skipped);
        assert_eq!(
            receipt.occurrence.detail.as_deref(),
            Some("Checkout is busy.")
        );

        let future: RunNowReply = serde_json::from_value(json!({
            "occurrence": {
                "request_id": "manual-def",
                "due": 1735722000000u64,
                "outcome": "future_state"
            }
        }))
        .unwrap();
        assert_eq!(future.occurrence.outcome, OccurrenceOutcome::Other);

        let deleted: DeleteReply = serde_json::from_value(json!({"deleted": true})).unwrap();
        assert!(deleted.deleted);
    }
}
