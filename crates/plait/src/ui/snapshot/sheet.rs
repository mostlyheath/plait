use super::*;
use plait_client::update::{Manifest, Offer, Version};

/// Use the sidebar fixture's mixed thread states so its attention count is visible.
pub(super) fn walk(h: &mut Harness, suffix: &str) {
    capture_both(h, &format!("sheet-{suffix}"));
    h.update(|p, _, _| p.sidebar = false);
    capture_both(h, &format!("sheet-hidden-{suffix}"));
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
        p.updates = super::super::update::UpdateState::Available(Arc::new(Offer {
            version: Version::parse("1.2.3").unwrap(),
            manifest: Manifest {
                schema_version: 1,
                version: "1.2.3".into(),
                tag: "v1.2.3".into(),
                revision: "scene".into(),
                host_build_id: "scene".into(),
                artifacts: Default::default(),
            },
        }));
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
