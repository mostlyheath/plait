//! Both header forms share the same thread, checkout and tab model.
use super::*;
use plait_client::{model::Snapshot as GitSnapshot, preferences::ThreadTabs};

const PROJECT: &str = "/projects/forecast";

fn checkout_snapshot(branch: &str) -> GitSnapshot {
    GitSnapshot {
        root: Path::new(PROJECT).into(),
        branch: branch.into(),
        changes: vec![plait_client::model::Change {
            path: "src/review.rs".into(),
            original: None,
            index: b' ',
            worktree: b'M',
        }],
        named: true,
        tracking: None,
        plain: false,
        elapsed: Duration::ZERO,
    }
}

pub(super) fn fixture(output: Option<&Path>, width: f32, height: f32) -> Harness {
    let root = Path::new(PROJECT);
    let mut h = Harness::with_host(root, output, width, height, None);
    h.connect(root, false);
    tabs::install(&mut h, 3);
    h.change_snapshot(|s| {
        s.status = ThreadStatus::Idle;
        s.live_text.clear();
        s.tools.clear();
    });
    h.update(|p, _, cx| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 3;
        p.preferences.value.thread_tabs = Some(ThreadTabs::Hide);
        p.host_local = false;
        p.host_workspace.source = super::super::host_workspace::Source::Host(
            plait_client::host::workspace::HostIdentity {
                id: "scene-host".into(),
                name: "studio".into(),
                platform: "darwin".into(),
            },
        );
        p.state.projects[0].machines[0].name = "studio".into();
        p.host.connection.error = None;
        p.state.repository.data.snapshot =
            Load::Ready(Arc::new(checkout_snapshot("retry-forecast")));
        p.workspace.thread_files.inject(
            THREAD,
            root.into(),
            vec![plait_client::git::LineCount {
                path: "src/review.rs".into(),
                added: Some(12),
                removed: Some(3),
                untracked: false,
            }],
        );
        p.themes.forced = Some(false);
        p.apply_theme(cx);
    });
    h
}

pub(super) fn walk(output: Option<&Path>) {
    narrow_chip_scenes(output);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut h = fixture(output, width, height);
        capture_both(&mut h, &format!("header-title-{suffix}"));
        h.update(|p, _, _| p.workspace.panel_change_total = Some("+12 −3".into()));
        h.capture(&format!("header-panel-totals-{suffix}"));
        h.update(|p, _, _| {
            p.host.connection.threads_mut()[0].title = Some(
                "Improve retry handling for forecast requests that reach the upstream rate limit"
                    .into(),
            );
        });
        h.capture(&format!("header-title-narrow-{suffix}"));
        for (name, state, _label) in [
            ("working", ThreadStatus::Running, "Working"),
            ("approval", ThreadStatus::AwaitingApproval, "Needs approval"),
            ("failed", ThreadStatus::Error, "Failed"),
            ("interrupted", ThreadStatus::Interrupted, "Interrupted"),
        ] {
            h.change_snapshot(|s| {
                s.status = state;
                s.pending_approval = if state == ThreadStatus::AwaitingApproval {
                    thread_snapshot(Path::new(PROJECT), true).pending_approval
                } else {
                    None
                };
            });
            h.capture(&format!("header-{name}-{suffix}"));
            #[cfg(debug_assertions)]
            assert!(
                h.draw_a11y()
                    .nodes
                    .iter()
                    .any(|n| n.path.contains("header-state")
                        && n.value.as_deref().or(n.label.as_deref()) == Some(_label))
            );
        }
        h.update(|p, _, _| p.sidebar = false);
        h.capture(&format!("header-no-sidebar-{suffix}"));
        h.update(|_, window, _| window.toggle_fullscreen());
        h.capture(&format!("header-full-screen-{suffix}"));
        h.update(|p, window, _| {
            window.toggle_fullscreen();
            p.sidebar = true;
        });
        h.change_snapshot(|s| s.status = ThreadStatus::Idle);
        tabs::install(&mut h, 3);
        capture_both(&mut h, &format!("header-tabs-{suffix}"));
        tabs::install(&mut h, 20);
        h.capture(&format!("header-tabs-overflow-{suffix}"));
        worktrees_view::install(&mut h, 3);
        h.capture(&format!("header-worktrees-{suffix}"));
        #[cfg(debug_assertions)]
        assert_project_header(&mut h, "Worktrees");
        h.update(|p, w, cx| {
            p.host.connection.connected = false;
            p.open_draft(w, cx);
        });
        h.capture(&format!("header-draft-{suffix}"));
        #[cfg(debug_assertions)]
        assert_project_header(&mut h, super::super::session::UNTITLED);
    }

    let mut h = fixture(output, 720., 600.);
    h.capture("header-title-compact");
    #[cfg(debug_assertions)]
    assert!(
        !h.draw_a11y()
            .nodes
            .iter()
            .any(|n| n.path.contains("checkout-branch"))
    );
}

