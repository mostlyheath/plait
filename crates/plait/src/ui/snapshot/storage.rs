//! Storage fixtures never read or approve a real project's output.
use super::*;
use plait_client::host::build_output::{Info, Kind, Phase, Progress};
use plait_protocol::build_output::Outcome;
fn choice(h: &mut Harness) {
    h.update(|p, w, cx| {
        p.show_project_settings(w, cx);
        p.prepare_build_settings(cx);
        p.build_output.info = Some(Info {
            schema: 1,
            worktree_build_output_v1: true,
            policy: serde_json::from_value(serde_json::json!({
                "schema": 1, "project": p.host.creation.checkout,
                "identity": {"dev": "1", "ino": "1"},
                "revision": 1, "paths": ["target"]
            }))
            .unwrap(),
            suggestions: vec!["runtime/node_modules".into()],
            seed: Some(
                serde_json::from_value(serde_json::json!({
                    "bytes": 2_576_980_378u64, "files": 130_000, "verified_at": 1
                }))
                .unwrap(),
            ),
        });
        p.build_output.note = None;
    });
    h.settle();
    h.update(|p, _, _| p.settings_scroll.set_offset(point(px(0.), px(-10000.))));
}
fn cold(h: &mut Harness) {
    super::worktrees_view::install(h, 4);
    h.update(|p, _, _| {
        p.worktrees.selected = 3;
        p.worktrees.seed_bytes = Some(2_576_980_378);
        p.worktrees.rows[3].build_output = Some(Outcome::cold(
            "The source checkout is busy or unavailable and there is no idle build seed.",
        ));
        p.worktrees.sizes[3]
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .build = 0;
        p.worktrees.prepare();
    });
}
fn seeding(h: &mut Harness) {
    h.update(|p, _, _| {
        p.build_output.progress.insert(
            "scene-creation".into(),
            (
                Kind::Worktree,
                Progress {
                    schema: 1,
                    worktree_build_output_v1: true,
                    request_id: "scene-creation".into(),
                    thread_id: None,
                    phase: Phase::Cloning,
                    files: 3255,
                    outcome: None,
                },
            ),
        );
        p.build_output.prepare_progress();
    });
}
pub(super) fn walk(repo: &Path, out: Option<&Path>, suffix: &str, width: f32, height: f32) {
    for (name, install) in [
        ("storage-cache-choice", choice as fn(&mut Harness)),
        ("storage-cold-fallback", cold),
        ("storage-seeding", seeding),
    ] {
        let mut h = Harness::new(repo, out, width, height);
        h.connect(repo, false);
        install(&mut h);
        h.capture(&format!("{name}-{suffix}"));
    }
}
#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn storage_controls_remain_accessible_wide_and_narrow() {
    if !isolated("storage::storage_controls_remain_accessible_wide_and_narrow") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-storage-scenes-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    for (width, height) in [(1380., 860.), (900., 600.)] {
        for (name, install, required) in [
            (
                "storage-cache-choice",
                choice as fn(&mut Harness),
                &["Approve", "Seed · 2.4 GiB"][..],
            ),
            ("storage-cold-fallback", cold, &["Cold"]),
            ("storage-seeding", seeding, &["Skip"]),
        ] {
            let mut h = Harness::new(&repo, None, width, height);
            h.connect(&repo, false);
            install(&mut h);
            for _ in 0..2 {
                let frame = h.draw_a11y();
                frame.assert_no_duplicates(name);
                for required in required {
                    assert!(
                        frame
                            .nodes
                            .iter()
                            .any(|n| n.label.as_deref() == Some(*required)),
                        "{name} missing {required} at {width}"
                    );
                }
            }
        }
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
