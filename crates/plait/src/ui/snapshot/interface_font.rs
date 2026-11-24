//! Interface font preference: bundled IBM Plex Sans vs System UI font across
//! the conversation, sidebar, and Settings → Appearance.

use super::*;
use crate::ui::theme::{PLEX_UI_FONT, SYSTEM_UI_FONT, bundled_ui_font_loaded, ui_font};
use plait_client::preferences::InterfaceFont;

const REQUEST: &str =
    "Summarize the retry backoff changes and list the three affected modules with counts.";
const MACHINE: &str = "studio";
const THREAD_TITLE: &str = "Review retry backoff";

const ANSWER: &str = r#"The retry pass is ready across **3 modules** (`14` added lines, `4` removed):

1. **Client queue** — caps exponential backoff at `1200 ms` across 4 attempts.
2. **Status summary** — keeps counts aligned while active threads update in place.
3. **Settings → Appearance** — switches between **Plex Sans** (bundled Regular, Medium, SemiBold) and **System**.

```rust
pub fn backoff_ms(attempt: u32) -> u64 {
    (150u64 << attempt.min(3)).min(1200)
}
```"#;

fn stage_conversation(harness: &mut Harness, repository: &Path) -> bool {
    let mut previous_hide_inspector = false;
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
        previous_hide_inspector =
            std::mem::replace(&mut plait.preferences.value.hide_inspector, true);
        let threads = plait.host.connection.threads_mut();
        threads[0].title = Some(THREAD_TITLE.into());
        threads[0].status = ThreadStatus::Idle;
        threads[1].title = Some("Check queue status".into());
        for project in &mut plait.state.projects {
            project.machines[0].name = MACHINE.into();
        }
        plait.sidebar = true;
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });
    previous_hide_inspector
}

fn show_appearance(harness: &mut Harness) {
    harness.update(|plait, window, cx| {
        plait.show_settings(true, window, cx);
        plait.settings_tab = SettingsTab::Appearance;
    });
    // Scroll after layout so both font rows are visible at the narrow size.
    harness.draw();
    harness.update(|plait, _, _| plait.settings_scroll.scroll_to_bottom());
}

fn capture_choice(harness: &mut Harness, choice: InterfaceFont, name: &str, suffix: &str) {
    harness.update(|plait, _, cx| {
        plait.preferences.value.interface_font = choice;
        plait.apply_interface_font(cx);
        plait.settings_open = false;
    });
    harness.capture(&format!("interface-font-{name}-{suffix}"));

    show_appearance(harness);
    harness.capture(&format!("interface-font-{name}-appearance-{suffix}"));
}

/// Conversation, sidebar, and Settings → Appearance in both bundled Plex Sans
/// (dark and light defaults) and System UI font.
pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    let previous_hide_inspector = stage_conversation(harness, repository);
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

    harness.update(|plait, _, cx| {
        plait.settings_open = false;
        plait.preferences.value.hide_inspector = previous_hide_inspector;
        plait.preferences.value.interface_font = InterfaceFont::Plex;
        plait.apply_interface_font(cx);
        plait.rebuild_projects(cx);
    });
}

#[cfg(debug_assertions)]
fn assert_font_controls_fit(harness: &mut Harness, scene: &str) {
    use crate::ui::theme::{TYPE_CONTROL_SIZE, TYPE_CONTROL_WEIGHT};

    let frame = harness.draw_a11y();
    frame.assert_no_duplicates(scene);
    harness.update(|plait, window, _| {
        let clip = plait.settings_scroll.bounds();
        for choice in InterfaceFont::ALL {
            let label = choice.label();
            let name = format!("Interface font: {label}");
            let bounds = frame
                .nodes
                .iter()
                .find(|node| {
                    node.role == Role::RadioButton && node.label.as_deref() == Some(name.as_str())
                })
                .unwrap_or_else(|| panic!("{scene}: missing {name}"))
                .bounds;
            assert!(
                clip.contains(&bounds.origin) && clip.contains(&bounds.bottom_right()),
                "{scene}: {name} is clipped by the scroll view ({bounds:?} outside {clip:?})"
            );
            let text = window.text_system().shape_line(
                label.into(),
                px(TYPE_CONTROL_SIZE),
                &[TextRun {
                    len: label.len(),
                    font: Font {
                        weight: TYPE_CONTROL_WEIGHT,
                        ..font(ui_font())
                    },
                    ..Default::default()
                }],
                None,
            );
            // The shared segment reserves 11 points of padding and a 1-point border per side.
            assert!(
                text.width <= bounds.size.width - px(24.) + px(1.),
                "{scene}: {name} has insufficient label width"
            );
        }
    });
}

fn assert_prose_controls_use(harness: &mut Harness, expected: &str) {
    let call = tool("font-probe", "read", ToolStatus::Done, "{}", "");
    harness.update(|plait, _, cx| {
        crate::ui::changes::assert_hunk_action_font(plait, cx, expected);
        crate::ui::tool_card::assert_tool_caption_font(plait, &call, cx, expected);
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
        let mut harness = Harness::with_host(&repository, None, width, height, None);
        stage_conversation(&mut harness, &repository);
        harness.update(|plait, window, _| {
            assert!(
                !plait.shows_inspector(Plait::wide(window)),
                "font fixture exposes the repository path"
            );
        });
        assert!(bundled_ui_font_loaded(), "bundled IBM Plex Sans must load");
        assert_eq!(ui_font().as_ref(), PLEX_UI_FONT);
        harness.update(|plait, _, _| {
            assert_eq!(plait.preferences.value.interface_font, InterfaceFont::Plex);
        });

        assert_prose_controls_use(&mut harness, PLEX_UI_FONT);
        show_appearance(&mut harness);
        #[cfg(debug_assertions)]
        assert_font_controls_fit(
            &mut harness,
            &format!("interface-font-plex-appearance-{suffix}"),
        );

        // Switch to System font: applies immediately and updates preference.
        harness.update(|plait, _, cx| {
            plait.set_interface_font(InterfaceFont::System, cx);
        });
        assert_eq!(ui_font().as_ref(), SYSTEM_UI_FONT);
        assert_prose_controls_use(&mut harness, SYSTEM_UI_FONT);
        #[cfg(debug_assertions)]
        assert_font_controls_fit(
            &mut harness,
            &format!("interface-font-system-appearance-{suffix}"),
        );
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
