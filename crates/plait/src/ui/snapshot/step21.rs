//! Step 21 scenes and behavioral checks: message box, changed-files strip,
//! and approval in place of the message box.

use super::*;
use plait_client::{
    git::LineCount,
    host::{
        QueueMode, Queued, ThinkingLevel, Usage,
        harness::{HarnessId, ThreadHarness},
    },
};

fn configure_model(plait: &mut Plait, name: &str, context_window: Option<u64>) {
    let status = plait.host.connection.status.as_mut().unwrap();
    status.protocol_version = 4;
    let model = status
        .models
        .iter_mut()
        .find(|m| m.choice() == choice())
        .unwrap();
    model.name = name.into();
    model.reasoning = Some(true);
    model.thinking_levels = Some(vec![
        ThinkingLevel::Off,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
    ]);
    model.context_window = context_window;
}

fn setup_base_thread(harness: &mut Harness, repository: &Path, name: &str) {
    harness.fresh_composer(repository);
    harness.update(|plait, _, cx| {
        plait.state.projects[0].machines[0].name = "studio".into();
        plait.navigation.invalidate_projects();
        configure_model(plait, name, Some(200_000));
        let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
        snapshot.status = ThreadStatus::Idle;
        snapshot.thinking_level = Some(ThinkingLevel::Low);
        snapshot.usage = None;
        snapshot.approval_mode = Some(ApprovalMode::Ask);
        snapshot.harness = None;
        snapshot.pending_approval = None;
        snapshot.queued.clear();
        plait.workspace.thread_controls.note = None;
        plait.workspace.thread_controls.usage_open = false;
        plait.workspace.thread_files = Default::default();
        plait.workspace.clear_approval_reply();
        plait.preferences.value.hide_changed_files = false;
        plait.host.submissions = Default::default();
        plait.sync_composer(cx);
    });
}

fn sample_counts() -> Vec<LineCount> {
    let count = |path: &str, added, removed, untracked| LineCount {
        path: path.into(),
        added,
        removed,
        untracked,
    };
    vec![
        count("notes.md", Some(12), Some(0), true),
        count("src/review.rs", Some(48), Some(5), false),
        count("README.md", Some(0), Some(3), false),
        count("assets/icon.png", None, None, false),
    ]
}

fn many_counts() -> Vec<LineCount> {
    (1..=9)
        .map(|i| LineCount {
            path: format!("crates/plait/src/ui/sample_{i}.rs").into(),
            added: Some(i * 7),
            removed: Some(i * 2),
            untracked: i == 1,
        })
        .collect()
}

