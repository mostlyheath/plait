use super::*;
use plait_client::{
    automations::AutomationItem,
    host::automations::{LastResult, OccurrenceOutcome, Schedule, State, Summary},
};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn sample_automations(repository: &Path) -> Vec<AutomationItem> {
    let now = now_ms();
    let mut active = AutomationItem::from_summary(Summary {
        id: "auto-active".into(),
        name: "Daily dependency check".into(),
        schedule: Schedule::Weekdays {
            time: "09:00".into(),
            time_zone: "America/New_York".into(),
        },
        target_path: repository.to_path_buf(),
        state: State::Active,
        next_run: Some(now + 3 * 3_600_000),
        last_result: Some(LastResult {
            due: now - 15 * 60_000,
            outcome: OccurrenceOutcome::Queued,
            detail: None,
        }),
        pause_reason: None,
        error: None,
        thread_id: Some(THREAD.into()),
    });
    active.thread_id = Some(THREAD.into());

    let mut paused = AutomationItem::from_summary(Summary {
        id: "auto-paused".into(),
        name: "Nightly test sweep".into(),
        schedule: Schedule::Daily {
            time: "22:00".into(),
            time_zone: "UTC".into(),
        },
        target_path: repository.to_path_buf(),
        state: State::Paused,
        next_run: None,
        last_result: Some(LastResult {
            due: now - 3 * 3_600_000,
            outcome: OccurrenceOutcome::Paused,
            detail: Some("target missing".into()),
        }),
        pause_reason: None,
        error: None,
        thread_id: Some("thr_0c44a9d1".into()),
    });
    paused.thread_id = Some("thr_0c44a9d1".into());
    paused.pause_reason = Some("target missing".into());

    let preparing = AutomationItem::from_summary(Summary {
        id: "auto-preparing".into(),
        name: "Weekly changelog".into(),
        schedule: Schedule::Weekly {
            days: vec![1],
            time: "10:00".into(),
            time_zone: "UTC".into(),
        },
        target_path: repository.to_path_buf(),
        state: State::Preparing,
        next_run: None,
        last_result: None,
        pause_reason: None,
        error: None,
        thread_id: None,
    });

    let mut failed = AutomationItem::from_summary(Summary {
        id: "auto-failed".into(),
        name: "Docs link check".into(),
        schedule: Schedule::Hourly {
            minute: 15,
            time_zone: "UTC".into(),
        },
        target_path: repository.join("docs"),
        state: State::Failed,
        next_run: None,
        last_result: None,
        pause_reason: None,
        error: None,
        thread_id: None,
    });
    failed.error = Some("This folder is inside a Git project. Choose the project instead.".into());

    vec![active, paused, preparing, failed]
}

pub(super) fn scenes(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut h = Harness::with_host(repository, output, width, height, None);
    h.connect(repository, false);
    h.update(|p, _, cx| {
        p.sidebar = true;
        p.automations
            .set_for_test(true, sample_automations(repository));
        p.rebuild_projects(cx);
        p.navigation.invalidate_rows();
    });
    capture_both(&mut h, &format!("automations-{suffix}"));
}

#[core::prelude::v1::test]
fn automations_sidebar_and_thread_header_draw_cleanly() {
    if !isolated("automations::automations_sidebar_and_thread_header_draw_cleanly") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-automations-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::with_host(&repository, None, 1380., 860., None);
    h.connect(&repository, false);
    h.update(|p, _, cx| {
        p.sidebar = true;
        p.rebuild_projects(cx);
    });
    h.draw();

    // Older hosts hide the Automations section completely.
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-hidden");
        assert!(!frame.has_node(Role::TreeItem, |node| {
            node.label
                .as_deref()
                .is_some_and(|label| label.starts_with("Automations section"))
        }));
        assert!(!frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Run now")
        }));
    }

    // Supported hosts show the section, all four states, and the selected thread's header.
    let items = sample_automations(&repository);
    h.update(|p, _, _| {
        p.automations.set_for_test(true, items);
        p.navigation.invalidate_rows();
    });
    h.draw();

    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-visible");
        assert!(frame.has_node(Role::TreeItem, |node| {
            node.label
                .as_deref()
                .is_some_and(|label| label.starts_with("Automations section"))
        }));
        for expected in [
            "Daily dependency check · Active",
            "Nightly test sweep · Paused. Daily at 22:00. target missing",
            "Weekly changelog · Preparing",
            "Docs link check · Failed",
        ] {
            assert!(
                frame.has_node(Role::TreeItem, |node| {
                    node.label
                        .as_deref()
                        .is_some_and(|label| label.contains(expected))
                }),
                "missing automation card: {expected}"
            );
        }
        for label in ["Run now", "Pause", "Delete"] {
            assert!(
                frame.has_node(Role::Button, |node| node.label.as_deref() == Some(label)),
                "missing automation header control: {label}"
            );
        }
    }

    // Delete asks for confirmation; Escape cancels without deleting.
    h.update(|p, _, _| {
        p.automations.set_confirm_delete(Some("auto-active".into()));
    });
    h.draw();
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-delete-confirm");
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Delete automation")
        }));
        assert!(frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Cancel")
        }));
    }

    h.keystroke("escape");
    h.update(|p, _, _| {
        assert!(p.automations.confirm_delete.is_none());
        assert_eq!(p.automations.items.len(), 4);
    });

    // Deleting an automation leaves its thread selected as an ordinary thread.
    h.update(|p, _, _| {
        let epoch = p.automations.epoch;
        assert!(p.automations.apply_deleted(epoch, "auto-active"));
        p.navigation.invalidate_rows();
    });
    h.draw();
    h.update(|p, _, _| {
        assert_eq!(p.host.thread.selected.as_deref(), Some(THREAD));
        assert!(p.automations.by_thread(THREAD).is_none());
    });
    #[cfg(debug_assertions)]
    {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("automations-after-delete");
        assert!(!frame.has_node(Role::Button, |node| {
            node.label.as_deref() == Some("Run now")
        }));
    }

    drop(h);
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        scenes(&repository, output.as_deref(), suffix, width, height);
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
