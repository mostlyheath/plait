use super::*;
use plait_client::update::{Manifest, Offer, Version};

/// Use the sidebar fixture's mixed thread states so its attention count is visible.
pub(super) fn walk(h: &mut Harness, suffix: &str) {
    capture_both(h, &format!("sheet-{suffix}"));
    h.update(|p, window, _| {
        let body = p.workspace.work_area;
        let rail = p.panel_width(crate::ui::panels::Panel::Sidebar);
        assert!(body.left() >= px(rail));
        assert!(body.right() <= window.viewport_size().width - px(8.));
        assert!(body.bottom() <= window.viewport_size().height - px(8.));
    });
    h.update(|p, _, _| p.sidebar = false);
    capture_both(h, &format!("sheet-hidden-{suffix}"));
    h.update(|p, window, _| {
        let body = p.workspace.work_area;
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
    h.update(|p, _, _| p.updates = Default::default());
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
    h.update(|p, _, _| assert!(!p.visible_sidebar()));
    h.keystroke("cmd-alt-j");
    h.update(|p, _, _| assert_eq!(p.host.thread.selected.as_deref(), Some("thr_0c44a9d1")));
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
