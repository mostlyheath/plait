use super::*;
use plait_client::host::checkout_queue::{State, Summary};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn thread(
    id: String,
    checkout: PathBuf,
    title: &str,
    status: ThreadStatus,
    updated: u64,
) -> ThreadSummary {
    ThreadSummary {
        id,
        checkout,
        model: choice(),
        title: Some(title.into()),
        status,
        created_at: Some(updated - 60_000),
        updated_at: Some(updated),
        harness: Some(HarnessId::Pi),
        ..Default::default()
    }
}

fn reset(p: &mut Plait) {
    p.sidebar = true;
    p.host.connection.status.as_mut().unwrap().protocol_version = 4;
    p.navigation.collapsed.clear();
    p.navigation.archived = false;
    p.navigation.menu = None;
    p.navigation.available.clear();
    p.preferences.value.navigation = Default::default();
    p.navigation.scroll.scroll_to(ListOffset::default());
}

fn rebuild_scene_projects(p: &mut Plait, cx: &mut Context<Plait>) {
    p.rebuild_projects(cx);
    // The wide inspector prints the checkout path; keep the card fixtures portable.
    p.preferences.value.hide_inspector = true;
    // The workspace also draws these labels in sidebar captures.
    for project in &mut p.state.projects {
        project.machines[0].name = "studio".into();
    }
}

fn states(h: &mut Harness, repository: &Path) {
    h.connect(repository, false);
    h.update(|p, _, cx| {
        reset(p);
        let now = now();
        use ThreadStatus::*;
        *p.host.connection.threads_mut() = [
            ("Review the permission check", AwaitingApproval),
            ("Repair the failing tests", Error),
            ("Resume the interrupted import", Interrupted),
            ("Read the parser results", Idle),
            ("Build the desktop", Running),
            ("Wait for the same checkout", Idle),
            ("Plan a release", Idle),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (title, status))| {
            let id = if index == 4 {
                THREAD.into()
            } else {
                format!("cards-{index}")
            };
            thread(
                id,
                repository.into(),
                title,
                status,
                now - (index as u64 + 1) * 60_000,
            )
        })
        .collect();
        let threads = p.host.connection.threads_mut();
        threads[5].checkout_queue = Some(Summary::new("request".into(), State::Waiting, 1, 1));
        // An older summary can have a title and no branch or details line.
        threads[6].updated_at = None;
        threads[6].model = ModelChoice::default();
        threads[6].harness = None;
        p.preferences
            .value
            .navigation
            .unread
            .insert("cards-3".into());
        rebuild_scene_projects(p, cx);
    });
}

pub(super) fn install_many(h: &mut Harness, repository: &Path) {
    h.update(|p, _, cx| {
        reset(p);
        let now = now();
        let projects = [
            repository.to_owned(),
            repository.join("api"),
            repository.join("data"),
            repository.join("desktop"),
            repository.join("docs"),
            repository.join("runtime"),
        ];
        let threads = (0..300)
            .map(|index| {
                let project = &projects[index / 50];
                let checkout = if index % 4 == 0 {
                    let path = project.join(format!("worktree-{index}"));
                    p.preferences.value.navigation.remember_worktree(
                        path.clone(),
                        project.clone(),
                        format!("plait/review-{index}"),
                    );
                    path
                } else {
                    project.clone()
                };
                thread(
                    if index == 0 {
                        THREAD.into()
                    } else {
                        format!("many-cards-{index}")
                    },
                    checkout,
                    &format!("Review change {}", index + 1),
                    if index % 13 == 0 {
                        ThreadStatus::Running
                    } else {
                        ThreadStatus::Idle
                    },
                    now - (index as u64 + 1) * 60_000,
                )
            })
            .collect();
        *p.host.connection.threads_mut() = threads;
        p.navigation
            .available
            .extend(projects.into_iter().map(|project| (project, true)));
        rebuild_scene_projects(p, cx);
    });
}

fn item(h: &mut Harness, index: usize) -> Bounds<Pixels> {
    h.window
        .update(&mut h.cx, |p, _, _| {
            p.navigation
                .scroll
                .bounds_for_item(index)
                .expect("measured card or project")
        })
        .unwrap()
}

