//! Message box scenes and accessibility checks at wide and narrow window sizes.

use super::*;
use plait_client::host::{
    ThinkingLevel, Usage,
    harness::{HarnessId, ThreadHarness},
};

fn configure_reasoning_model(plait: &mut Plait, name: &str, context_window: Option<u64>) {
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

fn setup_idle_thread(harness: &mut Harness, repository: &Path, name: &str) {
    harness.fresh_composer(repository);
    harness.update(|plait, _, cx| {
        configure_reasoning_model(plait, name, Some(200_000));
        let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
        snapshot.status = ThreadStatus::Idle;
        snapshot.thinking_level = Some(ThinkingLevel::Low);
        snapshot.usage = None;
        snapshot.approval_mode = Some(ApprovalMode::Ask);
        snapshot.harness = None;
        plait.workspace.thread_controls.note = None;
        plait.workspace.thread_controls.usage_open = false;
        plait.host.submissions = Default::default();
        plait.sync_composer(cx);
    });
}

fn setup_codex_thread(harness: &mut Harness) {
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
            mode: Some("ask".into()),
            reported: None,
        });
        snapshot.usage = None;
        plait.workspace.thread_controls.note = None;
        plait.sync_composer(cx);
    });
}

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    // 1. Idle thread, context use absent.
    setup_idle_thread(harness, repository, "Claude Sonnet");
    harness.capture(&format!("message-box-idle-{suffix}"));

    // 2. Context use shown.
    harness.update(|plait, _, cx| {
        Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap()).usage = Some(Usage {
            input: 12_400,
            output: 1_800,
            cache_read: 141_800,
            cache_write: 0,
            reasoning: Some(900),
            total: 156_000,
            context_used: Some(156_000),
            context_window: Some(200_000),
        });
        plait.sync_composer(cx);
        cx.notify();
    });
    harness.capture(&format!("message-box-context-{suffix}"));

    // 3. Running thread (Stop visible) + locked control with its reason.
    harness.update(|plait, window, cx| {
        let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
        snapshot.status = ThreadStatus::Running;
        snapshot.approval_mode = Some(ApprovalMode::Ask);
        plait.sync_composer(cx);
        // Activating the locked model control records its lock reason in the box.
        plait.open_thread_models(window, cx);
    });
    harness.capture(&format!("message-box-running-locked-{suffix}"));

    // 4. Different-approval-mode notice during a run.
    harness.update(|plait, _, cx| {
        let id = plait.host.thread.selected.clone().unwrap();
        plait.workspace.thread_controls.note = None;
        plait.set_thread_approval(id, Some(ApprovalMode::Auto), cx);
        plait.sync_composer(cx);
    });
    harness.capture(&format!("message-box-mode-differs-{suffix}"));

    // 5. Narrow-friendly wrapping with long model name, effort, approval, context, Stop, Send.
    harness.update(|plait, _, cx| {
        configure_reasoning_model(
            plait,
            "Sample model with an exceptionally long readable model name",
            Some(200_000),
        );
        plait.sync_composer(cx);
        cx.notify();
    });
    harness.capture(&format!("message-box-wrap-{suffix}"));

    // 6. Harness with its own permission mode.
    setup_codex_thread(harness);
    harness.capture(&format!("message-box-harness-permissions-{suffix}"));

    // 7. Draft tab: project and harness above, model/effort/permissions inside the box.
    harness.update(|plait, window, cx| {
        configure_reasoning_model(plait, "Claude Sonnet", Some(200_000));
        plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
        plait.open_draft(window, cx);
    });
    harness.type_draft("Add retries to the status refresh");
    harness.capture(&format!("message-box-draft-{suffix}"));
}

