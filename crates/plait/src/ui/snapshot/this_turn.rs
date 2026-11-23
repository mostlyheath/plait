//! Recorded-turn scenes use the same host fixtures as the closing line tests.
use super::*;

fn harness(repo: &Path, output: Option<&Path>, width: f32, height: f32) -> Harness {
    let mut h = Harness::with_host(repo, output, width, height, None);
    let snapshot = plait_client::model::Snapshot {
        root: repo.to_owned(),
        branch: "main".into(),
        named: true,
        tracking: None,
        changes: vec![],
        elapsed: Duration::ZERO,
        plain: false,
    };
    h.update(move |p, _, _| {
        p.state.repository.refresh();
        let job = p.state.repository.next_job().expect("fixture status job");
        p.state.repository.complete(
            job,
            plait_client::state::Reply::Status(Ok(Arc::new(snapshot))),
        );
    });
    h
}

pub(super) fn scenes(h: &mut Harness, repo: &Path, suffix: &str) {
    undo::prepare(h, repo);
    for (name, status) in [
        ("finished", ThreadStatus::Idle),
        ("working", ThreadStatus::Running),
        ("failed", ThreadStatus::Error),
        ("undone", ThreadStatus::Idle),
        ("earlier", ThreadStatus::Idle),
        ("no-record", ThreadStatus::Idle),
    ] {
        h.change_snapshot(|s| s.status = status);
        h.update(|p, _, _| {
            p.preferences.value.hide_inspector = false;
            p.workspace.turn_records.selection.latest();
            let state = match name {
                "undone" => "undone",
                "working" => "recording",
                _ => "ready",
            };
            let mut data = vec![
                undo::record("last", &["last"], state),
                undo::record("first", &["first"], "ready"),
            ];
            match name {
                "undone" => {
                    let mut child = undo::record("restored", &[], "ready");
                    child["kind"] = "undo".into();
                    child["undoes"] = "last".into();
                    data.insert(0, child);
                }
                "earlier" => p.workspace.turn_records.selection.earlier("first"),
                "no-record" => data.clear(),
                _ => {}
            }
            let now = crate::ui::commits::now() as u64 * 1000;
            for record in &mut data {
                record["created_at"] = (now
                    - if record["id"] == "first" {
                        600_000
                    } else {
                        120_000
                    })
                .into();
            }
            undo::inject(p, data);
        });
        h.capture(&format!("this-turn-{name}-{suffix}"));
    }
}

#[core::prelude::v1::test]
fn this_turn_refuses_with_closing_line() {
    if !isolated("this_turn::this_turn_refuses_with_closing_line") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-turn-gates-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = harness(&repo, None, 1380., 860.);
    undo::prepare(&mut h, &repo);
    h.update(|p, _, _| {
        for (connected, current, busy) in [
            (true, true, false),
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            p.host.connection.connected = connected;
            p.workspace.turn_records.loaded = current;
            p.host.thread.controlling = busy;
            let records = &p.workspace.turn_records;
            let panel = plait_client::history::this_turn::panel(
                &records.lines,
                &records.selection,
                ThreadStatus::Idle,
                p.undo_context(),
            )
            .unwrap();
            let action = panel.action.unwrap();
            let refusal = p.undo_refusal(records.lines.latest.as_ref().unwrap());
            assert_eq!(
                (!action.enabled).then_some((action.label, action.detail)),
                refusal
            );
            assert_eq!(action.enabled, connected && current && !busy);
        }
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn this_turn_scenes() {
    if !isolated("this_turn::this_turn_scenes") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-turn-scenes-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    if let Some(path) = &output {
        std::fs::create_dir_all(path).unwrap();
    }
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut h = harness(&repo, output.as_deref(), width, height);
        scenes(&mut h, &repo, suffix);
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