fn set_light(harness: &mut Harness, light: Option<bool>) {
    harness.update(move |plait, _, cx| {
        plait.themes.forced = light;
        plait.apply_theme(cx);
    });
}

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    // --- Message box scenes ---
    setup_base_thread(harness, repository, "Claude Sonnet");
    harness.update(|plait, window, cx| {
        window.blur(cx);
        plait.sync_composer(cx);
    });
    harness.capture(&format!("box-idle-{suffix}"));

    harness.composer(|composer, window, cx| {
        composer.type_text(DRAFT, window, cx);
    });
    harness.capture(&format!("box-focused-{suffix}"));

    set_light(harness, Some(true));
    harness.capture(&format!("box-focused-light-{suffix}"));
    set_light(harness, None);

    // Running with empty draft: Stop in coral.
    setup_base_thread(harness, repository, "Claude Sonnet");
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        snapshot.approval_mode = Some(ApprovalMode::Ask);
    });
    harness.update(|plait, _, cx| plait.sync_composer(cx));
    harness.capture(&format!("box-running-{suffix}"));

    // Queued follow-up and steering rows above the draft.
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        snapshot.queued = vec![
            Queued {
                id: "step21-q1".into(),
                request_id: "step21-r1".into(),
                text: "Check the narrow layout after updating the strip.".into(),
                mode: QueueMode::FollowUp,
                approval: ApprovalMode::Ask,
            },
            Queued {
                id: "step21-q2".into(),
                request_id: "step21-r2".into(),
                text: "Keep the existing public API unchanged.".into(),
                mode: QueueMode::Steer,
                approval: ApprovalMode::Ask,
            },
        ];
    });
    harness.composer(|composer, window, cx| {
        composer.type_text("Draft ready to queue while running", window, cx);
    });
    harness.update(|plait, _, cx| plait.sync_composer(cx));
    harness.capture(&format!("box-queued-{suffix}"));

    // Locked control during a run with lock reason notice.
    setup_base_thread(harness, repository, "Claude Sonnet");
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        snapshot.approval_mode = Some(ApprovalMode::Ask);
    });
    harness.update(|plait, window, cx| {
        plait.sync_composer(cx);
        plait.open_thread_models(window, cx);
    });
    harness.capture(&format!("box-locked-{suffix}"));

    // Mode differs notice during a run.
    harness.update(|plait, _, cx| {
        let id = plait.host.thread.selected.clone().unwrap();
        plait.workspace.thread_controls.note = None;
        plait.set_thread_approval(id, Some(ApprovalMode::Auto), cx);
        plait.sync_composer(cx);
    });
    harness.capture(&format!("box-mode-differs-{suffix}"));

    // Wrapping settings row with long model name, Full access in sand, and context usage.
    setup_base_thread(harness, repository, "Claude Sonnet");
    harness.update(|plait, _, cx| {
        let mut catalog = sample_harnesses("ready", None);
        let model = catalog
            .harnesses
            .iter_mut()
            .find(|harness| harness.id == HarnessId::Codex)
            .unwrap()
            .models
            .as_mut()
            .unwrap()
            .first_mut()
            .unwrap();
        model.name = "Sample Codex model with a long display name".into();
        plait.inject_harnesses(Some(catalog));
        let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
        snapshot.status = ThreadStatus::Idle;
        snapshot.model = codex_choice();
        snapshot.harness = Some(ThreadHarness {
            id: HarnessId::Codex,
            version: Some("0.170.0".into()),
            mode: Some("full_access".into()),
            reported: Some("full_access".into()),
        });
        snapshot.usage = Some(Usage {
            input: 12_400,
            output: 1_800,
            cache_read: 141_800,
            cache_write: 0,
            reasoning: Some(900),
            total: 156_000,
            context_used: Some(156_000),
            context_window: Some(200_000),
        });
        plait.sync_thread_harness();
        plait.sync_composer(cx);
    });
    harness.capture(&format!("box-wrap-{suffix}"));

    // --- Changed-files strip scenes ---
    setup_base_thread(harness, repository, "Claude Sonnet");
    let root = repository.to_owned();
    harness.update(move |plait, _, cx| {
        plait
            .workspace
            .thread_files
            .inject(THREAD, root, sample_counts());
        // Closed at first by default.
        assert!(plait.workspace.thread_files.collapsed);
        cx.notify();
    });
    harness.capture(&format!("strip-closed-{suffix}"));

    set_light(harness, Some(true));
    harness.capture(&format!("strip-closed-light-{suffix}"));
    set_light(harness, None);

    harness.update(|plait, _, cx| {
        plait.workspace.thread_files.collapsed = false;
        cx.notify();
    });
    harness.capture(&format!("strip-open-{suffix}"));

    let root = repository.to_owned();
    harness.update(move |plait, _, cx| {
        plait
            .workspace
            .thread_files
            .inject(THREAD, root, many_counts());
        plait.workspace.thread_files.collapsed = false;
        cx.notify();
    });
    harness.capture(&format!("strip-many-{suffix}"));

    harness.update(|plait, _, cx| {
        plait.preferences.value.hide_changed_files = true;
        cx.notify();
    });
    harness.capture(&format!("strip-hidden-{suffix}"));
    harness.update(|plait, _, _| {
        plait.preferences.value.hide_changed_files = false;
        plait.workspace.thread_files = Default::default();
    });

    // --- Approval scenes ---
    setup_base_thread(harness, repository, "Claude Sonnet");
    harness.composer(|composer, window, cx| {
        composer.type_text("Keep this draft across approval", window, cx);
    });
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::AwaitingApproval;
        snapshot.pending_approval = Some(Approval {
            id: "s21-cmd".into(),
            tool_id: "t2".into(),
            kind: ApprovalKind::Command,
            summary: "Run cargo test --workspace".into(),
            details: r#"{"command":"cargo","args":["test","--workspace"],"cwd":"."}"#.into(),
            dirty_paths: Vec::new(),
            risk: "Runs in the command sandbox with a read-only checkout and no network.".into(),
        });
    });
    harness.capture(&format!("approval-command-{suffix}"));

    set_light(harness, Some(true));
    harness.capture(&format!("approval-command-light-{suffix}"));
    set_light(harness, None);

    harness.change_snapshot(|snapshot| {
        snapshot.pending_approval = Some(Approval {
            id: "s21-edit".into(),
            tool_id: "t3".into(),
            kind: ApprovalKind::Edit,
            summary: "Edit src/review.rs".into(),
            details: r#"{"path":"src/review.rs","old_text":"read_status();","new_text":"queue_status();"}"#.into(),
            dirty_paths: Vec::new(),
            risk: "Modifies src/review.rs in the current checkout.".into(),
        });
    });
    harness.capture(&format!("approval-edit-{suffix}"));

    harness.change_snapshot(|snapshot| {
        snapshot.pending_approval = Some(Approval {
            id: "s21-protected".into(),
            tool_id: "t4".into(),
            kind: ApprovalKind::Edit,
            summary: "Edit notes.md and src/review.rs".into(),
            details: r##"{"path":"notes.md","old_text":"# Review","new_text":"# Updated review"}"##
                .into(),
            dirty_paths: vec!["notes.md".into(), "src/review.rs".into()],
            risk: "Writes files in the checkout; pre-existing dirty files are protected by Pi."
                .into(),
        });
    });
    harness.capture(&format!("approval-protected-paths-{suffix}"));

    harness.change_snapshot(|snapshot| {
        snapshot.pending_approval = Some(Approval {
            id: "s21-mcp".into(),
            tool_id: "t5".into(),
            kind: ApprovalKind::Command,
            summary: "Use search from docs".into(),
            details: r#"{"query":"setup guide"}"#.into(),
            dirty_paths: Vec::new(),
            risk: "Only this exact MCP call is approved. The server can use the network.".into(),
        });
    });
    harness.capture(&format!("approval-mcp-{suffix}"));

    // Reply instead draws the message box below the approval card with the draft intact.
    harness.update(|plait, window, cx| {
        plait.workspace.show_composer_for_approval("s21-mcp".into());
        plait.focus_composer(window, cx);
        cx.notify();
    });
    harness.capture(&format!("approval-reply-instead-{suffix}"));

    // Narrow / dense approval with long arguments, risk, and dirty paths.
    harness.update(|plait, _, _| plait.workspace.clear_approval_reply());
    harness.change_snapshot(|snapshot| {
        snapshot.pending_approval = Some(Approval {
            id: "s21-narrow".into(),
            tool_id: "t6".into(),
            kind: ApprovalKind::Command,
            summary: "Run workspace formatting, clippy, and full test checks".into(),
            details: r#"{"command":"cargo","args":["clippy","--workspace","--all-targets","--","-D","warnings"],"cwd":"crates/plait"}"#.into(),
            dirty_paths: vec![
                "crates/plait/src/ui/thread_controls.rs".into(),
                "crates/plait/src/ui/thread_files.rs".into(),
            ],
            risk: "Runs trusted command in the checkout with full local environment access.".into(),
        });
    });
    harness.capture(&format!("approval-narrow-{suffix}"));

    // Leave the harness clean for later walks.
    harness.update(|plait, _, cx| {
        plait.workspace.clear_approval_reply();
        plait.workspace.thread_files = Default::default();
        plait.preferences.value.hide_changed_files = false;
        plait.themes.forced = None;
        plait.apply_theme(cx);
    });
}

