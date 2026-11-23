//! A scripted host exercises choice, retry and late acknowledgement ownership.
use super::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[core::prelude::v1::test]
fn choice_flow() {
    if !isolated("admission::choice_flow") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-admission-ui-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let root = repository.canonicalize().unwrap();
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = store.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let main = root.clone();
    let server = std::thread::spawn(move || {
        let parent = worktree_fixture::parent(&main);
        let mut receipt = Value::Null;
        let mut snapshot = Value::Null;
        let mut lost = false;
        while !done.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            let Some(request) = scripted_request(&mut stream, Duration::from_secs(3)) else {
                continue;
            };
            seen.lock().unwrap().push(request.clone());
            let result = match request["op"].as_str().unwrap() {
                "admission_info" => {
                    json!({"schema":1,"worktree_at_start_v1":true,"linked_continuation_v1":true,"queue":false,"strict_reader":false,"checkout":main,"identity":parent.identity,"project":main,"project_identity":parent.main_identity,"busy":true,"auto_worktree":false,"parent":parent,"refusal":null})
                }
                "create_admitted_thread" if request["placement"] == "project" => {
                    json!({"schema":1,"admission":{"request_id":request["request_id"],"state":"choice","thread_id":null,"checkout":main,"worktree":null,"source_thread_id":null,"error":null}})
                }
                "create_admitted_thread" => {
                    if receipt.is_null() {
                        let id = "12345678-1234-4abc-8abc-123456789abc";
                        let tree = worktree_fixture::create(&parent, id);
                        snapshot = json!({"id":id,"checkout":tree.path,"model":{"provider":request["provider"],"model_id":request["model_id"]},"status":"idle","messages":[],"live_text":"","tools":[],"pending_approval":null,"error":null,"truncated":false,"harness":{"id":request["harness"],"mode":request["mode"],"version":null,"reported":null}});
                        receipt = json!({"schema":1,"admission":{"request_id":request["request_id"],"state":"ready","thread_id":id,"checkout":tree.path,"worktree":tree,"source_thread_id":null,"error":null}});
                    }
                    if !lost {
                        lost = true;
                        continue;
                    }
                    receipt.clone()
                }
                "watch" => json!({"thread":snapshot}),
                "list_worktrees" => json!({"schema":1,"operations":[]}),
                "submit" => {
                    json!({"thread_id":request["thread_id"],"accepted":true,"request_id":request["request_id"]})
                }
                _ => Value::Null,
            };
            let reply = if result.is_null() {
                json!({"ok":false,"error":{"code":"invalid_request","message":"not supported"}})
            } else {
                json!({"ok":true,"result":result})
            };
            let _ = writeln!(stream, "{reply}");
        }
    });
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|p, w, cx| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 6;
        p.inject_harnesses(Some(claude_harnesses("ready", None)));
        p.preferences.value.default_harness = HarnessId::Claude;
        p.open_draft(w, cx);
        let composer = p.draft.composer.clone().unwrap();
        composer.update(cx, |composer, cx| {
            composer.restore_draft("Keep my first message".into(), 3, None, cx)
        });
        p.send_draft("Keep my first message".into(), 3, w, cx);
    });
    wait(&mut harness, |p| p.draft_isolation_choice());
    harness.update(|p, _, cx| {
        assert!(p.isolation_choice(cx).is_some());
        assert!(p.draft.sending.is_some());
        assert!(p.creation_bar(cx).is_none());
        p.draft.open = false;
        assert!(p.creation_bar(cx).is_some());
        p.draft.open = true;
    });
    #[cfg(debug_assertions)]
    for _ in 0..2 {
        let frame = harness.draw_a11y();
        frame.assert_no_duplicates("admission busy choice");
        for label in ["Own worktree", "Wait", "Cancel"] {
            assert!(
                frame.has_node(gpui::Role::Button, |node| node.label.as_deref()
                    == Some(label))
            );
        }
        assert!(
            frame.has_node(gpui::Role::Label, |node| node.value.as_deref()
                == Some("Project busy"))
        );
    }
    harness.update(|p, w, cx| p.accept_isolation(w, cx));
    wait(&mut harness, |p| !p.draft_creating());
    harness.update(|p, w, cx| {
        assert!(p.draft.pending.is_some());
        p.retry_draft(w, cx);
        // The person moves away while the retried acknowledgement is in flight.
        p.select_host_thread(THREAD.into(), w, cx);
    });
    wait(&mut harness, |p| {
        p.host
            .connection
            .threads()
            .iter()
            .any(|t| t.id == "12345678-1234-4abc-8abc-123456789abc")
    });
    harness.update(|p,_,_|{
        assert!(!p.draft_visible());
        let created=p.host.connection.threads().iter().find(|t|t.id=="12345678-1234-4abc-8abc-123456789abc").unwrap();
        assert_eq!(created.harness,Some(HarnessId::Claude));assert_ne!(created.checkout,root);
        assert_ne!(p.host.thread.selected.as_deref(),Some(created.id.as_str()));
        let tab=p.tabs.model.tabs().iter().find(|tab|matches!(tab.session().target(),plait_client::tabs::TabTarget::Existing{id,..} if id==&created.id)).unwrap();
        assert_eq!(tab.session().target().checkout().root,created.checkout);

        assert!(p.notice.as_deref().unwrap().contains("uncommitted files"));
        assert!(!p.workspace.composers.contains_key(&created.id));
    });
    let seen = requests.lock().unwrap();
    let creates: Vec<_> = seen
        .iter()
        .filter(|r| r["op"] == "create_admitted_thread" && r["placement"] == "worktree")
        .collect();
    assert_eq!(creates.len(), 2);
    assert_eq!(creates[0], creates[1]);
    assert_eq!(creates[0]["harness"], "claude");
    drop(seen);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}
