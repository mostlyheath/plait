//! Desktop undo tests and scenes. Every host reply below is scripted sample data.
use super::*;
use crate::ui::{
    Workspace,
    turn_records::{Stamp, TurnRecords},
};
use plait_client::{
    history::records,
    host::{self, TurnRecord, UndoTurnReply},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) fn record(id: &str, requests: &[&str], state: &str) -> Value {
    json!({"id":id,"request_ids":requests,"request_ids_offset":0,"request_ids_total":requests.len(),"kind":"turn","undoes":null,"state":state,"head_moved":false,"after_restart":false,
        "paths":[{"path":"src/review.rs","change":"modified","added":3,"removed":1},{"path":"README.md","change":"modified","added":2,"removed":0},{"path":"src/generated.rs","change":"added","added":12,"removed":0},{"path":"notes/plan.toml","change":"added","added":2,"removed":0}],"paths_offset":0,"paths_total":4,
        "not_covered":[{"path":"build/","reason":"ignored_directory"},{"path":"vendor/","reason":"nested_repository"}],"not_covered_offset":0,"not_covered_total":2,"created_at":1})
}
fn reply(state: &str, id: &str) -> Value {
    json!({"state":state,"changed_since":if state == "blocked" { vec!["src/review.rs"] } else {vec![]},"changed_since_total":if state == "blocked" {1} else {0},"not_covered":[{"path":"build/","reason":"ignored_directory"}],"not_covered_total":1,"record_id":id,"after_restart":false})
}
pub(super) fn prepare(harness: &mut Harness, repository: &Path) {
    harness.connect(repository, false);
    harness.change_snapshot(|s| {
        s.status = ThreadStatus::Idle;
        s.live_text.clear();
        s.live_reasoning.clear();
        s.pending_approval = None;
        s.records_revision = Some(1);
        s.messages = vec![
            Message {
                id: "request-first".into(),
                role: MessageRole::User,
                text: "Keep refresh work off the UI thread.".into(),
                ..Default::default()
            },
            Message {
                id: "answer-first".into(),
                role: MessageRole::Assistant,
                text: "Refresh now runs in the background.".into(),
                ..Default::default()
            },
            Message {
                id: "request-last".into(),
                role: MessageRole::User,
                text: "Document the behavior and regenerate the file list.".into(),
                ..Default::default()
            },
            Message {
                id: "answer-last".into(),
                role: MessageRole::Assistant,
                text: "Updated the guide and generated file list. The checks pass.".into(),
                ..Default::default()
            },
        ];
        s.tools = vec![Tool {
            id: "command-last".into(),
            name: "command".into(),
            status: ToolStatus::Done,
            input: r#"{"command":"generate-file-list"}"#.into(),
            output: "Generated src/generated.rs".into(),
            after: Some(Some("request-last".into())),
        }];
    });
    harness.update(|plait, _, _| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 5;
        plait.workspace.turn_records = Default::default();
        plait.workspace.thread_files = Default::default();
        plait.settings_open = false;
        plait.overlay = None;
        plait.workspace.history = Default::default();
        let data = vec![
            record("last", &["last"], "ready"),
            record("first", &["first"], "ready"),
        ];
        inject(plait, data);
    });
}
pub(super) fn inject(plait: &mut Plait, data: Vec<Value>) {
    let snapshot = plait.host.thread.snapshot.as_ref().unwrap();
    plait.workspace.turn_records.inject(
        data.into_iter()
            .map(|v| serde_json::from_value(v).unwrap())
            .collect(),
        snapshot,
        plait.host.connection.epoch,
        plait.host.thread.watch_epoch,
    );
}
pub(super) fn wait(harness: &mut Harness, done: impl Fn(&Plait) -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        harness.settle();
        if harness
            .window
            .update(&mut harness.cx, |p, _, _| done(p))
            .unwrap()
        {
            break;
        }
        assert!(Instant::now() < end, "undo background work did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn setup(name: &str) -> (PathBuf, PathBuf, Harness) {
    let scratch = std::env::temp_dir().join(format!("plait-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    prepare(&mut harness, &repository);
    (scratch, repository, harness)
}

/// A private, owner-only socket using the real Rust client and async UI paths.
pub(super) struct Host {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub(super) requests: Arc<Mutex<Vec<Value>>>,
    socket: PathBuf,
}
impl Host {
    pub(super) fn start(mut answer: impl FnMut(&Value) -> Option<Value> + Send + 'static) -> Self {
        let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
        std::fs::create_dir_all(&store).unwrap();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = store.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let thread = std::thread::spawn(move || {
            while !quit.load(Ordering::SeqCst) {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("scripted host: {e}"),
                };
                // Accepted sockets inherit nonblocking mode on macOS.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                seen.lock().unwrap().push(request.clone());
                if let Some(value) = answer(&request) {
                    let envelope = if value.get("ok").is_some() {
                        value
                    } else {
                        json!({"ok":true,"result":value})
                    };
                    writeln!(socket, "{envelope}").unwrap();
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
            requests,
            socket,
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
        std::fs::remove_file(&self.socket).unwrap();
    }
}
fn history_reply() -> Value {
    json!({"items":[],"before":null})
}

#[core::prelude::v1::test]
fn undo_control_and_shortcut_share_the_gate() {
    if !isolated("undo::undo_control_and_shortcut_share_the_gate") {
        return;
    }
    let action = crate::ui::actions::ACTIONS
        .iter()
        .find(|a| a.id == "thread.undo_last_turn")
        .unwrap();
    let key = action.default_key().unwrap();
    assert_eq!(action.category, "Thread");
    assert!(key.modifiers.platform && key.modifiers.alt);
    assert_ne!(key.key, "z");
    let (scratch, _, mut harness) = setup("undo-gates");
    harness.update(|plait, window, cx| {
        let line = plait.workspace.turn_records.lines.latest.clone().unwrap();
        assert!(plait.undo_refusal(&line).is_none());
        assert_eq!(plait.shortcut_undo().as_deref(), Some("last"));
        plait.workspace.turn_records.projected_generation += 1;
        assert!(
            plait.shortcut_undo().is_none(),
            "raw records cannot enable an older projection"
        );
        plait.workspace.turn_records.projected_generation -= 1;
        plait.host.connection.connected = false;
        assert!(plait.shortcut_undo().is_none());
        plait.host.connection.connected = true;
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 4;
        assert!(plait.shortcut_undo().is_none());
        assert!(plait.legacy_record_tip().contains("newer host"));
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 5;
        plait.settings_open = true;
        assert!(plait.shortcut_undo().is_none());
        plait.settings_open = false;
        plait.workspace.tab = Workspace::Changes;
        assert!(plait.shortcut_undo().is_none());
        plait.workspace.tab = Workspace::Conversation;
        plait.toggle_command_palette(window, cx);
        assert!(plait.shortcut_undo().is_none());
        plait.overlay = None;
        plait.host.connection.threads_mut()[1].status = ThreadStatus::Running;
        assert_eq!(plait.undo_refusal(&line).unwrap().0, "Wait");
        plait.host.connection.threads_mut()[1].status = ThreadStatus::Idle;
        for (state, label) in [("recording", "Recording"), ("failed", "Unavailable")] {
            inject(plait, vec![record("last", &["last"], state)]);
            assert_eq!(
                plait
                    .undo_refusal(plait.workspace.turn_records.lines.latest.as_ref().unwrap())
                    .unwrap()
                    .0,
                label
            );
            assert!(plait.shortcut_undo().is_none());
        }
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn undo_redo_and_changed_files_follow_scripted_records() {
    if !isolated("undo::undo_redo_and_changed_files_follow_scripted_records") {
        return;
    }
    let (scratch, _, mut harness) = setup("undo-roundtrip");
    std::fs::write(
        scratch.join("plait/src/generated.rs"),
        "generated by a command\n",
    )
    .unwrap();
    let mut data = vec![record("last", &["last"], "ready")];
    let mut undos = 0;
    let host = Host::start(move |request| {
        Some(match request["op"].as_str().unwrap() {
            "undo_turn" => {
                undos += 1;
                assert_eq!(request["protocol_version"], 5);
                assert!(request.get("leave_changed").is_none());
                assert_eq!(
                    request["record_id"],
                    if undos == 1 { "last" } else { "undo-1" }
                );
                let id = format!("undo-{undos}");
                data[0]["state"] = json!("undone");
                let mut undo = record(&id, &["undo-request"], "ready");
                undo["kind"] = json!("undo");
                undo["undoes"] = request["record_id"].clone();
                data.insert(0, undo);
                reply("undone", &id)
            }
            "turn_records" => json!({"records":data,"before":null}),
            "history" => history_reply(),
            op => panic!("unexpected request {op}"),
        })
    });
    harness.draw();
    harness.keystroke("cmd-alt-u");
    wait(&mut harness, |p| {
        p.records_current()
            && p.workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .is_some_and(|l| l.undone)
    });
    // A held key cannot immediately redo the operation when its reply arrives.
    harness.held_keystroke("cmd-alt-u");
    assert_eq!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["op"] == "undo_turn")
            .count(),
        1
    );
    harness.release();
    wait(&mut harness, |p| {
        p.workspace
            .thread_files
            .counted()
            .any(|c| c.path == Path::new("src/generated.rs"))
    });
    harness.update(|plait, window, cx| {
        assert!(!plait.workspace.turn_records.pending());
        assert_eq!(
            plait
                .workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .unwrap()
                .target
                .as_deref(),
            Some("undo-1")
        );
        assert!(
            plait
                .workspace
                .turn_records
                .paths()
                .contains(&PathBuf::from("src/generated.rs"))
        );
        plait.undo_run("last", false, window, cx);
    });
    wait(&mut harness, |p| {
        p.records_current()
            && p.workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .is_some_and(|l| l.target.as_deref() == Some("undo-2"))
    });
    harness.update(|plait, _, _| {
        assert!(
            !plait
                .workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .unwrap()
                .undone
        )
    });
    let requests = host.requests.lock().unwrap();
    let undos: Vec<_> = requests.iter().filter(|r| r["op"] == "undo_turn").collect();
    assert_eq!(undos.len(), 2);
    assert_ne!(undos[0]["request_id"], undos[1]["request_id"]);
    drop(requests);
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Scripted restoration and records exercise the desktop's real async client path.
fn shortcut_host(repository: &Path) -> Host {
    let path = repository.join("approval.txt");
    std::fs::write(&path, "allowed\n").unwrap();
    let mut original = record("last", &["last"], "ready");
    original["paths"] = json!([{"path":"approval.txt","change":"modified","added":1,"removed":1}]);
    original["paths_total"] = json!(1);
    let mut data = vec![original];
    let mut undos = 0;
    Host::start(move |request| {
        Some(match request["op"].as_str().unwrap() {
            "undo_turn" => {
                assert_eq!(request["record_id"], data[0]["id"]);
                assert_eq!(request["protocol_version"], 5);
                assert!(request.get("leave_changed").is_none());
                undos += 1;
                let id = format!("undo-{undos}");
                data[0]["state"] = json!("undone");
                let mut undo = record(&id, &["undo-request"], "ready");
                undo["paths"] = data[0]["paths"].clone();
                undo["paths_total"] = json!(1);
                undo["kind"] = json!("undo");
                undo["undoes"] = request["record_id"].clone();
                data.insert(0, undo);
                std::fs::write(
                    &path,
                    if undos % 2 == 1 {
                        "before\n"
                    } else {
                        "allowed\n"
                    },
                )
                .unwrap();
                reply("undone", &id)
            }
            "turn_records" => json!({"records":data,"before":null}),
            "history" => history_reply(),
            op => panic!("unexpected request {op}"),
        })
    })
}

fn restored(harness: &mut Harness, repository: &Path, count: usize) {
    let target = format!("undo-{count}");
    wait(harness, |p| {
        p.records_current()
            && p.workspace.turn_records.pending.is_none()
            && p.workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .is_some_and(|line| line.target.as_ref() == Some(&target))
    });
    harness.update(|p, _, _| {
        assert_eq!(p.shortcut_undo().as_deref(), Some("last"));
        assert_eq!(
            p.workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .unwrap()
                .undone,
            count % 2 == 1
        );
    });
    assert_eq!(
        std::fs::read_to_string(repository.join("approval.txt")).unwrap(),
        if count % 2 == 1 {
            "before\n"
        } else {
            "allowed\n"
        }
    );
}

fn undo_requests(host: &Host) -> Vec<Value> {
    host.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r["op"] == "undo_turn")
        .cloned()
        .collect()
}

#[core::prelude::v1::test]
fn shortcut_redoes_without_release_events() {
    if !isolated("undo::shortcut_redoes_without_release_events") {
        return;
    }
    let (scratch, repository, mut harness) = setup("undo-no-release");
    let host = shortcut_host(&repository);
    harness.update(|p, w, cx| p.focus_composer(w, cx));
    harness.draw();
    // Separate presses have is_held=false even when Command key-up never arrives.
    // Do not insert a focus change, modifier change, refresh, or unrelated key.
    for count in 1..=4 {
        harness.keystroke("cmd-alt-u");
        restored(&mut harness, &repository, count);
    }
    let requests = undo_requests(&host);
    assert_eq!(requests.len(), 4);
    for pair in requests.windows(2) {
        assert_ne!(pair[0]["request_id"], pair[1]["request_id"]);
    }
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn held_shortcut_cannot_redo_after_the_reply() {
    if !isolated("undo::held_shortcut_cannot_redo_after_the_reply") {
        return;
    }
    let (scratch, repository, mut harness) = setup("undo-held");
    let host = shortcut_host(&repository);
    harness.draw();
    harness.keystroke("cmd-alt-u");
    harness.held_keystroke("cmd-alt-u");
    restored(&mut harness, &repository, 1);
    for _ in 0..3 {
        harness.held_keystroke("cmd-alt-u");
    }
    harness.update(|p, _, _| assert!(p.workspace.turn_records.pending.is_none()));
    assert_eq!(undo_requests(&host).len(), 1);
    assert_eq!(
        std::fs::read_to_string(repository.join("approval.txt")).unwrap(),
        "before\n"
    );
    harness.keystroke("cmd-alt-u");
    restored(&mut harness, &repository, 2);
    assert_eq!(undo_requests(&host).len(), 2);
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn shortcut_control_and_menu_can_alternate() {
    if !isolated("undo::shortcut_control_and_menu_can_alternate") {
        return;
    }
    let (scratch, repository, mut harness) = setup("undo-routes");
    let host = shortcut_host(&repository);
    harness.draw();
    harness.keystroke("cmd-alt-u");
    restored(&mut harness, &repository, 1);
    // The closing-line activation calls undo_run with this run and flag.
    harness.update(|p, w, cx| p.undo_run("last", false, w, cx));
    restored(&mut harness, &repository, 2);
    harness.update(|p, w, cx| p.undo_run("last", false, w, cx));
    restored(&mut harness, &repository, 3);
    harness.update(|p, w, cx| {
        Arc::make_mut(p.host.thread.snapshot.as_mut().unwrap()).records_revision = Some(2);
        p.turn_records_snapshot(w, cx);
        p.focus_composer(w, cx);
    });
    restored(&mut harness, &repository, 3);
    harness.keystroke("cmd-alt-u");
    restored(&mut harness, &repository, 4);
    // Menu and palette actions have no key event and must execute directly.
    harness.update(|_, w, cx| w.dispatch_action(Box::new(crate::ui::UndoLastTurn), cx));
    restored(&mut harness, &repository, 5);
    harness.update(|p, _, cx| {
        p.preferences
            .value
            .keymap
            .insert("thread.undo_last_turn".into(), "ctrl-shift-u".into());
        crate::ui::actions::install(cx, &p.preferences.value.keymap);
    });
    harness.keystroke("ctrl-shift-u");
    restored(&mut harness, &repository, 6);
    harness.keystroke("cmd-alt-u");
    assert_eq!(undo_requests(&host).len(), 6);
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn shortcut_without_a_retained_turn_is_inert() {
    if !isolated("undo::shortcut_without_a_retained_turn_is_inert") {
        return;
    }
    let (scratch, repository, mut harness) = setup("undo-empty");
    let host = shortcut_host(&repository);
    harness.update(|p, _, _| inject(p, vec![]));
    harness.draw();
    harness.update(|p, w, cx| {
        // The Thread menu keeps the shared action available; no target is a no-op.
        assert!(w.is_action_available(&crate::ui::UndoLastTurn, cx));
        assert!(p.shortcut_undo().is_none());
    });
    harness.keystroke("cmd-alt-u");
    harness.keystroke("cmd-alt-u");
    harness.held_keystroke("cmd-alt-u");
    harness.update(|_, w, cx| w.dispatch_action(Box::new(crate::ui::UndoLastTurn), cx));
    harness.update(|p, _, _| {
        assert!(p.workspace.turn_records.pending.is_none());
        assert!(p.workspace.turn_records.note.is_none());
        assert!(p.workspace.turn_records.blocked.is_none());
    });
    assert!(undo_requests(&host).is_empty());
    assert_eq!(
        std::fs::read_to_string(repository.join("approval.txt")).unwrap(),
        "allowed\n"
    );
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn redo_shortcut_keeps_the_control_refusals() {
    if !isolated("undo::redo_shortcut_keeps_the_control_refusals") {
        return;
    }
    let (scratch, repository, mut harness) = setup("redo-refusals");
    std::fs::write(repository.join("src/review.rs"), "changed by the person\n").unwrap();
    harness.update(|p, _, _| {
        let mut undo = record("undo-1", &["undo-request"], "ready");
        undo["kind"] = json!("undo");
        undo["undoes"] = json!("last");
        inject(p, vec![undo, record("last", &["last"], "undone")]);
    });
    let host = Host::start(|request| {
        assert_eq!(request["op"], "undo_turn");
        assert_eq!(request["record_id"], "undo-1");
        assert!(request.get("leave_changed").is_none());
        Some(reply("blocked", "undo-1"))
    });
    for gate in 0..4 {
        harness.update(|p, w, cx| {
            match gate {
                0 => p.host.thread.controlling = true,
                1 => {
                    Arc::make_mut(p.host.thread.snapshot.as_mut().unwrap()).status =
                        ThreadStatus::Running
                }
                2 => p.host.connection.threads_mut()[1].status = ThreadStatus::Running,
                _ => assert!(
                    p.state
                        .repository
                        .request_write(plait_client::git::WriteOp::StageAll)
                ),
            }
            let line = p.workspace.turn_records.lines.latest.as_ref().unwrap();
            assert_eq!(
                p.undo_refusal(line),
                Some((
                    "Wait",
                    "A run or another write is active in this checkout. Wait until it finishes."
                ))
            );
            assert!(p.shortcut_undo().is_none());
            p.undo_run("last", false, w, cx);
        });
        harness.draw();
        harness.keystroke("cmd-alt-u");
        harness.update(|p, _, _| {
            assert!(p.workspace.turn_records.pending.is_none());
            p.host.thread.controlling = false;
            Arc::make_mut(p.host.thread.snapshot.as_mut().unwrap()).status = ThreadStatus::Idle;
            p.host.connection.threads_mut()[1].status = ThreadStatus::Idle;
            *p.state.repository.write_mut() = Default::default();
        });
    }
    assert!(undo_requests(&host).is_empty());
    for shortcut in [true, false] {
        if shortcut {
            harness.keystroke("cmd-alt-u");
        } else {
            harness.update(|p, w, cx| p.undo_run("last", false, w, cx));
        }
        wait(&mut harness, |p| p.workspace.turn_records.blocked.is_some());
        harness.update(|p, _, cx| {
            let blocked = &p.workspace.turn_records.blocked.as_ref().unwrap().reply;
            assert_eq!(blocked.record_id, "undo-1");
            assert_eq!(blocked.changed_since, [PathBuf::from("src/review.rs")]);
            assert!(p.shortcut_undo().is_none());
            p.cancel_undo_panel(cx);
        });
    }
    assert_eq!(undo_requests(&host).len(), 2);
    assert_eq!(
        std::fs::read_to_string(repository.join("src/review.rs")).unwrap(),
        "changed by the person\n"
    );
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn blocked_undo_applies_only_after_undo_the_rest() {
    if !isolated("undo::blocked_undo_applies_only_after_undo_the_rest") {
        return;
    }
    let (scratch, _, mut harness) = setup("undo-blocked");
    let mut applied = false;
    let host = Host::start(move |request| {
        Some(match request["op"].as_str().unwrap() {
            "undo_turn" if request["leave_changed"] == true => {
                applied = true;
                reply("undone", "partial")
            }
            "undo_turn" => reply("blocked", "last"),
            "turn_records" => {
                let mut data = vec![record(
                    "last",
                    &["last"],
                    if applied { "undone" } else { "ready" },
                )];
                if applied {
                    let mut undo = record("partial", &["partial-request"], "ready");
                    undo["kind"] = json!("undo");
                    undo["undoes"] = json!("last");
                    undo["paths"] =
                        json!([{"path":"README.md","change":"modified","added":0,"removed":2}]);
                    undo["paths_total"] = json!(1);
                    data.insert(0, undo);
                }
                json!({"records":data,"before":null})
            }
            "history" => history_reply(),
            op => panic!("unexpected {op}"),
        })
    });
    harness.update(|p, w, c| p.undo_run("last", false, w, c));
    wait(&mut harness, |p| p.workspace.turn_records.blocked());
    harness.draw();
    harness.update(|p, _, c| {
        assert!(p.shortcut_undo().is_none());
        p.cancel_undo_panel(c);
    });
    assert_eq!(
        host.requests.lock().unwrap().len(),
        1,
        "Cancel sends nothing"
    );
    harness.update(|p, w, c| p.undo_run("last", false, w, c));
    wait(&mut harness, |p| p.workspace.turn_records.blocked());
    harness.update(|p, w, c| p.undo_run("last", true, w, c));
    wait(&mut harness, |p| {
        p.records_current()
            && p.workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .is_some_and(|l| l.undone)
    });
    harness.update(|p, _, _| {
        let line = p.workspace.turn_records.lines.latest.as_ref().unwrap();
        assert_eq!(line.files.len(), 1);
        assert_eq!(line.files[0].path, PathBuf::from("README.md"));
    });
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn stale_record_replies_cannot_cross_thread_revision_or_epoch() {
    if !isolated("undo::stale_record_replies_cannot_cross_thread_revision_or_epoch") {
        return;
    }
    let (scratch, repository, mut harness) = setup("undo-stale");
    let host = Host::start(|request| {
        assert_eq!(request["op"], "turn_records");
        std::thread::sleep(Duration::from_millis(25));
        Some(json!({"records":[record("wrong", &["last"], "ready")],"before":null}))
    });
    for change in 0..5 {
        prepare(&mut harness, &repository);
        harness.update(|p, w, c| {
            p.fetch_turn_records(w, c);
            match change {
                0 => {
                    p.host.select(Some("different".into()));
                }
                1 => {
                    Arc::make_mut(p.host.thread.snapshot.as_mut().unwrap()).records_revision =
                        Some(2);
                }
                2 => {
                    p.host.connection.epoch += 1;
                }
                3 => {
                    p.host.thread.watch_epoch += 1;
                }
                _ => {
                    p.workspace.turn_records.generation += 1;
                }
            }
        });
        std::thread::sleep(Duration::from_millis(60));
        harness.settle();
        harness.update(|p, _, _| {
            assert!(
                p.workspace
                    .turn_records
                    .data
                    .iter()
                    .all(|r| r.id != "wrong")
            )
        });
    }
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

pub(super) fn undo_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    prepare(harness, repository);
    capture_both(harness, &format!("undo-finished-{suffix}"));
    harness.draw();
    let position = harness
        .window
        .update(&mut harness.cx, |p, _, _| {
            p.text_point(
                &crate::ui::selection::Owner::Message("answer-first".into()),
                0,
                0,
            )
            .expect("earlier answer on screen")
        })
        .unwrap();
    harness.pointer(PlatformInput::MouseMove(MouseMoveEvent {
        position,
        pressed_button: None,
        modifiers: Modifiers::default(),
    }));
    harness.capture(&format!("undo-earlier-hover-{suffix}"));
    harness.update(|p, _, _| {
        let mut undo = record("undone", &["undo-request"], "ready");
        undo["kind"] = json!("undo");
        undo["undoes"] = json!("last");
        inject(
            p,
            vec![
                undo,
                record("last", &["last"], "undone"),
                record("first", &["first"], "ready"),
            ],
        );
    });
    harness.capture(&format!("undo-redo-{suffix}"));
    prepare(harness, repository);
    harness.update(|p, _, _| {
        p.workspace.turn_records.block(
            "last",
            serde_json::from_value(reply("blocked", "last")).unwrap(),
        )
    });
    harness.capture(&format!("undo-blocked-{suffix}"));
    harness.update(|p, _, _| {
        let mut record = record("last", &["last"], "ready");
        record["not_covered"] = json!(
            (0..10000)
                .map(|n| json!({"path":format!("excluded-{n:05}/"),"reason":"unreadable"}))
                .collect::<Vec<_>>()
        );
        record["not_covered_total"] = json!(10000);
        inject(p, vec![record]);
        p.workspace.turn_records.block(
            "last",
            serde_json::from_value(reply("blocked", "last")).unwrap(),
        );
    });
    harness.capture(&format!("undo-many-exclusions-{suffix}"));
    prepare(harness, repository);
    harness.update(|p, _, _| inject(p, vec![record("last", &["last"], "failed")]));
    harness.capture(&format!("undo-failed-{suffix}"));
    harness.update(|p, _, _| inject(p, vec![record("last", &["last"], "recording")]));
    harness.capture(&format!("undo-recording-{suffix}"));
    harness.update(|p, _, _| {
        let mut r = record("last", &["last"], "ready");
        r["after_restart"] = json!(true);
        inject(p, vec![r]);
    });
    harness.capture(&format!("undo-after-restart-{suffix}"));
    prepare(harness, repository);
    harness.update(|p, _, _| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 4;
        p.workspace.turn_records = Default::default();
    });
    harness.capture(&format!("undo-older-host-{suffix}"));
    harness.update(|p, _, _| {
        p.workspace.turn_records = Default::default();
    });
}
impl TurnRecords {
    pub fn inject(
        &mut self,
        records: Vec<TurnRecord>,
        snapshot: &host::Snapshot,
        epoch: u64,
        watch: u64,
    ) {
        self.lines = Arc::new(records::project(snapshot, &records, true));
        self.data = Arc::new(records);
        self.loaded = true;
        self.projected_generation = self.generation;
        self.data_generation = self.generation;
        self.stamp = Some(Stamp {
            thread: snapshot.id.clone(),
            epoch,
            watch,
            revision: snapshot.records_revision,
            generation: self.generation,
        });
    }
    pub fn block(&mut self, run: &str, reply: UndoTurnReply) {
        let coverage = self
            .lines
            .runs
            .get(run)
            .map(|line| line.not_covered.as_slice())
            .unwrap_or_default();
        self.blocked = Some(crate::ui::turn_records::Blocked::new(
            run.into(),
            reply,
            coverage,
        ));
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn blocked(&self) -> bool {
        self.blocked.is_some()
    }
    pub fn paths(&self) -> Vec<PathBuf> {
        self.data
            .iter()
            .flat_map(|r| r.paths.iter().map(|p| p.path.clone()))
            .collect()
    }
}

#[core::prelude::v1::test]
fn a_lost_undo_reply_reuses_the_same_request() {
    if !isolated("undo::a_lost_undo_reply_reuses_the_same_request") {
        return;
    }
    let (scratch, _, mut harness) = setup("undo-retry");
    let mut first = None;
    let host = Host::start(move |request| match request["op"].as_str().unwrap() {
        "undo_turn" => {
            if let Some(first) = &first {
                assert_eq!(request, first);
                Some(reply("undone", "applied"))
            } else {
                first = Some(request.clone());
                None
            }
        }
        "turn_records" => {
            let mut undo = record("applied", &["undo-request"], "ready");
            undo["kind"] = json!("undo");
            undo["undoes"] = json!("last");
            Some(json!({"records":[undo, record("last", &["last"], "undone")],"before":null}))
        }
        "history" => Some(history_reply()),
        op => panic!("unexpected {op}"),
    });
    harness.update(|p, w, c| p.undo_run("last", false, w, c));
    wait(&mut harness, |p| p.workspace.turn_records.retry.is_some());
    harness.update(|p, w, c| {
        assert!(p.shortcut_undo().is_none());
        p.undo_run("last", false, w, c);
    });
    wait(&mut harness, |p| {
        p.records_current()
            && p.workspace
                .turn_records
                .lines
                .latest
                .as_ref()
                .is_some_and(|l| l.undone)
    });
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn an_incomplete_undo_refreshes_without_replaying() {
    if !isolated("undo::an_incomplete_undo_refreshes_without_replaying") {
        return;
    }
    let (scratch, _, mut harness) = setup("undo-incomplete");
    let host = Host::start(|request| {
        Some(match request["op"].as_str().unwrap() {
            "undo_turn" => {
                json!({"ok":false,"error":{"code":"undo_incomplete","message":"Some files may have changed."}})
            }
            "turn_records" => json!({"records":[record("last", &["last"], "ready")],"before":null}),
            "history" => history_reply(),
            op => panic!("unexpected {op}"),
        })
    });
    harness.update(|p, w, c| p.undo_run("last", false, w, c));
    wait(&mut harness, |p| {
        p.workspace.turn_records.note.is_some() && p.records_current()
    });
    harness.update(|p, _, _| {
        let (_, label, detail) = p.workspace.turn_records.note.as_ref().unwrap();
        assert_eq!(*label, "Incomplete");
        assert!(detail.contains("some files may have changed"));
        assert!(p.workspace.turn_records.retry.is_none() && p.shortcut_undo().is_none());
    });
    harness.draw();
    assert_eq!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["op"] == "undo_turn")
            .count(),
        1
    );
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn a_blocked_earlier_run_remeasures_its_row() {
    if !isolated("undo::a_blocked_earlier_run_remeasures_its_row") {
        return;
    }
    let (scratch, _, mut harness) = setup("undo-measure");
    harness.change_snapshot(|s| {
        s.messages.extend((0..24).map(|i| Message {
            id: format!("later-{i}"),
            role: MessageRole::Assistant,
            text: "A later paragraph keeps the earlier run in the scrollable history.".into(),
            ..Default::default()
        }))
    });
    let host = Host::start(|request| {
        assert_eq!(request["op"], "undo_turn");
        Some(reply("blocked", "first"))
    });
    harness.update(|p, _, _| {
        p.workspace.live_scroll.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        })
    });
    harness.draw();
    // The disclosure and closing line share one measured list item.
    let closing_item = harness
        .window
        .update(&mut harness.cx, |p, _, _| {
            p.workspace
                .live_layout
                .folded_row_index("request-first")
                .unwrap()
        })
        .unwrap();
    let before = harness
        .window
        .update(&mut harness.cx, |p, _, _| {
            p.workspace
                .live_scroll
                .bounds_for_item(closing_item)
                .unwrap()
                .size
                .height
        })
        .unwrap();
    harness.update(|p, w, c| p.undo_run("first", false, w, c));
    wait(&mut harness, |p| p.workspace.turn_records.blocked());
    harness.draw();
    let after = harness
        .window
        .update(&mut harness.cx, |p, _, _| {
            p.workspace
                .live_scroll
                .bounds_for_item(closing_item)
                .unwrap()
                .size
                .height
        })
        .unwrap();
    assert!(
        after > before + px(100.),
        "the panel must remeasure its closing row"
    );
    harness.update(|p, _, c| p.cancel_undo_panel(c));
    harness.draw();
    harness.update(|p, _, _| {
        assert_eq!(
            p.workspace
                .live_scroll
                .bounds_for_item(closing_item)
                .unwrap()
                .size
                .height,
            before
        )
    });
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn undo_ack_survives_selection() {
    if !isolated("undo::undo_ack_survives_selection") {
        return;
    }
    let (scratch, _, mut harness) = setup("undo-owner");
    let host = Host::start(|request| {
        assert_eq!(request["op"], "undo_turn");
        std::thread::sleep(Duration::from_millis(25));
        Some(reply("blocked", "last"))
    });
    harness.update(|p, w, c| {
        p.undo_run("last", false, w, c);
        p.host.select(Some("other".into()));
        p.workspace.turn_records = Default::default();
    });
    wait(&mut harness, |p| {
        p.host
            .undo
            .get(&p.host.thread_key(THREAD))
            .is_some_and(|operation| !operation.sending)
    });
    harness.update(|p, _, c| {
        assert!(p.workspace.turn_records.blocked.is_none());
        assert!(p.workspace.turn_records.pending.is_none());
        assert!(p.host.undo.get(&p.host.thread_key("other")).is_none());
        p.host.select(Some(THREAD.into()));
        p.restore_undo_operation();
        assert!(p.workspace.turn_records.blocked.is_some());
        p.cancel_undo_panel(c);
        p.restore_undo_operation();
        assert!(p.workspace.turn_records.blocked.is_none());
    });
    assert_eq!(host.requests.lock().unwrap().len(), 1);
    drop(host);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}
