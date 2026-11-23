use super::*;
use crate::host::{Message, ThreadStatus, Tool, ToolStatus};
use serde_json::{Value, json};

fn record(id: &str, requests: &[&str], state: &str) -> Value {
    json!({"id":id,"request_ids":requests,"request_ids_offset":0,"request_ids_total":requests.len(),"kind":"turn","undoes":null,"state":state,"head_moved":false,"after_restart":false,"paths":[{"path":"cmd.txt","change":"modified","added":2,"removed":1}],"paths_offset":0,"paths_total":1,"not_covered":[],"not_covered_offset":0,"not_covered_total":0,"created_at":1})
}
fn decode(v: Value) -> TurnRecord {
    serde_json::from_value(v).unwrap()
}
fn message(id: &str, user: bool) -> Message {
    Message {
        id: id.into(),
        role: if user {
            MessageRole::User
        } else {
            MessageRole::Assistant
        },
        text: id.into(),
        ..Default::default()
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        messages: vec![
            message("request-one", true),
            message("answer-one", false),
            message("request-steer", true),
            message("answer-two", false),
            message("request-next", true),
            message("answer-next", false),
        ],
        tools: vec![Tool {
            id: "tool".into(),
            name: "command".into(),
            status: ToolStatus::Done,
            after: Some(Some("answer-one".into())),
            input: String::new(),
            output: String::new(),
        }],
        ..Default::default()
    }
}
#[test]
fn steering_closes_once_and_queued_followups_close_separately() {
    let records = vec![
        decode(record("next", &["next"], "ready")),
        decode(record("one", &["one", "steer"], "ready")),
    ];
    let lines = project(&snapshot(), &records, true);
    assert_eq!(lines.rows.len(), 1);
    let first = &lines.rows[&Key::Message("answer-two".into())];
    assert_eq!(first.steps, Some(1));
    assert_eq!(first.files[0].path, PathBuf::from("cmd.txt"));
    assert_eq!(lines.latest.as_ref().unwrap().run, "next");
    assert!(lines.covered.contains(&Key::Message("answer-one".into())));
}
#[test]
fn no_answer_closes_at_the_last_tool_or_request() {
    let mut snapshot = snapshot();
    snapshot.messages.truncate(1);
    snapshot.tools[0].after = Some(Some("request-one".into()));
    snapshot.messages.push(message("request-next", true));
    let lines = project(
        &snapshot,
        &[
            decode(record("one", &["one"], "ready")),
            decode(record("next", &["next"], "failed")),
        ],
        true,
    );
    assert_eq!(lines.rows[&Key::Tool("tool".into())].run, "one");
    assert_eq!(lines.latest.unwrap().availability, Availability::Failed);
}
#[test]
fn unloaded_history_does_not_invent_counts_or_close_an_unfinished_page() {
    let records = vec![
        decode(record("one", &["one", "steer"], "ready")),
        decode(record("next", &["next"], "ready")),
    ];
    let mut snapshot = snapshot();
    snapshot.messages.drain(..2);
    snapshot.tools.clear();
    let lines = project(&snapshot, &records, false);
    assert_eq!(lines.rows[&Key::Message("answer-two".into())].steps, None);
    assert!(lines.latest.is_none());
    snapshot.messages.truncate(2);
    assert!(project(&snapshot, &records, false).rows.is_empty());
    snapshot.messages.clear();
    assert!(project(&snapshot, &records, true).latest.is_none());
}
#[test]
fn every_record_and_undo_state_decodes_and_gates() {
    for (state, expected) in [
        ("recording", Availability::Recording),
        ("ready", Availability::Ready),
        ("failed", Availability::Failed),
        ("undone", Availability::Unavailable),
    ] {
        let mut v = record("r", &["next"], state);
        v["after_restart"] = json!(true);
        v["head_moved"] = json!(true);
        v["not_covered"] = json!([{"path":"build/","reason":"ignored_directory"}]);
        v["not_covered_total"] = json!(1);
        let line = project(&snapshot(), &[decode(v)], true).latest.unwrap();
        assert_eq!(line.availability, expected);
        assert_eq!(line.target.is_some(), state == "ready");
        assert!(
            line.detail.contains("while the host was down")
                && line.detail.contains("HEAD moved")
                && line.detail.contains("build/")
        );
    }
    for state in ["blocked", "undone"] {
        let reply: host::UndoTurnReply = serde_json::from_value(json!({"state":state,"changed_since":[],"changed_since_total":0,"not_covered":[],"not_covered_total":0,"record_id":"r","after_restart":false})).unwrap();
        assert_eq!(reply.state == host::UndoState::Blocked, state == "blocked");
    }
    let mut s = snapshot();
    s.status = ThreadStatus::Running;
    assert!(
        project(&s, &[decode(record("next", &["next"], "recording"))], true)
            .latest
            .is_none()
    );
}
#[test]
fn undo_and_redo_follow_the_leaf_and_show_only_its_restored_files() {
    let original = record("r", &["next"], "undone");
    let mut undo = record("u", &["undo-request"], "ready");
    undo["kind"] = json!("undo");
    undo["undoes"] = json!("r");
    undo["paths"][0]["path"] = json!("restored.txt");
    let line = project(
        &snapshot(),
        &[decode(undo.clone()), decode(original.clone())],
        true,
    )
    .latest
    .unwrap();
    assert!(line.undone);
    assert_eq!(line.target.as_deref(), Some("u"));
    assert_eq!(line.files[0].path, PathBuf::from("restored.txt"));
    undo["state"] = json!("undone");
    let mut redo = undo.clone();
    redo["id"] = json!("redo");
    redo["undoes"] = json!("u");
    redo["state"] = json!("ready");
    let line = project(
        &snapshot(),
        &[decode(redo), decode(undo), decode(original)],
        true,
    )
    .latest
    .unwrap();
    assert!(!line.undone);
    assert_eq!(line.target.as_deref(), Some("redo"));
}
#[test]
fn pages_append_all_three_lists_and_reject_gaps_and_changed_headers() {
    let mut first = record("r", &["one"], "ready");
    first["request_ids_total"] = json!(2);
    first["paths_total"] = json!(2);
    first["not_covered_total"] = json!(1);
    let mut next = first.clone();
    next["request_ids"] = json!(["steer"]);
    next["request_ids_offset"] = json!(1);
    next["paths_offset"] = json!(1);
    next["paths"][0]["path"] = json!("second");
    next["not_covered"] = json!([{"path":"out/","reason":"ignored_directory"}]);
    let mut records = Vec::new();
    append(&mut records, vec![decode(first.clone())]).unwrap();
    append(&mut records, vec![decode(next.clone())]).unwrap();
    assert!(complete(&records[0]));
    for field in ["paths_offset", "request_ids_offset", "not_covered_total"] {
        let mut bad = next.clone();
        bad[field] = json!(99);
        let mut records = vec![decode(first.clone())];
        assert!(append(&mut records, vec![decode(bad)]).is_err());
    }
}

