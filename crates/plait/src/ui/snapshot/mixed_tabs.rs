//! Draft transitions beside a project tab keep both content kinds independent.
use super::*;
use crate::ui::tab_lifecycle::Intent;

#[core::prelude::v1::test]
fn closing_drafts_selects_worktrees_on_either_side() {
    if !isolated("mixed_tabs::closing_drafts_selects_worktrees_on_either_side") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-mixed-close-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    for right in [false, true] {
        for action in ["discard", "escape", "close-empty", "close-text"] {
            let mut h = Harness::new(&repo, None, 900., 600.);
            h.connect(&repo, false);
            h.update(|p, w, cx| {
                p.reset_tabs_for_test();
                p.park_workspace(cx);
                p.session.data = Some(plait_client::session::Session::empty("/host".into()));
                p.host.connection.connected = false;
                if !right {
                    p.open_worktrees(repo.clone(), w, cx);
                }
                p.open_draft(w, cx);
                let draft = p.tabs.model.active().unwrap();
                if right {
                    p.open_worktrees(repo.clone(), w, cx);
                    p.request_tab(Intent::Activate(draft), w, cx);
                }
                p.host.connection.connected = true;
                p.sync_composer(cx);
            });
            if matches!(action, "discard" | "close-text") {
                h.type_draft("A draft beside Worktrees");
            }
            h.update(|p, _, _| p.host.connection.connected = false);
            match action {
                "discard" => h.update(|p, w, cx| p.discard_draft(w, cx)),
                "escape" => h.keystroke("escape"),
                _ => h.keystroke("cmd-w"),
            }
            h.update(|p, w, cx| {
                assert_eq!(p.tabs.model.tabs().len(), 1, "{action}, right={right}");
                assert!(p.worktrees_visible());
                assert_eq!(p.tabs.rows[0].title, "Worktrees");
                assert!(p.tabs.rows[0].worktrees);
                assert!(p.tabs.draft_titles.is_empty());
                assert!(p.tabs.draft_row.is_none());
                assert!(p.draft.id.is_none() && p.draft.composer.is_none());
                let kept = usize::from(action == "close-text");
                assert_eq!(p.tabs.model.closed_len(), kept);
                assert_eq!(p.test_saved_drafts().len(), kept);
                // Neither draft action is allowed to erase or close a project tab.
                p.discard_draft(w, cx);
                p.escape_draft(w, cx);
                assert!(p.worktrees_visible());
                assert_eq!(p.tabs.model.tabs().len(), 1);
            });
            if action != "close-text" {
                h.keystroke("cmd-shift-t");
                h.update(|p, _, _| {
                    assert!(p.worktrees_visible());
                    assert_eq!(p.tabs.model.tabs().len(), 1);
                });
            }
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn title_edits_leave_thread_and_worktrees_rows_out_of_the_draft_cache() {
    if !isolated("mixed_tabs::title_edits_leave_thread_and_worktrees_rows_out_of_the_draft_cache") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-mixed-title-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.open_worktrees(repo.clone(), w, cx);
        p.open_draft(w, cx);
        p.host.connection.connected = true;
        p.sync_composer(cx);
    });
    h.type_draft("Draft");
    let (rows, thread, worktrees) = h
        .window
        .update(&mut h.cx, |p, _, _| {
            assert_eq!(p.tabs.rows.len(), 3);
            assert_eq!(p.tabs.draft_titles.len(), 1);
            assert_eq!(p.tabs.draft_row.as_ref().unwrap().1, 2);
            (
                p.tabs.rows.as_ptr(),
                p.tabs.rows[0].title.as_ptr(),
                p.tabs.rows[1].title.as_ptr(),
            )
        })
        .unwrap();
    h.type_draft(" renamed");
    h.window
        .update(&mut h.cx, |p, _, _| {
            assert_eq!(p.tabs.rows.as_ptr(), rows);
            assert_eq!(p.tabs.rows[0].title.as_ptr(), thread);
            assert_eq!(p.tabs.rows[1].title.as_ptr(), worktrees);
            assert_eq!(p.tabs.rows[1].title, "Worktrees");
            assert!(p.tabs.rows[1].hover.starts_with("Worktrees ·"));
            assert_eq!(p.tabs.rows[2].title, "Draft renamed");
            assert_eq!(p.tabs.draft_titles.len(), 1);
        })
        .unwrap();
    h.update(|p, _, _| p.host.connection.connected = false);
    h.keystroke("cmd-2");
    h.update(|p, _, cx| {
        assert!(p.worktrees_visible());
        assert!(p.draft.id.is_none());
        assert!(p.tabs.draft_row.is_none());
        let cached = p.tabs.draft_titles.clone();
        p.refresh_draft_title(cx);
        assert_eq!(p.tabs.draft_titles, cached);
        assert_eq!(p.tabs.rows[1].title, "Worktrees");
    });
    h.keystroke("cmd-3");
    h.update(|p, _, cx| {
        assert_eq!(p.current_draft_title(), "Draft renamed");
        assert_eq!(draft_text(p, cx), "Draft renamed");
        assert_eq!(p.tabs.draft_titles.len(), 1);
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn stage_all_from_worktrees_refuses_the_previous_checkout() {
    if !isolated("mixed_tabs::stage_all_from_worktrees_refuses_the_previous_checkout") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-stage-worktrees-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::with_host(&repo, None, 900., 600., None);
    h.update(|p, w, cx| p.open_worktrees(scratch.join("other"), w, cx));
    h.draw();
    h.update(|_, w, cx| w.dispatch_action(Box::new(crate::ui::StageAll), cx));
    h.update(|p, _, _| {
        assert!(p.worktrees_visible());
        assert_eq!(
            p.notice.as_deref(),
            Some("Checkout identity is not verified.")
        );
        assert!(!p.state.repository.write().busy());
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
