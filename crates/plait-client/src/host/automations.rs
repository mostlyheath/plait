//! Additive host-owned automation operations. Blocking I/O belongs off the UI thread.
use super::{Client, Error, worktree};
use std::collections::HashSet;

pub use plait_protocol::host::automations::*;

fn invalid() -> Error {
    Error::Protocol("Invalid automation reply from host. Refresh before trying again.".into())
}

/// Hosts that predate automations or run without `PLAIT_AUTOMATIONS=1` reject
/// `list_automations` with `invalid_request`.
pub fn is_unsupported(error: &Error) -> bool {
    matches!(error, Error::Rejected(reason) if reason.code == "invalid_request")
}

fn valid_summary(summary: &Summary) -> bool {
    worktree::bounded_id(&summary.id)
        && !summary.name.trim().is_empty()
        && worktree::path(&summary.target_path)
        && summary
            .thread_id
            .as_deref()
            .is_none_or(worktree::bounded_id)
}

fn valid_automation(automation: &Automation) -> bool {
    worktree::bounded_id(&automation.id)
        && !automation.definition.name.trim().is_empty()
        && worktree::path(automation.definition.target.path())
        && automation
            .thread_id
            .as_deref()
            .is_none_or(worktree::bounded_id)
        && automation
            .admission_request_id
            .as_deref()
            .is_none_or(worktree::bounded_id)
}

fn valid_created(created: &CreatedAutomation) -> bool {
    worktree::bounded_id(&created.id)
        && created
            .admission_request_id
            .as_deref()
            .is_none_or(worktree::bounded_id)
        && created
            .thread_id
            .as_deref()
            .is_none_or(worktree::bounded_id)
        && created.target_path.as_deref().is_none_or(worktree::path)
}

/// True when the host gave a definitive refusal to `create_automation`, so a
/// saved creation request ID can be cleared. Transport, protocol, unknown
/// outcomes, and `internal` errors without a definitive outcome retain the
/// request ID so retries never create duplicates.
pub fn is_definitive_create_error(error: &Error) -> bool {
    let Error::Rejected(reason) = error else {
        return false;
    };
    match reason.outcome.as_deref() {
        Some("not_sent" | "rejected") => true,
        Some(_) => false,
        None => reason.code != "internal",
    }
}

/// Host automation ID derived from a creation `request_id`
/// (`runtime/src/automations/service.ts`: `sha256(request_id)` hex).
pub fn automation_id_for_request(request_id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(request_id.as_bytes()))
}

/// Outcome of asking the host once whether a prior creation `request_id`
/// already created an automation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExistingCheck {
    Found {
        summary: Box<Summary>,
        detail: Option<Box<DetailReply>>,
    },
    NotFound,
    Unknown,
}

/// Asks the host once (using the existing `get_automation` path, or falling
/// back to `list_automations` if `get_automation` is unsupported) whether
/// `request_id` already created an automation.
pub fn check_existing(client: &Client, request_id: &str) -> ExistingCheck {
    let id = automation_id_for_request(request_id);
    match get(client, &id) {
        Ok(detail) => {
            let last_result = detail.occurrences.first().map(|latest| LastResult {
                due: latest.due,
                outcome: latest.outcome,
                detail: latest.detail.clone(),
            });
            let summary = Summary {
                id: detail.automation.id.clone(),
                name: detail.automation.definition.name.clone(),
                schedule: detail.automation.definition.schedule.clone(),
                target_path: detail.automation.definition.target.path().to_path_buf(),
                state: detail.automation.state,
                next_run: None,
                last_result,
                pause_reason: detail.automation.pause_reason.clone(),
                error: detail.automation.error.clone(),
                thread_id: detail.automation.thread_id.clone(),
            };
            ExistingCheck::Found {
                summary: Box::new(summary),
                detail: Some(Box::new(detail)),
            }
        }
        Err(Error::Rejected(reason)) if reason.code == "not_found" => ExistingCheck::NotFound,
        Err(Error::Rejected(reason)) if reason.code == "invalid_request" => match list(client) {
            Ok(Some(summaries)) => match summaries.into_iter().find(|s| s.id == id) {
                Some(summary) => ExistingCheck::Found {
                    summary: Box::new(summary),
                    detail: None,
                },
                None => ExistingCheck::NotFound,
            },
            _ => ExistingCheck::Unknown,
        },
        Err(_) => ExistingCheck::Unknown,
    }
}