#[test]
fn reasoning_and_text_share_exactly_one_closing_line() {
    let mut snapshot = Snapshot {
        messages: vec![
            message("request-last", true),
            message("answer", false),
            message("request-next", true),
        ],
        ..Default::default()
    };
    snapshot.messages[1].reasoning = "reasoning".into();
    let records = [decode(record("last", &["last"], "ready"))];
    for only_reasoning in [false, true] {
        if only_reasoning {
            snapshot.messages[1].text.clear();
        }
        let lines = project(&snapshot, &records, true);
        let closings = transcript::rows(&snapshot)
            .into_iter()
            .filter_map(closing_key)
            .filter(|key| lines.rows.contains_key(key))
            .count();
        assert_eq!(
            closings, 1,
            "reasoning and message rows must not duplicate the control"
        );
    }
}

#[test]
fn a_failed_undo_without_file_effects_keeps_the_original_action() {
    let mut failed = record("attempt", &["undo"], "no_effect");
    failed["kind"] = json!("undo");
    failed["undoes"] = json!("run");
    failed["paths"] = json!([]);
    failed["paths_total"] = json!(0);
    let lines = project(
        &snapshot(),
        &[decode(failed), decode(record("run", &["next"], "ready"))],
        true,
    );
    let line = lines.latest.unwrap();
    assert_eq!(line.target.as_deref(), Some("run"));
    assert_eq!(line.availability, Availability::Ready);
    assert!(!line.undone);
}

