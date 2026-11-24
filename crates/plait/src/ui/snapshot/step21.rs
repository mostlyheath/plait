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
        plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
        let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
        snapshot.status = ThreadStatus::Idle;
        snapshot.model = ModelChoice {
            provider: "openai".into(),
            model_id: "gpt-5.3-codex".into(),
        };
        snapshot.harness = Some(ThreadHarness {
            id: HarnessId::Codex,
            version: Some(NEWER.into()),
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

#[core::prelude::v1::test]
fn step21_draft_persistence_approval_keys_ime_and_strip_counts() {
    if !isolated("step21::step21_draft_persistence_approval_keys_ime_and_strip_counts") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-step21-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut harness = Harness::new(&repository, None, width, height);
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

        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
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
        let mut submitted = false;
        harness
            .window
            .update(&mut harness.cx, |plait, window, cx| {
                let composer = plait.workspace.composers.get(THREAD).unwrap().clone();
                composer.update(cx, |composer, cx| {
                    composer.replace_and_mark_text_in_range(None, "nihon", None, window, cx);
                    assert!(
                        composer.marked_text_range(window, cx).is_some(),
                        "{suffix}: IME marked range is active"
                    );
                    assert!(
                        !composer.can_send(),
                        "{suffix}: Send is disabled during IME composition"
                    );
                    composer.submit_from_button(cx);
                    submitted = composer.saved_draft().2.is_some();
                    composer.unmark_text(window, cx);
                    assert!(composer.marked_text_range(window, cx).is_none());
                    assert!(composer.can_send());
                });
            })
            .unwrap();
        assert!(!submitted, "{suffix}: IME composition never submits");

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

        // 6. Strip counts match real Git Changes for the fixture repository.
        harness.change_snapshot(|snapshot| {
            snapshot.status = ThreadStatus::Idle;
        });
        harness.update(|plait, _, cx| {
            plait.workspace.thread_files = Default::default();
            assert!(plait.workspace.thread_files.collapsed);
            let paths = plait
                .state
                .repository
                .snapshot()
                .ready()
                .expect("fixture repository status")
                .changes
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>();
            assert!(!paths.is_empty());
            plait
                .workspace
                .thread_files
                .inject(THREAD, repository.clone(), Vec::new());
            plait.workspace.thread_files.set_paths(paths);
            plait.thread_files_status(cx);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            harness.settle();
            let ready = harness
                .window
                .update(&mut harness.cx, |plait, _, _| {
                    plait.workspace.thread_files.counted().count() > 0
                })
                .unwrap();
            if ready {
                break;
            }
            assert!(Instant::now() < deadline, "strip counts did not arrive");
            std::thread::sleep(Duration::from_millis(10));
        }
        #[cfg(debug_assertions)]
        {
            let (expected_files, expected_added, expected_removed) = harness
                .window
                .update(&mut harness.cx, |plait, _, _| {
                    let counted: Vec<_> = plait.workspace.thread_files.counted().cloned().collect();
                    let changes = &plait.state.repository.snapshot().ready().unwrap().changes;
                    assert_eq!(counted.len(), changes.len());
                    (
                        counted.len(),
                        counted.iter().filter_map(|c| c.added).sum::<usize>(),
                        counted.iter().filter_map(|c| c.removed).sum::<usize>(),
                    )
                })
                .unwrap();
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("step21 strip closed ({suffix})"));
            let want = format!(
                "{expected_files} files changed by this thread, {expected_added} lines added, {expected_removed} removed"
            );
            assert!(
                frame.has_node(Role::Button, |n| n.label.as_deref() == Some(want.as_str())),
                "{suffix}: strip summary label matches Changes counts: expected {want}"
            );
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