fn hover(h: &mut Harness, position: Point<Pixels>) {
    h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
        position,
        pressed_button: None,
        modifiers: Modifiers::default(),
    }));
}

fn click(h: &mut Harness, position: Point<Pixels>, button: MouseButton) {
    h.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button,
        position,
        click_count: 1,
        modifiers: Modifiers::default(),
        first_mouse: false,
    }));
    h.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button,
        position,
        click_count: 1,
        modifiers: Modifiers::default(),
    }));
}

#[cfg(debug_assertions)]
fn assert_menu_actions(h: &mut Harness, labels: &[&str]) {
    let (sidebar_width, window_height) = h
        .window
        .update(&mut h.cx, |p, window, _| {
            (
                px(p.panel_width(super::super::panels::Panel::Sidebar)),
                window.viewport_size().height,
            )
        })
        .unwrap();
    let frame = h.draw_a11y();
    for &label in labels {
        assert!(
            frame.has_node(Role::Button, |node| {
                node.label.as_deref() == Some(label)
                    && node.bounds.left() >= px(0.)
                    && node.bounds.top() >= px(0.)
                    && node.bounds.right() <= sidebar_width
                    && node.bounds.bottom() <= window_height
                    && node.bounds.size.width > px(0.)
                    && node.bounds.size.height > px(0.)
            }),
            "missing sidebar menu action within the window: {label}"
        );
    }
}

pub(super) fn walk(h: &mut Harness, repository: &Path, suffix: &str) {
    let hide_inspector = h
        .window
        .update(&mut h.cx, |p, _, _| p.preferences.value.hide_inspector)
        .unwrap();
    states(h, repository);
    capture_both(h, &format!("cards-states-{suffix}"));
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        for (state, title) in [
            ("Needs approval", "Review the permission check"),
            ("Failed", "Repair the failing tests"),
            ("Interrupted", "Resume the interrupted import"),
            ("Done", "Read the parser results. Unread activity"),
            ("Working", "Build the desktop"),
        ] {
            assert!(
                frame.has_node(Role::TreeItem, |node| node
                    .label
                    .as_ref()
                    .is_some_and(|label| label.contains(&format!("{state}. {title}")))),
                "missing accessible card: {state}, {title}"
            );
        }
    }

    let approval = item(h, 1);
    assert!(
        item(h, 7).size.height < approval.size.height,
        "a card with no metadata is shorter"
    );
    hover(h, approval.center());
    h.capture(&format!("cards-selected-hover-{suffix}"));
    let action = point(approval.right() - px(14.), approval.top() + px(14.));
    hover(h, action);
    h.draw();
    click(h, action, MouseButton::Left);
    h.update(|p, _, _| {
        assert_eq!(p.navigation.menu.as_deref(), Some("cards-0"));
        assert_eq!(p.host.thread.selected.as_deref(), Some(THREAD));
    });
    h.capture(&format!("cards-thread-menu-{suffix}"));
    #[cfg(debug_assertions)]
    assert_menu_actions(h, &["Rename thread…", "Archive thread"]);
    h.update(|p, _, _| p.navigation.menu = None);
    h.draw();
    click(h, approval.center(), MouseButton::Right);
    h.update(|p, _, _| {
        assert_eq!(p.navigation.menu.as_deref(), Some("cards-0"));
    });
    #[cfg(debug_assertions)]
    assert_menu_actions(h, &["Rename thread…", "Archive thread"]);
    h.update(|p, _, _| p.navigation.menu = None);
    h.draw();

    let project = item(h, 0);
    let action = point(project.right() - px(11.), project.center().y);
    hover(h, action);
    h.draw();
    click(h, action, MouseButton::Left);
    h.update(|p, _, _| {
        assert_eq!(
            p.navigation.menu.as_deref(),
            Some(format!("project:{}", repository.display()).as_str())
        )
    });
    h.capture(&format!("cards-project-menu-{suffix}"));
    #[cfg(debug_assertions)]
    assert_menu_actions(h, &["Project settings…", "Remove from sidebar"]);

    h.update(|p, _, cx| {
        p.navigation.menu = None;
        let checkout = repository.join("worktree");
        p.host.connection.threads_mut()[0].checkout = checkout.clone();
        p.preferences.value.navigation.remember_worktree(
            checkout,
            repository.into(),
            "plait/review-permission-check".into(),
        );
        rebuild_scene_projects(p, cx);
    });
    h.capture(&format!("cards-worktree-{suffix}"));
    assert!(
        item(h, 1).size.height > approval.size.height,
        "the known worktree branch adds a measured line"
    );
    h.update(|p, window, cx| p.begin_rename("cards-0".into(), window, cx));
    h.capture(&format!("cards-rename-{suffix}"));
    h.keystroke("escape");
    h.update(|p, _, _| assert!(p.navigation.rename.is_none()));

    h.update(|p, _, _| {
        p.host.connection.threads_mut()[1].archived = Some(true);
        p.navigation.archived = true;
    });
    h.capture(&format!("cards-archived-{suffix}"));
    h.update(|p, _, _| {
        p.navigation.collapsed.insert(repository.into());
        p.navigation.invalidate_rows();
    });
    h.capture(&format!("cards-folded-{suffix}"));
    h.update(|p, _, _| {
        p.navigation.available.insert(repository.into(), false);
    });
    h.capture(&format!("cards-missing-project-{suffix}"));

    install_many(h, repository);
    h.capture(&format!("cards-many-{suffix}"));
    h.update(|p, _, _| {
        assert_eq!(p.navigation.scroll.item_count(), 306);
        assert!(
            p.navigation.scroll.bounds_for_item(20).is_none(),
            "offscreen rows stay unmeasured"
        );
    });
    if suffix == "narrow" {
        let project = item(h, 0).center();
        click(h, project, MouseButton::Left);
        h.update(|p, _, _| assert!(p.navigation.collapsed.contains(repository)));
        h.draw();
        h.keystroke("enter");
        // GPUI activates a button when the pressed key is released.
        h.pointer(PlatformInput::KeyUp(KeyUpEvent {
            keystroke: Keystroke::parse("enter").unwrap(),
        }));
        h.update(|p, _, _| assert!(!p.navigation.collapsed.contains(repository)));
        for _ in 0..18 {
            h.draw();
            h.keystroke("down");
        }
        h.update(|p, window, cx| {
            assert!(p.navigation.scroll.logical_scroll_top().item_ix > 0);
            window.focus(&p.focus, cx);
        });
    }
    h.update(|p, _, _| {
        p.navigation.scroll.scroll_to(ListOffset {
            item_ix: 150,
            offset_in_item: px(0.),
        });
    });
    h.capture(&format!("cards-many-scrolled-{suffix}"));

    // Restore the ordinary two-thread scene for the shared shortcut checks.
    h.update(|p, _, _| {
        reset(p);
        p.preferences.value.hide_inspector = hide_inspector;
    });
    h.connect(repository, false);
    h.update(|p, _, cx| p.rebuild_projects(cx));
}

