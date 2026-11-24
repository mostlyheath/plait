//! Recorded-turn scenes use the same host fixtures as the closing line tests.
use super::*;

pub(super) fn harness(repo: &Path, output: Option<&Path>, width: f32, height: f32) -> Harness {
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

#[core::prelude::v1::test]
fn the_panel_model_refuses_where_the_closing_line_does() {
    if !isolated("this_turn::the_panel_model_refuses_where_the_closing_line_does") {
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
