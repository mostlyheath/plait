//! Synthetic recorded Changes scenes and socket-backed UI reply guards.
use super::undo::{Host, inject, prepare, record, setup, wait};
use super::*;
use crate::ui::{Workspace, turn_review::TurnReview};
use plait_client::review::recorded::{Record, Target};
use serde_json::{Value, json};

const PATCH: &str = "diff --git a/src/review.rs b/src/review.rs\n--- a/src/review.rs\n+++ b/src/review.rs\n@@ -1,3 +1,4 @@\n pub fn refresh() {\n-    read_on_frame();\n+    queue_status();\n+    notify_when_ready();\n }\n";
// These synthetic patches need only the line and hunk metadata drawn by the scene.
fn fixture_diff(bytes: &[u8], truncated: bool, elapsed: Duration) -> plait_protocol::model::Diff {
    use plait_protocol::model::{Diff, DiffKind, DiffLine};
    use plait_protocol::patch::{self, FileHeaders};
    let mut lines = Vec::new();
    let mut hunks = Vec::new();
    let (mut old, mut new) = (None, None);
    for line in String::from_utf8_lossy(bytes).lines() {
        if line.starts_with("diff ") {
            old = None;
            new = None;
        }
        if let Some(hunk) = line.strip_prefix("@@ ") {
            let mut ranges = hunk.split_whitespace();
            old = ranges
                .next()
                .and_then(|v| v.strip_prefix('-'))
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.parse::<usize>().ok());
            new = ranges
                .next()
                .and_then(|v| v.strip_prefix('+'))
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.parse::<usize>().ok());
        }
        let kind = if line.starts_with("@@") {
            DiffKind::Hunk
        } else if old.is_none() && new.is_none() {
            DiffKind::Header
        } else if line.starts_with('+') {
            DiffKind::Added
        } else if line.starts_with('-') {
            DiffKind::Removed
        } else {
            DiffKind::Context
        };
        let old_line = if kind == DiffKind::Removed || line.starts_with(' ') {
            old
        } else {
            None
        };
        let new_line = if kind == DiffKind::Added || line.starts_with(' ') {
            new
        } else {
            None
        };
        if old_line.is_some() {
            old = old.map(|n| n.saturating_add(1));
        }
        if new_line.is_some() {
            new = new.map(|n| n.saturating_add(1));
        }
        if kind == DiffKind::Hunk {
            hunks.push(lines.len());
        }
        lines.push(DiffLine {
            text: Arc::from(plait_client::display_text::escape(line)),
            kind,
            old_line,
            new_line,
        });
    }
    if lines.is_empty() {
        return Diff::message(
            "No text diff in this layer. The file may have changed; Refresh to update status.",
            elapsed,
        );
    }
    let patch::Rows { rows, widest } = patch::rows(&lines, FileHeaders::Omit);
    Diff {
        lines,
        rows,
        widest_row: widest,
        hunks,
        patch_digest: None,
        truncated,
        elapsed,
    }
}
fn target(p: &Plait, record: &str, path: Option<&str>) -> Target {
    Target {
        thread: p.host.thread.selected.clone().unwrap(),
        checkout: p.host.creation.checkout.clone(),
        record: record.into(),
        original: "last".into(),
        path: path.map(PathBuf::from),
    }
}
pub(super) fn scene(
    h: &mut Harness,
    repository: &Path,
    kind: &str,
    lines: Option<Arc<plait_client::model::Diff>>,
) {
    prepare(h, repository);
    h.update(|p,_,_| {
        let mut data=record("last", &["last"], "ready");
        data["paths"][0]["added"]=json!(2);
        if kind=="empty" { data["paths"]=json!([]); data["paths_total"]=json!(0); }
        if kind=="undo" { data["id"]=json!("restore"); data["kind"]=json!("undo"); data["undoes"]=json!("last"); data["paths"].as_array_mut().unwrap().truncate(1); data["paths_total"]=json!(1); }
        if kind=="long" { data["paths"][0]["path"]=json!("src/components/review/very_long_module_name_describing_recorded_checkout_changes.rs"); }
        let id=if kind=="undo" {"restore"} else {"last"};
        if kind=="undo" { inject(p,vec![data,record("last", &["last"], "undone")]); } else { inject(p,vec![data]); }
        let target=target(p,id,None);
        let mut saved=Record::prepare(&p.workspace.turn_records.data,&target).unwrap();
        saved.describe(&p.workspace.turn_records.data,p.host.thread.snapshot.as_deref());
        p.workspace.turn_review=TurnReview { selected:saved.selected(&target), target:Some(target), record:Some(Arc::new(saved)), diff:(!matches!(kind,"empty"|"loading"|"unavailable"|"old")).then(||lines.unwrap_or_else(||Arc::new(fixture_diff(PATCH.as_bytes(),false,Duration::from_millis(18))))), epoch:Some(p.host.connection.epoch), ..Default::default() };
        if kind=="loading" {p.workspace.turn_review.loading=true;}
        if kind=="unavailable" {p.workspace.turn_review.error=Some(("Unavailable","The saved content is unavailable.".into()));}
        if kind=="old" {p.workspace.turn_review.older=true;p.workspace.turn_review.error=Some(("Older host","Turn review needs a newer host. Current Git changes are still available.".into()));}
        if kind=="clean" { p.state.repository.complete(plait_client::state::Job::Status {epoch:p.state.repository.epoch(),path:repository.into()}, plait_client::state::Reply::Status(Ok(Arc::new(plait_client::model::Snapshot {root:repository.into(),branch:"main".into(),named:true,tracking:None,changes:vec![],elapsed:Duration::ZERO,plain:false})))); p.state.repository.view.changes.selected=None; }
        p.workspace.diff_scroll=UniformListScrollHandle::new(); p.workspace.file_scroll=UniformListScrollHandle::new();p.workspace.tab=Workspace::Changes;
    });
}
fn changed_patch_scene(h: &mut Harness, repository: &Path, suffix: &str) {
    scene(h, repository, "ready", None);
    h.update(|p, _, cx| {
        let panel = p.tabs.model.panel_mut().unwrap();
        panel.open_on(plait_client::panel::PanelKind::Changes);
        panel.select_scope(plait_client::panel::Scope::ThisTurn);
        p.workspace.tab = Workspace::Conversation;
        let mut bookmark = p.capture_tab_bookmark(None, cx);
        bookmark.changes.anchor.as_mut().unwrap().fingerprint = "previous patch".into();
        bookmark.changes.selection = Some((1, 2));
        p.workspace.restore = Some(bookmark);
        p.workspace.restore_pending[1] = true;
        p.restore_ready_bookmarks(cx);
    });
    capture_both(h, &format!("tabs-patch-changed-{suffix}"));
    #[cfg(debug_assertions)]
    assert!(
        h.draw_a11y()
            .nodes
            .iter()
            .any(|n| n.path.contains("patch-restore-notice"))
    );
}

