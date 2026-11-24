//! Recorded work and closing lines drawn from injected turn records.
use super::*;
use plait_client::{host::UndoTurnReply, preferences::Turns};
use serde_json::{Value, json};

fn record(outcome: &str) -> Value {
    let mut record = undo::record("changes", &["changes"], "ready");
    record["outcome"] = outcome.into();
    record["paths"] = json!([
        {"path": "src/refresh.rs", "change": "modified", "added": 14, "removed": 3},
        {"path": "tests/queue.rs", "change": "added", "added": 21, "removed": 0},
        {"path": "docs/polling.md", "change": "deleted", "added": 0, "removed": 6}
    ]);
    record["paths_total"] = 3.into();
    record
}

fn install_records(h: &mut Harness, records: Vec<Value>) {
    h.update(|p, _, _| {
        undo::inject(p, records);
        p.workspace.turn_records.totals =
            super::super::turn_records::TurnRecords::file_totals(&p.workspace.turn_records.data);
        p.reconcile_live_folding();
    });
}

fn stage(h: &mut Harness, repository: &Path) {
    h.connect(repository, false);
    h.change_snapshot(|s| {
        s.status = ThreadStatus::Idle;
        s.messages = vec![
            Message { id: "request-changes".into(), role: MessageRole::User,
                text: "Move the refresh off the UI thread and cover it with a test.".into(),
                ..Default::default() },
            Message { id: "progress".into(), role: MessageRole::Assistant,
                text: "I found the polling loop in the refresh handler.".into(),
                reasoning: "Check which task owns the queue.".into(), ..Default::default() },
            Message { id: "answer".into(), role: MessageRole::Assistant,
                text: "Refresh now runs in the background. The queue test covers a second request arriving during a refresh.".into(),
                ..Default::default() },
        ];
        s.tools = vec![
            tool("read-refresh", "read", ToolStatus::Done, r#"{"path":"src/refresh.rs"}"#, "fn refresh()"),
            tool("test-queue", "command", ToolStatus::Done, r#"{"command":"cargo test queue"}"#, "1 passed"),
        ];
        s.tools[0].after = Some(Some("request-changes".into()));
        s.tools[1].after = Some(Some("progress".into()));
        s.live_text.clear();
        s.live_reasoning.clear();
        s.pending_approval = None;
        s.truncated = false;
        s.records_revision = Some(1);
    });
    h.update(|p, _, cx| {
        p.state.projects[0].machines[0].name = "studio".into();
        p.preferences.value.turns = Turns::Bubbles;
        p.preferences.value.fold_finished_work = Some(true);
        p.preferences.value.hide_inspector = true;
        p.settings_open = false;
        p.overlay = None;
        p.workspace.tab = super::super::Workspace::Conversation;
        p.workspace.turn_records = Default::default();
        p.workspace.thread_files = Default::default();
        p.host.connection.status.as_mut().unwrap().protocol_version = 7;
        p.host.connection.threads_mut()[0].status = ThreadStatus::Idle;
        p.workspace.live_scroll.scroll_to_end();
        p.sync_composer(cx);
    });
    install_records(h, vec![record("finished")]);
}

pub(super) fn scenes(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut harness = this_turn::harness(repository, output, width, height);
    walk(&mut harness, repository, suffix);
}

fn walk(h: &mut Harness, repository: &Path, suffix: &str) {
    let name = |state: &str| {
        if suffix == "wide" {
            state.to_owned()
        } else {
            format!("{state}-{suffix}")
        }
    };
    stage(h, repository);
    if suffix == "narrow" {
        h.capture("closing-line-narrow");
    }
    h.capture(&name("work-folded"));
    capture_both(h, &name("closing-line-finished"));
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        for label in ["Finished · 2 steps", "3 files +35 −9", "Undo", "Review"] {
            assert!(
                frame
                    .nodes
                    .iter()
                    .any(|node| node.label.as_deref() == Some(label)),
                "{label}"
            );
        }
        assert!(!frame.nodes.iter().any(|node| {
            node.label
                .as_deref()
                .is_some_and(|label| label.contains("polling loop"))
        }));
    }
    h.update(|p, _, cx| {
        assert_eq!(p.work_open_at(1), Some(false));
        p.toggle_finished_work("request-changes", cx);
        assert_eq!(p.work_open_at(1), Some(true));
        assert!(p.capture_tab_bookmark(None, cx).details.folded.is_empty());
    });
    h.capture(&name("work-opened"));
    h.update(|p, _, cx| {
        p.toggle_finished_work("request-changes", cx);
        p.toggle_record_files("changes", cx);
    });
    h.capture(&name("closing-line-files-open"));
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        for (change, path) in [
            ("Modified", "src/refresh.rs"),
            ("Added", "tests/queue.rs"),
            ("Deleted", "docs/polling.md"),
        ] {
            assert!(
                frame.nodes.iter().any(|node| node
                    .label
                    .as_deref()
                    .is_some_and(|label| label.starts_with(change) && label.contains(path))),
                "{change} {path}"
            );
        }
    }
    let mut many = record("finished");
    many["paths"] = (0..42)
        .map(|i| {
            json!({"path":format!("tests/case_{i}.rs"),
        "change":"modified","added":2,"removed":1})
        })
        .collect();
    many["paths_total"] = 45.into();
    install_records(h, vec![many]);
    h.capture(&name("closing-line-many-files"));
    h.update(|p, _, cx| p.toggle_record_files("changes", cx));
    install_records(h, vec![record("finished")]);

    h.update(|p, window, cx| {
        p.open_conversation_find(window, cx);
        p.workspace
            .conversation_find
            .field
            .clone()
            .unwrap()
            .update(cx, |field, cx| field.set_text("polling loop", cx));
    });
    h.capture(&name("work-find-inside"));
    #[cfg(debug_assertions)]
    assert!(h.draw_a11y().nodes.iter().any(|node| {
        node.label
            .as_deref()
            .is_some_and(|name| name.contains("polling loop") && name.starts_with("Plait message"))
    }));
    h.update(|p, window, cx| {
        assert_eq!(p.work_open_at(1), Some(true));
        p.close_conversation_find(window, cx);
        assert_eq!(p.work_open_at(1), Some(false));
        p.preferences.value.fold_finished_work = Some(false);
    });
    h.capture(&name("work-setting-off"));

    h.update(|p, _, _| p.preferences.value.fold_finished_work = Some(true));
    for (status, outcome, scene, answer) in [
        (
            ThreadStatus::Error,
            "failed",
            "closing-line-failed",
            "The queue test failed while waiting for refresh.",
        ),
        (
            ThreadStatus::Stopped,
            "stopped",
            "closing-line-stopped",
            "Stopped while checking the queue.",
        ),
    ] {
        h.change_snapshot(|s| {
            s.status = status;
            s.messages[2].text = answer.into();
            s.tools[1].status = if status == ThreadStatus::Error {
                ToolStatus::Error
            } else {
                ToolStatus::Interrupted
            };
            s.tools[1].output = answer.into();
        });
        install_records(h, vec![record(outcome)]);
        h.capture(&name(scene));
        if status == ThreadStatus::Error {
            h.capture(&name("work-failed-open"));
        }
    }

    stage(h, repository);
    let mut original = record("finished");
    original["state"] = "undone".into();
    let mut child = record("finished");
    child["id"] = "restore".into();
    child["kind"] = "undo".into();
    child["undoes"] = "changes".into();
    child["outcome"] = Value::Null;
    install_records(h, vec![child, original]);
    h.capture(&name("closing-line-undone"));

    install_records(h, vec![record("finished")]);
    h.update(|p, _, _| {
        let refused: UndoTurnReply = serde_json::from_value(json!({
            "state":"blocked","changed_since":["src/refresh.rs"],"changed_since_total":1,
            "not_covered":[],"not_covered_total":0,"record_id":"changes","after_restart":false
        }))
        .unwrap();
        p.workspace.turn_records.block("changes", refused);
    });
    h.capture(&name("closing-line-undo-refused"));
    h.update(|p, _, cx| p.cancel_undo_panel(cx));
    let mut empty = record("finished");
    empty["paths"] = json!([]);
    empty["paths_total"] = 0.into();
    h.change_snapshot(|s| {
        s.messages.remove(1);
        s.messages[0].text = "Which task owns the refresh queue?".into();
        s.messages[1].text = "The background refresh task owns the queue.".into();
        s.tools.clear();
    });
    install_records(h, vec![empty]);
    h.capture(&name("work-no-work"));
    h.capture(&name("closing-line-no-files"));
    stage(h, repository);
    h.update(|p, _, _| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 4;
        p.workspace.turn_records = Default::default();
        p.reconcile_live_folding();
    });
    h.capture(&name("closing-line-older-host"));
    h.change_snapshot(|s| {
        long_run(s, true);
        s.messages.truncate(1);
        for t in &mut s.tools {
            t.after = Some(Some("request-changes".into()));
        }
    });
    h.capture(&name("work-running"));
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn opening_work_keeps_the_disclosure_in_place() {
    if !isolated("finished_work::opening_work_keeps_the_disclosure_in_place") {
        return;
    }
    let repo = fixture_repository(&PathBuf::from(std::env::var_os("HOME").unwrap()).join("layout"));
    let mut harness = this_turn::harness(&repo, None, 1024., 740.);
    stage(&mut harness, &repo);
    let disclosure_top = |h: &mut Harness| {
        h.draw_a11y()
            .nodes
            .iter()
            .find(|node| node.label.as_deref() == Some("2 steps · 1 read, 1 command"))
            .expect("work disclosure is visible")
            .bounds
            .top()
    };
    for turns in [Turns::Bubbles, Turns::Line] {
        harness.update(|p, _, cx| {
            p.set_turns(turns, cx);
            p.workspace.live_scroll.scroll_to_end();
        });
        let before = disclosure_top(&mut harness);
        harness.update(|p, _, cx| p.toggle_finished_work("request-changes", cx));
        let after = disclosure_top(&mut harness);
        assert!(
            (after - before).abs() <= px(1.),
            "{turns:?}: {before:?} -> {after:?}"
        );
        harness.update(|p, _, cx| p.toggle_finished_work("request-changes", cx));
    }
}