fn wait(h: &mut Harness, condition: impl Fn(&Plait) -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    let executor = h.cx.update(|cx| cx.background_executor().clone());
    loop {
        // Advancing can run foreground callbacks, so do it outside an App borrow.
        executor.advance_clock(Duration::from_millis(150));
        h.settle();
        if h.window.update(&mut h.cx, |p, _, _| condition(p)).unwrap() {
            return;
        }
        assert!(Instant::now() < end, "admission did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[core::prelude::v1::test]
fn concurrent_drafts() {
    if !isolated("admission::concurrent_drafts") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-two-admissions-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let root = repository.canonicalize().unwrap();
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = store.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = requests.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let main = root.clone();
    let server = std::thread::spawn(move || {
        let parent = worktree_fixture::parent(&main);
        let mut first = String::new();
        let mut tree = None;
        let mut ready = false;
        let mut snapshots = std::collections::HashMap::new();
        let mut watches = Vec::new();
        while !done.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            let Some(request) = scripted_request(&mut stream, Duration::from_secs(3)) else {
                continue;
            };
            seen.lock().unwrap().push(request.clone());
            let receipt = |request_id: &str,
                           state: &str,
                           id: Option<&str>,
                           checkout: &Path,
                           worktree: Value| json!({"schema":1,"admission":{"request_id":request_id,"state":state,"thread_id":id,"checkout":checkout,"worktree":worktree,"source_thread_id":null,"error":null}});
            let result = match request["op"].as_str().unwrap() {
                "admission_info" => {
                    json!({"schema":1,"worktree_at_start_v1":true,"linked_continuation_v1":true,"queue":false,"strict_reader":false,"checkout":main,"identity":parent.identity,"project":main,"project_identity":parent.main_identity,"busy":true,"auto_worktree":false,"parent":parent,"refusal":null})
                }
                "create_admitted_thread" => {
                    let request_id = request["request_id"].as_str().unwrap();
                    if first.is_empty() {
                        first = request_id.into();
                    }
                    if request_id == first && request["placement"] == "project" {
                        receipt(request_id, "choice", None, &main, Value::Null)
                    } else {
                        let id = if request_id == first {
                            "aaaaaaaa-1234-4abc-8abc-123456789abc"
                        } else {
                            "bbbbbbbb-1234-4abc-8abc-123456789abc"
                        };
                        if request_id == first && tree.is_none() {
                            tree = Some(worktree_fixture::create(&parent, id));
                        }
                        let checkout = if request_id == first {
                            tree.as_ref().unwrap().path.clone()
                        } else {
                            main.clone()
                        };
                        snapshots.insert(id.to_owned(),json!({"id":id,"checkout":checkout,"model":{"provider":request["provider"],"model_id":request["model_id"]},"status":"idle","messages":[],"live_text":"","tools":[],"pending_approval":null,"error":null,"truncated":false}));
                        if request_id == first {
                            receipt(
                                request_id,
                                "preparing",
                                Some(id),
                                &checkout,
                                serde_json::to_value(tree.as_ref().unwrap()).unwrap(),
                            )
                        } else {
                            receipt(request_id, "ready", Some(id), &checkout, Value::Null)
                        }
                    }
                }
                "admission_status" => {
                    assert_eq!(request["request_id"], first);
                    let tree = tree.as_ref().unwrap();
                    receipt(
                        &first,
                        if ready { "ready" } else { "preparing" },
                        Some(&tree.thread_id),
                        &tree.path,
                        serde_json::to_value(tree).unwrap(),
                    )
                }
                "watch" => json!({"thread":snapshots[request["thread_id"].as_str().unwrap()]}),
                "submit" => {
                    if request["thread_id"] == "bbbbbbbb-1234-4abc-8abc-123456789abc" {
                        assert!(!ready);
                        ready = true;
                    }
                    json!({"thread_id":request["thread_id"],"accepted":true,"request_id":request["request_id"]})
                }
                "list_worktrees" => json!({"schema":1,"operations":[]}),
                _ => Value::Null,
            };
            let reply = if result.is_null() {
                json!({"ok":false,"error":{"code":"invalid_request","message":"not supported"}})
            } else {
                json!({"ok":true,"result":result})
            };
            let _ = writeln!(stream, "{reply}");
            if request["op"] == "watch" {
                watches.push(stream);
            }
        }
    });
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    h.update(|p, w, cx| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 6;
        p.open_draft(w, cx);
        p.draft.composer.clone().unwrap().update(cx, |c, cx| {
            c.restore_draft("First in a worktree".into(), 1, None, cx)
        });
        p.send_draft("First in a worktree".into(), 1, w, cx);
    });
    wait(&mut h, |p| p.draft_isolation_choice());
    h.update(|p, w, cx| {
        p.draft.composer.clone().unwrap().update(cx, |c, cx| {
            c.restore_draft("First edited for worktree".into(), 2, None, cx)
        });
        p.accept_isolation(w, cx);
        assert!(p.draft_creating());
        p.open_draft(w, cx);
        assert!(!p.draft_pending());
        p.draft.composer.clone().unwrap().update(cx, |c, cx| {
            c.restore_draft("Second while preparing".into(), 1, None, cx)
        });
        p.send_draft("Second while preparing".into(), 1, w, cx);
    });
    wait(&mut h, |_| {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["op"] == "submit")
            .count()
            == 2
    });
    wait(&mut h, |p| {
        p.tabs
            .model
            .tabs()
            .iter()
            .filter(|t| !t.session().target().is_draft())
            .count()
            == 3
    });
    h.update(|p, _, _| {
        assert_eq!(
            p.host.thread.selected.as_deref(),
            Some("bbbbbbbb-1234-4abc-8abc-123456789abc")
        );
        assert!(
            !p.tabs
                .lifecycle
                .drafts
                .values()
                .any(|d| d.pending.is_some())
        );
    });
    let requests = requests.lock().unwrap();
    let submits: Vec<_> = requests.iter().filter(|r| r["op"] == "submit").collect();
    assert_eq!(submits.len(), 2);
    assert_ne!(submits[0]["request_id"], submits[1]["request_id"]);
    assert_eq!(submits[0]["text"], "Second while preparing");
    assert_eq!(submits[1]["text"], "First edited for worktree");
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["op"] == "create_admitted_thread" && r["placement"] == "worktree")
            .count(),
        1
    );
    drop(requests);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A host that answers a busy choice, then a ready worktree, and records every request.