pub(super) fn compact(repository: &Path, output: Option<&Path>) {
    let mut h = Harness::with_host(repository, output, 720., 600., None);
    states(&mut h, repository);
    capture_both(&mut h, "cards-narrow");
    assert!(item(&mut h, 1).size.width <= px(236.));
}

#[core::prelude::v1::test]
fn card_evidence_keeps_fixture_paths_out_of_the_window() {
    if !isolated("cards::card_evidence_keeps_fixture_paths_out_of_the_window") {
        return;
    }
    let repository =
        fixture_repository(&PathBuf::from(std::env::var_os("HOME").unwrap()).join("card-evidence"));
    let mut harness = Harness::with_host(&repository, None, 1380., 860., None);
    for stage in [states, install_many] {
        stage(&mut harness, &repository);
        harness.update(|plait, _, _| {
            assert_eq!(plait.active_project().machines[0].name, "studio");
            assert!(
                !plait.shows_inspector(true),
                "the wide inspector displays the fixture checkout path"
            );
        });
    }
}

#[core::prelude::v1::test]
fn sidebar_cards_draw_and_navigate() {
    if !isolated("cards::sidebar_cards_draw_and_navigate") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-cards-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut h = Harness::with_host(&repository, None, width, height, None);
        walk(&mut h, &repository, suffix);
    }
    compact(&repository, None);
    std::fs::remove_dir_all(scratch).unwrap();
}
