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
    super::sheet::walk(h, suffix);
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
