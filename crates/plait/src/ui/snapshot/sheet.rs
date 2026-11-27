use super::*;
use plait_client::update::{Manifest, Offer, Version};

/// Use the sidebar fixture's mixed thread states so its attention count is visible.
pub(super) fn walk(h: &mut Harness, suffix: &str) {
    // Keep the captures independent of the workstation and scratch path.
    let (machine, panel_open) = h
        .window
        .update(&mut h.cx, |p, _, _| {
            (
                std::mem::replace(&mut p.state.projects[0].machines[0].name, "studio".into()),
                p.tabs
                    .model
                    .panel_mut()
                    .map(|panel| std::mem::take(&mut panel.open)),
            )
        })
        .unwrap();
    h.update(|p, _, _| {
        assert_eq!(p.active_project().machines[0].name, "studio");
        assert!(
            !p.panel_is_open(),
            "sheet captures show the conversation with the panel closed"
        );
    });
    capture_both(h, &format!("sheet-{suffix}"));
    h.update(|p, window, _| {
        let body = p.workspace.sheet_body;
        let rail = p.panel_width(crate::ui::panels::Panel::Sidebar);
        assert!(body.left() >= px(rail));
        assert!(body.right() <= window.viewport_size().width - px(8.));
        assert!(body.bottom() <= window.viewport_size().height - px(8.));
    });
    h.update(|p, _, _| p.sidebar = false);
    capture_both(h, &format!("sheet-hidden-{suffix}"));
    h.update(|p, window, _| {
        let body = p.workspace.sheet_body;
        assert_eq!(body.left(), px(0.));
        assert_eq!(body.right(), window.viewport_size().width);
    });
    #[cfg(debug_assertions)]
    assert!(
        !h.draw_a11y()
            .nodes
            .iter()
            .any(|n| n.path.contains("machine-switcher"))
    );

    h.update(|p, _, _| p.sidebar = true);
    foot(h, suffix, "connected", "Connected");
    h.update(|p, _, _| p.navigation.menu = Some("machines".into()));
    h.capture(&format!("sidebar-foot-menu-{suffix}"));
    #[cfg(debug_assertions)]
    assert!(h.draw_a11y().has_node(Role::Button, |n| n.label.as_deref()
        == Some("Manage machines…")));

    h.update(|p, _, _| {
        p.navigation.menu = None;
        p.machines.connecting = Some(p.machines.active.clone());
    });
    foot(h, suffix, "connecting", "Connecting…");
    h.update(|p, _, _| {
        p.machines.connecting = None;
        p.host.connection.connected = false;
    });
    foot(h, suffix, "offline", "Offline");
    picker_scene(h, suffix);
    h.update(|p, _, _| {
        p.host.connection.connected = true;
        p.updates = update_offer();
    });
    foot(h, suffix, "update", "Connected");
    h.update(move |p, _, _| {
        p.updates = Default::default();
        p.state.projects[0].machines[0].name = machine;
        if let (Some(panel), Some(open)) = (p.tabs.model.panel_mut(), panel_open) {
            panel.open = open;
        }
    });
}

fn picker_scene(h: &mut Harness, suffix: &str) {
    let saved = h
        .window
        .update(&mut h.cx, |p, _, _| p.preferences.value.machines.clone())
        .unwrap();
    h.update(|p, window, cx| {
        use plait_client::{machines::Transport, remote::ssh::SshDestination};
        let target = p
            .preferences
            .value
            .machines
            .add(
                "Studio",
                Transport::Ssh {
                    destination: SshDestination::new("dev@studio").unwrap(),
                    port: None,
                    store: None,
                },
            )
            .unwrap();
        p.preferences
            .value
            .machines
            .add(
                "Build host",
                Transport::Tailnet {
                    host: "builder".into(),
                    port: 7744,
                },
            )
            .unwrap();
        p.machines.failed_switch = Some(target);
        p.open_machine_picker(window, cx);
    });
    capture_both(h, &format!("machine-picker-offline-failed-{suffix}"));
    h.update(move |p, _, _| {
        p.preferences.value.machines = saved;
        p.navigation.menu = None;
        p.machines.failed_switch = None;
    });
}

