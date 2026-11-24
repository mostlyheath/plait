//! The same sample conversation drawn with both turn styles.
use super::*;
use plait_client::{
    preferences::Turns,
    tabs::bookmark::{TextPosition, ViewDetails},
};

pub(super) fn walk(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut scene = Harness::new(repository, output, width, height);
    let harness = &mut scene;
    let narrow = suffix == "narrow";
    let capture = |harness: &mut Harness, name: &str| {
        if narrow {
            harness.capture(&format!("{name}-narrow"));
        } else {
            harness.capture(name);
        }
    };
    messages::stage(harness, repository);
    harness.update(|plait, _, _| {
        plait.state.projects[0].machines[0].name = "Sample Mac".into();
        plait.preferences.value.hide_inspector = true;
    });
    capture(harness, "turns-line");
    harness.update(|plait, _, _| {
        plait.preferences.value.turns = Turns::Bubbles;
        plait.workspace.live_scroll.remeasure();
    });
    capture(harness, "bubbles");
    harness.update(|plait, _, cx| {
        plait.themes.forced = Some(true);
        plait.apply_theme(cx);
    });
    capture(harness, "bubbles-light");
    harness.update(|plait, _, cx| {
        plait.themes.forced = Some(false);
        plait.apply_theme(cx);
    });

    harness.update(|plait, _, _| {
        let position = |id: &str, block, byte| TextPosition {
            source: format!("message:{id}"),
            block,
            byte,
        };
        plait.workspace.reading.restore_selection(
            THREAD,
            &ViewDetails {
                selection: Some((position("request", 1, 0), position("answer", 0, 12))),
                ..Default::default()
            },
        );
    });
    capture(harness, "bubbles-selection");
    harness.update(|plait, window, cx| {
        plait.clear_reading_selection();
        plait.open_conversation_find(window, cx);
        let field = plait.workspace.conversation_find.field.clone().unwrap();
        field.update(cx, |field, cx| field.set_text("API", cx));
    });
    harness.draw();
    capture(harness, "bubbles-find");
    harness.update(|plait, window, cx| plait.close_conversation_find(window, cx));
    harness.change_snapshot(|snapshot| {
        snapshot.messages[0].text =
            "Keep the public API stable while splitting the implementation into smaller modules. "
                .repeat(6);
    });
    capture(harness, "bubbles-long-request");
    images::install(harness, 3);
    capture(harness, "bubbles-images");
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        snapshot.messages.truncate(1);
        snapshot.live_text = "I’ll compare the two views and check their layout.".into();
    });
    capture(harness, "bubbles-streaming");
    let mut previous = None;
    harness.update(|plait, _, _| {
        previous = plait.preferences.value.fold_earlier_turns;
        plait.preferences.value.fold_earlier_turns = Some(true);
    });
    folded_turns::install_long_thread(harness, repository);
    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to(ListOffset::default()));
    capture(harness, "bubbles-folded-turns");
    harness.update(|plait, _, _| plait.preferences.value.fold_earlier_turns = previous);
}

#[core::prelude::v1::test]
#[ignore = "writes turn-style PNGs; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn turn_snapshots() {
    if !isolated("bubbles::turn_snapshots") {
        return;
    }
    let output = PathBuf::from(std::env::var_os("PLAIT_SNAPSHOT_DIR").unwrap());
    std::fs::create_dir_all(&output).unwrap();
    let repository =
        fixture_repository(&PathBuf::from(std::env::var_os("HOME").unwrap()).join("turn-scenes"));
    for (width, height, suffix) in [(1380., 860., "wide"), (900., 600., "narrow")] {
        walk(&repository, Some(&output), suffix, width, height);
    }
}

#[core::prelude::v1::test]
fn selecting_across_authors_copies_the_same_text_in_both_styles() {
    if !isolated("bubbles::selecting_across_authors_copies_the_same_text_in_both_styles") {
        return;
    }
    let repository = fixture_repository(
        &PathBuf::from(std::env::var_os("HOME").unwrap()).join("bubble-selection"),
    );
    let mut harness = Harness::new(&repository, None, 900., 600.);
    messages::stage(&mut harness, &repository);
    for style in [Turns::Line, Turns::Bubbles] {
        harness.update(|plait, _, _| {
            plait.clear_reading_selection();
            plait.preferences.value.turns = style;
            plait.workspace.live_scroll.remeasure();
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            for author in ["Your message: ", "Plait message: "] {
                assert!(frame.nodes.iter().any(|node| {
                    node.label
                        .as_deref()
                        .is_some_and(|label| label.starts_with(author))
                }));
            }
        }
        let start = messages::caret(&mut harness, "request", 0, 0);
        let end = messages::caret(&mut harness, "answer", 0, "I’ll keep th".len());
        harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: start,
            click_count: 1,
            ..Default::default()
        }));
        harness.pointer(PlatformInput::MouseMove(MouseMoveEvent {
            position: end,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        }));
        harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: end,
            click_count: 1,
            ..Default::default()
        }));
        harness.update(|plait, _, _| {
            assert_eq!(plait.selected_text().as_deref(), Some("Keep the public API.\n\nYes, build it.\nKeep the implementation modular.\n\nI’ll keep th"));
        });
    }
}

#[core::prelude::v1::test]
fn changing_style_keeps_the_readers_row() {
    if !isolated("bubbles::changing_style_keeps_the_readers_row") {
        return;
    }
    let repository =
        fixture_repository(&PathBuf::from(std::env::var_os("HOME").unwrap()).join("bubble-scroll"));
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Idle;
        snapshot.tools.clear();
        snapshot.live_text.clear();
        snapshot.live_reasoning.clear();
        snapshot.messages = (0..100)
            .map(|index| Message {
                id: format!("message-{index}"),
                role: if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                text: format!("Sample message {index}."),
                ..Default::default()
            })
            .collect();
    });
    harness.update(|plait, _, cx| {
        plait.set_fold_earlier_turns(false, cx);
        plait.workspace.live_scroll.scroll_to(ListOffset {
            item_ix: 40,
            offset_in_item: px(0.),
        });
    });
    for turns in [Turns::Line, Turns::Bubbles] {
        harness.update(|plait, _, cx| plait.set_turns(turns, cx));
        harness.draw();
        harness.update(|plait, _, _| {
            assert_eq!(plait.workspace.live_scroll.logical_scroll_top().item_ix, 40)
        });
    }
}