fn scripted_busy_host(
    root: PathBuf,
    listener: UnixListener,
    seen: Arc<Mutex<Vec<Value>>>,
    done: Arc<AtomicBool>,
    mut lose_first: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let parent = worktree_fixture::parent(&root);
        let mut receipt = Value::Null;
        let mut snapshot = Value::Null;
        while !done.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            let Some(request) = scripted_request(&mut stream, Duration::from_secs(10)) else {
                continue;
            };
            seen.lock().unwrap().push(request.clone());
            let result = match request["op"].as_str().unwrap() {
                "admission_info" => {
                    json!({"schema":1,"worktree_at_start_v1":true,"linked_continuation_v1":true,"queue":false,"strict_reader":false,"checkout":root,"identity":parent.identity,"project":root,"project_identity":parent.main_identity,"busy":true,"auto_worktree":false,"parent":parent,"refusal":null})
                }
                "create_admitted_thread" if request["placement"] == "project" => {
                    json!({"schema":1,"admission":{"request_id":request["request_id"],"state":"choice","thread_id":null,"checkout":root,"worktree":null,"source_thread_id":null,"error":null}})
                }
                "create_admitted_thread" => {
                    if receipt.is_null() {
                        let id = "22345678-1234-4abc-8abc-123456789abc";
                        let tree = worktree_fixture::create(&parent, id);
                        snapshot = json!({"id":id,"checkout":tree.path,"model":{"provider":request["provider"],"model_id":request["model_id"]},"status":"idle","messages":[],"live_text":"","tools":[],"pending_approval":null,"error":null,"truncated":false,"harness":{"id":request["harness"],"mode":request["mode"],"version":null,"reported":null}});
                        receipt = json!({"schema":1,"admission":{"request_id":request["request_id"],"state":"ready","thread_id":id,"checkout":tree.path,"worktree":tree,"source_thread_id":null,"error":null}});
                    }
                    if std::mem::take(&mut lose_first) {
                        continue;
                    }
                    receipt.clone()
                }
                "watch" => json!({"thread":snapshot}),
                "list_worktrees" => json!({"schema":1,"operations":[]}),
                "submit" => {
                    json!({"thread_id":request["thread_id"],"accepted":true,"request_id":request["request_id"]})
                }
                _ => Value::Null,
            };
            let reply = if result.is_null() {
                json!({"ok":false,"error":{"code":"invalid_request","message":"not supported"}})
            } else {
                json!({"ok":true,"result":result})
            };
            let _ = writeln!(stream, "{reply}");
        }
    })
}