#[cfg(debug_assertions)]
fn click(harness: &mut Harness, label: &str) {
    let position = harness
        .draw_a11y()
        .nodes
        .iter()
        .find(|n| n.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("missing control {label}"))
        .bounds
        .center();
    harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }));
    harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }));
}

#[core::prelude::v1::test]
fn approval_and_draft() {
    if !isolated("step21::approval_and_draft") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-step21-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut harness = Harness::with_host(&repository, None, width, height, None);
        setup_base_thread(&mut harness, &repository, "Claude Sonnet");

        // 1. Draft typed before approval arrives survives hiding, Reply instead, and answering.
        harness.composer(|composer, window, cx| {
            composer.type_text("Preserve this unsent draft", window, cx);
        });
        let before_rev = harness
            .window
            .update(&mut harness.cx, |plait, _, cx| {
                let composer = plait.workspace.composers.get(THREAD).unwrap().read(cx);
                assert_eq!(composer.text(), "Preserve this unsent draft");
                composer.saved_draft().1
            })
            .unwrap();

        harness.change_snapshot(|snapshot| {
            snapshot.status = ThreadStatus::AwaitingApproval;
            snapshot.pending_approval = Some(Approval {
                id: "s21-keep-draft".into(),
                tool_id: "t2".into(),
                kind: ApprovalKind::Command,
                summary: "Run cargo test".into(),
                details: r#"{"command":"cargo","args":["test"]}"#.into(),
                dirty_paths: vec!["notes.md".into()],
                risk: "Runs in the command sandbox.".into(),
            });
        });

        harness.update(|plait, _, _| {
            plait.set_thread_notice(THREAD.into(), "Review the previous request.".into(), true);
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            assert!(
                frame.has_node(Role::Label, |n| {
                    n.value.as_deref().or(n.label.as_deref())
                        == Some("Review the previous request.")
                }),
                "{suffix}: thread notices stay visible above an approval"
            );
            frame.assert_no_duplicates(&format!("step21 approval hidden box ({suffix})"));
            assert!(
                frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Allow once")),
                "{suffix}: Allow once present"
            );
            assert!(
                frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Deny once")),
                "{suffix}: Deny once present"
            );
            assert!(
                frame.has_node(Role::Button, |n| {
                    n.label.as_deref() == Some("Reply instead")
                }),
                "{suffix}: Reply instead present"
            );
            assert!(
                !frame.has_node(Role::MultilineTextInput, |_| true),
                "{suffix}: message box is not drawn while waiting for approval"
            );
        }
        #[cfg(not(debug_assertions))]
        harness.draw();

        // Composer entity stayed alive with its draft intact while not drawn.
        harness.update(|plait, _, cx| {
            let composer = plait.workspace.composers.get(THREAD).unwrap().read(cx);
            assert_eq!(composer.text(), "Preserve this unsent draft");
            assert_eq!(composer.saved_draft().1, before_rev);
        });

        // 2. Activating Reply instead draws the box below the approval card with the same draft.
        #[cfg(debug_assertions)]
        {
            click(&mut harness, "Reply instead");
            harness.update(|plait, window, cx| {
                assert!(
                    plait.workspace.composers[THREAD]
                        .focus_handle(cx)
                        .is_focused(window)
                );
            });
        }
        #[cfg(not(debug_assertions))]
        harness.update(|plait, window, cx| {
            plait
                .workspace
                .show_composer_for_approval("s21-keep-draft".into());
            plait.focus_composer(window, cx);
            cx.notify();
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("step21 reply instead ({suffix})"));
            assert!(
                frame.has_node(Role::MultilineTextInput, |n| {
                    n.value.as_deref() == Some("Preserve this unsent draft")
                }),
                "{suffix}: draft is visible in composer after Reply instead"
            );
            assert!(
                !frame.has_node(Role::Button, |n| {
                    n.label.as_deref() == Some("Reply instead")
                }),
                "{suffix}: Reply instead hides once the box is open"
            );
        }
        #[cfg(not(debug_assertions))]
        harness.draw();

        // 3. Approval shortcuts still work with focus inside the composer and enforce repeat guard.
        assert!(
            !harness.approval_press("cmd-alt-y", "s21-keep-draft", true),
            "{suffix}: held repeat is ignored even with focus in composer"
        );
        assert!(
            harness.answers("cmd-alt-y", "s21-keep-draft"),
            "{suffix}: Allow once shortcut works with focus in composer"
        );
        harness.update(|plait, _, cx| {
            assert!(
                !plait
                    .workspace
                    .showing_composer_for_approval("s21-keep-draft"),
                "{suffix}: Reply instead state resets when answered"
            );
            let composer = plait.workspace.composers.get(THREAD).unwrap().read(cx);
            assert_eq!(
                composer.text(),
                "Preserve this unsent draft",
                "{suffix}: draft survives answering approval"
            );
        });

        // Deny once shortcut works with focus elsewhere (window blurred / conversation).
        harness.new_approval("s21-deny-key");
        harness.update(|plait, window, cx| {
            window.blur(cx);
            assert!(
                !plait
                    .workspace
                    .showing_composer_for_approval("s21-deny-key")
            );
        });
        assert!(
            harness.answers("cmd-alt-.", "s21-deny-key"),
            "{suffix}: Deny once works with focus anywhere in the window"
        );

        // Clear approval and verify draft is still present in the restored message box.
        harness.change_snapshot(|snapshot| {
            snapshot.status = ThreadStatus::Idle;
            snapshot.pending_approval = None;
        });
        harness.update(|plait, _, cx| {
            plait.host.connection.error = None;
            plait.sync_composer(cx);
            let composer = plait.workspace.composers.get(THREAD).unwrap().read(cx);
            assert_eq!(composer.text(), "Preserve this unsent draft");
        });

        // 4. IME composition does not submit on Enter.
        harness.update(|plait, window, cx| plait.focus_composer(window, cx));
        harness.composer(|composer, window, cx| {
            composer.replace_and_mark_text_in_range(None, "nihon", None, window, cx);
            assert!(composer.marked_text_range(window, cx).is_some());
            assert!(!composer.can_send());
        });
        harness.draw();
        harness.keystroke("enter");
        harness.composer(|composer, window, cx| {
            assert!(
                composer.saved_draft().2.is_none(),
                "Enter submitted marked text"
            );
            composer.unmark_text(window, cx);
            assert!(composer.can_send());
        });

        // 5. Running thread: empty draft shows Stop; typing switches to Queue message.
        harness.composer(|composer, _, cx| {
            composer.restore_draft(String::new(), 100, None, cx);
        });
        harness.change_snapshot(|snapshot| {
            snapshot.status = ThreadStatus::Running;
        });
        harness.update(|plait, _, cx| plait.sync_composer(cx));
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("step21 running stop ({suffix})"));
            assert!(frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Stop")));
            assert!(!frame.has_node(Role::Button, |n| n.label.as_deref()
                == Some("Queue message")));
        }
        harness.composer(|composer, window, cx| {
            composer.type_text("Follow up", window, cx);
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("step21 running queue ({suffix})"));
            assert!(frame.has_node(Role::Button, |n| {
                n.label.as_deref() == Some("Queue message")
            }));
            assert!(!frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Stop")));
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn strip_counts_from_checkout() {
    if !isolated("step21::strip_counts_from_checkout") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-strip-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    setup_base_thread(&mut harness, &repository, "Claude Sonnet");
    // Only the source file belongs to this turn. README.md and notes.md also
    // changed in the checkout and must stay out of its summary.
    let mut record: plait_client::host::TurnRecord =
        serde_json::from_value(undo::record("last", &["request"], "ready")).unwrap();
    record.paths.truncate(1);
    record.paths_total = 1;
    harness.update(|plait, _, cx| plait.record_thread_files(&[record], cx));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.settle();
        let ready = harness
            .window
            .update(&mut harness.cx, |plait, _, _| {
                plait.workspace.thread_files.counted().count() != 0
            })
            .unwrap();
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "strip counts did not arrive");
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.update(|plait, _, _| {
        assert_eq!(
            plait
                .state
                .repository
                .snapshot()
                .ready()
                .unwrap()
                .changes
                .len(),
            3
        );
        let counted = plait.workspace.thread_files.counted().collect::<Vec<_>>();
        assert_eq!(counted.len(), 1);
        assert_eq!(counted[0].path, Path::new("src/review.rs"));
        assert_eq!((counted[0].added, counted[0].removed), (Some(2), Some(1)));
        assert!(plait.workspace.thread_files.collapsed);
    });
    #[cfg(debug_assertions)]
    inspect_strip(&mut harness);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[cfg(debug_assertions)]
fn inspect_strip(harness: &mut Harness) {
    click(
        harness,
        "1 file changed by this thread, 2 lines added, 1 removed",
    );
    click(harness, "Modified src/review.rs. Show in Changes");
    harness.update(|plait, _, _| {
        let repo = &plait.state.repository;
        let selected = repo.selected_diff().expect("strip row selects its diff");
        assert_eq!(
            repo.snapshot().ready().unwrap().changes[selected.file].path,
            Path::new("src/review.rs")
        );
        assert_eq!(selected.mode, DiffMode::Unstaged);
    });
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn review_approval_shortcuts_respect_panel_visibility() {
    if !isolated("step21::review_approval_shortcuts_respect_panel_visibility") {
        return;
    }
    let home = PathBuf::from(std::env::var_os(SCENE_HOME).unwrap());
    let repository = fixture_repository(&home);
    let mut harness = Harness::with_host(&repository, None, 1380., 860., None);
    harness.connect(&repository, true);
    harness.draw();
    harness.keystroke("cmd-l");
    assert!(harness.draw_a11y().has_node(Role::Button, |node| {
        node.label.as_deref() == Some("Allow once")
    }));
    assert!(
        harness.answers("cmd-alt-y", "a1"),
        "the approval is beside the panel"
    );

    harness.new_approval("covered");
    harness.update(|plait, window, cx| plait.fill_panel(window, cx));
    assert!(!harness.draw_a11y().has_node(Role::Button, |node| {
        node.label.as_deref() == Some("Allow once")
    }));
    assert!(
        !harness.answers("cmd-alt-y", "covered"),
        "Fill hides the call"
    );
    assert!(
        !harness.answers("cmd-alt-.", "covered"),
        "Fill hides the call"
    );

    harness.update(|plait, window, cx| plait.fill_panel(window, cx));
    assert!(
        harness.answers("cmd-alt-.", "covered"),
        "the call is visible again"
    );
}

#[cfg(debug_assertions)]
fn checkout_strip(harness: &mut Harness, repository: &Path, status: plait_client::model::Snapshot) {
    let counts = plait_engine::git::line_counts(repository, &["src/review.rs".into()]).unwrap();
    assert_eq!((counts[0].added, counts[0].removed), (Some(2), Some(1)));
    harness.update(|plait, _, _| {
        plait.state.repository.data.snapshot = Load::Ready(Arc::new(status));
        plait
            .workspace
            .thread_files
            .inject(THREAD, repository.to_owned(), counts);
        plait.workspace.thread_files.collapsed = false;
    });
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn review_strip_reselects_file_after_status_refresh() {
    use plait_client::{
        panel::{PanelKind, Scope},
        state::{Job, Reply},
    };
    if !isolated("step21::review_strip_reselects_file_after_status_refresh") {
        return;
    }
    let home = PathBuf::from(std::env::var_os(SCENE_HOME).unwrap());
    let repository = fixture_repository(&home);
    let mut harness = Harness::with_host(&repository, None, 900., 600., None);
    setup_base_thread(&mut harness, &repository, "Sample model");
    let status = plait_engine::git::status(&repository).unwrap();
    let mut stale = plait_engine::git::status(&repository).unwrap();
    stale
        .changes
        .retain(|change| change.path != Path::new("src/review.rs"));
    checkout_strip(&mut harness, &repository, stale);
    click(&mut harness, "Changed src/review.rs. Show in Changes");
    harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let panel = plait.tabs.model.panel().unwrap();
            assert_eq!(
                (panel.open, panel.kind, panel.scope),
                (true, PanelKind::Changes, Scope::Uncommitted)
            );
            assert!(plait.panel_fills(), "the panel covers the conversation");
            let repo = &mut plait.state.repository;
            // Deliver status and check the selection before the diff read.
            let job = Job::Status {
                epoch: repo.epoch(),
                path: repository,
            };
            repo.complete(job, Reply::Status(Ok(Arc::new(status))));
            let selected = repo.selected_diff().unwrap();
            assert_eq!(selected.path, Path::new("src/review.rs"));
            assert_eq!(selected.mode, DiffMode::Unstaged);
        })
        .unwrap();
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn review_strip_opens_changes_with_fresh_counts() {
    use plait_client::panel::{PanelKind, Scope};
    if !isolated("step21::review_strip_opens_changes_with_fresh_counts") {
        return;
    }
    let home = PathBuf::from(std::env::var_os(SCENE_HOME).unwrap());
    let repository = fixture_repository(&home);
    let mut harness = Harness::with_host(&repository, None, 1380., 860., None);
    setup_base_thread(&mut harness, &repository, "Sample model");
    checkout_strip(
        &mut harness,
        &repository,
        plait_engine::git::status(&repository).unwrap(),
    );
    harness.update(|plait, _, _| {
        assert!(!plait.panel_is_open());
        // These counts were loaded for an older status before the panel closed.
        plait
            .workspace
            .work_counts
            .insert("src/review.rs".into(), "+1 −1".into());
        plait.workspace.work_total = Some("+1 −1".into());
    });
    click(&mut harness, "Modified src/review.rs. Show in Changes");
    harness.update(|plait, _, _| {
        let panel = plait.tabs.model.panel().unwrap();
        assert_eq!(
            (panel.open, panel.kind, panel.scope),
            (true, PanelKind::Changes, Scope::Uncommitted)
        );
        assert!(
            !plait.panel_fills(),
            "the wide panel leaves the conversation visible"
        );
        assert_eq!(
            plait.state.repository.selected_diff().unwrap().path,
            Path::new("src/review.rs")
        );
        assert!(
            plait.workspace.work_total.is_none(),
            "Changes must refresh the totals"
        );
        assert!(
            plait.workspace.work_counts.is_empty(),
            "Changes must refresh file counts"
        );
    });
}
