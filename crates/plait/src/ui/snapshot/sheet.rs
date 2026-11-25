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
            n.label.as_deref() == Some("Install 1.2.3 and Restart")
        }));
    }
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