#[test]
fn large_exclusion_lists_prepare_a_bounded_hover_preview() {
    let mut v = record("run", &["next"], "ready");
    v["not_covered"] = json!((0..10_000).map(|i| json!({"path":format!("excluded-{i:05}/{}", "x".repeat(1000)),"reason":"unreadable"})).collect::<Vec<_>>());
    v["not_covered_total"] = json!(10_000);
    let line = project(&snapshot(), &[decode(v)], true).latest.unwrap();
    assert_eq!(line.not_covered.len(), 10_000);
    assert!(line.detail.contains("9988 more not-covered paths"));
    assert!(!line.detail.contains("excluded-00012"));
    assert!(line.detail.len() < 5000);
}

#[test]
fn recorded_display_escapes_controls_without_changing_identity() {
    let path = "src\u{2066}/review\u{202e}\u{200b}.rs";
    let id = "\u{202e}record";
    let mut data = record(id, &["next"], "ready");
    data["paths"][0]["path"] = json!(path);
    data["not_covered"] = json!([{"path":"build\u{0085}/", "reason":"ignored_directory"}]);
    data["not_covered_total"] = json!(1);
    let records = [decode(data)];
    let mut snapshot = snapshot();
    snapshot.title = Some("Thread\u{2028}name".into());
    snapshot.messages[4].text = "Review\u{2066}this\u{e0001}".into();
    let line = project(&snapshot, &records, true).latest.unwrap();
    assert_eq!(line.run, id);
    let file = &line.files[0];
    assert_eq!(file.path, PathBuf::from(path));
    assert_eq!(file.name, "review\\u202e\\u200b.rs");
    for detail in [&file.review_detail, &file.current_detail, &line.detail] {
        assert!(detail.contains("src\\u2066/review\\u202e\\u200b.rs"));
    }
    assert!(line.detail.contains("build\\u0085/"));

    let target = crate::review::recorded::Target {
        thread: "thread".into(),
        checkout: "/checkout".into(),
        record: id.into(),
        original: id.into(),
        path: Some(path.into()),
    };
    let mut saved = crate::review::recorded::Record::prepare(&records, &target).unwrap();
    assert_eq!(saved.id, id);
    assert_eq!(saved.selected(&target), Some(0));
    assert_eq!(saved.files[0].path, PathBuf::from(path));
    assert_eq!(
        saved.files[0].display_path,
        "src\\u2066/review\\u202e\\u200b.rs"
    );
    assert_eq!(saved.files[0].directory, "src\\u2066");
    assert_eq!(saved.files[0].name, file.name);
    assert!(saved.label.starts_with("Turn · "));
    assert!(!saved.label.contains("record"));
    assert!(!saved.detail.contains("\\u202erecord"));
    saved.describe(&records, Some(&snapshot));
    assert!(saved.label.starts_with("Turn · Review\\u2066"));
    assert!(saved.detail.contains("Review\\u2066this\\udb40\\udc01"));
    assert!(saved.detail.contains("Thread\\u2028name"));
    assert!(saved.detail.contains("build\\u0085/"));

    let patch = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+new\u{202e}\n"
    );
    let diff = crate::git::recorded_diff(patch.as_bytes(), false, std::time::Duration::ZERO);
    assert!(diff.lines[0].text.contains("review\\u202e\\u200b.rs"));
    assert_eq!(&*diff.lines.last().unwrap().text, "+new\\u202e");
    assert_eq!(diff.lines.last().unwrap().new_line, Some(1));
    assert!(
        patch.contains('\u{202e}'),
        "display preparation must not mutate the saved patch"
    );
}