/// The message sent to the new worktree is the one on screen when Own worktree is chosen.
#[core::prelude::v1::test]
fn edited_choice_draft() {
    if !isolated("admission::edited_choice_draft") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-admission-edit-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let root = repository.canonicalize().unwrap();
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = store.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let server = scripted_busy_host(
        root.clone(),
        listener,
        requests.clone(),
        stop.clone(),
        false,
    );
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|p, w, cx| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 6;
        p.open_draft(w, cx);
        let composer = p.draft.composer.clone().unwrap();
        composer.update(cx, |composer, cx| {
            composer.restore_draft("First wording".into(), 3, None, cx)
        });
        p.send_draft("First wording".into(), 3, w, cx);
    });
    wait(&mut harness, |p| p.draft_isolation_choice());
    harness.update(|p, w, cx| {
        // The composer stays editable while the person decides.
        let composer = p.draft.composer.clone().unwrap();
        composer.update(cx, |composer, cx| {
            composer.restore_draft("Edited wording".into(), 4, None, cx)
        });
        p.accept_isolation(w, cx);
    });
    wait(&mut harness, |p| !p.draft_pending());
    let end = Instant::now() + Duration::from_secs(10);
    let submitted = loop {
        harness.settle();
        if let Some(submit) = requests
            .lock()
            .unwrap()
            .iter()
            .find(|r| r["op"] == "submit")
        {
            break submit.clone();
        }
        assert!(Instant::now() < end, "no submit");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(submitted["text"], "Edited wording");
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A window closed while its worktree thread was being made leaves the request
/// with the saved draft; sending it again reaches that thread, not a second one.
#[core::prelude::v1::test]
fn reopened_creation() {
    if !isolated("admission::reopened_creation") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-admission-reopen-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let root = repository.canonicalize().unwrap();
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = store.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let server = scripted_busy_host(root.clone(), listener, requests.clone(), stop.clone(), true);
    let mut saved = None;
    {
        let mut harness = Harness::new(&repository, None, 900., 600.);
        harness.connect(&repository, false);
        harness.update(|p, w, cx| {
            p.host.connection.status.as_mut().unwrap().protocol_version = 6;
            p.open_draft(w, cx);
            let composer = p.draft.composer.clone().unwrap();
            composer.update(cx, |composer, cx| {
                composer.restore_draft("Only once".into(), 3, None, cx)
            });
            p.send_draft("Only once".into(), 3, w, cx);
        });
        wait(&mut harness, |p| p.draft_isolation_choice());
        harness.update(|p, w, cx| p.accept_isolation(w, cx));
        // The worktree exists on the host; its reply never arrives.
        wait(&mut harness, |p| !p.draft_creating());
        harness.update(|p, _, cx| {
            p.capture_session(cx);
            saved = p.session.data.clone();
        });
    }
    let saved = saved.unwrap();
    let draft = saved.new_draft_for(&root).unwrap();
    assert_eq!(draft.text, "Only once");
    assert!(draft.creation.is_some());
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|p, w, cx| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 6;
        let (id, tab) = saved.draft_tabs.iter().next().unwrap();
        let target = plait_client::tabs::TabTarget::Draft {
            id: id.clone(),
            checkout: p.tab_checkout(tab.checkout.clone()),
        };
        p.session.data = Some(saved);
        p.open_tab_target(target, w, cx);
        assert!(p.draft_pending());
        p.retry_draft(w, cx);
    });
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        harness.settle();
        if requests.lock().unwrap().iter().any(|r| r["op"] == "submit") {
            break;
        }
        assert!(Instant::now() < end, "no submit");
        std::thread::sleep(Duration::from_millis(5));
    }
    let seen = requests.lock().unwrap();
    let creates: Vec<_> = seen
        .iter()
        .filter(|r| r["op"] == "create_admitted_thread" && r["placement"] == "worktree")
        .collect();
    assert_eq!(creates.len(), 2);
    assert_eq!(creates[0], creates[1]);
    let submits: Vec<_> = seen.iter().filter(|r| r["op"] == "submit").collect();
    assert_eq!(submits.len(), 1);
    assert_eq!(submits[0]["text"], "Only once");
    assert_eq!(
        submits[0]["thread_id"],
        "22345678-1234-4abc-8abc-123456789abc"
    );
    drop(seen);
    drop(harness);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A failed capability probe is forgotten, so continuation returns once the
/// checkout's hold is resolved; a fresh answer in between is kept.
#[core::prelude::v1::test]
fn failed_capability_probe_is_retried() {
    if !isolated("admission::failed_capability_probe_is_retried") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-admission-retry-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|p, _, cx| {
        let epoch = p.host.connection.epoch;
        let root = p.host.creation.checkout.clone();
        p.admission.capabilities = Some((epoch, root.clone(), false));
        p.retry_admission_capability(epoch, root, Duration::ZERO, cx);
    });
    wait(&mut harness, |p| p.admission.capabilities.is_none());
    harness.update(|p, _, cx| {
        let epoch = p.host.connection.epoch;
        let root = p.host.creation.checkout.clone();
        p.admission.capabilities = Some((epoch, root.clone(), false));
        p.retry_admission_capability(epoch, root.clone(), Duration::from_millis(50), cx);
        p.admission.capabilities = Some((epoch, root, true));
    });
    std::thread::sleep(Duration::from_millis(100));
    harness.settle();
    harness.update(|p, _, _| assert!(p.admission.capabilities.as_ref().is_some_and(|c| c.2)));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}
