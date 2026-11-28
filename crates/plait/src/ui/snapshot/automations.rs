use super::*;
use plait_client::{
    automation_form::{POLICY_UNAVAILABLE_ERROR, TargetKind, today_date_in_time_zone},
    automations::AutomationItem,
    host::{
        ThinkingLevel,
        automations::{
            LastResult, OccurrenceOutcome, Schedule, State, Summary, automation_id_for_request,
        },
        harness::ThreadHarness,
        policy::CommandPolicy as HostCommandPolicy,
    },
    machines::MachineId,
    session::Journal,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn sample_automations(repository: &Path) -> Vec<AutomationItem> {
    let now = now_ms();
    let mut active = AutomationItem::from_summary(Summary {
        id: "auto-active".into(),
        name: "Daily dependency check".into(),
        schedule: Schedule::Weekdays {
            time: "09:00".into(),
            time_zone: "America/New_York".into(),
        },
        target_path: repository.to_path_buf(),
        state: State::Active,
        next_run: Some(now + 3 * 3_600_000),
        last_result: Some(LastResult {
            due: now - 15 * 60_000,
            outcome: OccurrenceOutcome::Queued,
            detail: None,
        }),
        pause_reason: None,
        error: None,
        thread_id: Some(THREAD.into()),
    });
    active.thread_id = Some(THREAD.into());

    let mut paused = AutomationItem::from_summary(Summary {
        id: "auto-paused".into(),
        name: "Nightly test sweep".into(),
        schedule: Schedule::Daily {
            time: "22:00".into(),
            time_zone: "UTC".into(),
        },
        target_path: repository.to_path_buf(),
        state: State::Paused,
        next_run: None,
        last_result: Some(LastResult {
            due: now - 3 * 3_600_000,
            outcome: OccurrenceOutcome::Paused,
            detail: Some("target missing".into()),
        }),
        pause_reason: None,
        error: None,
        thread_id: Some("thr_0c44a9d1".into()),
    });
    paused.thread_id = Some("thr_0c44a9d1".into());
    paused.pause_reason = Some("target missing".into());

    let preparing = AutomationItem::from_summary(Summary {
        id: "auto-preparing".into(),
        name: "Weekly changelog".into(),
        schedule: Schedule::Weekly {
            days: vec![1],
            time: "10:00".into(),
            time_zone: "UTC".into(),
        },
        target_path: repository.to_path_buf(),
        state: State::Preparing,
        next_run: None,
        last_result: None,
        pause_reason: None,
        error: None,
        thread_id: None,
    });

    let mut failed = AutomationItem::from_summary(Summary {
        id: "auto-failed".into(),
        name: "Docs link check".into(),
        schedule: Schedule::Hourly {
            minute: 15,
            time_zone: "UTC".into(),
        },
        target_path: repository.join("docs"),
        state: State::Failed,
        next_run: None,
        last_result: None,
        pause_reason: None,
        error: None,
        thread_id: None,
    });
    failed.error = Some("This folder is inside a Git project. Choose the project instead.".into());

    vec![active, paused, preparing, failed]
}

pub(super) fn scenes(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut h = Harness::with_host(repository, output, width, height, None);
    h.connect(repository, false);
    h.update(|p, _, cx| {
        p.sidebar = true;
        p.automations
            .set_for_test(true, sample_automations(repository));
        p.rebuild_projects(cx);
        p.navigation.invalidate_rows();
    });
    capture_both(&mut h, &format!("automations-{suffix}"));

    // Empty automation creation form (dark and light, wide and narrow).
    h.update(|p, w, cx| {
        p.open_automation_form(w, cx);
        if let Some(form) = &mut p.automation_form {
            form.command_policy = Some(plait_client::host::policy::CommandPolicy::Sandboxed);
            form.time_zone
                .update(cx, |field, cx| field.set_text("America/New_York", cx));
            form.once_date
                .update(cx, |field, cx| field.set_text("2026-06-01", cx));
        }
    });
    capture_both(&mut h, &format!("automation-form-empty-{suffix}"));

    // Form prefilled from the open thread (dark and light, wide and narrow).
    h.change_snapshot(|snapshot| {
        if let Some(last_user) = snapshot
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == MessageRole::User)
        {
            last_user.text = "Audit Cargo.toml for outdated dependencies and summarize any security advisories.\nDo not edit files unless a patch version is backward compatible.".into();
        }
    });
    h.update(|p, w, cx| {
        p.close_automation_form(false, w, cx);
        p.open_automation_form_from_thread(THREAD.into(), w, cx);
        if let Some(form) = &mut p.automation_form {
            form.command_policy = Some(plait_client::host::policy::CommandPolicy::Sandboxed);
            form.time_zone
                .update(cx, |field, cx| field.set_text("America/New_York", cx));
            form.once_date
                .update(cx, |field, cx| field.set_text("2026-06-01", cx));
        }
    });
    capture_both(&mut h, &format!("automation-form-prefilled-{suffix}"));
}

