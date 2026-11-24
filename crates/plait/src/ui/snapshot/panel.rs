//! Exercise the three readers in the same headless sheet used by a thread.
use super::*;
use plait_client::panel::{PanelKind, Scope};

pub(super) fn scenes(h: &mut Harness, repo: &Path, suffix: &str) {
    turn_review::scene(h, repo, "ready", None);
    h.update(|p, _, _| {
        p.workspace.tab = crate::ui::Workspace::Conversation;
        p.state.projects[0].machines[0].name = "studio".into();
        let mut latest = undo::record("last", &["last"], "ready");
        latest["created_at"] = serde_json::json!((crate::ui::commits::now() - 30) * 1000);
        latest["paths"][0]["added"] = serde_json::json!(2);
        undo::inject(p, vec![latest]);
        p.tabs.model.panel_mut().unwrap().close();
    });
    h.capture(&format!("panel-closed-{suffix}"));
    h.update(|p, _, _| {
        p.tabs
            .model
            .panel_mut()
            .unwrap()
            .open_on(PanelKind::Changes)
    });
    h.capture(&format!("panel-changes-turn-{suffix}"));
    h.update(|p, _, _| {
        let mut earlier = undo::record("first", &["first"], "ready");
        let now = crate::ui::commits::now();
        earlier["created_at"] = serde_json::json!((now - 630) * 1000);
        let mut latest = undo::record("last", &["last"], "ready");
        latest["created_at"] = serde_json::json!((now - 30) * 1000);
        undo::inject(p, vec![latest, earlier]);
        p.workspace.turn_records.selection.earlier("first");
        let target = plait_client::review::recorded::Target {
            thread: THREAD.into(),
            checkout: repo.into(),
            record: "first".into(),
            original: "first".into(),
            path: None,
        };
        let record = plait_client::review::recorded::Record::prepare(
            &p.workspace.turn_records.data,
            &target,
        )
        .unwrap();
        p.workspace.turn_review.selected = record.selected(&target);
        p.workspace.turn_review.target = Some(target);
        p.workspace.turn_review.record = Some(Arc::new(record));
    });
    h.capture(&format!("panel-changes-turn-earlier-{suffix}"));
    h.update(|p, _, cx| {
        p.tabs
            .model
            .panel_mut()
            .unwrap()
            .select_scope(Scope::Uncommitted);
        p.clear_turn_review();
        p.state.repository.data.snapshot = Load::Ready(Arc::new(plait_client::model::Snapshot {
            root: repo.into(),
            branch: "main".into(),
            named: true,
            tracking: None,
            changes: vec![plait_client::model::Change {
                path: "src/review.rs".into(),
                original: None,
                index: b'M',
                worktree: b'M',
            }],
            elapsed: Duration::ZERO,
            plain: false,
        }));
        p.state
            .repository
            .select(0, plait_client::model::DiffMode::Unstaged);
        p.pump(cx);
    });
    h.capture(&format!("panel-changes-uncommitted-{suffix}"));
    h.update(|p, _, cx| {
        p.state
            .repository
            .select(0, plait_client::model::DiffMode::Staged);
        p.pump(cx);
    });
    h.capture(&format!("panel-changes-staged-{suffix}"));
    h.update(|p, _, _| {
        undo::inject(p, vec![]);
        p.tabs
            .model
            .panel_mut()
            .unwrap()
            .select_scope(Scope::ThisTurn);
        assert!(!p.panel_recorded());
    });
    h.capture(&format!("panel-no-record-{suffix}"));
    h.update(|p, _, _| p.tabs.model.panel_mut().unwrap().open_on(PanelKind::Files));
    let path = Path::new("src/review.rs");
    let (stamp, bytes) = plait_engine::file_view::read_bytes(repo, path).unwrap();
    h.update(|p, _, _| {
        p.workspace.file_viewer.root = repo.into();
        p.workspace.file_viewer.path = Some(path.into());
        p.workspace.file_viewer.opened = Some(Arc::new(plait_client::file_view::classify(
            stamp, bytes, path,
        )));
    });
    h.capture(&format!("panel-files-{suffix}"));
    let log = plait_engine::git::log(repo, None, 0, 10).unwrap();
    let id = log.commits[0].id.clone();
    let patch = plait_engine::git::show(repo, &id).unwrap();
    h.update(|p, _, _| {
        p.tabs
            .model
            .panel_mut()
            .unwrap()
            .open_on(PanelKind::History);
        p.state.repository.data.commits.list = Load::Ready(log);
        p.state.repository.view.history.selected = Some(id);
        p.state.repository.view.history.patch = Load::Ready(Arc::new(patch));
    });
    h.capture(&format!("panel-history-{suffix}"));
    h.update(|p, _, _| p.tabs.model.panel_mut().unwrap().set_fill(true));
    h.capture(&format!("panel-fill-{suffix}"));
    h.update(|p, _, _| {
        p.tabs.model.panel_mut().unwrap().set_fill(false);
        p.preferences.value.panel_width = Some(384);
    });
    if suffix == "narrow" {
        h.update(|p, _, _| assert!(p.panel_fills()));
        h.capture("panel-narrow");
    } else {
        h.capture("panel-dragged");
    }
    h.update(|p, _, _| {
        p.preferences.value.panel_width = None;
        p.workspace.turn_review.older = true;
        p.tabs
            .model
            .panel_mut()
            .unwrap()
            .open_on(PanelKind::Changes);
    });
    h.capture(&format!("panel-older-host-{suffix}"));
    h.update(|p, _, _| p.tabs.model.panel_mut().unwrap().close());
}