#[core::prelude::v1::test]
#[ignore = "renders the machine picker"]
fn render_machine_picker() {
    if !isolated("sheet::render_machine_picker") {
        return;
    }
    let output = PathBuf::from(std::env::var_os("PLAIT_SNAPSHOT_DIR").unwrap());
    let scratch = std::env::temp_dir().join(format!("picker-scene-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    for (suffix, width, height) in [("wide", 1440., 900.), ("narrow", 680., 620.)] {
        let mut h = Harness::with_host(&repo, Some(&output), width, height, None);
        h.connect(&repo, false);
        h.update(|p, _, _| {
            p.host.connection.connected = false;
        });
        picker_scene(&mut h, suffix);
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

fn foot(h: &mut Harness, suffix: &str, name: &str, _state: &str) {
    h.capture(&format!("sidebar-foot-{name}-{suffix}"));
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        let button =
            |label: &str| frame.has_node(Role::Button, |n| n.label.as_deref() == Some(label));
        assert!(button("Needs you, 4"));
        assert!(button("Settings…"));
        assert_eq!(
            button("Update available. Open Settings to install."),
            name == "update"
        );
        let machine = frame
            .nodes
            .iter()
            .find(|n| n.path.contains("machine-switcher") && n.role == Role::Button)
            .unwrap();
        assert!(
            machine
                .label
                .as_deref()
                .unwrap()
                .ends_with(&format!(", {_state}. Switch machine…"))
        );
    }
}

fn update_offer() -> super::super::update::UpdateState {
    super::super::update::UpdateState::Available(Arc::new(Offer {
        version: Version::parse("1.2.3").unwrap(),
        manifest: Manifest {
            schema_version: 1,
            version: "1.2.3".into(),
            tag: "v1.2.3".into(),
            revision: "scene".into(),
            host_build_id: "scene".into(),
            artifacts: Default::default(),
        },
    }))
}

fn picker_harness(test: &str) -> Option<(Harness, PathBuf)> {
    if !isolated(test) {
        return None;
    }
    let scratch = std::env::temp_dir().join(format!("plait-picker-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::with_host(&repo, None, 900., 600., None);
    h.connect(&repo, false);
    Some((h, scratch))
}

#[core::prelude::v1::test]
fn machine_picker_focus_and_composer_keys() {
    let Some((mut h, scratch)) = picker_harness("sheet::machine_picker_focus_and_composer_keys")
    else {
        return;
    };
    h.update(|p, w, cx| {
        p.workspace.composers[THREAD].update(cx, |c, cx| {
            c.set_enabled(true, None, cx);
            c.set_busy(true, cx);
            c.set_queue_enabled(true, true, cx);
            c.set_send_blocked(None, cx);
            c.restore_draft("unsent".into(), 1, None, cx);
        });
        p.focus_composer(w, cx);
    });
    for (key, move_focus) in [
        ("escape", false),
        ("enter", false),
        ("escape", true),
        ("enter", true),
    ] {
        h.keystroke("ctrl-m");
        h.draw();
        h.update(|p, w, cx| {
            assert_eq!(p.navigation.menu.as_deref(), Some("machines"));
            assert!(p.machine_trigger.is_focused(w));
            let c = p.workspace.composers[THREAD].read(cx);
            assert!(c.can_send() && c.can_stop());
        });
        if move_focus {
            let composer = h
                .window
                .update(&mut h.cx, |p, _, cx| {
                    p.workspace.composers[THREAD].focus_handle(cx)
                })
                .unwrap();
            // Two inputs in one frame: the focus-out listener has not run yet.
            h.cx.update_window(h.window.into(), |_, w, cx| {
                w.focus(&composer, cx);
                assert!(composer.is_focused(w));
                w.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            })
            .unwrap();
        } else {
            h.keystroke(key);
        }
        h.update(|p, _, cx| {
            assert!(p.navigation.menu.is_none());
            let c = p.workspace.composers[THREAD].read(cx);
            assert!(!c.hint().contains("again to stop"));
            assert_eq!(c.saved_draft(), ("unsent", 1, &None));
        });
    }
    // Focus-out notifications require an active platform window.
    h.update(|_, w, _| w.activate_window());
    h.draw();
    h.keystroke("ctrl-m");
    h.draw();
    h.keystroke("tab");
    h.draw();
    h.update(|p, _, _| assert!(p.navigation.menu.is_none()));
    h.keystroke("ctrl-m");
    h.draw();
    h.update(|p, w, cx| p.focus_composer(w, cx));
    h.draw();
    h.update(|p, _, _| assert!(p.navigation.menu.is_none()));
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn machine_picker_shortcut_invalidates_remote_folder_and_commands() {
    let Some((mut h, scratch)) =
        picker_harness("sheet::machine_picker_shortcut_invalidates_remote_folder_and_commands")
    else {
        return;
    };
    h.update(|p, w, cx| {
        use plait_client::machines::Transport;
        for name in ["first", "second"] {
            p.preferences
                .value
                .machines
                .add(
                    name,
                    Transport::Tailnet {
                        host: name.into(),
                        port: 7744,
                    },
                )
                .unwrap();
        }
        p.host_local = false;
        p.machines.active = p.preferences.value.machines.list()[1].id.clone();
        p.open_remote_browser(w, cx);
    });
    h.draw();
    h.keystroke("ctrl-3");
    h.update(|p, w, cx| {
        let destination = p.preferences.value.machines.list()[2].id.clone();
        assert_eq!(p.machines.connecting.as_ref(), Some(&destination));
        let origin = p.machines.active.clone();
        p.use_machine_client(p.host_client(), destination, false, w, cx);
        assert!(p.overlay.is_none());
        assert!(p.remote_browser.is_none());
        p.run_command(
            &origin,
            super::super::commands::Command::OpenRemoteProject("/first/only".into()),
            None,
            w,
            cx,
        );
        assert!(!p.lists().opened.contains(&PathBuf::from("/first/only")));

        p.show_settings_for_project("/second/project".into(), w, cx);
        p.use_machine_client(p.host_client(), origin, false, w, cx);
        assert!(!p.settings_open);
        assert!(p.settings_project.is_none());
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn machine_picker_keeps_highlight_when_reconnect_appears() {
    let Some((mut h, scratch)) =
        picker_harness("sheet::machine_picker_keeps_highlight_when_reconnect_appears")
    else {
        return;
    };
    h.update(|p, w, cx| {
        p.preferences
            .value
            .machines
            .add(
                "Builder",
                plait_client::machines::Transport::Tailnet {
                    host: "builder".into(),
                    port: 7744,
                },
            )
            .unwrap();
        p.open_machine_picker(w, cx);
    });
    h.draw();
    h.keystroke("down");
    h.update(|p, _, cx| {
        p.host.connection.connected = false;
        cx.notify();
    });
    h.draw();
    h.keystroke("enter");
    h.update(|p, _, _| {
        assert_eq!(
            p.machines.connecting.as_ref(),
            Some(&p.preferences.value.machines.list()[1].id)
        )
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn machine_picker_update_respects_pending_destination() {
    let Some((mut h, scratch)) =
        picker_harness("sheet::machine_picker_update_respects_pending_destination")
    else {
        return;
    };
    h.update(|p, w, cx| {
        use super::super::machine_update::{MachineUpdate, Stage};
        let target = p
            .preferences
            .value
            .machines
            .add(
                "Builder",
                plait_client::machines::Transport::Tailnet {
                    host: "builder".into(),
                    port: 7744,
                },
            )
            .unwrap();
        p.use_machine(target.clone(), w, cx);
        assert_eq!(p.machines.connecting.as_ref(), Some(&target));
        let id = p.machines.active.clone();
        p.machines.update = Some(MachineUpdate {
            id: id.clone(),
            name: "Outgoing".into(),
            stage: Stage::Updating,
        });
        p.finish_host_update(
            id,
            Ok(plait_client::remote::install::Updated::Current),
            w,
            cx,
        );
        assert_eq!(p.machines.connecting.as_ref(), Some(&target));
        assert!(matches!(
            p.machines.update.as_ref().unwrap().stage,
            Stage::Current
        ));
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn foot_activation_and_next_thread_shortcut() {
    if !isolated("sheet::foot_activation_and_next_thread_shortcut") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-foot-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::with_host(&repo, None, 900., 600., None);
    h.connect(&repo, false);
    h.update(|p, _, _| {
        p.updates = update_offer();
        p.host.connection.threads_mut()[1].status = ThreadStatus::AwaitingApproval;
    });

    // Focus the machine, close its menu with Enter, then Tab to Update.
    super::settings_modal::click(&mut h, 20., 577.);
    h.update(|p, _, _| assert_eq!(p.navigation.menu.as_deref(), Some("machines")));
    let enter = |h: &mut Harness| {
        h.draw();
        h.keystroke("enter");
        h.pointer(PlatformInput::KeyUp(KeyUpEvent {
            keystroke: Keystroke::parse("enter").unwrap(),
        }));
    };
    enter(&mut h);
    h.update(|p, _, _| assert!(p.navigation.menu.is_none()));
    h.keystroke("tab");
    enter(&mut h);
    h.update(|p, _, _| assert!(p.settings_open));
    h.keystroke("escape");

    // Opening from a shortcut routes list keys away from the workspace.
    h.keystroke("ctrl-m");
    h.draw();
    h.keystroke("down");
    h.update(|p, _, _| {
        assert_eq!(
            p.machines.cursor,
            Some(super::super::machine_picker::Choice::Add)
        )
    });
    enter(&mut h);
    h.update(|p, _, _| {
        assert_eq!(p.settings_tab, SettingsTab::Machines);
        assert!(p.settings_open && p.machines.form.is_some());
    });
    h.keystroke("escape");
    h.keystroke("ctrl-m");
    h.draw();
    h.keystroke("escape");
    h.update(|p, _, _| assert!(p.navigation.menu.is_none()));
    h.keystroke("ctrl-m");
    h.draw();
    super::settings_modal::click(&mut h, 840., 580.);
    h.update(|p, _, _| assert!(p.navigation.menu.is_none()));
    h.update(|p, _, _| {
        p.preferences
            .value
            .machines
            .add(
                "Builder",
                plait_client::machines::Transport::Tailnet {
                    host: "builder".into(),
                    port: 7744,
                },
            )
            .unwrap();
    });
    h.keystroke("ctrl-2");
    h.update(|p, _, _| {
        assert_eq!(
            p.machines.connecting.as_ref(),
            Some(&p.preferences.value.machines.list()[1].id)
        );
        assert_eq!(p.machines.active, Default::default());
    });
    h.keystroke("ctrl-1");
    h.update(|p, _, _| assert!(p.machines.connecting.is_none()));

    // The window action remains reachable while all foot controls are hidden.
    h.keystroke("cmd-b");
    h.update(|p, _, _| assert!(!p.sidebar));
    h.keystroke("cmd-alt-j");
    h.update(|p, _, _| assert_eq!(p.host.thread.selected.as_deref(), Some("thr_0c44a9d1")));
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn update_button_opens_about_settings_and_closes_machine_menu() {
    if !isolated("sheet::update_button_opens_about_settings_and_closes_machine_menu") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-foot-update-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::with_host(&repo, None, 900., 600., None);
    h.connect(&repo, false);
    h.update(|p, _, _| {
        p.updates = update_offer();
        p.settings_tab = SettingsTab::NewThreads;
        p.navigation.menu = Some("machines".into());
    });
    #[cfg(debug_assertions)]
    {
        let update = h
            .draw_a11y()
            .nodes
            .into_iter()
            .find(|n| n.path.contains("sidebar-update") && n.role == Role::Button)
            .expect("sidebar Update button")
            .bounds
            .center();
        super::settings_modal::click(&mut h, f32::from(update.x), f32::from(update.y));
        h.update(|p, _, _| {
            assert!(p.settings_open);
            assert_eq!(p.settings_tab, SettingsTab::About);
            assert!(p.navigation.menu.is_none());
        });
        assert!(h.draw_a11y().has_node(Role::Button, |n| {
            n.label.as_deref() == Some("Third-party notices")
        }));
        assert!(h.draw_a11y().has_node(Role::Button, |n| {
            n.label.as_deref() == Some("Install 1.2.3 and Restart")
        }));
    }
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn machine_picker_preserves_unconfirmed_admission_on_origin() {
    let Some((mut h, scratch)) =
        picker_harness("sheet::machine_picker_preserves_unconfirmed_admission_on_origin")
    else {
        return;
    };
    h.window.update(&mut h.cx, |p,w,cx| {
        let origin = p.machines.active.clone();
        std::sync::Arc::make_mut(p.host.thread.snapshot.as_mut().unwrap()).status = ThreadStatus::Idle;
        p.admission.continuation = p.host.thread.snapshot.as_ref().map(|s| s.id.clone());
        let expected = plait_client::ownership::CheckoutKey {
            host: None,
            root: p.state.repository.path().to_owned(),
            device: 1,
            inode: 2,
        };
        p.state.reopen_checkout(expected.clone());
        p.start_continuation(w,cx);
        let original = p.admission.pending.clone().unwrap();
        assert!(matches!(&original, plait_client::host::admission::Request::CreateAdmittedThread { placement: plait_client::host::admission::Placement::Worktree, source_thread_id: Some(source), .. } if source == THREAD));
        let legacy = serde_json::to_value(&p.admission.direct.as_ref().unwrap().legacy).unwrap();
        let id = p.host.creation.pending_create.clone().unwrap();
        let remote = plait_client::machines::MachineId::new("studio").unwrap();
        p.use_machine_client(p.host_client(),remote,false,w,cx);
        assert!(p.host.creation.pending_create.is_none());
        p.host.connection.connected = true;
        p.retry_create(w,cx);
        assert!(p.admission.pending.is_none());
        let mut other = expected.clone();
        other.root = "/different".into();
        p.state.open_checkout(other.root.clone());
        p.state.reopen_checkout(other.clone());
        p.use_machine_client(p.host_client(),origin,true,w,cx);
        assert_eq!(p.state.checkout_key(), Some(&other));
        p.host.connection.connected = true;
        p.host.creation.checkout = "/different".into();
        p.retry_create(w,cx);
        assert_eq!(p.host.creation.pending_create.as_ref(),Some(&id));
        assert_eq!(p.admission.pending.as_ref(), Some(&original));
        assert_eq!(p.admission.direct.as_ref().unwrap().expected.as_ref(), Some(&expected));
        assert_eq!(serde_json::to_value(&p.admission.direct.as_ref().unwrap().legacy).unwrap(), legacy);
    }).unwrap();
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn machine_picker_retry_completes_confirmed_removal() {
    let Some((mut h, scratch)) =
        picker_harness("sheet::machine_picker_retry_completes_confirmed_removal")
    else {
        return;
    };
    h.update(|p, w, cx| {
        p.preferences.value.machines =
            serde_json::from_str(r#"[{"id":"local2","name":"Home","transport":{"type":"local"}}]"#)
                .unwrap();
        let remote = p
            .preferences
            .value
            .machines
            .add(
                "studio",
                plait_client::machines::Transport::Tailnet {
                    host: "studio".into(),
                    port: 7744,
                },
            )
            .unwrap();
        let local = plait_client::machines::MachineId::new("local2").unwrap();
        p.machines.active = remote.clone();
        p.remove_machine(remote.clone(), w, cx);
        assert_eq!(p.machines.connecting.take(), Some(local.clone()));
        p.machines.failed_switch = Some(local.clone());
        p.activate_choice(super::super::machine_picker::Choice::Retry, w, cx);
        assert_eq!(p.machines.connecting.take(), Some(local.clone()));
        p.finish_machine_switch(p.host_client(), local, true, w, cx);
        assert!(p.preferences.value.machines.get(&remote).is_none());
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn machine_picker_cancelling_restore_saves_this_mac() {
    let Some((mut h, scratch)) =
        picker_harness("sheet::machine_picker_cancelling_restore_saves_this_mac")
    else {
        return;
    };
    h.update(|p, w, cx| {
        let remote = p
            .preferences
            .value
            .machines
            .add(
                "Studio",
                plait_client::machines::Transport::Tailnet {
                    host: "studio".into(),
                    port: 7744,
                },
            )
            .unwrap();
        p.preferences.value.selected_machine = remote.clone();
        p.machines.connecting = Some(remote);
        p.pick_machine_number(1, w, cx);
        assert!(p.machines.connecting.is_none());
        assert_eq!(p.preferences.value.selected_machine, p.machines.active);
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn machine_palette_hides_a_displaced_shortcut() {
    let Some((mut h, scratch)) =
        picker_harness("sheet::machine_palette_hides_a_displaced_shortcut")
    else {
        return;
    };
    h.update(|p, w, cx| {
        let remote = p
            .preferences
            .value
            .machines
            .add(
                "Studio",
                plait_client::machines::Transport::Tailnet {
                    host: "studio".into(),
                    port: 7744,
                },
            )
            .unwrap();
        p.preferences
            .value
            .keymap
            .insert("tab.close".into(), "ctrl-2".into());
        super::super::actions::install(cx, &p.preferences.value.keymap);
        let items = p.command_items(w, cx);
        let key = format!("machine:{}", remote.as_str());
        assert!(
            items
                .iter()
                .find(|row| row.key.as_ref() == key)
                .unwrap()
                .hint
                .is_empty()
        );
        assert!(
            !items
                .iter()
                .find(|row| row.key.as_ref() == "action:tab.close")
                .unwrap()
                .hint
                .is_empty()
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
