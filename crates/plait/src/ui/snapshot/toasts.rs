//! Thread alert toasts, injected directly: no host activity posts them here.
use super::*;

/// Toasts for the fixture's first two threads; returns their ids.
fn two(h: &mut Harness) -> (String, String) {
    let mut ids = Vec::new();
    h.update(|p, _, cx| {
        ids = p
            .host
            .connection
            .threads
            .iter()
            .take(2)
            .map(|thread| thread.id.clone())
            .collect();
        assert_eq!(ids.len(), 2, "the fixture lists two threads");
        let now = std::time::Instant::now();
        p.toasts.push(
            &ids[0],
            "Add retries to the client".into(),
            "forecast · Done".into(),
            now,
        );
        p.toasts.push(
            &ids[1],
            "Run the integration tests".into(),
            "forecast · Needs approval".into(),
            now,
        );
        cx.notify();
    });
    (ids[0].clone(), ids[1].clone())
}

pub(super) fn walk(repo: &Path, out: Option<&Path>, suffix: &str, width: f32, height: f32) {
    let mut h = Harness::new(repo, out, width, height);
    h.connect(repo, false);
    two(&mut h);
    h.capture(&format!("toasts-{suffix}"));
}

/// Only the toast scenes, for checking their placement quickly.
#[core::prelude::v1::test]
#[ignore = "writes PNG files; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn toast_snapshots() {
    if !isolated("toasts::toast_snapshots") {
        return;
    }
    let output = PathBuf::from(std::env::var_os("PLAIT_SNAPSHOT_DIR").expect("PLAIT_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&output).unwrap();
    let scratch = std::env::temp_dir().join(format!("plait-toast-shots-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    walk(&repo, Some(&output), "wide", 1380., 860.);
    walk(&repo, Some(&output), "narrow", 900., 600.);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn toasts_are_labelled_and_dismissable_wide_and_narrow() {
    if !isolated("toasts::toasts_are_labelled_and_dismissable_wide_and_narrow") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-toast-scenes-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    for (width, height) in [(1380., 860.), (900., 600.)] {
        let mut h = Harness::new(&repo, None, width, height);
        h.connect(&repo, false);
        let (first, second) = two(&mut h);
        let frame = h.draw_a11y();
        frame.assert_no_duplicates("toasts");
        for required in [
            "Add retries to the client: forecast · Done",
            "Open Add retries to the client: forecast · Done",
            "Dismiss alert for Run the integration tests",
        ] {
            assert!(
                frame
                    .nodes
                    .iter()
                    .any(|n| n.label.as_deref() == Some(required)),
                "toasts missing {required} at {width}"
            );
        }
        h.update(|p, _, cx| p.dismiss_toast(&second, cx));
        h.update(|p, _, _| {
            let left: Vec<_> = p.toasts.items().iter().map(|t| t.thread.clone()).collect();
            assert_eq!(left, [first.clone()]);
        });
        // Reading the thread clears its toast, as it clears its unread marker.
        h.update(|p, _, cx| p.mark_thread_read(&first, cx));
        h.update(|p, _, _| assert!(p.toasts.items().is_empty()));
        // A toast for a thread the host no longer lists is dropped, not drawn.
        h.update(|p, _, cx| {
            p.toasts.push(
                "gone",
                "Gone".into(),
                "x · Done".into(),
                std::time::Instant::now(),
            );
            p.retain_toasts(cx);
            assert!(p.toasts.items().is_empty());
            // A reconnect clears every alert.
            p.toasts.push(
                &first,
                "A".into(),
                "x · Done".into(),
                std::time::Instant::now(),
            );
            p.clear_toasts();
            assert!(p.toasts.items().is_empty());
        });
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
