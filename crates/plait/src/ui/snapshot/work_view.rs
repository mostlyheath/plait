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