#[core::prelude::v1::test]
fn automations_sidebar_and_thread_header_draw_cleanly() {
    if !isolated("automations::automations_sidebar_and_thread_header_draw_cleanly") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-automations-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::with_host(&repository, None, 1380., 860., None);
    h.connect(&repository, false);
    h.update(|p, _, cx| {
        p.sidebar = true;
        p.rebuild_projects(cx);
    });
    h.draw();

    // Older hosts hide the Automations section completely.
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-hidden");
        assert!(!frame.has_node(Role::TreeItem, |node| {
            node.label
                .as_deref()
                .is_some_and(|label| label.starts_with("Automations section"))
        }));
        assert!(!frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Run now")
        }));
    }

    // Supported hosts show the section, all four states, and the selected thread's header.
    let items = sample_automations(&repository);
    h.update(|p, _, _| {
        p.automations.set_for_test(true, items);
        p.navigation.invalidate_rows();
    });
    h.draw();

    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-visible");
        assert!(frame.has_node(Role::TreeItem, |node| {
            node.label
                .as_deref()
                .is_some_and(|label| label.starts_with("Automations section"))
        }));
        for expected in [
            "Daily dependency check · Active",
            "Nightly test sweep · Paused. Daily at 22:00. target missing",
            "Weekly changelog · Preparing",
            "Docs link check · Failed",
        ] {
            assert!(
                frame.has_node(Role::TreeItem, |node| {
                    node.label
                        .as_deref()
                        .is_some_and(|label| label.contains(expected))
                }),
                "missing automation card: {expected}"
            );
        }
        for label in ["Run now", "Pause", "Delete"] {
            assert!(
                frame.has_node(Role::Button, |node| node.label.as_deref() == Some(label)),
                "missing automation header control: {label}"
            );
        }
    }

    h.update(|p, _, _| {
        p.automations.set_confirm_delete(Some("auto-failed".into()));
        p.automations
            .set_note("auto-failed".into(), "Delete refused".into(), true);
        p.navigation.invalidate_rows();
    });
    h.draw();
    #[cfg(debug_assertions)]
    assert!(
        h.draw_a11y()
            .has_node(Role::Label, |node| node.value.as_deref()
                == Some("Delete refused"))
    );

    // Delete asks for confirmation; Escape cancels without deleting.
    h.update(|p, _, _| {
        p.automations.set_confirm_delete(Some("auto-active".into()));
    });
    h.draw();
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-delete-confirm");
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Delete automation")
        }));
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Cancel")
        }));
    }

    h.keystroke("escape");
    h.update(|p, _, _| {
        assert!(p.automations.confirm_delete.is_none());
        assert_eq!(p.automations.items.len(), 4);
    });

    // Deleting an automation leaves its thread selected as an ordinary thread.
    h.update(|p, _, _| {
        let epoch = p.automations.epoch;
        assert!(p.automations.apply_deleted(epoch, "auto-active"));
        p.navigation.invalidate_rows();
    });
    h.draw();
    h.update(|p, _, _| {
        assert_eq!(p.host.thread.selected.as_deref(), Some(THREAD));
        assert!(p.automations.by_thread(THREAD).is_none());
    });
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-after-delete");
        assert!(!frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Run now")
        }));
    }

    drop(h);
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        scenes(&repository, output.as_deref(), suffix, width, height);
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn automation_form_entry_points_validation_and_prefill() {
    if !isolated("automations::automation_form_entry_points_validation_and_prefill") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-autoform-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::with_host(&repository, None, 1380., 860., None);
    h.connect(&repository, false);
    h.update(|p, w, cx| {
        p.sidebar = true;
        p.rebuild_projects(cx);
        // When automations are unsupported, entry points stay hidden and opening is a no-op.
        let items = p.command_items(w, cx);
        assert!(
            !items
                .iter()
                .any(|item| item.title.as_ref() == "New automation…")
        );
        assert!(
            !items
                .iter()
                .any(|item| item.title.as_ref() == "Make automation from this thread")
        );
        p.open_automation_form(w, cx);
        assert!(p.automation_form.is_none());
        p.open_automation_form_from_thread(THREAD.into(), w, cx);
        assert!(p.automation_form.is_none());
        p.open_thread_menu(THREAD.into(), w, cx);
    });
    h.draw();
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        assert!(!frame.has_node(Role::TreeItem, |node| {
            node.label.as_deref() == Some("New automation…")
        }));
        assert!(!frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Make automation from this thread")
        }));
    }

    // Supported host exposes sidebar row, palette entries, and thread menu item.
    h.update(|p, w, cx| {
        p.navigation.menu = None;
        p.automations.set_for_test(true, Vec::new());
        p.navigation.invalidate_rows();
        let items = p.command_items(w, cx);
        assert!(
            items
                .iter()
                .any(|item| item.title.as_ref() == "New automation…")
        );
        assert!(
            items.iter().any(
                |item| item.title.as_ref() == "Make automation from this thread" && item.enabled
            )
        );
        p.open_thread_menu(THREAD.into(), w, cx);
    });
    h.draw();
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-entry-points");
        assert!(frame.has_node(Role::TreeItem, |node| {
            node.label.as_deref() == Some("New automation…")
        }));
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Make automation from this thread")
        }));
    }

    // Empty form opens, validates inline on submit, and closes on Escape.
    h.update(|p, w, cx| {
        p.open_automation_form(w, cx);
        assert!(p.automation_form.is_some());
    });
    h.draw();
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-form-empty");
        assert!(frame.has_node(Role::Dialog, |node| {
            node.label.as_deref() == Some("New automation")
        }));
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Create")
        }));
    }

    h.update(|p, w, cx| {
        p.submit_automation_form(w, cx);
        assert!(p.automation_form.as_ref().is_some_and(|f| f.submitted_once));
        assert!(p.automation_form.as_ref().is_some_and(|f| !f.submitting));
    });
    h.draw();
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-form-validation-errors");
        assert!(frame.has_node(Role::Alert, |_| true));
    }

    h.keystroke("escape");
    h.update(|p, _, _| {
        assert!(p.automation_form.is_none());
    });

    // Make automation from this thread prefills project, harness, model, and last user message.
    h.update(|p, w, cx| {
        p.open_automation_form_from_thread(THREAD.into(), w, cx);
        let form = p.automation_form.as_ref().expect("prefilled form open");
        assert_eq!(form.project_checkout, repository);
        assert_eq!(form.harness, HarnessId::Pi);
        assert_eq!(form.model, Some(choice()));
        assert_eq!(form.caller_thread_id.as_deref(), Some(THREAD));
        assert_eq!(form.instructions.read(cx).saved_draft().0, "Go ahead.");
    });
    h.draw();
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-form-prefilled");
        assert!(frame.has_node(Role::Dialog, |node| {
            node.label.as_deref() == Some("Make automation from thread")
        }));
    }

    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