#[core::prelude::v1::test]
fn message_box_states_name_their_controls_and_notices_at_both_sizes() {
    if !isolated("message_box::message_box_states_name_their_controls_and_notices_at_both_sizes") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-message-box-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut harness = Harness::new(&repository, None, width, height);

        // 1. Idle thread, context use absent.
        setup_idle_thread(&mut harness, &repository, "Claude Sonnet");
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("message-box idle ({suffix})"));
            assert!(frame.has_node(Role::Image, |n| n.label.as_deref() == Some("Pi")));
            assert!(frame.has_node(Role::Button, |n| {
                n.label.as_deref() == Some("Change thread model: Claude Sonnet")
            }));
            assert!(frame.has_node(Role::Button, |n| {
                n.label.as_deref() == Some("Thinking level: Low")
            }));
            assert!(frame.has_node(Role::Button, |n| {
                n.label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Run settings: "))
            }));
            assert!(frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Send")));
            assert!(!frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Stop")));
            assert!(!frame.has_node(Role::Button, |n| {
                n.label.as_deref().is_some_and(|l| l.contains("context"))
            }));
        }
        #[cfg(not(debug_assertions))]
        harness.draw();

        // An older host cannot change a thread's settings: the box names
        // what the thread runs with instead of offering the controls.
        let mut protocol = 0;
        harness.update(|plait, _, _| {
            let status = plait.host.connection.status.as_mut().unwrap();
            protocol = std::mem::replace(&mut status.protocol_version, 2);
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("message-box older host ({suffix})"));
            let named = |wanted: &[&str]| {
                frame.has_node(Role::Label, |n| {
                    n.value
                        .as_deref()
                        .or(n.label.as_deref())
                        .is_some_and(|l| wanted.iter().any(|part| l.contains(part)))
                })
            };
            assert!(named(&[" / "]), "{suffix}: the saved model is named");
            // Whichever mode the next message uses, it is said in words.
            assert!(
                named(&["Approve each action", "Automatic approval"]),
                "{suffix}: how actions are approved is named"
            );
            assert!(!frame.has_node(Role::Button, |n| {
                n.label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Change thread model"))
            }));
            assert!(frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Send")));
        }
        harness.update(|plait, _, _| {
            plait
                .host
                .connection
                .status
                .as_mut()
                .unwrap()
                .protocol_version = protocol;
        });

        // 2. Context use shown + toggling expanded detail.
        harness.update(|plait, _, cx| {
            Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap()).usage = Some(Usage {
                input: 10_000,
                output: 2_000,
                cache_read: 144_000,
                cache_write: 0,
                reasoning: None,
                total: 156_000,
                context_used: Some(156_000),
                context_window: Some(200_000),
            });
            plait.workspace.thread_controls.usage_open = true;
            cx.notify();
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("message-box context ({suffix})"));
            assert!(frame.has_node(Role::Button, |n| {
                n.label.as_deref() == Some("78% context high")
            }));
            assert!(frame.has_node(Role::Label, |n| {
                n.value
                    .as_deref()
                    .or(n.label.as_deref())
                    .is_some_and(|l| l.contains("Context 156000 / 200000 tokens"))
            }));
        }
        #[cfg(not(debug_assertions))]
        harness.draw();

        // 3. Running thread: Stop visible, model locked with its reason, mode-differs notice.
        harness.update(|plait, window, cx| {
            plait.workspace.thread_controls.usage_open = false;
            let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
            snapshot.status = ThreadStatus::Running;
            snapshot.approval_mode = Some(ApprovalMode::Ask);
            let id = snapshot.id.clone();
            plait.set_thread_approval(id, Some(ApprovalMode::Auto), cx);
            plait.sync_composer(cx);
            plait.open_thread_models(window, cx);
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("message-box running ({suffix})"));
            assert!(frame.has_node(Role::Button, |n| n.label.as_deref() == Some("Stop")));
            assert!(frame.has_node(Role::Label, |n| {
                n.value.as_deref().or(n.label.as_deref())
                    == Some("Available after the current run ends.")
            }));
            assert!(frame.has_node(Role::Label, |n| {
                n.value.as_deref().or(n.label.as_deref())
                    == Some(
                        "This run asks before actions. Auto-approve actions applies from your next message.",
                    )
            }));
        }
        #[cfg(not(debug_assertions))]
        harness.draw();

        // 4. Harness with its own permission mode; picker anchors to its control.
        setup_codex_thread(&mut harness);
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("message-box harness ({suffix})"));
            assert!(frame.has_node(Role::Button, |n| {
                n.label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Permissions: "))
            }));
        }
        #[cfg(not(debug_assertions))]
        harness.draw();
        harness.update(|plait, _, _| {
            assert!(
                plait.anchors.borrow().contains_key("thread-permissions"),
                "thread-permissions records its anchor bounds ({suffix})"
            );
            assert!(
                plait.anchors.borrow().contains_key("thread-model"),
                "thread-model records its anchor bounds ({suffix})"
            );
        });

        // 5. Draft tab: project and harness above the box, model/effort/permissions inside.
        harness.update(|plait, window, cx| {
            configure_reasoning_model(plait, "Claude Sonnet", Some(200_000));
            plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
            plait.open_draft(window, cx);
        });
        harness.type_draft("Draft message");
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("message-box draft ({suffix})"));
            assert!(frame.has_node(Role::Button, |n| {
                n.label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Project: "))
            }));
            assert!(frame.has_node(Role::Button, |n| {
                n.label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Harness: "))
            }));
            assert!(frame.has_node(Role::Button, |n| {
                n.label.as_deref().is_some_and(|l| l.starts_with("Model: "))
            }));
            assert!(frame.has_node(Role::Button, |n| {
                n.label
                    .as_deref()
                    .is_some_and(|l| l.starts_with("Permissions: "))
            }));
        }
        #[cfg(not(debug_assertions))]
        harness.draw();
        harness.update(|plait, _, _| {
            let anchors = plait.anchors.borrow();
            let project = anchors.get("draft-project").copied().expect("draft-project anchor");
            let harness_btn = anchors.get("draft-harness").copied().expect("draft-harness anchor");
            let model = anchors.get("draft-model").copied().expect("draft-model anchor");
            let perms = anchors
                .get("draft-permissions")
                .copied()
                .expect("draft-permissions anchor");
            assert!(
                project.top() < model.top() && harness_btn.top() < model.top(),
                "project and harness sit above the message box ({suffix}): project={project:?}, harness={harness_btn:?}, model={model:?}"
            );
            assert!(
                perms.top() >= model.top(),
                "permissions sits inside the message box settings row ({suffix}): model={model:?}, perms={perms:?}"
            );
        });
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