fn narrow_chip_scenes(output: Option<&Path>) {
    for (width, scene, chip_count) in [
        (900., "header-chips-narrow", 2),
        (828., "header-chips-title-minimum", 1),
        (780., "header-chips-project-only", 0),
    ] {
        let mut h = fixture(output, width, 600.);
        h.update(|p, _, _| {
            p.host.connection.threads_mut()[0].title = Some(
                "Queue the status refresh instead of reading it inline when the checkout changes"
                    .into(),
            );
            p.tabs
                .model
                .panel_mut()
                .unwrap()
                .open_on(plait_client::panel::PanelKind::Changes);
            p.workspace.panel_change_total = Some("+128 −30".into());
            if chip_count == 0 {
                p.preferences.value.sidebar_width = Some(296);
                p.host.connection.threads_mut()[0].status = ThreadStatus::AwaitingApproval;
            }
        });
        if chip_count == 2 {
            capture_both(&mut h, "header-chips-short-names");
            #[cfg(debug_assertions)]
            assert!(assert_chip_labels(&mut h, 2) >= 159.);
        }
        h.update(|p, _, _| {
            p.state.projects[0].name = "forecast-observation-processing-service".into();
            p.state.repository.data.snapshot = Load::Ready(Arc::new(checkout_snapshot(
                "feature/retry-rate-limited-upstream-requests",
            )));
        });
        capture_both(&mut h, scene);
        #[cfg(debug_assertions)]
        {
            let title = assert_chip_labels(&mut h, chip_count);
            if chip_count > 0 {
                assert!(title >= 159., "title width {title}");
            }
            if chip_count == 1 {
                assert!((159.0..=161.0).contains(&title), "title width {title}");
                h.update(|p, _, _| p.host.connection.threads_mut()[0].title = Some("Queue".into()));
                assert!(assert_chip_labels(&mut h, 2) < 160., "short title");
            }
        }
    }
}

#[cfg(debug_assertions)]
fn assert_chip_labels(h: &mut Harness, chip_count: usize) -> f32 {
    let frame = h.draw_a11y();
    let label_width = |id: &str| {
        frame
            .nodes
            .iter()
            .find(|node| node.path.contains("sheet-header") && node.path.ends_with(id))
            .map(|node| f32::from(node.bounds.size.width))
    };
    let project = label_width("project-name");
    let branch = label_width("branch-name");
    assert_eq!(
        project.is_some(),
        chip_count >= 1,
        "project label {project:?}"
    );
    assert_eq!(branch.is_some(), chip_count == 2, "branch label {branch:?}");
    assert!(
        project.is_none_or(|w| w >= 39.),
        "project label width {project:?}"
    );
    assert!(
        branch.is_none_or(|w| w >= 52.),
        "branch label width {branch:?}"
    );
    label_width("header-title.title").expect("thread title")
}

#[core::prelude::v1::test]
fn narrow_header_chips() {
    if isolated("header::narrow_header_chips") {
        let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
        narrow_chip_scenes(output.as_deref());
    }
}

#[cfg(debug_assertions)]
fn assert_project_header(h: &mut Harness, title: &str) {
    let frame = h.draw_a11y();
    assert!(
        frame
            .nodes
            .iter()
            .any(|node| node.path.contains("header-title")
                && node.value.as_deref().or(node.label.as_deref()) == Some(title))
    );
    assert!(!frame.nodes.iter().any(
        |node| node.path.contains("header-state") || node.path.contains("header-panel-toggle")
    ));
}

