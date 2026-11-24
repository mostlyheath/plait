//! Interface font preference: bundled IBM Plex Sans vs System UI font across
//! the conversation, sidebar, and Settings → Appearance.

use super::*;
use crate::ui::theme::{PLEX_UI_FONT, SYSTEM_UI_FONT, bundled_ui_font_loaded, ui_font};
use plait_client::preferences::InterfaceFont;

const REQUEST: &str =
    "Summarize the retry backoff changes and list the three affected modules with counts.";

const ANSWER: &str = r#"The retry pass is ready across **3 modules** (`14` added lines, `4` removed):

1. **Client queue** — caps exponential backoff at `1200 ms` across 4 attempts.
2. **Status summary** — keeps counts aligned while active threads update in place.
3. **Settings → Appearance** — switches between **Plex Sans** (bundled Regular, Medium, SemiBold) and **System**.

```rust
pub fn backoff_ms(attempt: u32) -> u64 {
    (150u64 << attempt.min(3)).min(1200)
}
```"#;

fn stage_conversation(harness: &mut Harness, repository: &Path) {
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Idle;
        snapshot.messages = vec![
            Message {
                id: "interface-font-request".into(),
                role: MessageRole::User,
                text: REQUEST.into(),
                ..Default::default()
            },
            Message {
                id: "interface-font-answer".into(),
                role: MessageRole::Assistant,
                text: ANSWER.into(),
                ..Default::default()
            },
        ];
        snapshot.tools.clear();
        snapshot.live_text.clear();
        snapshot.live_reasoning.clear();
    });
    harness.update(|plait, _, _| {
        plait.sidebar = true;
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });
}

fn capture_choice(harness: &mut Harness, choice: InterfaceFont, name: &str, suffix: &str) {
    harness.update(|plait, _, cx| {
        plait.preferences.value.interface_font = choice;
        plait.apply_interface_font(cx);
        plait.settings_open = false;
    });
    harness.capture(&format!("interface-font-{name}-{suffix}"));

    harness.update(|plait, _, cx| {
        plait.settings_open = true;
        plait.settings_tab = SettingsTab::Appearance;
        cx.notify();
    });
    // Draw once so Settings measures its scroll height, then scroll to show
    // Interface font beside Code font.
    harness.draw();
    harness.update(|plait, _, cx| {
        plait.settings_scroll.scroll_to_bottom();
        cx.notify();
    });
    harness.capture(&format!("interface-font-{name}-appearance-{suffix}"));
}

/// Conversation, sidebar, and Settings → Appearance in both bundled Plex Sans
/// (dark and light defaults) and System UI font.
pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    stage_conversation(harness, repository);
    capture_choice(harness, InterfaceFont::Plex, "plex", suffix);

    // Also inspect the bundled font in the light default theme.
    harness.update(|plait, _, cx| {
        plait.settings_open = false;
        plait.themes.forced = Some(true);
        plait.apply_theme(cx);
    });
    harness.capture(&format!("interface-font-plex-light-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.themes.forced = Some(false);
        plait.apply_theme(cx);
    });

    capture_choice(harness, InterfaceFont::System, "system", suffix);

    // Restore the default interface font so later scenes in the walk match it.
    harness.update(|plait, _, cx| {
        plait.settings_open = false;
        plait.preferences.value.interface_font = InterfaceFont::Plex;
        plait.apply_interface_font(cx);
    });
}

#[core::prelude::v1::test]
fn interface_font_switches_and_loads_older_preferences_as_plex() {
    if !isolated("interface_font::interface_font_switches_and_loads_older_preferences_as_plex") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repository = fixture_repository(&home.join("interface-font"));

    // Write an older preferences file with no `interface_font` key; startup
    // must load it as `plex` and draw with IBM Plex Sans.
    let prefs_path = plait_client::preferences::default_path().expect("scratch preferences path");
    std::fs::create_dir_all(prefs_path.parent().unwrap()).unwrap();
    std::fs::write(&prefs_path, r#"{"hide_inspector":false}"#).unwrap();

    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut harness = Harness::new(&repository, None, width, height);
        stage_conversation(&mut harness, &repository);
        assert!(bundled_ui_font_loaded(), "bundled IBM Plex Sans must load");
        assert_eq!(ui_font().as_ref(), PLEX_UI_FONT);
        harness.update(|plait, _, _| {
            assert_eq!(plait.preferences.value.interface_font, InterfaceFont::Plex);
        });

        // Open Settings → Appearance and check accessible radio buttons.
        harness.update(|plait, _, _| {
            plait.settings_open = true;
            plait.settings_tab = SettingsTab::Appearance;
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("interface-font-plex-appearance-{suffix}"));
            for label in ["Interface font: Plex Sans", "Interface font: System"] {
                assert!(
                    frame.has_node(Role::RadioButton, |n| n.label.as_deref() == Some(label)),
                    "{suffix}: missing radio button {label:?}"
                );
            }
        }
        harness.draw();

        // Switch to System font: applies immediately and updates preference.
        harness.update(|plait, _, cx| {
            plait.set_interface_font(InterfaceFont::System, cx);
        });
        assert_eq!(ui_font().as_ref(), SYSTEM_UI_FONT);
        harness.update(|plait, _, _| {
            assert_eq!(
                plait.preferences.value.interface_font,
                InterfaceFont::System
            );
            plait.settings_open = false;
        });
        #[cfg(debug_assertions)]
        {
            let frame = harness.draw_a11y();
            frame.assert_no_duplicates(&format!("interface-font-system-{suffix}"));
        }
        harness.draw();

        // Switch back to Plex Sans for the next window size.
        harness.update(|plait, _, cx| {
            plait.set_interface_font(InterfaceFont::Plex, cx);
        });
        assert_eq!(ui_font().as_ref(), PLEX_UI_FONT);
    }
}
