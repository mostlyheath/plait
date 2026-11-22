//! Pickers open beside the control that opened them.
use super::*;

#[core::prelude::v1::test]
#[ignore = "writes PNG files; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn picker_snapshots() {
    if !isolated("pickers::picker_snapshots") {
        return;
    }
    let output = PathBuf::from(std::env::var_os("PLAIT_SNAPSHOT_DIR").expect("PLAIT_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&output).unwrap();
    let scratch = std::env::temp_dir().join(format!("plait-picker-shots-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    for (width, height, suffix) in [(1380., 860., "wide"), (900., 600., "narrow")] {
        let mut h = Harness::new(&repo, Some(&output), width, height);
        h.connect(&repo, false);
        h.update(|p, window, cx| p.open_draft(window, cx));
        h.draw();
        h.update(|p, window, cx| {
            p.anchor_next("draft-model");
            p.open_model_picker(
                super::super::model_picker::ModelPurpose::NewThread,
                window,
                cx,
            );
        });
        h.capture(&format!("picker-draft-model-{suffix}"));
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The model chip under a new thread's composer opens its picker right above
/// itself, not at the top of the window.
#[core::prelude::v1::test]
fn the_draft_model_picker_opens_at_its_chip() {
    if !isolated("pickers::the_draft_model_picker_opens_at_its_chip") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-picker-anchor-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    h.update(|p, window, cx| p.open_draft(window, cx));
    h.draw();
    h.update(|p, window, cx| {
        let chip = p.anchors.borrow().get("draft-model").copied();
        let chip = chip.expect("the model chip records where it was drawn");
        assert!(
            chip.top() > px(430.),
            "the chip sits in the lower half: {chip:?}"
        );
        p.anchor_next("draft-model");
        p.open_model_picker(
            super::super::model_picker::ModelPurpose::NewThread,
            window,
            cx,
        );
        let palette = p
            .overlay
            .as_ref()
            .expect("the picker opened")
            .palette
            .clone();
        assert_eq!(palette.read(cx).anchor, Some(chip));
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}
