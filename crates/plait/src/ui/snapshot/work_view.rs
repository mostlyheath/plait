use super::*;

fn center(h: &mut Harness, label: &str) -> Point<Pixels> {
    h.draw_a11y()
        .nodes
        .iter()
        .find(|n| n.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("missing control {label}"))
        .bounds
        .center()
}
fn click(h: &mut Harness, label: &str) {
    let position = center(h, label);
    h.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }));
    h.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }));
}

#[core::prelude::v1::test]
fn work_view_scenes() {
    if !isolated("work_view::work_view_scenes") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-work-scenes-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    if let Some(dir) = &output {
        std::fs::create_dir_all(dir).unwrap();
    }
    scenes(&repo, output.as_deref());
    std::fs::remove_dir_all(scratch).unwrap();
}

pub(super) fn scenes(repo: &Path, output: Option<&Path>) {
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut h = this_turn::harness(repo, output, width, height);
        turn_review::scene(&mut h, repo, "ready", None);
        h.update(|p, _, _| {
            p.workspace.tab = crate::ui::Workspace::Conversation;
            p.preferences.value.hide_inspector = false;
        });
        h.draw();
        if width > 1150. {
            click(&mut h, "Open in Work view");
        } else {
            click(&mut h, "Work");
        }

        h.draw();
        h.update(|p, _, _| {
            assert!(p.draws_work());
            assert!(!p.visible_sidebar());
        });
        h.capture(&format!("work-this-turn-{suffix}"));
        assert!(
            !h.draw_a11y()
                .has_node(Role::Group, |n| n.label.as_deref() == Some("Commit"))
        );
        click(&mut h, "Chat");
        h.update(|p, _, _| assert!(p.visible_sidebar()));
        h.keystroke("cmd-\\");
        h.keystroke("cmd-shift-2");
        h.capture(&format!("work-changes-focused-{suffix}"));
        h.keystroke("cmd-f");
        h.update(|p, _, cx| {
            assert!(p.workspace.diff_find.open);
            p.workspace
                .diff_find
                .field
                .as_ref()
                .unwrap()
                .update(cx, |f, cx| f.set_text("queue_status", cx));
        });
        undo::wait(&mut h, |p| p.workspace.diff_find.matches.len() == 1);
        h.keystroke("escape");
        h.update(|p, _, _| {
            let repo = &mut p.state.repository;
            let snapshot = plait_client::model::Snapshot {
                root: repo.path().to_owned(),
                branch: "main".into(),
                named: true,
                tracking: None,
                plain: false,
                elapsed: Duration::ZERO,
                changes: vec![plait_client::model::Change {
                    path: "src/review.rs".into(),
                    original: None,
                    index: b' ',
                    worktree: b'M',
                }],
            };
            repo.refresh();
            let job = repo.next_job().unwrap();
            repo.complete(
                job,
                plait_client::state::Reply::Status(Ok(Arc::new(snapshot))),
            );
            repo.select(0, plait_client::model::DiffMode::Unstaged);
            let diff = p.workspace.turn_review.diff.clone().unwrap();
            let job = repo.next_job().unwrap();
            repo.complete(job, plait_client::state::Reply::Diff(Ok(diff)));
        });
        click(&mut h, "Uncommitted");
        h.update(|p, _, _| {
            p.workspace
                .work_counts
                .insert("src/review.rs".into(), "+2 −1".into());
        });
        h.capture(&format!("work-uncommitted-{suffix}"));
        assert!(
            h.draw_a11y()
                .has_node(Role::Group, |n| n.label.as_deref() == Some("Commit"))
        );
        let seam = center(&mut h, "Resize panes");
        h.pointer(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: seam,
            click_count: 1,
            ..Default::default()
        }));
        for shift in [12., 90.] {
            h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
                position: point(seam.x - px(shift), seam.y),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }));
            h.draw();
        }
        h.pointer(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(seam.x - px(90.), seam.y),
            click_count: 1,
            ..Default::default()
        }));
        h.update(|p, _, _| assert!(p.work_layout().unwrap().panes[0].rect.width < 400.));
        h.capture(&format!("work-divider-dragged-{suffix}"));
        pane_controls(&mut h, repo, suffix);
        h.keystroke("cmd-b");
        h.keystroke("cmd-\\");
        h.update(|p, _, _| assert!(p.visible_sidebar()));
        h.keystroke("cmd-b");
        h.keystroke("cmd-\\");
        h.keystroke("cmd-b");
        h.keystroke("cmd-\\");
        h.update(|p, _, _| assert!(!p.visible_sidebar()));
        h.update(|p, _, cx| {
            let a = p.tabs.model.active().unwrap();
            p.open_thread_tab("thr_0c44a9d1".into(), repo.to_owned(), cx);
            let b = p.tabs.model.active().unwrap();
            p.tabs
                .model
                .work_mut()
                .unwrap()
                .toggle(width, Some(false))
                .unwrap();
            assert_eq!(
                p.tabs
                    .model
                    .work()
                    .unwrap()
                    .layout(width, width, height)
                    .unwrap()
                    .panes[0]
                    .rect
                    .width,
                420.
            );
            p.tabs.model.activate(a).unwrap();
            assert!(
                p.tabs
                    .model
                    .work()
                    .unwrap()
                    .tree
                    .as_ref()
                    .unwrap()
                    .layout(width, height)
                    .panes[0]
                    .rect
                    .width
                    < 400.
            );
            p.tabs.model.activate(b).unwrap();
        });
        h.connect(repo, false);
        h.update(|p, _, _| undo::inject(p, vec![]));
        h.update(|p, w, cx| p.toggle_work(w, cx));
        h.update(|p, _, _| assert!(!p.tabs.model.work().unwrap().recorded));
        h.capture(&format!("work-no-record-{suffix}"));
        let mut small = this_turn::harness(repo, output, 780., height);
        small.connect(repo, false);
        small.keystroke("cmd-\\");
        small.update(|p, _, _| assert!(!p.tabs.model.work().unwrap().open));
        small.capture(&format!("work-disabled-{suffix}"));
        assert!(
            small
                .draw_a11y()
                .has_node(Role::Tab, |n| n.label.as_deref() == Some("Work")
                    && n.description.as_deref()
                        == Some("Work view needs a window at least 800 points wide."))
        );
    }
}