#[core::prelude::v1::test]
fn keys_focus_fill_and_tab_memory() {
    if !isolated("panel::keys_focus_fill_and_tab_memory") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repo = fixture_repository(&home.join("panel"));
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    h.draw();
    h.keystroke("cmd-l");
    h.draw();
    h.update(|p, _, _| {
        assert_eq!(p.panel_kind(), Some(PanelKind::Changes));
        assert!(!p.panel_fills());
    });
    h.keystroke("cmd-alt-right");
    h.update(|p, w, _| assert!(p.focus.is_focused(w)));
    h.keystroke("cmd-alt-left");
    h.update(|p, w, cx| {
        assert!(p.panel_focus[p.panel_focus_kind() as usize].contains_focused(w, cx));
    });
    h.keystroke("cmd-shift-3");
    h.draw();
    h.update(|p, _, _| assert_eq!(p.panel_kind(), Some(PanelKind::History)));
    h.keystroke("cmd-\\");
    h.update(|p, _, _| assert!(p.panel_fills()));
    h.update(|p, _, cx| {
        let first = p.tabs.model.active().unwrap();
        p.open_thread_tab("thr_0c44a9d1".into(), repo.clone(), cx);
        assert!(!p.panel_is_open());
        p.tabs.model.panel_mut().unwrap().open_on(PanelKind::Files);
        p.tabs.model.activate(first).unwrap();
        assert_eq!(p.panel_kind(), Some(PanelKind::History));
        assert!(p.panel_fills());
    });
    h.keystroke("cmd-shift-1");
    h.update(|p, _, _| assert!(!p.panel_is_open()));
}

#[core::prelude::v1::test]
fn filling_from_conversation_moves_find_into_panel() {
    if !isolated("panel::filling_from_conversation_moves_find_into_panel") {
        return;
    }
    use crate::ui::file_view::FindIn;
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repo = fixture_repository(&home.join("panel-focus"));
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    h.draw();
    h.keystroke("cmd-l");
    h.draw();
    h.update(|p, _, _| assert_eq!(p.find_target(), Some(FindIn::Changes)));
    h.update(|p, w, cx| {
        assert!(!p.panel_fills());
        w.focus(&p.focus, cx);
        assert!(p.focus.is_focused(w));
    });
    h.draw();
    h.keystroke("cmd-\\");
    h.draw();
    h.update(|p, w, cx| {
        assert!(p.panel_fills());
        assert!(
            p.panel_focus[plait_client::panes::PaneKind::Changes as usize].contains_focused(w, cx)
        );
        assert_eq!(p.find_target(), Some(FindIn::Changes));
    });
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn panel_changes_handoff_stays_on_the_scope_row() {
    if !isolated("panel::panel_changes_handoff_stays_on_the_scope_row") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repo = fixture_repository(&home.join("panel-bar"));
    let mut h = Harness::new(&repo, None, 1380., 860.);
    turn_review::scene(&mut h, &repo, "ready", None);
    h.update(|p, _, _| {
        p.workspace.tab = crate::ui::Workspace::Conversation;
        p.tabs
            .model
            .panel_mut()
            .unwrap()
            .open_on(PanelKind::Changes);
    });
    h.draw();
    let frame = h.draw_a11y();
    let bounds = |id: &str| {
        frame
            .nodes
            .iter()
            .find(|node| node.path.ends_with(id))
            .unwrap_or_else(|| panic!("missing {id}"))
            .bounds
    };
    let scope = bounds("panel-recorded");
    let editor = bounds("panel-editor");
    assert!(editor.center().y <= scope.bottom(), "the handoff wrapped");
    assert!(
        bounds("review-panel").right() - editor.right() <= px(12.),
        "the handoff is not at the right edge"
    );
}

#[core::prelude::v1::test]
fn panel_scenes() {
    if !isolated("panel::panel_scenes") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repo = fixture_repository(&home.join("panel-scenes"));
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut h = Harness::new(&repo, output.as_deref(), width, height);
        scenes(&mut h, &repo, suffix);
    }
}
