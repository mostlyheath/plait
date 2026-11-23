use super::*;
use plait_client::host::checkout_queue::{State, Summary};

pub(super) fn walk(h: &mut Harness, repository: &Path, suffix: &str) {
    h.connect(repository, false);
    h.update(|p, _, cx| {
        p.sidebar = true;
        p.host.connection.status.as_mut().unwrap().protocol_version = 3;
        p.navigation.attention = false;
        p.navigation.collapsed.clear();
        p.preferences.value.navigation = Default::default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        *p.host.connection.threads_mut() = [
            ("Build the desktop", ThreadStatus::Running),
            ("Review the edit", ThreadStatus::AwaitingApproval),
            ("Repair the tests", ThreadStatus::Error),
            ("Resume the import", ThreadStatus::Interrupted),
            ("Check the same checkout", ThreadStatus::Idle),
            ("Read the result", ThreadStatus::Idle),
            ("Plan the release", ThreadStatus::Idle),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (title, status))| ThreadSummary {
            id: if i == 0 {
                THREAD.into()
            } else {
                format!("sidebar-{i}")
            },
            checkout: repository.into(),
            title: Some(title.into()),
            status,
            created_at: Some(now - 120_000),
            updated_at: Some(now - 60_000),
            checkout_queue: (i == 4).then(|| Summary::new("request".into(), State::Waiting, 1, 1)),
            ..Default::default()
        })
        .collect();
        p.preferences
            .value
            .navigation
            .unread
            .insert("sidebar-5".into());
        p.rebuild_projects(cx);
    });
    h.capture(&format!("sidebar-states-{suffix}"));
    status_scenes(h, suffix);
    h.update(|p, _, _| p.navigation.attention = true);
    h.capture(&format!("sidebar-needs-me-{suffix}"));
    h.update(|p, _, cx| {
        p.navigation.attention = false;
        p.navigation.collapsed.insert(repository.into());
        p.navigation.invalidate_rows();
        cx.notify();
    });
    h.capture(&format!("sidebar-folded-{suffix}"));
    h.update(|p, _, cx| {
        p.host.connection.threads_mut().truncate(1);
        p.navigation.collapsed.clear();
        // Whichever list the window reads, nothing in it is unread.
        p.update_lists(|lists| lists.unread.clear(), cx);
        p.rebuild_projects(cx);
        assert_eq!(
            plait_client::navigation::needs_me_count(
                p.host.connection.threads(),
                &p.lists().unread
            ),
            0
        );
    });
    h.capture(&format!("sidebar-zero-{suffix}"));
    h.capture(&format!("status-nothing-needs-me-{suffix}"));
}

fn status_scenes(h: &mut Harness, suffix: &str) {
    use plait_client::update::{Manifest, Offer, Version};

    h.capture(&format!("status-connected-counts-{suffix}"));
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        for label in [
            "1 working",
            "1 finished",
            "1 failed",
            "1 waiting for approval",
        ] {
            assert_eq!(frame.nodes.iter().any(|n| n.value.as_deref() == Some(label) || n.label.as_deref() == Some(label)), suffix == "wide", "{label}");
        }
        assert!(frame.nodes.iter().any(|n| n.label.as_deref() == Some("Next: Review the edit · Waiting for approval")));
    }
    h.update(|p, _, _| p.navigation.menu = Some("machines".into()));
    h.capture(&format!("status-machines-menu-{suffix}"));
    h.update(|p, _, _| p.navigation.menu = None);
    h.update(|p, _, _| {
        p.host.connection.threads_mut()[1].title = Some(
            "Review the release checklist and verify the packaging changes for every supported machine before publishing the next desktop preview".into()
        );
    });
    h.capture(&format!("status-long-title-{suffix}"));
    h.update(|p, _, _| {
        p.host.connection.threads_mut()[1].title = Some("Review the edit".into());
        p.machines.connecting = Some(p.machines.active.clone());
    });
    h.capture(&format!("status-connecting-{suffix}"));
    h.update(|p, _, _| {
        p.machines.connecting = None;
        p.host.connection.connected = false;
    });
    h.capture(&format!("status-offline-{suffix}"));
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
    h.capture(&format!("status-update-{suffix}"));
    h.update(|p, _, _| p.updates = Default::default());
}

#[core::prelude::v1::test]
fn navigation_sidebar_scenes() {
    if !isolated("sidebar::navigation_sidebar_scenes") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-sidebar-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    for (suffix, w, h) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut harness = Harness::with_host(&repo, output.as_deref(), w, h, None);
        harness.update(|p, _, _| p.preferences.value.hide_inspector = true);
        walk(&mut harness, &repo, suffix);
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