#[core::prelude::v1::test]
fn clicking_find_opened_work_does_not_pin_it_open() {
    if !isolated("finished_work::clicking_find_opened_work_does_not_pin_it_open") {
        return;
    }
    let repo = fixture_repository(&PathBuf::from(std::env::var_os("HOME").unwrap()).join("find"));
    let mut harness = this_turn::harness(&repo, None, 1024., 740.);
    stage(&mut harness, &repo);
    harness.update(|p, window, cx| {
        assert_eq!(p.work_open_at(1), Some(false));
        p.open_conversation_find(window, cx);
        p.workspace
            .conversation_find
            .field
            .clone()
            .unwrap()
            .update(cx, |field, cx| field.set_text("polling loop", cx));
    });
    harness.draw();
    harness.update(|p, window, cx| {
        assert_eq!(p.work_open_at(1), Some(true));
        p.toggle_finished_work("request-changes", cx);
        assert_eq!(p.work_open_at(1), Some(true));
        p.close_conversation_find(window, cx);
        assert_eq!(p.work_open_at(1), Some(false));
    });
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn clipped_work_does_not_claim_a_complete_step_count() {
    if !isolated("finished_work::clipped_work_does_not_claim_a_complete_step_count") {
        return;
    }
    let repo =
        fixture_repository(&PathBuf::from(std::env::var_os("HOME").unwrap()).join("clipped"));
    let mut harness = this_turn::harness(&repo, None, 1024., 740.);
    stage(&mut harness, &repo);
    harness.change_snapshot(|snapshot| snapshot.truncated = true);
    install_records(&mut harness, vec![record("finished")]);
    let frame = harness.draw_a11y();
    assert!(!frame.nodes.iter().any(|node| {
        node.label
            .as_deref()
            .is_some_and(|label| label.contains("2 steps"))
    }));
    assert!(frame.has_node(Role::Group, |node| node.label.as_deref()
        == Some("Finished")));
    assert!(frame.has_node(Role::Button, |node| node.label.as_deref() == Some("Work")));
}

#[core::prelude::v1::test]
fn recorded_work_scenes_draw_with_injected_records() {
    if !isolated("finished_work::recorded_work_scenes_draw_with_injected_records") {
        return;
    }
    let repo = fixture_repository(&PathBuf::from(std::env::var_os("HOME").unwrap()).join("scenes"));
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        scenes(&repo, None, suffix, width, height);
    }
}