fn pane_controls(h: &mut Harness, repo: &Path, suffix: &str) {
    use crate::ui::{ClosePane, FocusNextPane, SplitPaneDown, SplitPaneRight, file_view::FindIn};
    use plait_client::panes::{PaneKind, PaneRefusal};
    let log = plait_engine::git::log(repo, None, 0, 10).unwrap();
    let commit = log.commits[0].id.clone();
    let patch = plait_engine::git::show(repo, &commit).unwrap();
    h.update(|p, _, _| {
        p.state.repository.data.commits.list = Load::Ready(log);
        p.state.repository.view.history.selected = Some(commit);
        p.state.repository.view.history.patch = Load::Ready(Arc::new(patch));
    });
    click(h, "Changes pane kind");
    h.capture(&format!("work-kind-menu-{suffix}"));
    let menu = h.draw_a11y();
    for name in ["History", "Files"] {
        assert!(menu.has_node(Role::MenuItem, |n| n.label.as_deref() == Some(name)));
    }
    assert!(!menu.has_node(Role::MenuItem, |n| matches!(
        n.label.as_deref(),
        Some("Thread" | "Changes")
    )));
    h.keystroke("escape");
    h.update(|p, _, _| assert!(p.tabs.model.work().unwrap().kind_menu.is_none()));
    click(h, "Changes pane kind");
    click(h, "History");
    h.capture(&format!("work-history-pane-{suffix}"));
    h.update(|p, _, _| assert_eq!(p.find_target(), None));
    h.keystroke("cmd-shift-2");
    if suffix == "wide" {
        h.update(|_, w, cx| w.dispatch_action(Box::new(SplitPaneRight), cx));
    } else {
        h.update(|_, w, cx| w.dispatch_action(Box::new(SplitPaneDown), cx));
    }
    select_fixture_file(h, repo);
    h.keystroke("cmd-f");
    h.update(|p, _, _| assert!(p.workspace.file_viewer.find.open));
    h.keystroke("escape");
    h.update(|p, _, _| assert_eq!(p.find_target(), Some(FindIn::File)));
    h.keystroke("cmd-alt-left");
    h.update(|p, _, _| assert_eq!(p.find_target(), Some(FindIn::Changes)));
    h.keystroke("cmd-alt-right");
    h.update(|p, _, _| assert_eq!(p.find_target(), Some(FindIn::File)));
    h.capture(&format!("work-three-panes-{suffix}"));
    h.keystroke("cmd-shift-1");
    h.update(|p, _, _| assert_eq!(p.find_target(), Some(FindIn::Conversation)));
    h.update(|_, w, cx| w.dispatch_action(Box::new(SplitPaneDown), cx));
    h.capture(&format!("work-split-down-{suffix}"));
    h.keystroke("cmd-shift-2");
    h.capture(&format!("work-four-panes-{suffix}"));
    let frame = h.draw_a11y();
    assert_eq!(
        frame
            .nodes
            .iter()
            .filter(
                |n| matches!(n.label.as_deref(), Some("Split right" | "Split down"))
                    && n.description.as_deref() == Some(PaneRefusal::AtPaneLimit.reason())
            )
            .count(),
        8
    );
    h.update(|p, _, _| {
        let panes = p.work_layout().unwrap().panes;
        for kind in [
            PaneKind::Thread,
            PaneKind::Changes,
            PaneKind::Files,
            PaneKind::History,
        ] {
            assert_eq!(panes.iter().filter(|p| p.kind == kind).count(), 1);
        }
    });
    for kind in ["cmd-shift-3", "cmd-shift-2"] {
        h.keystroke(kind);
        h.update(|_, w, cx| w.dispatch_action(Box::new(ClosePane), cx));
        h.draw();
    }
    click(h, "Close pane");
    h.draw();
    h.update(|p, w, cx| {
        assert!(!p.draws_work());
        assert!(!w.is_action_available(&FocusNextPane, cx));
    });
    click(h, "Work");
}