#[derive(serde::Deserialize)]
struct ListRecords {
    automations: Vec<serde_json::Value>,
}

impl ListRecords {
    fn summaries(self) -> Vec<Summary> {
        let mut seen = HashSet::new();
        let mut duplicates = HashSet::new();
        for record in &self.automations {
            if let Some(id) = record["id"].as_str()
                && !seen.insert(id)
            {
                duplicates.insert(id.to_owned());
            }
        }
        self.automations
            .into_iter()
            .enumerate()
            .map(|(index, record)| {
                let name = record["name"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("Unsupported automation")
                    .to_owned();
                if let Ok(summary) = serde_json::from_value::<Summary>(record)
                    && valid_summary(&summary)
                    && !duplicates.contains(&summary.id)
                {
                    return summary;
                }
                Summary {
                    // Card identity only. Host IDs cannot contain a colon.
                    id: format!("unsupported:{index}"),
                    name,
                    schedule: Schedule::Unsupported,
                    state: State::Unsupported,
                    target_path: Default::default(),
                    next_run: None,
                    last_result: None,
                    pause_reason: None,
                    error: None,
                    thread_id: None,
                }
            })
            .collect()
    }
}

/// Probes and lists automations on the host. Returns `Ok(None)` when the host
/// does not expose automation operations so older hosts hide the section.
pub fn list(client: &Client) -> Result<Option<Vec<Summary>>, Error> {
    match client.send::<_, ListRecords>(&Request::ListAutomations) {
        Ok(reply) => Ok(Some(reply.summaries())),
        Err(error) if is_unsupported(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Fetches the full automation record and occurrence receipts for `id`.
pub fn get(client: &Client, id: &str) -> Result<DetailReply, Error> {
    if !worktree::bounded_id(id) {
        return Err(Error::Protocol("Invalid automation ID.".into()));
    }
    let reply: DetailReply = client.send(&Request::GetAutomation { id: id.into() })?;
    if reply.automation.id != id || !valid_automation(&reply.automation) {
        return Err(invalid());
    }
    if reply
        .occurrences
        .iter()
        .any(|occ| !worktree::bounded_id(&occ.request_id))
    {
        return Err(invalid());
    }
    Ok(reply)
}

/// Pauses scheduled runs for `id` and returns the updated summary.
pub fn pause(client: &Client, id: &str) -> Result<Summary, Error> {
    if !worktree::bounded_id(id) {
        return Err(Error::Protocol("Invalid automation ID.".into()));
    }
    let reply: SummaryReply = client.send(&Request::PauseAutomation { id: id.into() })?;
    if reply.automation.id != id || !valid_summary(&reply.automation) {
        return Err(invalid());
    }
    Ok(reply.automation)
}

/// Resumes scheduled runs for `id` and returns the updated summary.
pub fn resume(client: &Client, id: &str) -> Result<Summary, Error> {
    if !worktree::bounded_id(id) {
        return Err(Error::Protocol("Invalid automation ID.".into()));
    }
    let reply: SummaryReply = client.send(&Request::ResumeAutomation { id: id.into() })?;
    if reply.automation.id != id || !valid_summary(&reply.automation) {
        return Err(invalid());
    }
    Ok(reply.automation)
}

/// Requests an immediate run of `id` using idempotent `request_id`.
pub fn run_now(client: &Client, id: &str, request_id: &str) -> Result<Occurrence, Error> {
    if !worktree::bounded_id(id) || !worktree::bounded_id(request_id) {
        return Err(Error::Protocol("Invalid automation or request ID.".into()));
    }
    let reply: RunNowReply = client.send(&Request::RunAutomationNow {
        id: id.into(),
        request_id: request_id.into(),
    })?;
    if !worktree::bounded_id(&reply.occurrence.request_id) {
        return Err(invalid());
    }
    Ok(reply.occurrence)
}

/// Creates an automation using idempotent `request_id`.
/// Supply `caller_thread_id` when creating from a thread so the host can
/// refuse creation from an automation's own thread.
pub fn create(
    client: &Client,
    definition: &Definition,
    request_id: &str,
    caller_thread_id: Option<&str>,
) -> Result<CreatedAutomation, Error> {
    if !worktree::bounded_id(request_id)
        || caller_thread_id.is_some_and(|id| !worktree::bounded_id(id))
    {
        return Err(Error::Protocol("Invalid creation or thread ID.".into()));
    }
    let reply: CreateReply = client.send(&Request::CreateAutomation {
        definition: Box::new(definition.clone()),
        request_id: request_id.into(),
        caller: caller_thread_id.map(|thread_id| Caller {
            thread_id: thread_id.into(),
        }),
    })?;
    if !valid_created(&reply.automation) {
        return Err(invalid());
    }
    Ok(reply.automation)
}

/// Deletes automation `id`. Its thread and conversation history remain on the host.
pub fn delete(client: &Client, id: &str) -> Result<bool, Error> {
    if !worktree::bounded_id(id) {
        return Err(Error::Protocol("Invalid automation ID.".into()));
    }
    let reply: DeleteReply = client.send(&Request::DeleteAutomation { id: id.into() })?;
    if !reply.deleted {
        return Err(invalid());
    }
    Ok(true)
}

#[cfg(all(test, feature = "local-host"))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        path::PathBuf,
    };

    fn scripted(
        name: &str,
        replies: Vec<serde_json::Value>,
    ) -> (
        Client,
        std::thread::JoinHandle<Vec<serde_json::Value>>,
        PathBuf,
    ) {
        let directory = plait_test_support::socket_temp_dir()
            .join(format!("plait-auto-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("host.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = std::thread::spawn(move || {
            replies
                .into_iter()
                .map(|reply| {
                    let (mut socket, _) = listener.accept().unwrap();
                    let mut line = String::new();
                    BufReader::new(socket.try_clone().unwrap())
                        .read_line(&mut line)
                        .unwrap();
                    writeln!(socket, "{reply}").unwrap();
                    serde_json::from_str(&line).unwrap()
                })
                .collect()
        });
        (Client::at_socket(path), server, directory)
    }

    #[test]
    fn older_host_returns_none_and_hides_automations() {
        let (client, server, directory) = scripted(
            "unsupported",
            vec![json!({
                "ok": false,
                "error": {"code": "invalid_request", "message": "Unknown operation."}
            })],
        );
        assert_eq!(list(&client).unwrap(), None);
        assert_eq!(
            server.join().unwrap(),
            vec![json!({"op": "list_automations"})]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn client_operations_send_exact_requests_and_validate_replies() {
        let summary_active = json!({
            "id": "auto_1",
            "name": "Nightly checks",
            "schedule": {"kind": "weekdays", "time": "09:00", "timeZone": "UTC"},
            "target_path": "/repo",
            "state": "active",
            "next_run": 1735725600000u64,
            "last_result": {"due": 1735722000000u64, "outcome": "queued", "detail": null}
        });
        let summary_paused = json!({
            "id": "auto_1",
            "name": "Nightly checks",
            "schedule": {"kind": "weekdays", "time": "09:00", "timeZone": "UTC"},
            "target_path": "/repo",
            "state": "paused",
            "next_run": null,
            "last_result": {"due": 1735722000000u64, "outcome": "queued", "detail": null}
        });
        let (client, server, directory) = scripted(
            "round-trip",
            vec![
                json!({"ok": true, "result": {"automations": [summary_active]}}),
                json!({
                    "ok": true,
                    "result": {
                        "automation": {
                            "id": "auto_1",
                            "thread_id": "thr_1",
                            "definition": {
                                "name": "Nightly checks",
                                "instructions": "Run tests",
                                "target": {"kind": "project", "checkout": "/repo"},
                                "harness": "pi",
                                "provider": "faux",
                                "model_id": "faux-1",
                                "approval": "auto",
                                "command_policy": "sandboxed",
                                "schedule": {"kind": "weekdays", "time": "09:00", "timeZone": "UTC"},
                                "catch_up_ms": 43200000
                            },
                            "revision": 1,
                            "state": "active",
                            "pause_reason": null,
                            "cursor": 1735722000000u64,
                            "missed_count": 0
                        },
                        "occurrences": []
                    }
                }),
                json!({"ok": true, "result": {"automation": summary_paused}}),
                json!({"ok": true, "result": {"automation": summary_active}}),
                json!({
                    "ok": true,
                    "result": {
                        "occurrence": {
                            "request_id": "manual-1",
                            "due": 1735722000000u64,
                            "outcome": "pending"
                        }
                    }
                }),
                json!({"ok": true, "result": {"deleted": true}}),
            ],
        );

        let listed = list(&client).unwrap().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "auto_1");

        let detail = get(&client, "auto_1").unwrap();
        assert_eq!(detail.automation.thread_id.as_deref(), Some("thr_1"));

        let paused = pause(&client, "auto_1").unwrap();
        assert_eq!(paused.state, State::Paused);

        let resumed = resume(&client, "auto_1").unwrap();
        assert_eq!(resumed.state, State::Active);

        let occurrence = run_now(&client, "auto_1", "click-1").unwrap();
        assert_eq!(occurrence.outcome, OccurrenceOutcome::Pending);

        assert!(delete(&client, "auto_1").unwrap());

        assert_eq!(
            server.join().unwrap(),
            vec![
                json!({"op": "list_automations"}),
                json!({"op": "get_automation", "id": "auto_1"}),
                json!({"op": "pause_automation", "id": "auto_1"}),
                json!({"op": "resume_automation", "id": "auto_1"}),
                json!({"op": "run_automation_now", "id": "auto_1", "request_id": "click-1"}),
                json!({"op": "delete_automation", "id": "auto_1"}),
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn list_isolates_unusable_records_and_preserves_host_paths() {
        let records: ListRecords = serde_json::from_value(json!({"automations": [null, {
            "id": "auto_1",
            "name": "Bad",
            "schedule": {"kind": "hourly", "minute": 0, "timeZone": "UTC"},
            "target_path": "relative/path",
            "state": "active",
            "next_run": null,
            "last_result": null
        }, {
            "id": "good", "name": "Good", "state": "active",
            "schedule": {"kind":"hourly","minute":0,"timeZone":"UTC"},
            "target_path": "/repo\u{0085}/"
        }]}))
        .unwrap();
        let items: Vec<_> = records
            .summaries()
            .into_iter()
            .map(crate::automations::AutomationItem::from_summary)
            .collect();
        assert_eq!(items.len(), 3);
        assert!(!items[0].supported());
        assert!(!items[1].supported());
        assert_eq!(items[1].summary.name, "Bad");
        assert!(items[2].supported());
        assert_eq!(
            items[2].summary.target_path.to_str(),
            Some("/repo\u{0085}/")
        );
    }

    #[test]
    fn pause_checks_identity() {
        let (client, server, directory) = scripted(
            "invalid",
            vec![json!({
                "ok": true,
                "result": {
                    "automation": {
                        "id": "other_id",
                        "name": "Wrong",
                        "schedule": {"kind": "hourly", "minute": 0, "timeZone": "UTC"},
                        "target_path": "/repo",
                        "state": "paused",
                        "next_run": null,
                        "last_result": null
                    }
                }
            })],
        );
        assert!(matches!(pause(&client, "auto_1"), Err(Error::Protocol(_))));
        server.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn create_sends_definition_and_validates_reply_and_error_classification() {
        let definition = Definition {
            name: "Nightly checks".into(),
            instructions: "Run tests".into(),
            target: Target::Worktree {
                checkout: "/repo".into(),
            },
            harness: super::super::harness::HarnessId::Pi,
            provider: "faux".into(),
            model_id: "faux-1".into(),
            mode: None,
            thinking: None,
            approval: super::super::ApprovalMode::Auto,
            command_policy: CommandPolicy::Sandboxed,
            schedule: Schedule::Daily {
                time: "09:00".into(),
                time_zone: "UTC".into(),
            },
            catch_up_ms: None,
        };
        let (client, server, directory) = scripted(
            "create",
            vec![json!({
                "ok": true,
                "result": {
                    "automation": {
                        "id": "auto_new",
                        "state": "preparing",
                        "admission_request_id": "automation-auto_new",
                        "cursor": 1735722000000u64,
                        "error": null
                    }
                }
            })],
        );
        let created = create(&client, &definition, "create-req-1", Some("thr_7")).unwrap();
        assert_eq!(created.id, "auto_new");
        assert_eq!(created.state, State::Preparing);
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "create_automation");
        assert_eq!(requests[0]["request_id"], "create-req-1");
        assert_eq!(requests[0]["caller"], json!({"thread_id": "thr_7"}));
        assert_eq!(requests[0]["definition"]["target"]["kind"], "worktree");
        std::fs::remove_dir_all(directory).unwrap();

        let rejected = Error::Rejected(super::super::HostError {
            retry_after_seconds: None,
            code: "permission_denied".into(),
            message: "An automation thread cannot create automations.".into(),
            outcome: None,
        });
        assert!(is_definitive_create_error(&rejected));

        // Internal error without a definitive outcome is treated as unconfirmed.
        let internal_no_outcome = Error::Rejected(super::super::HostError {
            retry_after_seconds: None,
            code: "internal".into(),
            message: "Database busy.".into(),
            outcome: None,
        });
        assert!(!is_definitive_create_error(&internal_no_outcome));

        // Internal error with an explicit definitive outcome is definitive.
        for definitive_outcome in ["not_sent", "rejected"] {
            let err = Error::Rejected(super::super::HostError {
                retry_after_seconds: None,
                code: "internal".into(),
                message: "Refused".into(),
                outcome: Some(definitive_outcome.into()),
            });
            assert!(is_definitive_create_error(&err));
        }

        let unknown = Error::Rejected(super::super::HostError {
            retry_after_seconds: None,
            code: "internal".into(),
            message: "Unknown".into(),
            outcome: Some("unknown".into()),
        });
        assert!(!is_definitive_create_error(&unknown));
        assert!(!is_definitive_create_error(&Error::Transport(
            "lost".into()
        )));
        assert!(!is_definitive_create_error(&Error::Protocol("bad".into())));
    }

    #[test]
    fn check_existing_queries_host_with_derived_id_and_handles_all_outcomes() {
        let request_id = "desktop-req-42";
        let expected_id = automation_id_for_request(request_id);
        assert_eq!(expected_id.len(), 64);

        let (client, server, directory) = scripted(
            "check-existing",
            vec![
                // 1. Not found -> NotFound.
                json!({
                    "ok": false,
                    "error": {"code": "not_found", "message": "Automation not found."}
                }),
                // 2. Found via get_automation -> Found.
                json!({
                    "ok": true,
                    "result": {
                        "automation": {
                            "id": expected_id,
                            "state": "preparing",
                            "admission_request_id": format!("automation-{expected_id}"),
                            "cursor": 1735722000000u64,
                            "error": null,
                            "definition": {
                                "name": "Nightly checks",
                                "instructions": "Run tests",
                                "target": {"kind": "project", "checkout": "/repo"},
                                "harness": "pi",
                                "provider": "faux",
                                "model_id": "faux-1",
                                "approval": "auto",
                                "command_policy": "sandboxed",
                                "schedule": {"kind": "daily", "time": "09:00", "timeZone": "UTC"},
                                "catch_up_ms": 43200000
                            }
                        },
                        "occurrences": []
                    }
                }),
                // 3. Internal error -> Unknown.
                json!({
                    "ok": false,
                    "error": {"code": "internal", "message": "Sidecar restarting."}
                }),
                // 4. get_automation unsupported -> falls back to list_automations.
                json!({
                    "ok": false,
                    "error": {"code": "invalid_request", "message": "Unknown operation."}
                }),
                json!({
                    "ok": true,
                    "result": {
                        "automations": [{
                            "id": expected_id,
                            "name": "Nightly checks",
                            "schedule": {"kind": "daily", "time": "09:00", "timeZone": "UTC"},
                            "target_path": "/repo",
                            "state": "active",
                            "next_run": null,
                            "last_result": null
                        }]
                    }
                }),
            ],
        );

        assert_eq!(check_existing(&client, request_id), ExistingCheck::NotFound);
        match check_existing(&client, request_id) {
            ExistingCheck::Found { summary, detail } => {
                assert_eq!(summary.id, expected_id);
                assert_eq!(summary.name, "Nightly checks");
                assert!(detail.is_some());
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert_eq!(check_existing(&client, request_id), ExistingCheck::Unknown);
        match check_existing(&client, request_id) {
            ExistingCheck::Found { summary, detail } => {
                assert_eq!(summary.id, expected_id);
                assert!(detail.is_none());
            }
            other => panic!("expected Found via list fallback, got {other:?}"),
        }

        let requests = server.join().unwrap();
        assert_eq!(
            requests,
            vec![
                json!({"op": "get_automation", "id": expected_id}),
                json!({"op": "get_automation", "id": expected_id}),
                json!({"op": "get_automation", "id": expected_id}),
                json!({"op": "get_automation", "id": expected_id}),
                json!({"op": "list_automations"}),
            ]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
