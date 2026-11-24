//! Message ownership, quoted context, and selection use the real turn layout.

use super::*;
use crate::ui::selection::Owner;

const REQUEST: &str = "> Keep the public API.\n\nYes, build it.\nKeep the implementation modular.";
const ANSWER: &str =
    "I’ll keep the API unchanged and separate the new presentation from the thread state.";

pub(super) fn stage(harness: &mut Harness, repository: &Path) {
    harness.connect(repository, false);
    harness.update(|plait, _, _| {
        plait.preferences.value.turns = plait_client::preferences::Turns::Line
    });
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Idle;
        snapshot.messages = vec![
            Message {
                id: "request".into(),
                role: MessageRole::User,
                text: REQUEST.into(),
                ..Default::default()
            },
            Message {
                id: "answer".into(),
                role: MessageRole::Assistant,
                text: ANSWER.into(),
                ..Default::default()
            },
        ];
        snapshot.tools.clear();
        snapshot.live_text.clear();
        snapshot.live_reasoning.clear();
    });
    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to(ListOffset::default()));
}

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    stage(harness, repository);
    for (theme, light) in [("dark", false), ("light", true)] {
        harness.update(|plait, _, cx| {
            plait.themes.forced = Some(light);
            plait.apply_theme(cx);
        });
        harness.capture(&format!("message-roles-{theme}-{suffix}"));
    }
    harness.update(|plait, _, cx| {
        plait.themes.forced = Some(false);
        plait.apply_theme(cx);
    });
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        snapshot.messages.truncate(1);
        snapshot.live_text = ANSWER.into();
    });
    harness.capture(&format!("message-roles-streaming-{suffix}"));
}

pub(super) fn caret(harness: &mut Harness, id: &str, block: usize, offset: usize) -> Point<Pixels> {
    harness.draw();
    harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait
                .text_point(&Owner::Message(id.to_owned().into()), block, offset)
                .expect("message text is drawn")
        })
        .unwrap()
}

#[core::prelude::v1::test]
fn request_panels_keep_quotes_selectable_and_out_of_the_turn_gutter() {
    if !isolated("messages::request_panels_keep_quotes_selectable_and_out_of_the_turn_gutter") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repository = fixture_repository(&home.join("messages"));
    for (width, height) in [(1380., 860.), (900., 600.)] {
        let mut harness = Harness::new(&repository, None, width, height);
        stage(&mut harness, &repository);
        let quote = caret(&mut harness, "request", 0, 0);
        let instruction = caret(&mut harness, "request", 1, 0);
        let answer = caret(&mut harness, "answer", 0, 0);
        assert!(quote.x > instruction.x, "quotes have their own inset");
        assert!(instruction.x > answer.x, "requests have panel padding");
        assert!(quote.y < instruction.y && instruction.y < answer.y);
        harness.update(|plait, _, _| {
            let viewport = plait.workspace.live_scroll.viewport_bounds();
            for point in [quote, instruction, answer] {
                assert!(viewport.contains(&point), "text fits the viewport");
            }
            assert_eq!(
                plait.workspace.formatted.messages["request"].source,
                REQUEST
            );
        });

        // Selecting across the panel's blocks copies visible text, not labels
        // or hidden quote marks. The source remains available to Copy message.
        let end = caret(
            &mut harness,
            "request",
            1,
            "Yes, build it.\nKeep the implementation modular.".len(),
        );
        harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: quote,
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
            ..Default::default()
        }));
        harness.update(|plait, _, _| {
            assert_eq!(
                plait.selected_text().as_deref(),
                Some("Keep the public API.\n\nYes, build it.\nKeep the implementation modular.")
            );
        });

        let long = "Keep the request readable when it wraps, without moving the turn line or hiding the end of the text. ".repeat(4);
        harness.change_snapshot(|snapshot| {
            snapshot.messages.truncate(1);
            snapshot.messages[0].text.clone_from(&long);
        });
        harness.update(|plait, _, _| {
            plait.clear_reading_selection();
            plait.workspace.live_scroll.scroll_to(ListOffset::default());
        });
        let first = caret(&mut harness, "request", 0, 0);
        let last = caret(&mut harness, "request", 0, long.len());
        assert!(last.y > first.y, "long requests wrap inside their panel");
        harness.update(|plait, _, _| {
            assert!(
                plait
                    .workspace
                    .live_scroll
                    .viewport_bounds()
                    .contains(&last)
            );
        });
    }
}

#[core::prelude::v1::test]
fn copy_message_cross_label_selection_and_image_only_requests() {
    if !isolated("messages::copy_message_cross_label_selection_and_image_only_requests") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repository = fixture_repository(&home.join("messages-controls"));
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    stage(&mut harness, &repository);

    let quote = caret(&mut harness, "request", 0, 0);
    let answer0 = caret(&mut harness, "answer", 0, 0);
    let view = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait.workspace.live_scroll.viewport_bounds()
        })
        .unwrap();

    // Copy message on the author line copies the original source with `>` marks.
    let copy_pt = point(view.right() - px(48.), quote.y - px(12. + 4. + 11. + 10.5));
    harness.pointer(PlatformInput::MouseMove(MouseMoveEvent {
        position: copy_pt,
        pressed_button: None,
        modifiers: Modifiers::default(),
    }));
    harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: copy_pt,
        click_count: 1,
        ..Default::default()
    }));
    harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position: copy_pt,
        click_count: 1,
        ..Default::default()
    }));
    harness.update(|_, _, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some(REQUEST)
        );
    });

    // Dragging across the panel and the Plait label skips visible labels.
    let answer_end = caret(&mut harness, "answer", 0, "I’ll keep th".len());
    harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: quote,
        click_count: 1,
        ..Default::default()
    }));
    harness.pointer(PlatformInput::MouseMove(MouseMoveEvent {
        position: answer_end,
        pressed_button: Some(MouseButton::Left),
        ..Default::default()
    }));
    harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position: answer_end,
        ..Default::default()
    }));
    harness.update(|plait, _, _| {
        assert_eq!(
            plait.selected_text().as_deref(),
            Some(
                "Keep the public API.\n\nYes, build it.\nKeep the implementation modular.\n\nI’ll keep th"
            )
        );
    });

    // The 22 pt header + 4 pt gap sit above selectable text: box.minY + 10 misses,
    // while box.minY + 36 (the axdrive messageSelectionSpan offset) selects text.
    let drag = |harness: &mut Harness, y: Pixels| {
        let from = point(answer0.x + px(3.), y);
        let to = point(answer0.x + px(160.), y);
        harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            click_count: 1,
            ..Default::default()
        }));
        harness.pointer(PlatformInput::MouseMove(MouseMoveEvent {
            position: to,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        }));
        harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: to,
            ..Default::default()
        }));
        harness
            .window
            .update(&mut harness.cx, |plait, _, _| plait.selected_text())
            .unwrap()
    };
    assert_eq!(drag(&mut harness, answer0.y - px(26.5)), None);
    let hit = drag(&mut harness, answer0.y - px(0.5)).expect("selection below author header");
    assert!(ANSWER.contains(&hit) && hit != ANSWER);

    // An image-only request omits the empty text line inside its panel.
    let img = super::images::fixtures()[0].0.clone();
    harness.change_snapshot(|snapshot| {
        snapshot.messages = vec![Message {
            id: "img-only".into(),
            role: MessageRole::User,
            text: String::new(),
            images: vec![img],
            ..Default::default()
        }];
    });
    harness.draw();
    harness.update(|plait, _, _| {
        assert_eq!(
            plait.text_point(&Owner::Message("img-only".into()), 0, 0),
            None
        );
    });
}
