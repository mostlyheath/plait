//! The header action across the host's checkout status rows.
use super::*;
use plait_client::model::Tracking;

pub(super) fn walk(output: Option<&Path>) {
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut h = header::fixture(output, width, height);
        for (scene, expected, counts) in [
            ("commit", Some("Commit…"), None),
            ("publish", Some("Publish"), None),
            ("push", Some("Push"), Some((2, 0))),
            ("pull", Some("Pull"), Some((0, 2))),
            ("diverged", Some("Push"), Some((1, 1))),
            ("clean", None, Some((0, 0))),
            ("plain-folder", None, None),
        ] {
            h.update(|p, _, _| {
                let old = p.state.repository.snapshot().ready().unwrap();
                p.state.repository.data.snapshot =
                    Load::Ready(Arc::new(plait_client::model::Snapshot {
                        root: old.root.clone(),
                        branch: old.branch.clone(),
                        named: true,
                        changes: if scene == "commit" {
                            old.changes.clone()
                        } else {
                            vec![]
                        },
                        tracking: counts.map(|(ahead, behind)| Tracking {
                            upstream: "origin/main".into(),
                            ahead,
                            behind,
                            gone: false,
                        }),
                        plain: scene == "plain-folder",
                        elapsed: Duration::ZERO,
                    }));
            });
            #[cfg(debug_assertions)]
            {
                let frame = h.draw_a11y();
                let action = frame
                    .nodes
                    .iter()
                    .find(|node| node.path.contains("header-git-step"));
                if scene == "diverged" {
                    assert!(
                        action
                            .and_then(|n| n.description.as_deref())
                            .unwrap()
                            .contains("Merge or rebase")
                    );
                }
                assert_eq!(action.and_then(|n| n.label.as_deref()), expected, "{scene}");
            }
            h.capture(&format!("git-step-{scene}-{suffix}"));
        }
    }
}

#[core::prelude::v1::test]
fn header_git_step_matches_status() {
    if isolated("git_step::header_git_step_matches_status") {
        walk(None);
    }
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn commit_step_opens_uncommitted_without_writing() {
    if !isolated("git_step::commit_step_opens_uncommitted_without_writing") {
        return;
    }
    let mut h = header::fixture(None, 1380., 860.);
    h.update(|p, _, _| {
        let panel = p.tabs.model.panel_mut().unwrap();
        panel.open_on(plait_client::panel::PanelKind::History);
        panel.select_scope(plait_client::panel::Scope::ThisTurn);
    });
    let position = h
        .draw_a11y()
        .nodes
        .into_iter()
        .find(|n| n.label.as_deref() == Some("Commit…"))
        .unwrap()
        .bounds
        .center();
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
    h.update(|p, window, cx| {
        let panel = p.tabs.model.panel().unwrap();
        assert!(panel.open);
        assert_eq!(panel.kind, plait_client::panel::PanelKind::Changes);
        assert_eq!(panel.scope, plait_client::panel::Scope::Uncommitted);
        assert!(
            p.workspace
                .commit_message
                .focus_handle(cx)
                .is_focused(window)
        );
        assert!(!p.state.repository.write().busy());
    });
}
