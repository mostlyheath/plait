//! Modal routes, focus containment and workspace preservation, without native input.
use super::*;
use crate::ui::{Workspace, selection::Owner};

fn click(harness: &mut Harness, x: f32, y: f32) {
    harness.draw();
    let position = point(px(x), px(y));
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

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.live_text = "Sample streaming text continues arriving.".into();
        snapshot.messages[1].text =
            "Sample reply streaming behind Settings; the thread keeps running.".into();
        snapshot.status = ThreadStatus::Running;
    });
    harness.update(|plait, window, cx| plait.show_settings(true, window, cx));
    harness.capture(&format!("tabs-settings-modal-{suffix}"));
    if suffix == "narrow" {
        harness.capture("settings-minimum-900");
    }
    harness.update(|plait, window, cx| plait.show_project_settings(window, cx));
    harness.capture(&format!("tabs-project-modal-{suffix}"));
    harness.connect(repository, true);
    harness.update(|plait, window, cx| plait.show_settings(true, window, cx));
    harness.capture(&format!("tabs-modal-approval-shield-{suffix}"));
    harness.update(|plait, window, cx| plait.show_settings(false, window, cx));
}

#[core::prelude::v1::test]
fn routes_close_and_restore_focus_from_every_view() {
    if !isolated("settings_modal::routes_close_and_restore_focus_from_every_view") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-routes-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    for view in [
        Workspace::Conversation,
        Workspace::Changes,
        Workspace::History,
        Workspace::Files,
    ] {
        harness.update(|plait, window, cx| {
            plait.workspace.tab = view;
            window.focus(&plait.focus, cx);
        });
        harness.draw();
        for route in 0..4 {
            harness.keystroke("cmd-,");
            harness.update(|plait, window, cx| {
                assert!(plait.settings_open);
                assert_eq!(plait.workspace.tab, view);
                assert!(plait.settings_focus.contains_focused(window, cx));
            });
            match route {
                0 => harness.keystroke("escape"),
                1 => harness.keystroke("cmd-,"),
                2 => click(&mut harness, 805., 54.),
                _ => click(&mut harness, 12., 140.),
            }
            harness.update(|plait, window, _| {
                assert!(!plait.settings_open, "close route {route}");
                assert_eq!(plait.workspace.tab, view);
                assert!(
                    plait.focus.is_focused(window),
                    "focus returns for route {route}"
                );
            });
        }
    }
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn focus_cycles_inside_and_workspace_shortcuts_are_inert() {
    if !isolated("settings_modal::focus_cycles_inside_and_workspace_shortcuts_are_inert") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-focus-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, true);
    harness.update(|plait, window, cx| {
        plait.focus_composer(window, cx);
        plait.show_settings(true, window, cx);
    });
    harness.draw();
    for key in ["tab", "shift-tab"] {
        for _ in 0..60 {
            harness.keystroke(key);
            harness.update(|plait, window, cx| {
                assert!(plait.settings_focus.contains_focused(window, cx))
            });
        }
    }
    for key in [
        "cmd-shift-2",
        "cmd-shift-3",
        "cmd-n",
        "cmd-p",
        "cmd-b",
        "cmd-l",
        "cmd-2",
        "cmd-f",
        "cmd-alt-y",
        "cmd-alt-.",
        "cmd-k",
        "cmd-w",
    ] {
        harness.keystroke(key);
        harness.update(|plait, _, _| {
            assert!(plait.settings_open, "{key}");
            assert_eq!(plait.workspace.tab, Workspace::Conversation, "{key}");
            assert_eq!(plait.host.thread.selected.as_deref(), Some(THREAD), "{key}");
            assert!(!plait.workspace.conversation_find.open, "{key}");
            assert!(!plait.draft.open, "{key}");
            assert!(plait.host.thread.decided_approvals.is_empty(), "{key}");
        });
    }
    harness.keystroke("escape");
    harness.update(|plait, window, cx| {
        assert!(
            plait.workspace.composers[THREAD]
                .focus_handle(cx)
                .is_focused(window)
        )
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn preserves_reading_find_draft_and_accepts_a_reply() {
    if !isolated("settings_modal::preserves_reading_find_draft_and_accepts_a_reply") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-reading-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.change_snapshot(reading);
    harness.composer(|composer, window, cx| composer.type_text("Unsent draft", window, cx));
    harness.update(|plait, window, cx| {
        plait.open_conversation_find(window, cx);
        plait
            .workspace
            .conversation_find
            .field
            .clone()
            .unwrap()
            .update(cx, |field, cx| field.set_text("sample", cx));
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });
    harness.draw();
    let position = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait
                .text_point(&Owner::Message("r2".into()), 0, 4)
                .unwrap()
        })
        .unwrap();
    harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 2,
        ..Default::default()
    }));
    harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 2,
        ..Default::default()
    }));
    harness.draw();
    let before = harness
        .window
        .update(&mut harness.cx, |plait, _, cx| {
            (
                plait.workspace.formatted.clone(),
                plait.workspace.live_scroll.logical_scroll_top(),
                plait.selected_text(),
                plait.workspace.composers[THREAD]
                    .read(cx)
                    .saved_draft()
                    .0
                    .to_owned(),
                plait.host.thread.watch_epoch,
            )
        })
        .unwrap();
    assert!(before.2.is_some(), "fixture has a selection");
    harness.keystroke("cmd-,");
    harness.draw();
    harness.keystroke("escape");
    harness.update(|plait, window, cx| {
        assert!(
            Arc::ptr_eq(&before.0, &plait.workspace.formatted),
            "opening does not rebuild conversation"
        );
        assert_eq!(before.1, plait.workspace.live_scroll.logical_scroll_top());
        assert_eq!(before.2, plait.selected_text());
        assert_eq!(
            before.3,
            plait.workspace.composers[THREAD].read(cx).saved_draft().0
        );
        assert_eq!(before.4, plait.host.thread.watch_epoch);
        assert!(plait.workspace.conversation_find.open);
        assert_eq!(
            plait
                .workspace
                .conversation_find
                .field
                .as_ref()
                .unwrap()
                .read(cx)
                .text(),
            "sample"
        );
        plait.show_settings(true, window, cx);
    });
    harness.update(|plait, window, cx| {
        let mut reply = (**plait.host.thread.snapshot.as_ref().unwrap()).clone();
        reply.messages.push(Message {
            id: "modal-reply".into(),
            role: MessageRole::Assistant,
            text: "Arrived while settings was open".into(),
            ..Default::default()
        });
        assert!(plait.host.apply_snapshot(
            plait.host.connection.epoch,
            plait.host.thread.watch_epoch,
            reply
        ));
        plait.conversation_changed(window, cx);
    });
    harness.update(|plait, _, _| {
        assert!(plait.settings_open);
        assert_eq!(
            plait
                .host
                .thread
                .snapshot
                .as_ref()
                .unwrap()
                .messages
                .last()
                .unwrap()
                .id,
            "modal-reply"
        );
    });
    harness.keystroke("escape");
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn escape_cancels_recording_and_secret_form_before_the_modal() {
    if !isolated("settings_modal::escape_cancels_recording_and_secret_form_before_the_modal") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-escape-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait.show_settings(true, window, cx);
        plait.settings_tab = SettingsTab::Keyboard;
        plait.record_shortcut(0, window, cx);
    });
    harness.draw();
    harness.keystroke("escape");
    harness.update(|plait, _, cx| {
        assert!(plait.settings_open);
        assert!(plait.keyboard.recording.is_none());
        plait.settings_tab = SettingsTab::Providers;
        let field = cx.new(|cx| crate::ui::text_field::TextField::new("API key", "", cx).secret());
        plait.providers.form = Some(crate::ui::settings_providers::Form::Key {
            provider: "fixture".into(),
            field,
        });
    });
    harness.draw();
    harness.keystroke("escape");
    harness.update(|plait, _, _| {
        assert!(plait.settings_open);
        assert!(plait.providers.form.is_none());
    });
    harness.keystroke("escape");
    harness.update(|plait, _, _| assert!(!plait.settings_open));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn project_replies_keep_their_target_after_dismissal() {
    if !isolated("settings_modal::project_replies_keep_their_target_after_dismissal") {
        return;
    }
    use crate::ui::settings_agent::AgentView;
    use plait_client::host::{
        agent::AgentResources,
        policy::{CommandPolicy, PolicyReply},
    };
    let scratch = std::env::temp_dir().join(format!("plait-modal-target-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        let original = plait.host.creation.checkout.clone();
        let target = original.join("other");
        plait.show_settings_for_project(target.clone(), window, cx);
        let key = (plait.host.connection.epoch, target.clone());
        let revision = plait.project_agent().revision;
        plait.project_policy_mut().busy = true;
        plait.project_agent_mut().busy = true;
        plait.show_settings_for_project(original.clone(), window, cx);
        assert_eq!(
            plait.settings_checkout(),
            target,
            "pending request fixes the target"
        );
        plait.show_settings(false, window, cx);
        let reply = || {
            Ok(AgentView::new(AgentResources {
                checkout: target.clone(),
                trusted: true,
                instructions: None,
                skills: vec![],
                templates: vec![],
                problems: vec![],
                mcp_servers: vec![],
            }))
        };
        assert!(!plait.accept_agent(&(key.0 + 1, key.1.clone()), revision, reply()));
        assert!(!plait.accept_agent(&key, revision + 1, reply()));
        assert!(plait.accept_agent(&key, revision, reply()));
        assert!(plait.project_agent().view.is_some());
        assert!(
            plait.agent.view.is_none(),
            "active checkout data is untouched"
        );
        assert!(plait.accept_policy(
            &key,
            Some(CommandPolicy::Trusted),
            Ok(PolicyReply {
                checkout: target.clone(),
                policy: CommandPolicy::Trusted
            }),
            cx
        ));
        assert_eq!(plait.project_policy().value, Some(CommandPolicy::Trusted));
        assert_eq!(plait.policy.value, None);
        assert!(!plait.settings_open, "a reply never reopens the modal");
        assert_eq!(plait.host.creation.checkout, original);
        assert_eq!(plait.host.thread.selected.as_deref(), Some(THREAD));
        assert!(!plait.accept_policy(
            &(key.0 + 1, key.1),
            None,
            Err(plait_client::host::Error::Protocol("stale".into())),
            cx
        ));
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn a_settings_picker_closes_before_the_modal() {
    if !isolated("settings_modal::a_settings_picker_closes_before_the_modal") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-picker-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait.show_settings(true, window, cx);
        plait.open_model_picker(ModelPurpose::DefaultModel, window, cx);
    });
    harness.draw();
    harness.keystroke("escape");
    harness.update(|plait, window, cx| {
        assert!(plait.settings_open);
        assert!(plait.overlay.is_none());
        assert!(plait.settings_focus.contains_focused(window, cx));
    });
    harness.keystroke("escape");
    harness.update(|plait, _, _| assert!(!plait.settings_open));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn menu_palette_and_sidebar_foot_open_settings() {
    if !isolated("settings_modal::menu_palette_and_sidebar_foot_open_settings") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-open-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::with_host(&repository, None, 900., 600., None);
    harness.connect(&repository, false);
    harness.draw();
    let rail = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait.panel_width(crate::ui::panels::Panel::Sidebar)
        })
        .unwrap();
    click(&mut harness, rail - 20., 577.);
    harness.update(|plait, _, _| assert!(plait.settings_open, "sidebar foot"));
    harness.keystroke("escape");
    harness.update(|_, window, cx| window.dispatch_action(Box::new(crate::ui::Settings), cx));
    harness.update(|plait, _, _| assert!(plait.settings_open, "menu action"));
    harness.keystroke("escape");
    for (query, page) in [
        ("Settings…", SettingsPage::Application),
        ("Project Settings…", SettingsPage::Project),
    ] {
        harness.keystroke("cmd-k");
        set_query(&mut harness, query);
        harness.draw();
        harness.keystroke("enter");
        harness.update(|plait, _, _| {
            assert!(plait.settings_open, "palette {query}");
            assert_eq!(plait.settings_page, page);
        });
        harness.keystroke("escape");
    }
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A creation reply that the person is waiting for, without a draft.
fn created(repository: &Path, request: &str, selection_epoch: u64) -> crate::ui::host::Created {
    crate::ui::host::Created {
        draft_id: None,
        request_id: request.into(),
        choice: choice(),
        harness: plait_client::host::harness::HarnessId::Pi,
        selection_epoch,
        checkout: Some(repository.to_owned()),
    }
}

fn created_reply(
    repository: &Path,
    id: &str,
) -> Result<plait_client::host::ThreadReply, plait_client::host::Error> {
    Ok(plait_client::host::ThreadReply {
        thread: Snapshot {
            id: id.into(),
            checkout: repository.to_owned(),
            model: choice(),
            ..Default::default()
        },
    })
}

#[core::prelude::v1::test]
fn a_thread_created_under_the_modal_opens_behind_it() {
    if !isolated("settings_modal::a_thread_created_under_the_modal_opens_behind_it") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-create-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        // As the model picker leaves things while the host creates the thread.
        plait.new_thread_open = true;
        plait.host.creation.pending_create = Some(("picked".into(), choice()));
        plait.host.creation.creating = true;
        plait.show_settings(true, window, cx);
    });
    harness.draw();
    harness.update(|plait, window, cx| {
        let epoch = plait.host.thread.watch_epoch;
        plait.finish_create(
            created(&repository, "picked", epoch),
            created_reply(&repository, "thr_picked"),
            window,
            cx,
        );
        assert!(plait.settings_open, "the modal stays open");
        assert!(plait.settings_focus.contains_focused(window, cx));
        assert_eq!(plait.host.thread.selected.as_deref(), Some("thr_picked"));
    });
    harness.draw();
    harness.keystroke("escape");
    harness.update(|plait, window, cx| {
        assert!(!plait.settings_open);
        assert!(
            plait.workspace.composers["thr_picked"]
                .focus_handle(cx)
                .is_focused(window),
            "closing lands in the new thread's composer"
        );
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn a_sent_draft_stays_in_place_and_opens_behind_the_modal() {
    if !isolated("settings_modal::a_sent_draft_stays_in_place_and_opens_behind_the_modal") {
        return;
    }
    use crate::ui::new_thread::Sending;
    let scratch = std::env::temp_dir().join(format!("plait-modal-draft-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait.open_draft(window, cx);
        // As `send_draft` leaves things, without a socket.
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        super::many_drafts::set_pending(plait, "drafted");
        plait.draft.sending = Some(Sending {
            images: Vec::new(),
            generation: plait.draft.generation,
            request_id: "drafted".into(),
            text: "Sent before Settings".into(),
            revision: 1,
            approval: Some(ApprovalMode::Ask),
            choice: Some(ApprovalMode::Ask),
        });
        let bar = plait.creation_bar(cx).is_some();
        plait.show_settings(true, window, cx);
        assert!(plait.draft_visible(), "the draft is still on screen");
        assert_eq!(
            plait.creation_bar(cx).is_some(),
            bar,
            "the workspace keeps its rows"
        );
    });
    harness.draw();
    harness.update(|plait, window, cx| {
        super::many_drafts::finish(
            plait,
            "drafted",
            created_reply(&repository, "thr_drafted"),
            window,
            cx,
        );
        assert!(plait.settings_open);
        assert_eq!(
            plait.host.thread.selected.as_deref(),
            Some("thr_drafted"),
            "the draft's thread opens where the draft was"
        );
        assert_eq!(
            plait.host.submissions.all("thr_drafted")[0].text,
            "Sent before Settings"
        );
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn escape_keeps_the_current_policy_before_the_modal() {
    if !isolated("settings_modal::escape_keeps_the_current_policy_before_the_modal") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-trust-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait.show_project_settings(window, cx);
        plait.project_policy_mut().confirm = true;
    });
    harness.draw();
    harness.keystroke("escape");
    harness.update(|plait, window, cx| {
        assert!(plait.settings_open, "Escape closes the confirmation first");
        assert!(!plait.project_policy().confirm);
        assert!(plait.settings_focus.contains_focused(window, cx));
    });
    harness.keystroke("escape");
    harness.update(|plait, _, _| assert!(!plait.settings_open));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// GPUI runs an assistive action on its element directly, past the scrim and
/// the action shield; the test platform cannot deliver one, so this checks
/// the gate those actions consult and the focus an assistive Focus moves.
#[core::prelude::v1::test]
fn assistive_actions_reach_only_the_modal_and_what_opens_above_it() {
    if !isolated("settings_modal::assistive_actions_reach_only_the_modal_and_what_opens_above_it") {
        return;
    }
    use crate::ui::theme::beneath_modal;
    let scratch = std::env::temp_dir().join(format!("plait-modal-assist-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    // GPUI reports focus moving out only in an active window.
    harness.update(|_, window, _| window.activate_window());
    harness.draw();
    let composer = harness
        .window
        .update(&mut harness.cx, |plait, window, cx| {
            let composer = plait.workspace.composers[THREAD].focus_handle(cx);
            assert!(
                !beneath_modal(None, window, cx),
                "nothing covers the window"
            );
            plait.show_settings(true, window, cx);
            plait.settings_tab = SettingsTab::Keyboard;
            composer
        })
        .unwrap();
    harness.draw();
    harness.update(|plait, window, cx| {
        let search = plait.keyboard.search.as_ref().unwrap().focus_handle(cx);
        assert!(beneath_modal(None, window, cx), "a workspace button");
        assert!(beneath_modal(Some(&composer), window, cx), "the composer");
        assert!(!beneath_modal(Some(&search), window, cx), "a modal field");
        // As an assistive Focus action on the composer does.
        window.focus(&composer, cx);
    });
    harness.draw();
    harness.update(|plait, window, cx| {
        assert!(
            plait.settings_focus.contains_focused(window, cx),
            "focus returns to the modal"
        );
        plait.open_model_picker(ModelPurpose::DefaultModel, window, cx);
    });
    harness.draw();
    harness.update(|plait, window, cx| {
        let picker = plait.overlay.as_ref().unwrap().palette.focus_handle(cx);
        assert!(
            !beneath_modal(Some(&picker), window, cx),
            "a picker above it"
        );
        assert!(picker.is_focused(window), "a picker keeps the keyboard");
        plait.close_overlay(true, window, cx);
        plait.show_settings(false, window, cx);
    });
    harness.draw();
    harness.update(|_, window, cx| assert!(!beneath_modal(None, window, cx)));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn modal_blocks_tab_keys_pointer_and_assistive_focus() {
    if !isolated("settings_modal::modal_blocks_tab_keys_pointer_and_assistive_focus") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-tabs-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    super::tabs::install(&mut h, 20);
    h.update(|p, window, cx| {
        p.host.connection.connected = false;
        window.activate_window();
        window.focus(&p.focus, cx);
    });
    h.draw();
    h.keystroke("cmd-9");
    h.keystroke("cmd-w");
    h.keystroke("cmd-1");
    h.draw();
    h.update(|p, window, cx| window.focus(&p.focus, cx));
    h.keystroke("tab");
    h.keystroke("tab");
    let (ids, active, row_focus) = h
        .window
        .update(&mut h.cx, |p, window, cx| {
            assert_eq!(p.tabs.model.closed_len(), 1);
            (
                p.tabs
                    .model
                    .tabs()
                    .iter()
                    .map(|tab| tab.id())
                    .collect::<Vec<_>>(),
                p.tabs.model.active(),
                window.focused(cx).unwrap(),
            )
        })
        .unwrap();
    h.keystroke("cmd-shift-\\");
    h.update(|p, _, _| assert!(p.tabs.list_open));
    for project in [false, true] {
        h.update(|p, window, cx| {
            if project {
                p.show_project_settings(window, cx);
            } else {
                p.show_settings(true, window, cx);
            }
            assert!(!p.tabs.list_open, "opening settings dismisses overflow");
        });
        h.draw();
        for key in [
            "cmd-1",
            "cmd-2",
            "cmd-3",
            "cmd-4",
            "cmd-5",
            "cmd-6",
            "cmd-7",
            "cmd-8",
            "cmd-9",
            "cmd-w",
            "cmd-shift-w",
            "cmd-shift-t",
            "cmd-t",
            "cmd-n",
            "cmd-shift-[",
            "cmd-shift-]",
            "ctrl-tab",
            "ctrl-shift-tab",
            "ctrl-shift-left",
            "ctrl-shift-right",
            "cmd-shift-\\",
        ] {
            h.keystroke(key);
            h.update(|p, window, cx| {
                assert!(p.settings_open, "{key}");
                assert_eq!(p.tabs.model.active(), active, "{key}");
                assert_eq!(
                    p.tabs
                        .model
                        .tabs()
                        .iter()
                        .map(|tab| tab.id())
                        .collect::<Vec<_>>(),
                    ids,
                    "{key}"
                );
                assert_eq!(p.tabs.model.closed_len(), 1, "{key}");
                assert!(!p.tabs.list_open, "{key}");
                assert!(p.settings_focus.contains_focused(window, cx), "{key}");
            });
        }
        h.update(|_, window, cx| {
            assert!(crate::ui::theme::beneath_modal(
                Some(&row_focus),
                window,
                cx
            ));
            // The same focus path used by an assistive action on a cached row.
            window.focus(&row_focus, cx);
        });
        h.draw();
        h.update(|p, window, cx| assert!(p.settings_focus.contains_focused(window, cx)));
        for button in [MouseButton::Middle, MouseButton::Left] {
            h.draw();
            h.pointer(PlatformInput::MouseDown(MouseDownEvent {
                button,
                position: point(px(350.), px(20.)),
                click_count: 1,
                ..Default::default()
            }));
            h.pointer(PlatformInput::MouseUp(MouseUpEvent {
                button,
                position: point(px(350.), px(20.)),
                click_count: 1,
                ..Default::default()
            }));
            h.update(|p, window, cx| {
                assert_eq!(p.tabs.model.active(), active);
                assert_eq!(
                    p.tabs
                        .model
                        .tabs()
                        .iter()
                        .map(|tab| tab.id())
                        .collect::<Vec<_>>(),
                    ids
                );
                // An outside click may dismiss Settings; it never reaches the row below.
                p.show_settings(true, window, cx);
            });
        }
        h.draw();
        h.keystroke("escape");
        h.update(|p, _, _| {
            assert!(!p.settings_open);
            assert!(!p.tabs.list_open);
            assert_eq!(p.tabs.model.active(), active);
            assert_eq!(p.tabs.model.closed_len(), 1);
        });
    }
    h.draw();
    h.keystroke("cmd-2");
    h.update(|p, _, _| assert_ne!(p.tabs.model.active(), active));
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn creation_parking_finishes_beneath_the_modal() {
    if !isolated("settings_modal::creation_parking_finishes_beneath_the_modal") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-modal-save-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    let (journal, _) =
        plait_client::session::Journal::open(&scratch.join("session"), "/fixture-host".into())
            .unwrap();
    h.update(|p, window, cx| {
        p.host.connection.connected = false;
        p.session.journal = Some(journal);
        p.new_thread_open = true;
        p.host.creation.pending_create = Some(("picked".into(), choice()));
        p.host.creation.creating = true;
        p.show_settings(true, window, cx);
        let epoch = p.host.thread.watch_epoch;
        p.finish_create(
            created(&repository, "picked", epoch),
            created_reply(&repository, "thr_picked"),
            window,
            cx,
        );
        assert!(p.settings_open);
    });
    undo::wait(&mut h, |p| {
        !p.tabs.lifecycle.busy && p.host.thread.selected.as_deref() == Some("thr_picked")
    });
    h.draw();
    h.update(|p, window, cx| {
        assert!(p.settings_open);
        assert!(p.settings_focus.contains_focused(window, cx));
    });
    h.keystroke("escape");
    h.update(|p, window, cx| {
        assert!(!p.settings_open);
        assert!(
            p.workspace.composers["thr_picked"]
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
