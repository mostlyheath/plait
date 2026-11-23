//! Code size preference: Settings → Appearance control and code views at 14 points.

use super::*;
use crate::ui::theme::{code_line_height, code_size};
use plait_client::preferences::CODE_SIZE_DEFAULT;

const REQUEST: &str = "Show the code block and diff for the code size helper.";

const ANSWER: &str = r#"Clamped in `plait-client` and read wherever the code font is drawn:

```rust
pub fn code_line_height() -> f32 {
    code_size() + 7.
}
```

```diff
- .text_size(px(12.))
- .line_height(px(19.))
+ .text_size(px(code_size()))
+ .line_height(px(code_line_height()))
```"#;

fn stage_conversation(harness: &mut Harness, repository: &Path) {
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Idle;
        snapshot.messages = vec![
            Message {
                id: "code-size-request".into(),
                role: MessageRole::User,
                text: REQUEST.into(),
                ..Default::default()
            },
            Message {
                id: "code-size-answer".into(),
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

/// Settings → Appearance with the Code size control, and a conversation with
/// a code block and a diff drawn at 14 points.
pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    stage_conversation(harness, repository);
    harness.update(|plait, _, cx| {
        plait.preferences.value.set_code_size(14);
        plait.apply_code_size(cx);
        plait.settings_open = true;
        plait.settings_tab = SettingsTab::Appearance;
    });
    // The control is the last on the page, below the themes: draw once so
    // the page knows its height, then show its end.
    harness.capture(&format!("code-size-appearance-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.settings_scroll.scroll_to_bottom();
        cx.notify();
    });
    harness.capture(&format!("code-size-appearance-{suffix}"));
    harness.update(|plait, _, _| {
        plait.settings_open = false;
    });
    harness.capture(&format!("code-size-conversation-{suffix}"));
    // Restore default so later scenes in the walk stay at 12 points.
    harness.update(|plait, _, cx| {
        plait.preferences.value.set_code_size(CODE_SIZE_DEFAULT);
        plait.apply_code_size(cx);
    });
}

#[core::prelude::v1::test]
fn code_size_setting_applies_immediately_at_both_window_sizes() {
    if !isolated("code_size::code_size_setting_applies_immediately_at_both_window_sizes") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repository = fixture_repository(&home.join("code-size"));
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut harness = Harness::new(&repository, None, width, height);
        stage_conversation(&mut harness, &repository);
        assert_eq!(code_size(), 12.);
        assert_eq!(code_line_height(), 19.);

        // Open Settings → Appearance and verify the Code size radio controls.
        harness.update(|plait, _, _| {
            plait.settings_open = true;
            plait.settings_tab = SettingsTab::Appearance;
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("code-size-appearance-{suffix}"));
            for label in ["Code size: 12 pt", "Code size: 13 pt", "Code size: 14 pt"] {
                assert!(
                    frame.has_node(Role::RadioButton, |n| n.label.as_deref() == Some(label)),
                    "{suffix}: missing radio button {label:?}"
                );
            }
        }
        harness.draw();

        // Changing the preference applies immediately and updates line heights.
        harness.update(|plait, _, cx| {
            plait.set_code_size(14, cx);
        });
        assert_eq!(code_size(), 14.);
        assert_eq!(code_line_height(), 21.);
        harness.update(|plait, _, _| {
            assert_eq!(plait.preferences.value.code_size(), 14);
        });
        harness.draw();

        // Conversation with a code block and a diff draws cleanly at 14 points.
        harness.update(|plait, _, _| {
            plait.settings_open = false;
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("code-size-conversation-{suffix}"));
        }
        harness.draw();

        // Reset back to default for the next window size.
        harness.update(|plait, _, cx| {
            plait.set_code_size(CODE_SIZE_DEFAULT, cx);
        });
        assert_eq!(code_size(), 12.);
    }
}
