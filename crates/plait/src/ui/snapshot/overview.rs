//! Portable Every thread scenes and an offscreen input test.
use super::*;
use plait_client::{model::Machine, navigation};

fn install(h: &mut Harness, paths: [&Path; 2]) {
    h.update(|p, _, _| {
        p.sidebar = true;
        p.preferences.value.hide_inspector = true;
        p.preferences.value.navigation = Default::default();
        let updated = super::super::navigation::now_ms();
        *p.host.connection.threads_mut() = [
            (THREAD, "Build the desktop", ThreadStatus::Running),
            (
                "approval",
                "Review the checkout guard",
                ThreadStatus::AwaitingApproval,
            ),
            ("failed", "Fix the test runner", ThreadStatus::Error),
            ("finished", "Read the parser results", ThreadStatus::Idle),
            (
                "interrupted",
                "Resume the import",
                ThreadStatus::Interrupted,
            ),
            ("last", "Plan a release", ThreadStatus::Idle),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (id, title, status))| ThreadSummary {
            id: id.into(),
            checkout: paths[usize::from(i == 5)].into(),
            status,
            title: Some(title.into()),
            model: choice(),
            harness: Some(HarnessId::Pi),
            created_at: Some(updated - 600_000),
            updated_at: Some(updated - (i as u64 + 1) * 60_000),
            ..Default::default()
        })
        .collect();
        let lists = &mut p.preferences.value.navigation;
        lists.unread.insert("finished".into());
        if paths[0] != paths[1] {
            let checkout = paths[0].join("checkout-guard");
            p.host.connection.threads_mut()[1].checkout = checkout.clone();
            lists.remember_worktree(checkout, paths[0].into(), "fix/checkout-guard".into());
        }
        projects(p);
    });
}

fn projects(p: &mut Plait) {
    let machine = Machine {
        name: "studio".into(),
        local: true,
    };
    p.state.projects = navigation::projects(
        p.host.connection.threads(),
        p.lists(),
        Path::new(""),
        &machine,
        false,
    );
    p.no_project.machines = vec![machine];
    p.navigation.invalidate_projects();
}