fn select_fixture_file(h: &mut Harness, repo: &Path) {
    let path = Path::new("src/review.rs");
    let (stamp, bytes) = plait_engine::file_view::read_bytes(repo, path).unwrap();
    let opened = plait_client::file_view::classify(stamp, bytes, path);
    h.update(|p, _, _| {
        let viewer = &mut p.workspace.file_viewer;
        viewer.path = Some(path.into());
        viewer.root = repo.into();
        viewer.opened = Some(Arc::new(opened));
        p.workspace.restore_pending[3] = false;
    });
}

#[core::prelude::v1::test]
fn work_view_tab_memory() {
    use crate::ui::tab_lifecycle::Intent;
    use plait_client::panes::{PaneCommand, PaneKind, SplitDirection};

    if !isolated("work_view::work_view_tab_memory") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-pane-memory-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = this_turn::harness(&repo, None, 900., 600.);
    undo::prepare(&mut h, &repo);
    tabs::install(&mut h, 2);
    h.update(|p, _, _| p.host.connection.connected = false);
    h.draw();
    h.keystroke("cmd-\\");
    h.keystroke("cmd-shift-2");
    h.keystroke("cmd-f");
    h.update(|p, _, cx| {
        p.workspace
            .diff_find
            .field
            .as_ref()
            .unwrap()
            .update(cx, |f, cx| f.set_text("refresh", cx));
    });
    h.update(|p, w, cx| p.pane_command(PaneCommand::Split(SplitDirection::Stacked), w, cx));
    select_fixture_file(&mut h, &repo);
    h.keystroke("cmd-f");
    h.update(|p, _, cx| {
        p.workspace
            .file_viewer
            .find
            .field
            .as_ref()
            .unwrap()
            .update(cx, |f, cx| f.set_text("notify", cx));
    });
    h.update(|p, w, cx| {
        assert_eq!(p.work_layout().unwrap().panes.len(), 3);
        assert!(p.durable_tab_bookmark(Some(w), cx).files.path.is_none());
        let next = p.tabs.model.tabs()[1].id();
        p.request_tab(Intent::Activate(next), w, cx);
    });
    h.draw();
    h.update(|p, w, cx| {
        assert!(!p.draws_work());
        p.toggle_work(w, cx);
    });
    h.draw();
    h.update(|p, w, cx| {
        assert_eq!(p.work_layout().unwrap().panes.len(), 2);
        assert!(p.workspace.file_viewer.path.is_none());
        let first = p.tabs.model.tabs()[0].id();
        p.request_tab(Intent::Activate(first), w, cx);
    });
    h.draw();
    undo::wait(&mut h, |p| p.workspace.file_viewer.path.is_some());
    h.update(|p, _, cx| {
        assert_eq!(p.work_layout().unwrap().panes.len(), 3);
        assert_eq!(p.focused_work_kind(), Some(PaneKind::Files));
        assert_eq!(
            p.workspace.file_viewer.path.as_deref(),
            Some(Path::new("src/review.rs"))
        );
        assert_eq!(
            p.workspace
                .file_viewer
                .find
                .field
                .as_ref()
                .unwrap()
                .read(cx)
                .text(),
            "notify"
        );
        assert!(p.workspace.diff_find.open);
        assert_eq!(
            p.workspace
                .diff_find
                .field
                .as_ref()
                .unwrap()
                .read(cx)
                .text(),
            "refresh"
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn work_scope_and_focus_round_trip() {
    if !isolated("work_view::work_scope_and_focus_round_trip") {
        return;
    }
    use plait_client::panes::PaneKind;
    let scratch = std::env::temp_dir().join(format!("plait-work-focus-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = this_turn::harness(&repo, None, 1200., 800.);
    turn_review::scene(&mut h, &repo, "ready", None);
    h.update(|p, _, _| p.workspace.tab = crate::ui::Workspace::Conversation);
    h.draw();
    h.update(|p, w, cx| {
        p.focus_composer(w, cx);
        assert!(p.visible_composer().unwrap().focus_handle(cx).is_focused(w));
    });
    click(&mut h, "Work");
    h.update(|p, w, _| {
        assert!(p.work_focus[PaneKind::Thread as usize].is_focused(w));
    });
    h.draw();
    h.update(|p, w, cx| {
        p.work_select_file(0, true, w, cx);
        assert_eq!(p.focused_work_kind(), Some(PaneKind::Changes));
        assert!(p.work_focus[PaneKind::Changes as usize].is_focused(w));
        p.choose_work_scope(false, w, cx);
        assert!(p.workspace.turn_review.target.is_none());
        p.toggle_work(w, cx);
        assert!(p.visible_composer().unwrap().focus_handle(cx).is_focused(w));
        let target = p.workspace.turn_review.target.as_ref().unwrap();
        assert_eq!(target.record, "last");
        assert!(target.path.is_none());
        p.clear_turn_review();
        p.toggle_work(w, cx);
        p.choose_work_scope(true, w, cx);
        assert!(p.workspace.turn_review.target.is_some());
        p.toggle_work(w, cx);
        assert!(p.workspace.turn_review.target.is_none());
    });
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn fallback_routes_only_chat_actions() {
    if !isolated("work_view::fallback_routes_only_chat_actions") {
        return;
    }
    use crate::ui::{Workspace, tab_lifecycle::Intent};
    use plait_client::panes::{PaneCommand, PaneKind, SplitDirection};
    use plait_client::tabs::TabTarget;
    let scratch = std::env::temp_dir().join(format!("plait-work-routing-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = this_turn::harness(&repo, None, 1200., 800.);
    h.connect(&repo, false);
    h.draw();
    h.update(|p, w, cx| {
        p.toggle_work(w, cx);
        p.route_work_tab(Workspace::Changes, w, cx);
        p.panels.viewport = 799.;
        assert!(p.visible_sidebar());
        let tree = p.tabs.model.work().unwrap().tree.as_ref().unwrap();
        let focused = tree.focused();
        p.toggle_work_kind_menu(focused, w, cx);
        assert!(p.tabs.model.work().unwrap().kind_menu.is_none());
        for command in [
            PaneCommand::Next,
            PaneCommand::Previous,
            PaneCommand::Split(SplitDirection::Stacked),
            PaneCommand::Kind(PaneKind::History),
            PaneCommand::Close,
        ] {
            p.pane_command(command, w, cx);
        }
        let tree = p.tabs.model.work().unwrap().tree.as_ref().unwrap();
        assert_eq!(tree.focused(), focused);
        assert_eq!(tree.pane_kind(focused), Some(PaneKind::Changes));
        assert_eq!(tree.pane_count(), 2);
        p.show_tab(Workspace::History, w, cx);
        p.panels.viewport = 1200.;
        assert!(p.work_layout().is_none());
        assert_eq!(p.workspace.tab, Workspace::History);
        p.toggle_work(w, cx);
        p.workspace.work_area.size.height = px(120.);
        let thread = p.tabs.model.active().unwrap();
        let target = TabTarget::Worktrees {
            checkout: p.tab_checkout(repo.clone()),
        };
        p.request_tab(Intent::Open(target), w, cx);
        assert!(p.work_layout().is_none());
        p.request_tab(Intent::Activate(thread), w, cx);
    });
    h.draw();
    h.update(|p, _, _| assert!(!p.tabs.model.work().unwrap().open));
    std::fs::remove_dir_all(scratch).unwrap();
}