#[core::prelude::v1::test]
fn changed_patch_restore_notice() {
    if !isolated("turn_review::changed_patch_restore_notice") {
        return;
    }
    let root = fixture_repository(&PathBuf::from(std::env::var_os(SCENE_HOME).unwrap()));
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    let mut h = this_turn::harness(&root, output.as_deref(), 900., 600.);
    changed_patch_scene(&mut h, &root, "narrow");
}

pub(super) fn walk(h: &mut Harness, repository: &Path, suffix: &str) {
    changed_patch_scene(h, repository, suffix);
    for (name, kind) in [
        ("turn-recorded", "ready"),
        ("turn-recorded-clean", "clean"),
        ("turn-recorded-empty", "empty"),
        ("turn-recorded-loading", "loading"),
        ("turn-recorded-unavailable", "unavailable"),
        ("turn-review-old-host", "old"),
        ("turn-recorded-undo", "undo"),
        ("turn-recorded-long-path", "long"),
    ] {
        scene(h, repository, kind, None);
        h.capture(&format!("{name}-{suffix}"));
    }
    scene(
        h,
        repository,
        "ready",
        Some(Arc::new(fixture_diff(
            PATCH.as_bytes(),
            true,
            Duration::ZERO,
        ))),
    );
    h.capture(&format!("turn-recorded-limited-{suffix}"));
    for (name, change, patch) in [
        ("empty-added", "added", b"diff --git a/empty b/empty\nnew file mode 100644\n--- /dev/null\n+++ b/empty\n".as_slice()),
        ("empty-deleted", "deleted", b"diff --git a/empty b/empty\ndeleted file mode 100644\n--- a/empty\n+++ /dev/null\n".as_slice()),
        ("latin1", "modified", b"diff --git a/latin1 b/latin1\n--- a/latin1\n+++ b/latin1\n@@ -1 +1 @@\n-caf\xe9\n+caf\xe8\n".as_slice()),
    ] {
        scene(h, repository, "ready", Some(Arc::new(fixture_diff(patch, false, Duration::ZERO))));
        h.update(|p, _, _| {
            let mut data = record("last", &["last"], "ready");
            data["paths"] = json!([{ "path": if name == "latin1" { "latin1" } else { "empty" }, "change": change, "added": if name == "latin1" { 1 } else { 0 }, "removed": if name == "latin1" { 1 } else { 0 } }]);
            data["paths_total"] = json!(1);
            inject(p, vec![data]);
            let mut saved = Record::prepare(&p.workspace.turn_records.data, p.workspace.turn_review.target.as_ref().unwrap()).unwrap();
            saved.describe(&p.workspace.turn_records.data, p.host.thread.snapshot.as_deref());
            p.workspace.turn_review.record = Some(Arc::new(saved));
        });
        h.capture(&format!("turn-recorded-{name}-{suffix}"));
    }
    scene(h, repository, "ready", None);
    h.update(|p, _, _| {
        let mut data = record("last", &["unloaded-request"], "ready");
        data["created_at"] = json!(now_ms() - 300_000);
        inject(p, vec![data]);
        let mut saved = Record::prepare(
            &p.workspace.turn_records.data,
            p.workspace.turn_review.target.as_ref().unwrap(),
        )
        .unwrap();
        saved.describe(
            &p.workspace.turn_records.data,
            p.host.thread.snapshot.as_deref(),
        );
        p.workspace.turn_review.record = Some(Arc::new(saved));
    });
    h.capture(&format!("turn-recorded-request-unloaded-{suffix}"));
    scene(h, repository, "ready", None);
    h.update(|p, w, cx| p.open_review_scopes(w, cx));
    h.capture(&format!("turn-recorded-original-picker-{suffix}"));
    scene(h, repository, "undo", None);
    h.update(|p, w, cx| {
        let mut restored = record("restore", &["undo-one"], "ready");
        restored["kind"] = json!("undo");
        restored["undoes"] = json!("last");
        let mut another = restored.clone();
        another["id"] = json!("another-undo");
        let now = now_ms();
        another["created_at"] = json!(now - 60_000);
        restored["created_at"] = json!(now - 120_000);
        let mut original = record("last", &["last"], "undone");
        original["created_at"] = json!(now - 300_000);
        inject(p, vec![another, restored, original]);
        p.open_review_scopes(w, cx);
    });
    h.capture(&format!("turn-recorded-undo-picker-{suffix}"));
    for empty in [false, true] {
        scene(h, repository, "old", None);
        h.update(|p, _, _| {
            p.workspace.tab = Workspace::Conversation;
            if empty {
                let mut data = record("last", &["last"], "ready");
                data["paths"] = json!([]);
                data["paths_total"] = json!(0);
                inject(p, vec![data]);
            }
        });
        h.capture(&format!(
            "turn-old-host-{}-{suffix}",
            if empty { "empty" } else { "links" }
        ));
    }
    h.update(|p, _, cx| {
        p.clear_turn_review();
        p.state.repository.refresh();
        p.pump(cx);
    });
    scene(h, repository, "ready", None);
    h.update(|p, _, cx| {
        let mut changes = p
            .state
            .repository
            .snapshot()
            .ready()
            .unwrap_or_else(|| panic!("recorded scene status: {:?}", p.state.repository.snapshot()))
            .changes
            .clone();
        for change in &mut changes {
            if change.path == Path::new("src/review.rs") {
                change.worktree = b' ';
            }
        }
        set_changes(p, changes);
        p.workspace.turn_review.current = true;
        p.sync_recorded_current(cx);
    });
    h.capture(&format!("turn-current-staged-{suffix}"));
    scene(h, repository, "ready", None);
    let snapshot = h
        .window
        .update(&mut h.cx, |p, _, _| {
            p.workspace.turn_review.current = true;
            p.state.repository.view.changes.selected = None;
            std::mem::replace(&mut p.state.repository.data.snapshot, Load::Loading)
        })
        .unwrap();
    h.capture(&format!("turn-current-loading-{suffix}"));
    h.window
        .update(&mut h.cx, |p, _, _| {
            p.state.repository.data.snapshot = snapshot
        })
        .unwrap();
    for kind in ["loading", "unavailable", "old"] {
        scene(h, repository, kind, None);
        h.update(|p, _, _| {
            p.workspace.turn_review.record = None;
            p.workspace.turn_review.selected = None;
        });
        h.capture(&format!("turn-record-{kind}-{suffix}"));
    }
    scene(h, repository, "ready", None);
    h.update(|p, _, cx| {
        p.themes.forced = Some(true);
        p.apply_theme(cx);
    });
    h.capture(&format!("turn-recorded-light-{suffix}"));
    h.update(|p, _, cx| {
        p.themes.forced = Some(false);
        p.apply_theme(cx);
        p.clear_turn_review();
        p.state.repository.refresh();
        p.pump(cx);
    });
}
fn info() -> Value {
    json!({"schema":1,"checkout":{"host_id":"host","id":"checkout","catalog_revision":1,"review_revision":0},"features":{"record_diff":true,"attribution":false,"stage_turn":false,"since_review":false},"retention_days":30})
}
fn encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for p in bytes.chunks(3) {
        let a = p[0];
        let b = *p.get(1).unwrap_or(&0);
        let c = *p.get(2).unwrap_or(&0);
        out.push(ALPHABET[(a >> 2) as usize] as char);
        out.push(ALPHABET[((a & 3) << 4 | b >> 4) as usize] as char);
        out.push(if p.len() > 1 {
            ALPHABET[((b & 15) << 2 | c >> 6) as usize] as char
        } else {
            '='
        });
        out.push(if p.len() > 2 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
fn diff(request: &Value) -> Value {
    json!({"schema":1,"checkout":info()["checkout"],"view_id":"view","path":request["path"],"side":null,"basis":"record","source_record_id":request["record_id"],"before":{"mode":"100644","fingerprint":"before","bytes":50},"after":{"mode":"100644","fingerprint":"after","bytes":60},"status":"ready","reason":null,"patch_bytes":PATCH.len(),"patch":{"token":"patch","offset":0,"total":1,"items":[{"byte_offset":0,"bytes_base64":encode(PATCH.as_bytes())}],"next_offset":null},"hunks":{"token":"hunks","offset":0,"total":0,"items":[],"next_offset":null},"review":{"state":"unreviewed","revision":0,"reviewed_at":null,"expires_at":null,"source_record_id":null,"can_mark":false},"truncated":false,"elapsed_ms":3})
}
fn answer(request: &Value) -> Option<Value> {
    match request["op"].as_str().unwrap() {
        "turn_review_info" => Some(info()),
        "turn_diff" => Some(diff(request)),
        "turn_records" => {
            Some(json!({"records":[record("last", &["last"], "ready")],"before":null}))
        }
        other => panic!("unexpected {other}"),
    }
}
#[core::prelude::v1::test]
fn hidden_recorded_patch_survives_info_refresh() {
    if !isolated("turn_review::hidden_recorded_patch_survives_info_refresh") {
        return;
    }
    let (scratch, _, mut h) = setup("panel-recorded-cache");
    let host = Host::start(answer);
    h.update(|p, w, cx| p.open_turn_review(target(p, "last", Some("README.md")), w, cx));
    wait(&mut h, |p| p.workspace.turn_review.diff.is_some());
    h.update(|p, w, cx| {
        p.show_tab(Workspace::Files, w, cx);
        p.refresh_turn_review(cx);
        assert!(p.workspace.turn_review.running);
    });
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.show_tab(Workspace::Changes, w, cx);
        assert_eq!(p.recorded_file().unwrap().path, Path::new("README.md"));
        assert!(
            p.workspace.turn_review.diff.is_some(),
            "lost the cached recorded patch"
        );
    });
    drop(host);
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn recorded_identity_survives_undo() {
    if !isolated("turn_review::recorded_identity_survives_undo") {
        return;
    }
    let (scratch, repository, mut h) = setup("review-binding");
    let host = Host::start(answer);
    h.update(|p, w, cx| p.open_turn_review(target(p, "last", Some("README.md")), w, cx));
    wait(&mut h, |p| p.workspace.turn_review.diff.is_some());
    h.update(|p, _, cx| {
        let mut leaf = record("restore", &["undo"], "ready");
        leaf["kind"] = json!("undo");
        leaf["undoes"] = json!("last");
        inject(
            p,
            vec![
                leaf,
                record("later", &["later"], "ready"),
                record("last", &["last"], "undone"),
            ],
        );
        p.refresh_turn_review(cx);
    });
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, _, _| {
        assert_eq!(
            p.workspace.turn_review.target.as_ref().unwrap().record,
            "last"
        );
        assert_eq!(p.recorded_file().unwrap().path, PathBuf::from("README.md"));
        assert_eq!(
            p.workspace.turn_review.record.as_ref().unwrap().files.len(),
            4
        );
    });
    assert!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["op"] == "turn_diff")
            .all(|r| r["record_id"] == "last" && r["path"] == "README.md")
    );
    drop(host);
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
    let _ = repository;
}
#[core::prelude::v1::test]
fn old_host_and_incomplete_pages() {
    if !isolated("turn_review::old_host_and_incomplete_pages") {
        return;
    }
    let (scratch, _, mut h) = setup("review-errors");
    let host = Host::start(|_| {
        Some(
            json!({"ok":false,"error":{"code":"invalid_request","message":"Unknown operation or field."}}),
        )
    });
    h.update(|p, w, cx| p.open_turn_review(target(p, "last", None), w, cx));
    wait(&mut h, |p| p.workspace.turn_review.older);
    h.update(|p, _, _| {
        assert!(p.workspace.turn_review.diff.is_none());
        assert_eq!(
            p.workspace.turn_review.error.as_ref().unwrap().0,
            "Older host"
        );
    });
    drop(host);
    let host = Host::start(|request| {
        if request["op"] == "turn_diff" {
            let mut value = diff(request);
            value["patch"]["total"] = json!(2);
            Some(value)
        } else {
            answer(request)
        }
    });
    h.update(|p, w, cx| {
        p.workspace.turn_review.older = false;
        p.open_turn_review(target(p, "last", None), w, cx);
    });
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, _, _| {
        assert!(p.workspace.turn_review.diff.is_none());
        assert_eq!(
            p.workspace.turn_review.error.as_ref().unwrap().0,
            "Unavailable"
        );
    });
    drop(host);
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
#[core::prelude::v1::test]
fn stale_review_replies() {
    if !isolated("turn_review::stale_review_replies") {
        return;
    }
    let (scratch, repository, mut h) = setup("review-stale");
    for phase in ["turn_review_info", "turn_diff", "turn_review_page"] {
        for change in 0..8 {
            prepare(&mut h, &repository);
            let host = Host::start(move |request| match request["op"].as_str().unwrap() {
                "turn_diff" if phase == "turn_review_page" => {
                    let mut value = diff(request);
                    value["patch"]["total"] = json!(2);
                    value["patch"]["items"][0]["bytes_base64"] =
                        json!(encode(&PATCH.as_bytes()[..50]));
                    value["patch"]["next_offset"] = json!(1);
                    Some(value)
                }
                "turn_review_page" => Some(
                    json!({"schema":1,"token":"patch","offset":1,"total":2,"items":[{"byte_offset":50,"bytes_base64":encode(&PATCH.as_bytes()[50..])}],"next_offset":null}),
                ),
                _ => answer(request),
            });
            h.update(|p, w, cx| {
                p.open_turn_review(target(p, "last", None), w, cx);
                // Each identity member is changed alone. Going offline prevents
                // a replacement read, so only the old response can install data.
                p.workspace.tab = Workspace::Conversation;
                p.host.connection.connected = false;
                match change {
                    0 => p.host.thread.selected = Some("other".into()),
                    1 => p.host.creation.checkout.push("other"),
                    2 => p.host.connection.epoch += 1,
                    3 => p.host.thread.watch_epoch += 1,
                    4 => {
                        p.state.repository.refresh();
                        p.pump(cx);
                    }
                    5 => p.workspace.turn_review.generation += 1,
                    6 => p.workspace.turn_review.current = true,
                    _ => {
                        p.workspace.turn_review.target.as_mut().unwrap().path =
                            Some("other.rs".into())
                    }
                }
            });
            wait(&mut h, |p| !p.workspace.turn_review.running);
            h.update(|p, _, _| {
                assert!(
                    p.workspace.turn_review.diff.is_none(),
                    "{phase}, identity {change}"
                );
                assert!(
                    p.workspace.turn_review.record.is_none(),
                    "{phase}, identity {change}"
                );
                p.clear_turn_review();
            });
            assert!(
                host.requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|r| r["op"] == phase)
            );
            drop(host);
        }
    }
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn scope_keys_and_clean_files() {
    if !isolated("turn_review::scope_keys_and_clean_files") {
        return;
    }
    let (scratch, repository, mut h) = setup("review-keys");
    let host = Host::start(answer);
    h.update(|p, _, _| {
        let mut undo = record("restore", &["undo"], "ready");
        undo["kind"] = json!("undo");
        undo["undoes"] = json!("last");
        undo["paths"].as_array_mut().unwrap().truncate(1);
        undo["paths_total"] = json!(1);
        let mut original = record("last", &["last"], "undone");
        original["paths"][1]["change"] = json!("deleted");
        inject(p, vec![undo, original]);
        let line = p.workspace.turn_records.lines.latest.as_ref().unwrap();
        assert_eq!(line.run, "last");
        assert_eq!(line.displayed, "restore");
        assert_eq!(line.files.len(), 1);
    });
    h.update(|p, w, cx| p.open_turn_review(target(p, "restore", None), w, cx));
    wait(&mut h, |p| p.workspace.turn_review.diff.is_some());
    h.update(|p, w, cx| {
        assert_eq!(
            p.workspace.turn_review.record.as_ref().unwrap().files.len(),
            1
        );
        assert!(p.panel_focus[plait_client::panes::PaneKind::Changes as usize].is_focused(w));
        p.open_review_scopes(w, cx);
    });
    wait(&mut h, |p| p.overlay.is_some());
    assert_eq!(shown(&mut h), ["All changes", "Restored", "Original"]);
    press(&mut h, "escape");
    h.update(|p, w, _| {
        assert!(p.overlay.is_none());
        assert!(p.panel_focus[plait_client::panes::PaneKind::Changes as usize].is_focused(w));
    });
    h.update(|p, w, cx| p.open_review_scopes(w, cx));
    wait(&mut h, |p| p.overlay.is_some());
    set_query(&mut h, "Original");
    press(&mut h, "enter");
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, w, cx| {
        assert_eq!(
            p.workspace.turn_review.target.as_ref().unwrap().record,
            "last"
        );
        assert_eq!(
            p.workspace.turn_review.record.as_ref().unwrap().files.len(),
            4
        );
        p.select_recorded_file(1, w, cx);
    });
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, _, cx| {
        assert_eq!(p.recorded_file().unwrap().path, PathBuf::from("README.md"));
        assert_eq!(p.recorded_file().unwrap().label, "Deleted");
        p.state.repository.data.snapshot = Load::Ready(Arc::new(plait_client::model::Snapshot {
            root: repository.clone(),
            branch: "main".into(),
            named: true,
            tracking: None,
            changes: vec![],
            elapsed: Duration::ZERO,
            plain: false,
        }));
        p.state.repository.view.changes.selected = None;
        p.review_comparison(true, cx);
        assert!(!p.recorded_current_available());
    });
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, _, cx| p.review_comparison(false, cx));
    wait(&mut h, |p| p.workspace.turn_review.diff.is_some());
    h.update(|p, _, _| assert_eq!(p.recorded_file().unwrap().path, PathBuf::from("README.md")));
    drop(host);
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn empty_and_more_links() {
    if !isolated("turn_review::empty_and_more_links") {
        return;
    }
    let (scratch, _, mut h) = setup("review-links");
    let host = Host::start(answer);
    h.update(|p, _, _| {
        let line = p.workspace.turn_records.lines.latest.as_ref().unwrap();
        assert_eq!(line.more_label, "1 more");
        assert_eq!(line.more_detail, "Review all 4 files in this record.");
        assert_eq!(line.displayed, "last");
    });
    // The no-path target used by the step count and N more opens the first file.
    h.update(|p, w, cx| p.open_turn_review(target(p, "last", None), w, cx));
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, _, _| assert_eq!(p.workspace.turn_review.selected, Some(0)));
    h.update(|p, w, cx| {
        let mut empty = record("last", &["last"], "ready");
        empty["paths"] = json!([]);
        empty["paths_total"] = json!(0);
        inject(p, vec![empty]);
        p.open_turn_review(target(p, "last", None), w, cx);
    });
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, _, _| {
        assert!(
            p.workspace
                .turn_review
                .record
                .as_ref()
                .unwrap()
                .files
                .is_empty()
        );
        assert!(p.workspace.turn_review.selected.is_none());
        assert!(p.workspace.turn_review.diff.is_none());
    });
    assert_eq!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["op"] == "turn_diff")
            .count(),
        1
    );
    drop(host);
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn current_waits_for_status_and_requires_a_file() {
    if !isolated("turn_review::current_waits_for_status_and_requires_a_file") {
        return;
    }
    let (scratch, repository, mut h) = setup("review-loading");
    scene(&mut h, &repository, "ready", None);
    h.update(|p, _, cx| {
        p.host.connection.connected = false;
        p.review_comparison(true, cx);
        assert!(p.workspace.turn_review.current);
        p.state.repository.refresh();
        assert!(!p.recorded_current_available());
        assert_eq!(p.recorded_current_message().0, "Reading diff…");
        p.pump(cx);
    });
    for kind in ["empty", "loading", "unavailable", "old"] {
        scene(&mut h, &repository, kind, None);
        h.update(|p, _, cx| {
            p.workspace.turn_review.record = None;
            p.workspace.turn_review.selected = None;
            p.workspace.turn_review.current = false;
            p.workspace.turn_review.older = false;
            p.review_comparison(true, cx);
            assert!(p.recorded_file().is_none());
            assert!(!p.workspace.turn_review.current, "{kind}");
        });
        h.draw();
    }
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn current_selects_a_present_layer() {
    if !isolated("turn_review::current_selects_a_present_layer") {
        return;
    }
    let (scratch, repository, mut h) = setup("review-layer");
    scene(&mut h, &repository, "ready", None);
    h.update(|p, _, cx| {
        use plait_client::{model::DiffMode, state::DiffKey};
        p.host.connection.connected = false;
        let changes = vec![plait_client::model::Change {
            path: "src/review.rs".into(),
            original: None,
            index: b'M',
            worktree: b' ',
        }];
        set_changes(p, changes);
        p.state.repository.view.changes.selected = Some(DiffKey {
            path: p.state.repository.snapshot().ready().unwrap().changes[0]
                .path
                .clone(),
            file: 0,
            mode: DiffMode::Unstaged,
        });
        p.review_comparison(true, cx);
        assert_eq!(
            p.state.repository.selected_diff().unwrap().mode,
            DiffMode::Staged
        );
        // A present staged selection stays selected when both layers exist.
        let mut changes = p
            .state
            .repository
            .snapshot()
            .ready()
            .unwrap()
            .changes
            .clone();
        changes[0].worktree = b'M';
        set_changes(p, changes);
        p.sync_recorded_current(cx);
        assert_eq!(
            p.state.repository.selected_diff().unwrap().mode,
            DiffMode::Staged
        );
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

fn set_changes(p: &mut Plait, changes: Vec<plait_client::model::Change>) {
    p.state.repository.data.snapshot = Load::Ready(Arc::new(plait_client::model::Snapshot {
        root: p.host.creation.checkout.clone(),
        branch: "main".into(),
        named: true,
        tracking: None,
        changes,
        elapsed: Duration::ZERO,
        plain: false,
    }));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[core::prelude::v1::test]
fn undo_picker_names_the_request_and_keeps_newest_first() {
    if !isolated("turn_review::undo_picker_names_the_request_and_keeps_newest_first") {
        return;
    }
    let (scratch, _, mut h) = setup("review-undo-names");
    let host = Host::start(answer);
    h.update(|p, w, cx| {
        let now = now_ms();
        let mut original = record("last", &["last"], "undone");
        original["created_at"] = json!(now - 300_000);
        let mut older = record("private-older-undo", &["private-request"], "ready");
        older["kind"] = json!("undo");
        older["undoes"] = json!("last");
        older["created_at"] = json!(now - 120_000);
        let mut newer = older.clone();
        newer["id"] = json!("private-newer-undo");
        newer["created_at"] = json!(now - 60_000);
        // Deliberately unordered input: the picker must put newest first.
        inject(p, vec![older, original, newer]);
        for (id, age) in [("last", "5m"), ("private-newer-undo", "1m"), ("private-older-undo", "2m")] {
            let mut saved = Record::prepare(&p.workspace.turn_records.data, &target(p, id, None)).unwrap();
            saved.describe(&p.workspace.turn_records.data, p.host.thread.snapshot.as_deref());
            assert_eq!(saved.label, "Turn · Document the behavior…");
            let prefix = if saved.restored { "Files an undo put back from" } else { "Files recorded during" };
            assert!(saved.detail.starts_with(&format!("{prefix} Document the behavior and regenerate the file list., {age} ago, in this thread.")));
            assert!(!saved.detail.contains("private-"));
            assert_eq!(saved.id, id);
        }
        p.open_review_scopes(w, cx);
    });
    wait(&mut h, |p| p.overlay.is_some());
    assert_eq!(
        shown(&mut h),
        ["All changes", "Restored", "Restored", "Original"]
    );
    press(&mut h, "down");
    press(&mut h, "enter");
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, w, cx| {
        assert_eq!(
            p.workspace.turn_review.target.as_ref().unwrap().record,
            "private-newer-undo"
        );
        assert_eq!(p.review_scope_text().0, "Turn · Document the behavior…");
        p.open_review_scopes(w, cx);
    });
    wait(&mut h, |p| p.overlay.is_some());
    set_query(&mut h, "2m ago");
    assert_eq!(shown(&mut h), ["Restored"]);
    press(&mut h, "enter");
    wait(&mut h, |p| !p.workspace.turn_review.running);
    h.update(|p, _, _| {
        assert_eq!(
            p.workspace.turn_review.target.as_ref().unwrap().record,
            "private-older-undo"
        )
    });
    for id in ["private-newer-undo", "private-older-undo"] {
        assert!(
            host.requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r["op"] == "turn_diff" && r["record_id"] == id)
        );
    }
    drop(host);
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn first_old_host_file_click_opens_current() {
    if !isolated("turn_review::first_old_host_file_click_opens_current") {
        return;
    }
    let (scratch, repository, mut h) = setup("review-old-click");
    let host = Host::start(|_| {
        Some(
            json!({"ok":false,"error":{"code":"invalid_request","message":"Unknown operation or field."}}),
        )
    });
    h.update(|p, w, cx| {
        assert!(!p.workspace.turn_review.older);
        p.open_turn_review(target(p, "last", Some("src/review.rs")), w, cx);
    });
    wait(&mut h, |p| {
        p.workspace.turn_review.older && p.workspace.turn_review.target.is_none()
    });
    h.update(|p, _, _| {
        let selected = p.state.repository.selected_diff().unwrap();
        assert_eq!(
            p.state.repository.snapshot().ready().unwrap().changes[selected.file].path,
            Path::new("src/review.rs")
        );
        let line = p.workspace.turn_records.lines.latest.as_ref().unwrap();
        let detail = p.closing_file_detail(&line.files[0], line);
        assert!(detail.contains("current Changes"));
        assert!(!detail.contains("recorded"));
        assert_eq!(p.selected_view(), Workspace::Changes);
    });
    // A second click uses the known capability and performs no further probe.
    let probes = host.requests.lock().unwrap().len();
    h.update(|p, w, cx| p.open_turn_review(target(p, "last", Some("README.md")), w, cx));
    assert_eq!(host.requests.lock().unwrap().len(), probes);
    // A stale old-host response must never redirect another thread or checkout.
    h.update(|p, w, cx| {
        p.workspace.turn_review.older = false;
        p.open_turn_review(target(p, "last", Some("src/review.rs")), w, cx);
        p.host.creation.checkout = repository.join("different");
        p.host.connection.connected = false;
        p.workspace.tab = Workspace::Conversation;
        p.state.repository.view.changes.selected = None;
    });
    h.update(|p, _, _| {
        assert_eq!(p.workspace.tab, Workspace::Conversation);
        assert!(p.state.repository.selected_diff().is_none());
    });
    drop(host);
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn recorded_wording_names_only_available_states() {
    if !isolated("turn_review::recorded_wording_names_only_available_states") {
        return;
    }
    let (scratch, repository, mut h) = setup("review-wording");
    for kind in ["loading", "unavailable", "old"] {
        scene(&mut h, &repository, kind, None);
        h.update(|p, _, _| {
            p.workspace.turn_review.record = None;
            let (label, detail) = p.review_scope_text();
            assert_eq!(label, "Recorded");
            assert!(!detail.contains("all current staged"));
            if let Some(error) = &p.workspace.turn_review.error {
                assert_eq!(detail, error.1);
            } else {
                assert!(detail.contains("Reading this run"));
            }
        });
    }
    scene(
        &mut h,
        &repository,
        "ready",
        Some(Arc::new(fixture_diff(
            PATCH.as_bytes(),
            true,
            Duration::ZERO,
        ))),
    );
    h.update(|p, _, _| {
        assert!(p.workspace.turn_review.diff.as_ref().unwrap().truncated);
        assert!(
            p.recorded_notice().is_none(),
            "the shared diff notice is sufficient"
        );
        let (label, detail) = crate::ui::changes::diff_limit_notice(true);
        assert_eq!(label, "Limited");
        assert!(!detail.contains("staging") && !detail.contains("reviewed"));
        let mut original = record("last", &["last"], "undone");
        original["paths"][0]["added"] = Value::Null;
        original["paths"][0]["removed"] = Value::Null;
        let mut one = record("undo-one", &["undo-one"], "ready");
        one["kind"] = json!("undo");
        one["undoes"] = json!("last");
        let mut two = one.clone();
        two["id"] = json!("undo-two");
        let now = now_ms();
        one["created_at"] = json!(now - 120_000);
        two["created_at"] = json!(now - 60_000);
        inject(p, vec![two, one, original]);
        let mut original =
            Record::prepare(&p.workspace.turn_records.data, &target(p, "last", None)).unwrap();
        assert_eq!(
            original.files[0].detail,
            "src/review.rs, Modified. Recorded during this run."
        );
        original.describe(
            &p.workspace.turn_records.data,
            p.host.thread.snapshot.as_deref(),
        );
        assert!(original.detail.contains('\n'));
        let detail = crate::ui::turn_review::review_picker_detail(&original);
        assert!(!detail.contains(['\n', '\r']));
        assert!(detail.contains("Not covered: build/"));
        let mut details = Vec::new();
        for id in ["undo-one", "undo-two"] {
            let mut saved =
                Record::prepare(&p.workspace.turn_records.data, &target(p, id, None)).unwrap();
            saved.describe(
                &p.workspace.turn_records.data,
                p.host.thread.snapshot.as_deref(),
            );
            assert!(saved.detail.contains("Document the behavior"));
            details.push(saved.detail);
        }
        assert_ne!(details[0], details[1]);
    });
    h.draw();
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