pub(super) fn walk(h: &mut Harness, repository: &Path, suffix: &str) {
    let hide_inspector = h
        .window
        .update(&mut h.cx, |p, _, _| p.preferences.value.hide_inspector)
        .unwrap();
    h.connect(repository, false);
    install(h, [Path::new("/work/forecast"), Path::new("/work/ledger")]);
    h.update(|p, window, cx| p.toggle_overview(window, cx));
    h.capture(&format!("overview-{suffix}"));
    h.keystroke("home");
    h.capture(&format!("overview-focus-moved-{suffix}"));
    h.update(|p, _, _| {
        for thread in p.host.connection.threads_mut() {
            if thread.id != "approval" {
                thread.status = ThreadStatus::Idle;
            }
        }
    });
    h.capture(&format!("overview-approval-{suffix}"));
    h.update(|p, _, _| {
        for thread in p.host.connection.threads_mut() {
            thread.status = ThreadStatus::Idle;
            thread.updated_at = thread.created_at;
        }
        p.preferences.value.navigation.unread.clear();
    });
    h.capture(&format!("overview-all-idle-{suffix}"));
    h.update(|p, _, _| {
        p.host.connection.threads_mut().truncate(5);
        projects(p);
    });
    h.capture(&format!("overview-one-project-{suffix}"));
    h.update(|p, _, _| {
        let seed = p.host.connection.threads()[0].clone();
        p.host.connection.set_threads(
            (0..300)
                .map(|index| {
                    let mut thread = seed.clone();
                    if index != 0 {
                        thread.id = format!("overview-{index}");
                        thread.title = Some(format!("Review change {index}"));
                    }
                    thread.checkout = format!("/work/project-{}", index / 50 + 1).into();
                    thread.status = if index % 13 == 0 {
                        ThreadStatus::Running
                    } else {
                        ThreadStatus::Idle
                    };
                    thread
                })
                .collect(),
        );
        projects(p);
    });
    h.capture(&format!("overview-many-{suffix}"));
    h.update(|p, _, _| p.sidebar = false);
    capture_both(h, &format!("overview-no-sidebar-{suffix}"));
    h.update(|p, _, _| {
        p.sidebar = true;
        p.host.connection.threads_mut().clear();
        projects(p);
    });
    h.capture(&format!("overview-empty-{suffix}"));
    h.update(|p, window, cx| p.close_overview(window, cx));
    // Keep later scenes on their regular fixture state.
    h.connect(repository, false);
    h.update(|p, _, cx| {
        p.preferences.value.hide_inspector = hide_inspector;
        p.rebuild_projects(cx);
    });
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn overview_controls() {
    if !isolated("overview::overview_controls") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-overview-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let repository = scratch.canonicalize().unwrap();
    let mut h = Harness::with_host(&repository, None, 1380., 860., None);
    h.connect(&repository, false);
    install(&mut h, [&repository, &repository]);
    h.update(|p, window, cx| window.focus(&p.focus, cx));
    h.draw();
    h.keystroke("cmd-shift-e");
    h.update(|p, window, cx| {
        assert!(
            p.overview_focus()
                .is_some_and(|focus| focus.is_focused(window))
        );
        assert_eq!(p.host.thread.selected.as_deref(), Some(THREAD));
        assert_eq!(p.filtered_threads(cx).len(), 6);
    });
    assert_cards(&mut h, "Build the desktop. Working", "3 need you");
    h.update(|p, _, _| p.host.connection.threads_mut()[0].status = ThreadStatus::Error);
    assert_cards(&mut h, "Build the desktop. Failed", "4 need you");
    h.keystroke("escape");
    h.update(|p, window, _| {
        assert!(p.overview.is_none());
        assert!(p.focus.is_focused(window));
    });

    h.keystroke("cmd-k");
    set_query(&mut h, "Every Thread");
    h.keystroke("enter");
    h.update(|p, _, _| assert!(p.overview.is_some()));
    h.draw();
    for (key, focused) in [
        ("home", "approval"),
        ("right", THREAD),
        ("left", "approval"),
        ("down", "interrupted"),
        ("up", "approval"),
        ("end", "last"),
    ] {
        h.keystroke(key);
        h.update(|p, _, _| {
            assert_eq!(
                p.overview.as_ref().unwrap().focused_thread(),
                Some(focused),
                "{key}"
            );
        });
    }
    h.keystroke("enter");
    undo::wait(&mut h, |p| !p.tabs.lifecycle.busy);
    h.update(|p, _, _| {
        assert!(p.overview.is_none());
        assert_eq!(
            p.host.thread.selected.as_deref(),
            Some("last"),
            "{:?}",
            p.notice
        );
    });

    let (x, y) = h
        .draw_a11y()
        .nodes
        .iter()
        .find(|n| n.path.contains("needs-me") && n.role == Role::Button)
        .map(|n| {
            (
                f32::from(n.bounds.center().x),
                f32::from(n.bounds.center().y),
            )
        })
        .expect("Needs you control");
    settings_modal::click(&mut h, x, y);
    h.update(|p, _, _| assert!(p.overview.is_some()));
    settings_modal::click(&mut h, x, y);
    h.update(|p, _, _| assert!(p.overview.is_none()));
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A project's path reads from the home directory, as elsewhere in Plait;
/// the full path stays in its hover text.
#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn overview_project_paths_read_from_home() {
    if !isolated("overview::overview_project_paths_read_from_home") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let (first, second) = (home.join("code/forecast"), home.join("code/ledger"));
    let mut h = Harness::with_host(&home, None, 1380., 860., None);
    h.connect(&home, false);
    install(&mut h, [&first, &second]);
    h.keystroke("cmd-shift-e");
    let frame = h.draw_a11y();
    // Text nodes carry what they draw as their value.
    let labels = frame
        .nodes
        .iter()
        .filter(|n| n.path.contains("overview-project-path"))
        .filter_map(|n| n.value.as_deref())
        .collect::<Vec<_>>();
    for shown in ["~/code/forecast", "~/code/ledger"] {
        assert!(labels.contains(&shown), "{shown} not in {labels:?}");
    }
    let home = home.display().to_string();
    assert!(
        !labels.iter().any(|label| label.starts_with(&home)),
        "a path names the home directory: {labels:?}"
    );
}

#[cfg(debug_assertions)]
fn assert_cards(h: &mut Harness, title: &str, count: &str) {
    let frame = h.draw_a11y();
    frame.assert_no_duplicates("Every thread");
    assert!(frame.has_node(Role::Button, |n| n.label.as_deref() == Some(title)));
    assert!(
        frame
            .nodes
            .iter()
            .any(|n| n.value.as_deref() == Some(count))
    );
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn overview_closes_when_a_project_view_opens() {
    if !isolated("overview::overview_closes_when_a_project_view_opens") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-overview-views-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let repository = scratch.canonicalize().unwrap();
    let mut h = Harness::with_host(&repository, None, 1380., 860., None);
    h.connect(&repository, false);
    install(&mut h, [&repository, &repository]);
    h.draw();
    // The project menu's Worktrees and Linear issues open a tab of their own;
    // Every thread must not stay drawn over it.
    type Open = fn(&mut Plait, PathBuf, &mut Window, &mut Context<Plait>);
    for (name, open) in [
        ("Worktrees", Plait::open_worktrees as Open),
        ("Linear issues", Plait::open_issues as Open),
    ] {
        h.keystroke("cmd-shift-e");
        h.update(|p, _, _| assert!(p.overview.is_some(), "{name}: opened"));
        let checkout = repository.clone();
        h.update(move |p, window, cx| open(p, checkout, window, cx));
        undo::wait(&mut h, |p| !p.tabs.lifecycle.busy);
        h.draw();
        h.update(|p, window, _| {
            assert!(
                p.worktrees_visible() || p.issues_visible(),
                "{name}: view shown"
            );
            assert!(p.overview.is_none(), "{name}: Every thread still covers it");
            assert!(p.focus.is_focused(window), "{name}: keys reach the view");
        });
        h.update(|p, window, cx| p.select_host_thread(THREAD.into(), window, cx));
        undo::wait(&mut h, |p| !p.tabs.lifecycle.busy);
    }
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
