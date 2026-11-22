//! Scripted additive host replies exercise the desktop without launching an app.
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
fn host_defaults_and_grouping() {
    if !isolated("worktrees::host_defaults_and_grouping") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-ui-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let main = repository.canonicalize().unwrap();
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = store.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = requests.clone();
    let stopping = Arc::new(AtomicBool::new(false));
    let done = stopping.clone();
    let root = main.clone();
    let server = std::thread::spawn(move || {
        let mut snapshot = Value::Null;
        while !done.load(Ordering::Relaxed) {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            let Some(request) = scripted_request(&mut stream, Duration::from_secs(3)) else {
                continue;
            };
            let result = match request["op"].as_str().unwrap() {
                "worktree_info" => {
                    json!({"schema":1,"worktree_threads_v1":true,"parent":worktree_fixture::parent(&root)})
                }
                "create_worktree_thread" => {
                    let index = observed.lock().unwrap().len() + 1;
                    let id = format!("{index:08x}-1234-4abc-8abc-123456789abc");
                    let parent = worktree_fixture::parent(&root);
                    let tree = worktree_fixture::create(&parent, &id);
                    observed.lock().unwrap().push(request.clone());
                    snapshot = json!({"id":id,"checkout":tree.path,"model":{"provider":request["provider"],"model_id":request["model_id"]},"status":"idle","messages":[],"live_text":"","tools":[],"pending_approval":null,"error":null,"truncated":false,"harness":{"id":request["harness"],"version":null,"mode":request["mode"],"reported":null}});
                    json!({"schema":1,"operation":{"request_id":request["request_id"],"thread_id":id,"state":"ready","worktree":tree,"error":null}})
                }
                "watch" => json!({"thread":snapshot}),
                "list_worktrees" => json!({"schema":1,"operations":[]}),
                _ => Value::Null,
            };
            let reply = if result.is_null() {
                json!({"ok":false,"error":{"code":"invalid_request","message":"Scripted host does not handle this operation."}})
            } else {
                json!({"ok":true,"result":result})
            };
            let _ = writeln!(stream, "{reply}");
        }
    });
    for (index, chosen) in [HarnessId::Codex, HarnessId::Claude]
        .into_iter()
        .enumerate()
    {
        let mut harness = Harness::new(&repository, None, 900., 600.);
        harness.connect(&repository, false);
        harness.update(|plait, window, cx| {
            plait
                .host
                .connection
                .status
                .as_mut()
                .unwrap()
                .protocol_version = 6;
            plait.inject_harnesses(Some(claude_harnesses("ready", None)));
            plait.preferences.value.default_harness = chosen;
            plait.new_worktree_thread(window, cx);
            assert!(plait.worktree_busy);
            assert!(plait.creation_bar(cx).is_some());
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            harness.settle();
            let ready = harness
                .window
                .update(&mut harness.cx, |plait, _, _| !plait.worktree_busy)
                .unwrap();
            if ready {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "host worktree completion timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let request = requests.lock().unwrap()[index].clone();
        assert_eq!(
            request["harness"],
            if chosen == HarnessId::Codex {
                "codex"
            } else {
                "claude"
            }
        );
        assert_eq!(
            request["mode"],
            if chosen == HarnessId::Codex {
                "full_access"
            } else {
                "auto"
            }
        );
        harness.update(|plait, _, _| {
            assert!(plait.worktree_pending.is_none(), "{:?}", plait.notice);
            let summary = plait
                .host
                .connection
                .threads
                .iter()
                .find(|thread| thread.harness == Some(chosen))
                .unwrap();
            let tree = plait
                .preferences
                .value
                .navigation
                .worktree(&summary.checkout)
                .unwrap();
            assert_eq!(tree.main, main);
            assert!(tree.creation_id.is_some());
            assert!(tree.identity.is_some());
            assert!(summary.checkout.join(".git").is_file());
        });
    }
    stopping.store(true, Ordering::Relaxed);
    server.join().unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn stale_worktree_request_is_retried_only_from_its_checkout() {
    if !isolated("worktrees::stale_worktree_request_is_retried_only_from_its_checkout") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-stale-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        // An unconfirmed request from another project is not resent here.
        let host = plait.tabs.host.clone();
        plait.worktree_pending = Some(super::super::worktree::Pending::for_test(
            scratch.join("other"),
            host.clone(),
        ));
        plait.host.connection.connected = false;
        plait.new_worktree_thread(window, cx);
        assert!(plait.worktree_pending.is_none());
        assert!(!plait.worktree_busy);
        assert_eq!(
            plait.notice.as_deref(),
            Some("Connect to the host to start a thread in a worktree.")
        );
        // From its own checkout, the same request is retried.
        let checkout = plait.host.creation.checkout.clone();
        plait.worktree_pending = Some(super::super::worktree::Pending::for_test(checkout, host));
        plait.new_worktree_thread(window, cx);
        assert!(plait.worktree_busy);
    });
    harness.settle();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn worktree_controls_say_when_busy_or_when_the_reply_is_stale() {
    if !isolated("worktrees::worktree_controls_say_when_busy_or_when_the_reply_is_stale") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-busy-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait.worktree_busy = true;
        plait.new_worktree_thread(window, cx);
        assert!(
            plait
                .notice
                .as_deref()
                .unwrap()
                .contains("still being created or removed")
        );
        plait.worktree_busy = false;
        plait.inject_harnesses(Some(claude_harnesses("ready", None)));
        plait.new_worktree_thread(window, cx);
        assert!(plait.worktree_busy, "{:?}", plait.notice);
        // The connection changes before the (failing, socket-less) reply lands.
        plait.host.connection.epoch += 1;
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while harness
        .window
        .update(&mut harness.cx, |plait, _, _| plait.worktree_busy)
        .unwrap()
    {
        assert!(Instant::now() < deadline, "worktree reply timed out");
        harness.settle();
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.update(|plait, _, _| {
        assert!(
            plait
                .notice
                .as_deref()
                .unwrap()
                .contains("connection changed"),
            "{:?}",
            plait.notice
        );
    });
    std::fs::remove_dir_all(scratch).unwrap();
}