struct ScriptedAutomationHost {
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
}

impl ScriptedAutomationHost {
    fn start(mut handle: impl FnMut(&Value) -> Value + Send + 'static) -> Self {
        let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
        std::fs::create_dir_all(&store).unwrap();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = store.join("host.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        listener.set_nonblocking(true).unwrap();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let server = std::thread::spawn(move || {
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
                seen.lock().unwrap().push(request.clone());
                let reply = handle(&request);
                let _ = writeln!(stream, "{reply}");
            }
        });
        Self {
            requests,
            stop,
            server: Some(server),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for ScriptedAutomationHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn wait_until(h: &mut Harness, condition: impl Fn(&Plait, &App) -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        h.settle();
        if h.window
            .update(&mut h.cx, |p, _, cx| condition(p, cx))
            .unwrap()
        {
            return;
        }
        assert!(Instant::now() < end, "automation form did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn attach_test_journal(h: &mut Harness) -> Arc<Journal> {
    let state_dir = PathBuf::from(std::env::var_os("PLAIT_DESKTOP_STATE_DIR").unwrap());
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    let (journal, _) = Journal::open(&state_dir, store).unwrap();
    let attached = journal.clone();
    h.update(|p, _, _| {
        p.session.journal = Some(attached);
    });
    journal
}

fn open_filled_form(
    h: &mut Harness,
    name: &str,
    instructions: &str,
    policy: Option<HostCommandPolicy>,
) -> String {
    let mut bound_id = String::new();
    h.update(|p, w, cx| {
        p.open_automation_form(w, cx);
        let form = p.automation_form.as_mut().expect("form open");
        form.name.update(cx, |field, cx| field.set_text(name, cx));
        form.instructions.update(cx, |editor, cx| {
            editor.restore_draft(instructions.into(), 0, None, cx);
        });
        form.command_policy = policy;
        bound_id = form.request_id.clone();
    });
    bound_id
}

#[core::prelude::v1::test]
fn automation_form_reconnect_clears_submitting_and_allows_dismiss() {
    if !isolated("automations::automation_form_reconnect_clears_submitting_and_allows_dismiss") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-autoform-reconnect-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let host = ScriptedAutomationHost::start(move |request| match request["op"].as_str() {
        // An error with no outcome, arriving after a reconnect: unconfirmed.
        Some("create_automation") => json!({
            "ok": false,
            "error": {"code": "internal", "message": "Connection lost."}
        }),
        Some("list_automations") => json!({"ok": true, "result": {"automations": []}}),
        _ => json!({"ok": false, "error": {"code": "invalid_request", "message": "unused"}}),
    });

    let mut h = Harness::new(&repository, None, 1380., 860.);
    h.connect(&repository, false);
    let journal = attach_test_journal(&mut h);
    h.update(|p, _, cx| {
        p.automations.set_for_test(true, Vec::new());
        p.rebuild_projects(cx);
    });

    let req_id = open_filled_form(
        &mut h,
        "Daily audit",
        "Check Cargo.toml",
        Some(HostCommandPolicy::Sandboxed),
    );
    h.window
        .update(&mut h.cx, |p, w, cx| {
            p.submit_automation_form(w, cx);
            assert!(p.automation_form.as_ref().is_some_and(|f| f.submitting));
        })
        .unwrap();
    // Stop after the host receives Create, before GPUI applies its reply.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !host
        .requests()
        .iter()
        .any(|r| r["op"] == "create_automation")
    {
        assert!(Instant::now() < deadline, "expected create_automation");
        h.cx.background_executor().tick();
    }
    h.draw();
    h.cx.update_window(h.window.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
    })
    .unwrap();
    h.update(|p, _, _| {
        assert!(p.automation_form.as_ref().is_some_and(|f| f.submitting));
        p.host.connection.epoch += 1;
    });

    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    h.update(|p, _, _| {
        let form = p.automation_form.as_ref().expect("form stays open");
        assert!(!form.submitting);
        assert_eq!(
            form.submit_error.as_ref().map(|(m, u)| (m.as_str(), *u)),
            Some((
                "Plait couldn't confirm this automation was created. Retry is safe.",
                true
            ))
        );
        assert_eq!(
            form.unconfirmed_attempt
                .as_ref()
                .map(|a| a.request_id.as_str()),
            Some(req_id.as_str())
        );
    });
    assert!(
        journal
            .automation_create(&MachineId::default(), &req_id)
            .unwrap()
            .is_some()
    );

    // Nothing is in flight now: click-outside, Escape, and Close must all work.
    super::settings_modal::click(&mut h, 12., 12.);
    h.update(|p, _, _| assert!(p.automation_form.is_none()));

    open_filled_form(
        &mut h,
        "Daily audit",
        "Check Cargo.toml",
        Some(HostCommandPolicy::Sandboxed),
    );
    h.draw();
    h.keystroke("escape");
    h.update(|p, _, _| assert!(p.automation_form.is_none()));

    open_filled_form(
        &mut h,
        "Daily audit",
        "Check Cargo.toml",
        Some(HostCommandPolicy::Sandboxed),
    );
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-form-close-ready");
        let close = frame
            .nodes
            .iter()
            .find(|node| {
                node.role == Role::Button && node.label.as_deref() == Some("Close automation form")
            })
            .expect("close button");
        let center = close.bounds.center();
        super::settings_modal::click(&mut h, f32::from(center.x), f32::from(center.y));
        h.update(|p, _, _| assert!(p.automation_form.is_none()));
    }

    drop(h);
    drop(host);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn automation_form_bound_id_retry_edit_choice_and_discard_flow() {
    if !isolated("automations::automation_form_bound_id_retry_edit_choice_and_discard_flow") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-autoform-retry-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let repo_str = repository.display().to_string();

    let mut create_calls = 0usize;
    let mut get_calls = 0usize;
    let host = ScriptedAutomationHost::start(move |request| match request["op"].as_str() {
        Some("create_automation") => {
            create_calls += 1;
            match create_calls {
                1..=3 | 5 | 6 => json!({
                    "ok": false,
                    "error": {"code": "internal", "message": "Database busy."}
                }),
                4 => json!({
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
                }),
                _ => json!({
                    "ok": true,
                    "result": {
                        "automation": {
                            "id": "auto_after_not_found",
                            "state": "preparing",
                            "admission_request_id": "automation-auto_after_not_found",
                            "cursor": 1735722000000u64,
                            "error": null
                        }
                    }
                }),
            }
        }
        Some("get_automation") => {
            get_calls += 1;
            match get_calls {
                1 => json!({
                    "ok": false,
                    "error": {"code": "internal", "message": "Host restarting."}
                }),
                2 => json!({
                    "ok": true,
                    "result": {
                        "automation": {
                            "id": request["id"],
                            "state": "preparing",
                            "admission_request_id": "automation-found",
                            "cursor": 1735722000000u64,
                            "error": null,
                            "definition": {
                                "name": "Second audit",
                                "instructions": "Check deps",
                                "target": {"kind": "project", "checkout": repo_str},
                                "harness": "pi",
                                "provider": "openai-codex",
                                "model_id": "gpt-5.6-luna",
                                "approval": "auto",
                                "command_policy": "sandboxed",
                                "schedule": {"kind": "weekdays", "time": "09:00", "timeZone": "UTC"},
                                "catch_up_ms": 43200000
                            }
                        },
                        "occurrences": []
                    }
                }),
                _ => json!({
                    "ok": false,
                    "error": {"code": "not_found", "message": "Automation not found."}
                }),
            }
        }
        Some("list_automations") => json!({"ok": true, "result": {"automations": []}}),
        _ => json!({"ok": false, "error": {"code": "invalid_request", "message": "unused"}}),
    });

    let mut h = Harness::new(&repository, None, 1380., 860.);
    h.connect(&repository, false);
    let journal = attach_test_journal(&mut h);
    let local = MachineId::default();
    h.update(|p, _, cx| {
        p.automations.set_for_test(true, Vec::new());
        p.rebuild_projects(cx);
    });

    // 1. First create hits `internal` with no outcome -> treated as unconfirmed and persisted.
    let first_id = open_filled_form(
        &mut h,
        "Daily audit",
        "Check Cargo.toml",
        Some(HostCommandPolicy::Sandboxed),
    );
    h.update(|p, w, cx| p.submit_automation_form(w, cx));
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    assert!(
        journal
            .automation_create(&local, &first_id)
            .unwrap()
            .is_some()
    );
    h.update(|p, _, _| {
        let form = p.automation_form.as_ref().unwrap();
        assert!(form.submit_error.as_ref().is_some_and(|(_, warn)| *warn));
        assert_eq!(
            form.unconfirmed_attempt
                .as_ref()
                .map(|a| a.request_id.as_str()),
            Some(first_id.as_str())
        );
    });

    // 2. Retrying without edits resends the exact same ID and definition.
    h.update(|p, w, cx| p.submit_automation_form(w, cx));
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    let creates: Vec<Value> = host
        .requests()
        .into_iter()
        .filter(|r| r["op"] == "create_automation")
        .collect();
    assert_eq!(creates.len(), 2);
    assert_eq!(creates[0]["request_id"], first_id);
    assert_eq!(creates[1]["request_id"], first_id);
    assert_eq!(creates[0]["definition"], creates[1]["definition"]);

    // 3. Editing after an unconfirmed attempt shows a clear choice before anything is sent.
    h.update(|p, w, cx| {
        let form = p.automation_form.as_ref().unwrap();
        form.name
            .update(cx, |field, cx| field.set_text("Renamed audit", cx));
        assert!(p.form_edited_after_unconfirmed(cx));
        // Cmd-Enter / submit does not send anything while the choice is pending.
        p.submit_automation_form(w, cx);
        assert!(!p.automation_form.as_ref().unwrap().submitting);
    });
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-form-unconfirmed-choice");
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Retry the previous attempt")
        }));
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Discard it and create with my changes")
        }));
    }
    assert_eq!(
        host.requests()
            .into_iter()
            .filter(|r| r["op"] == "create_automation")
            .count(),
        2
    );

    // 4. Choosing "Retry the previous attempt" resends the old persisted definition + old ID.
    h.update(|p, w, cx| p.retry_previous_automation_attempt(w, cx));
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    let creates: Vec<Value> = host
        .requests()
        .into_iter()
        .filter(|r| r["op"] == "create_automation")
        .collect();
    assert_eq!(creates.len(), 3);
    assert_eq!(creates[2]["request_id"], first_id);
    assert_eq!(creates[2]["definition"]["name"], "Daily audit");
    assert_eq!(creates[2]["definition"], creates[0]["definition"]);

    // 5. Discarding asks the host once with the old ID first; when host check fails,
    // warns plainly that a duplicate is possible before creating anything.
    h.update(|p, w, cx| p.discard_and_create_automation(w, cx));
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    let gets: Vec<Value> = host
        .requests()
        .into_iter()
        .filter(|r| r["op"] == "get_automation")
        .collect();
    assert_eq!(gets.len(), 1);
    assert_eq!(gets[0]["id"], automation_id_for_request(&first_id));
    assert_eq!(
        host.requests()
            .into_iter()
            .filter(|r| r["op"] == "create_automation")
            .count(),
        3
    );
    h.update(|p, _, _| {
        assert!(
            p.automation_form
                .as_ref()
                .unwrap()
                .unconfirmed_attempt
                .as_ref()
                .unwrap()
                .discard_warning
        );
    });
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-form-duplicate-warning");
        let alert = frame
            .nodes
            .iter()
            .find(|node| {
                node.role == Role::Alert
                    && node.path.contains("automation-unconfirmed-choice-banner")
            })
            .expect("duplicate warning alert");
        assert!(frame.has_node(Role::Label, |node| {
            node.path.starts_with(&alert.path)
                && node
                    .value
                    .as_deref()
                    .is_some_and(|v| v.contains("may create a duplicate"))
        }));
    }

    // Confirming discard after warning removes old entry, mints a fresh ID, and creates.
    h.update(|p, w, cx| p.discard_and_create_automation(w, cx));
    wait_until(&mut h, |p, _| p.automation_form.is_none());
    let creates: Vec<Value> = host
        .requests()
        .into_iter()
        .filter(|r| r["op"] == "create_automation")
        .collect();
    assert_eq!(creates.len(), 4);
    let new_id = creates[3]["request_id"].as_str().unwrap().to_owned();
    assert_ne!(new_id, first_id);
    assert_eq!(creates[3]["definition"]["name"], "Renamed audit");
    // Both discarded and resolved entries are removed from automation-creates.
    assert_eq!(journal.automation_create(&local, &first_id).unwrap(), None);
    assert_eq!(journal.automation_create(&local, &new_id).unwrap(), None);

    // 6. Discarding when the host reports the old ID ALREADY exists does not create a duplicate.
    let second_id = open_filled_form(
        &mut h,
        "Second audit",
        "Check deps",
        Some(HostCommandPolicy::Sandboxed),
    );
    h.update(|p, w, cx| p.submit_automation_form(w, cx));
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    h.update(|p, w, cx| {
        p.automation_form
            .as_ref()
            .unwrap()
            .name
            .update(cx, |field, cx| field.set_text("Second audit v2", cx));
        p.discard_and_create_automation(w, cx);
    });
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    assert_eq!(
        host.requests()
            .into_iter()
            .filter(|r| r["op"] == "create_automation")
            .count(),
        5
    );
    assert_eq!(journal.automation_create(&local, &second_id).unwrap(), None);
    h.update(|p, w, cx| {
        let form = p.automation_form.as_ref().unwrap();
        assert!(form.unconfirmed_attempt.is_none());
        assert_ne!(form.request_id, second_id);
        assert!(
            form.submit_error
                .as_ref()
                .is_some_and(|(m, _)| m.contains("already created"))
        );
        p.close_automation_form(false, w, cx);
    });

    // 7. Discarding when the host confirms `not_found` proceeds cleanly with a fresh ID.
    let third_id = open_filled_form(
        &mut h,
        "Third audit",
        "Check links",
        Some(HostCommandPolicy::Sandboxed),
    );
    h.update(|p, w, cx| p.submit_automation_form(w, cx));
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    h.update(|p, w, cx| {
        p.automation_form
            .as_ref()
            .unwrap()
            .name
            .update(cx, |field, cx| field.set_text("Third audit edited", cx));
        p.discard_and_create_automation(w, cx);
    });
    wait_until(&mut h, |p, _| p.automation_form.is_none());
    let creates: Vec<Value> = host
        .requests()
        .into_iter()
        .filter(|r| r["op"] == "create_automation")
        .collect();
    assert_eq!(creates.len(), 7);
    let fourth_id = creates[6]["request_id"].as_str().unwrap().to_owned();
    assert_ne!(fourth_id, third_id);
    assert_eq!(creates[6]["definition"]["name"], "Third audit edited");
    assert_eq!(journal.automation_create(&local, &third_id).unwrap(), None);
    assert_eq!(journal.automation_create(&local, &fourth_id).unwrap(), None);

    drop(h);
    drop(host);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn automation_form_policy_refusal_prefill_guards_and_timezone_once_date() {
    if !isolated(
        "automations::automation_form_policy_refusal_prefill_guards_and_timezone_once_date",
    ) {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-autoform-guards-{}", std::process::id()));
    let repository = fixture_repository(&scratch);

    let host = ScriptedAutomationHost::start(move |request| match request["op"].as_str() {
        Some("command_policy") => json!({
            "ok": false,
            "error": {"code": "internal", "message": "Policy store unavailable."}
        }),
        _ => json!({"ok": false, "error": {"code": "invalid_request", "message": "unused"}}),
    });

    let mut h = Harness::new(&repository, None, 1380., 860.);
    h.connect(&repository, false);
    attach_test_journal(&mut h);
    h.update(|p, _, cx| {
        p.automations.set_for_test(true, Vec::new());
        p.rebuild_projects(cx);
    });

    // Requirement 3: Never guess command policy; refuse to create when fetch fails.
    open_filled_form(&mut h, "Policy check", "Run audit", None);
    h.update(|p, w, cx| p.submit_automation_form(w, cx));
    wait_until(&mut h, |p, _| {
        p.automation_form.as_ref().is_some_and(|f| !f.submitting)
    });
    let requests = host.requests();
    assert!(requests.iter().any(|r| r["op"] == "command_policy"));
    assert!(!requests.iter().any(|r| r["op"] == "create_automation"));
    h.update(|p, _, _| {
        assert_eq!(
            p.automation_form
                .as_ref()
                .unwrap()
                .submit_error
                .as_ref()
                .map(|(m, u)| (m.as_str(), *u)),
            Some((POLICY_UNAVAILABLE_ERROR, false))
        );
    });

    // Requirement 8: Project and Worktree labels & hint do not suggest a placement difference.
    assert_eq!(TargetKind::Project.label(), "This project (own worktree)");
    assert_eq!(TargetKind::Worktree.label(), "An existing worktree");
    assert_eq!(TargetKind::Project.hint(), TargetKind::Worktree.hint());
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automation-form-target-labels");
        assert!(frame.has_node(Role::Tab, |node| {
            node.label.as_deref() == Some("This project (own worktree)")
        }));
        assert!(frame.has_node(Role::Tab, |node| {
            node.label.as_deref() == Some("An existing worktree")
        }));
    }

    // Requirement 5: Prefill sets mode/approval/thinking only once, never after user edits,
    // and only when the thread's harness matches form.harness.
    let claude_thread_id = "thr_0c44a9d1";
    let mut claude_snap = thread_snapshot(&repository, false);
    claude_snap.id = claude_thread_id.into();
    claude_snap.approval_mode = Some(ApprovalMode::Auto);
    claude_snap.thinking_level = Some(ThinkingLevel::High);
    claude_snap.harness = Some(ThreadHarness {
        id: HarnessId::Claude,
        version: None,
        mode: Some("bypass".into()),
        reported: None,
    });

    h.update(|p, w, cx| {
        p.close_automation_form(false, w, cx);
        if let Some(summary) = p
            .host
            .connection
            .threads_mut()
            .iter_mut()
            .find(|t| t.id == claude_thread_id)
        {
            summary.harness = Some(HarnessId::Claude);
        }
        p.open_automation_form_from_thread(claude_thread_id.into(), w, cx);
        // User edits mode and approval before prefill arrives; leaves thinking untouched.
        p.choose_automation_mode(HarnessId::Claude, "plan".into(), cx);
        p.choose_automation_approval(ApprovalMode::Ask, cx);
        p.apply_thread_prefill(&claude_snap, cx);
        let form = p.automation_form.as_ref().unwrap();
        assert_eq!(form.mode.as_deref(), Some("plan"));
        assert_eq!(form.approval, ApprovalMode::Ask);
        assert_eq!(form.thinking, Some(ThinkingLevel::High));

        // Second prefill call is ignored (prefill applies only once).
        let mut second = claude_snap.clone();
        second.thinking_level = Some(ThinkingLevel::Low);
        p.apply_thread_prefill(&second, cx);
        assert_eq!(
            p.automation_form.as_ref().unwrap().thinking,
            Some(ThinkingLevel::High)
        );
    });

    // Harness mismatch: switching form.harness prevents thread prefill from touching
    // mode/approval/thinking even when approval was not edited.
    let mut claude_ask_snap = claude_snap.clone();
    claude_ask_snap.approval_mode = Some(ApprovalMode::Ask);
    h.update(|p, w, cx| {
        p.close_automation_form(false, w, cx);
        p.open_automation_form_from_thread(claude_thread_id.into(), w, cx);
        p.choose_automation_harness(HarnessId::Codex, cx);
        let before_mode = p.automation_form.as_ref().unwrap().mode.clone();
        let before_thinking = p.automation_form.as_ref().unwrap().thinking;
        assert_eq!(
            p.automation_form.as_ref().unwrap().approval,
            ApprovalMode::Auto
        );
        p.apply_thread_prefill(&claude_ask_snap, cx);
        let form = p.automation_form.as_ref().unwrap();
        assert_eq!(form.harness, HarnessId::Codex);
        assert_eq!(form.mode, before_mode);
        assert_eq!(form.approval, ApprovalMode::Auto);
        assert_eq!(form.thinking, before_thinking);
    });

    // Requirement 6: Once date defaults to today's date in the chosen time zone, not UTC.
    let expected_kiritimati = today_date_in_time_zone("Pacific/Kiritimati").unwrap();
    let expected_honolulu = today_date_in_time_zone("Pacific/Honolulu").unwrap();
    assert_ne!(expected_kiritimati, expected_honolulu);

    h.update(|p, w, cx| {
        p.close_automation_form(false, w, cx);
        p.open_automation_form(w, cx);
        p.automation_form
            .as_ref()
            .unwrap()
            .time_zone
            .update(cx, |field, cx| field.set_text("Pacific/Kiritimati", cx));
    });
    wait_until(&mut h, |p, cx| {
        p.automation_form
            .as_ref()
            .is_some_and(|f| f.once_date.read(cx).text() == expected_kiritimati)
    });

    h.update(|p, _, cx| {
        p.automation_form
            .as_ref()
            .unwrap()
            .time_zone
            .update(cx, |field, cx| field.set_text("Pacific/Honolulu", cx));
    });
    wait_until(&mut h, |p, cx| {
        p.automation_form
            .as_ref()
            .is_some_and(|f| f.once_date.read(cx).text() == expected_honolulu)
    });

    // Once the user edits once_date manually, changing time_zone does not overwrite it.
    h.update(|p, _, cx| {
        let form = p.automation_form.as_ref().unwrap();
        form.once_date
            .update(cx, |field, cx| field.set_text("2028-04-10", cx));
        form.time_zone
            .update(cx, |field, cx| field.set_text("Pacific/Kiritimati", cx));
    });
    h.settle();
    h.update(|p, _, cx| {
        assert_eq!(
            p.automation_form
                .as_ref()
                .unwrap()
                .once_date
                .read(cx)
                .text(),
            "2028-04-10"
        );
    });

    drop(h);
    drop(host);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn automation_form_reopen_offers_the_unconfirmed_attempt() {
    if !isolated("automations::automation_form_reopen_offers_the_unconfirmed_attempt") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-autoform-reopen-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let host = ScriptedAutomationHost::start(move |request| match request["op"].as_str() {
        Some("create_automation") => json!({
            "ok": false,
            "error": {"code": "internal", "message": "Connection lost."}
        }),
        Some("list_automations") => json!({"ok": true, "result": {"automations": []}}),
        _ => json!({"ok": false, "error": {"code": "invalid_request", "message": "unused"}}),
    });
    let mut h = Harness::new(&repository, None, 1380., 860.);
    h.connect(&repository, false);
    let journal = attach_test_journal(&mut h);
    h.update(|p, _, cx| {
        p.automations.set_for_test(true, Vec::new());
        p.rebuild_projects(cx);
    });
    let req_id = open_filled_form(
        &mut h,
        "Daily audit",
        "Check Cargo.toml",
        Some(HostCommandPolicy::Sandboxed),
    );
    h.window
        .update(&mut h.cx, |p, w, cx| p.submit_automation_form(w, cx))
        .unwrap();
    wait_until(&mut h, |p, _| {
        p.automation_form
            .as_ref()
            .is_some_and(|f| f.unconfirmed_attempt.is_some())
    });
    // A stray close must not forget the attempt: reopening offers it again.
    h.draw();
    h.keystroke("escape");
    h.update(|p, _, _| assert!(p.automation_form.is_none()));
    assert!(
        journal
            .automation_create(&MachineId::default(), &req_id)
            .unwrap()
            .is_some()
    );
    h.window
        .update(&mut h.cx, |p, w, cx| p.open_automation_form(w, cx))
        .unwrap();
    wait_until(&mut h, |p, _| {
        p.automation_form
            .as_ref()
            .is_some_and(|f| f.unconfirmed_attempt.is_some())
    });
    h.update(|p, _, _| {
        let form = p.automation_form.as_ref().unwrap();
        assert_eq!(form.request_id, req_id);
        assert_eq!(
            form.unconfirmed_attempt
                .as_ref()
                .map(|a| a.request_id.as_str()),
            Some(req_id.as_str())
        );
        assert!(
            form.submit_error
                .as_ref()
                .is_some_and(|(message, warning)| *warning
                    && message.contains("may not have been confirmed: Daily audit"))
        );
    });
    drop(host);
}

#[core::prelude::v1::test]
fn automation_form_reopened_never_matches_a_closed_forms_replies() {
    if !isolated("automations::automation_form_reopened_never_matches_a_closed_forms_replies") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-autoform-token-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 1380., 860.);
    h.update(|p, _, cx| {
        p.automations.set_for_test(true, Vec::new());
        p.rebuild_projects(cx);
    });
    let open = |h: &mut Harness| {
        h.window
            .update(&mut h.cx, |p, w, cx| {
                p.open_automation_form(w, cx);
                p.automation_form_guard_values().unwrap()
            })
            .unwrap()
    };
    let first = open(&mut h);
    h.draw();
    h.keystroke("escape");
    let second = open(&mut h);
    // Every guard a late reply checks must differ, so the old form's replies are dropped.
    for (old, new) in first.iter().zip(second.iter()) {
        if *old != 0 || *new != 0 {
            assert_ne!(old, new);
        }
    }
    assert_ne!(first[0], second[0]);
}