#[core::prelude::v1::test]
fn header_forms() {
    if isolated("header::header_forms") {
        walk(None);
    }
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn non_thread_views_keep_their_title_with_tabs_enabled() {
    if !isolated("header::non_thread_views_keep_their_title_with_tabs_enabled") {
        return;
    }
    let mut h = fixture(None, 900., 600.);
    h.update(|p, _, _| {
        p.host.connection.connected = false;
        p.preferences.value.thread_tabs = Some(ThreadTabs::Show);
    });
    worktrees_view::install(&mut h, 3);
    assert_project_header(&mut h, "Worktrees");
    h.update(|p, w, cx| p.open_issues(PathBuf::from(PROJECT), w, cx));
    assert_project_header(&mut h, "Linear issues");
    h.update(|p, w, cx| p.open_draft(w, cx));
    assert_project_header(&mut h, super::super::session::UNTITLED);
    let frame = h.draw_a11y();
    assert!(!frame.has_node(Role::TabList, |n| n.label.as_deref() == Some("Open tabs")));
    assert_eq!(
        frame
            .nodes
            .iter()
            .filter(|n| n.path.ends_with("project-name"))
            .count(),
        1,
        "a draft shows its checkout only in the title form"
    );
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn panel_toggle_shows_only_known_checkout_totals() {
    if !isolated("header::panel_toggle_shows_only_known_checkout_totals") {
        return;
    }
    let mut h = fixture(None, 900., 600.);
    let totals = |h: &mut Harness| {
        h.draw_a11y().nodes.into_iter().find_map(|n| {
            n.path
                .ends_with("header-panel-totals")
                .then(|| n.value.or(n.label))
                .flatten()
        })
    };
    assert_eq!(totals(&mut h), None);
    h.update(|p, _, _| p.workspace.panel_change_total = Some("+12 −3".into()));
    assert_eq!(totals(&mut h).as_deref(), Some("+12 −3"));
    h.update(|p, _, _| p.state.repository.data.snapshot = Load::Empty);
    assert_eq!(totals(&mut h), None);
}

#[core::prelude::v1::test]
fn hidden_tabs_keep_their_keys_and_list() {
    if !isolated("header::hidden_tabs_keep_their_keys_and_list") {
        return;
    }
    let mut h = fixture(None, 900., 600.);
    h.update(|p, _, _| p.host.connection.connected = false);
    h.draw();
    h.keystroke("cmd-2");
    h.update(|p, _, _| assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[1].id())));
    h.keystroke("cmd-shift-\\");
    h.draw();
    h.update(|p, _, _| assert!(p.tabs.list_open));
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        assert!(frame.has_node(Role::Menu, |n| n.label.as_deref() == Some("All open tabs")));
        assert!(!frame.has_node(Role::TabList, |n| n.label.as_deref() == Some("Open tabs")));
        assert_eq!(
            frame
                .nodes
                .iter()
                .filter(|n| n.role == Role::MenuItem && n.path.contains("tab-list"))
                .count(),
            3
        );
    }
    h.keystroke("escape");
    h.keystroke("cmd-shift-]");
    h.keystroke("cmd-w");
    h.update(|p, _, _| assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[1].id())));
    h.keystroke("cmd-shift-t");
    h.update(|p, _, _| assert_eq!(p.tabs.model.tabs().len(), 3));
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn header_pills_reorder_close_and_open_a_draft() {
    if !isolated("header::header_pills_reorder_close_and_open_a_draft") {
        return;
    }
    let mut h = fixture(None, 1380., 860.);
    h.update(|p, _, _| {
        p.host.connection.connected = false;
        p.preferences.value.thread_tabs = Some(ThreadTabs::Show);
    });
    h.draw();
    let frame = h.draw_a11y();
    let tabs: Vec<_> = frame
        .nodes
        .iter()
        .filter(|n| n.role == Role::Tab && n.path.contains("thread-tab-"))
        .map(|n| n.bounds.center())
        .collect();
    assert_eq!(tabs.len(), 3);
    let press = |h: &mut Harness, position| {
        h.pointer(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        }));
    };
    let release = |h: &mut Harness, position| {
        h.pointer(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        }));
    };
    press(&mut h, tabs[1]);
    h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
        position: tabs[0],
        pressed_button: Some(MouseButton::Left),
        modifiers: Default::default(),
    }));
    release(&mut h, tabs[0]);
    h.update(|p, _, _| assert_eq!(p.tabs.rows[0].title, "Run the parser tests"));

    for (button, count) in [("Close Improve retry handling", 2), ("New thread tab", 3)] {
        let position = h
            .draw_a11y()
            .nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label.as_deref() == Some(button))
            .unwrap()
            .bounds
            .center();
        press(&mut h, position);
        release(&mut h, position);
        h.update(|p, _, _| assert_eq!(p.tabs.model.tabs().len(), count, "{button}"));
    }
    h.update(|p, _, _| assert!(p.draft.open));
}
