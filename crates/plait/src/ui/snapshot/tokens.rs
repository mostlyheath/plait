//! A test-only specimen drawn with the same control styles as the app.
use super::*;
use crate::ui::theme::*;
use plait_client::theme::{Source, Theme};

struct Tokens(Theme);

impl Render for Tokens {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = &self.0.palette;
        set_colors(p);
        let opaque = [
            ("Navigation", p.rail),
            ("Background", p.bg),
            ("Panel", p.panel),
            ("Raised", p.raised),
            ("Border", p.border),
            ("Outline", p.edge),
            ("Focus edge", p.focus_edge),
            ("Selection", p.selected),
            ("Emphasis", p.strong),
            ("Text", p.text),
            ("Secondary", p.muted),
            ("Hint", p.subtle),
            ("Faint", p.faint),
            ("Mint", p.mint),
            ("Blue", p.blue),
            ("Violet", p.violet),
            ("Coral", p.red),
            ("Amber", p.amber),
            ("Raised hover", p.raised_hover),
            ("Caution edge", p.warn_edge),
            ("Caution fill", p.warn_fill),
        ];
        let translucent = [
            ("Hover", p.hover),
            ("Reading selection", p.selection),
            ("Field selection", p.text_selection),
            ("Added fill", p.added_fill),
            ("Removed fill", p.removed_fill),
            ("Failed edge", p.failed_edge),
            ("Scrim", p.scrim),
        ];
        let swatch = |name, color: Rgba| {
            row()
                .w(px(134.))
                .gap_2()
                .child(
                    div()
                        .size(px(16.))
                        .rounded(px(3.))
                        .bg(color)
                        .border_1()
                        .border_color(rgb(p.edge)),
                )
                .child(name)
        };
        let samples = row()
            .flex_wrap()
            .gap_y_1()
            .children(opaque.map(|(name, color)| swatch(name, rgb(color))))
            .children(translucent.map(|(name, color)| swatch(name, rgba(color))));
        let surfaces = row().gap_2().children(
            [
                ("Navigation", p.rail),
                ("Background", p.bg),
                ("Panel", p.panel),
                ("Raised", p.raised),
            ]
            .map(|(name, surface)| {
                column()
                    .flex_1()
                    .p_2()
                    .gap_1()
                    .rounded(px(CONTROL_CORNER))
                    .bg(rgb(surface))
                    .border_1()
                    .border_color(rgb(p.edge))
                    .child(div().text_color(rgb(p.strong)).child(name))
                    .children(
                        [
                            ("Emphasis", p.strong),
                            ("Text", p.text),
                            ("Secondary", p.muted),
                            ("Hint", p.subtle),
                        ]
                        .map(|(label, color)| div().text_color(rgb(color)).child(label)),
                    )
            }),
        );
        let buttons = column().gap_2().children(
            [
                ("Primary", Tone::Primary),
                ("Standard", Tone::Filled),
                ("Quiet", Tone::Ghost),
            ]
            .map(|(name, tone)| {
                row().gap_2().child(div().w(px(80.)).child(name)).children(
                    ["Rest", "Hovered", "Pressed", "Focused", "Disabled"]
                        .into_iter()
                        .enumerate()
                        .map(|(i, state)| {
                            let control = toned_button((name, i), state, tone);
                            match i {
                                1 => hovered(control, tone),
                                2 => pressed(hovered(control, tone), tone),
                                3 => focused_control(control),
                                4 => control.disabled(true),
                                _ => control,
                            }
                        }),
                )
            }),
        );
        column()
            .size_full()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .text_size(px(12.))
            .p_4()
            .gap_3()
            .child(
                div()
                    .text_size(px(18.))
                    .text_color(rgb(p.strong))
                    .child(self.0.name.clone()),
            )
            .child(samples)
            .child(surfaces)
            .child(buttons)
    }
}

pub(super) fn walk(output: Option<&Path>, suffix: &str, width: f32, height: f32) {
    let render = output.is_some();
    let mut cx = HeadlessAppContext::with_platform(text_system(), Arc::new(Assets), move || {
        if render {
            gpui_platform::current_headless_renderer()
        } else {
            Ok(None)
        }
    });
    for theme in plait_client::theme::built_in().into_iter().filter(|theme| {
        theme.palette == plait_client::theme::DEFAULT_DARK
            || theme.palette == plait_client::theme::DEFAULT_LIGHT
            || matches!(theme.source, Source::Bundled { .. })
                && matches!(theme.name.as_str(), "Nord" | "Solarized Light")
    }) {
        let name = theme.name.to_lowercase().replace(' ', "-");
        let handle = cx
            .open_window(size(px(width), px(height)), |_, cx| {
                cx.new(|_| Tokens(theme))
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        if let Some(path) = output {
            cx.capture_screenshot(handle.into())
                .unwrap()
                .save(path.join(format!("tokens-{name}-{suffix}.png")))
                .unwrap();
        }
    }
    set_colors(&plait_client::theme::DEFAULT_DARK);
}

#[cfg(target_os = "macos")]
#[core::prelude::v1::test]
fn keyboard_focus_preserves_the_surface_behind_quiet_controls() {
    use crate::ui::Tab;

    struct FocusSpecimen {
        palette: plait_client::theme::Palette,
        surface: u32,
        focus: FocusHandle,
    }

    impl Render for FocusSpecimen {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            set_colors(&self.palette);
            column()
                .id("focus-specimen")
                .track_focus(&self.focus)
                .on_action(|_: &Tab, window, cx| window.focus_next(cx))
                .size_full()
                .bg(rgb(self.surface))
                .child(
                    toned_button("quiet", "", Tone::Ghost)
                        .absolute()
                        .left(px(24.))
                        .top(px(16.))
                        .w(px(60.)),
                )
        }
    }

    let Ok(Some(renderer)) = gpui_platform::current_headless_renderer() else {
        eprintln!("offscreen Metal unavailable");
        return;
    };
    drop(renderer);
    let mut cx = HeadlessAppContext::with_platform(
        text_system(),
        Arc::new(Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| cx.bind_keys([KeyBinding::new("tab", Tab, None)]));
    for palette in [
        plait_client::theme::DEFAULT_DARK,
        plait_client::theme::DEFAULT_LIGHT,
    ] {
        for (name, surface) in [
            ("panel", palette.panel),
            ("raised", palette.raised),
            ("navigation", palette.rail),
        ] {
            let handle = cx
                .open_window(size(px(112.), px(64.)), |window, cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    cx.new(|_| FocusSpecimen {
                        palette,
                        surface,
                        focus,
                    })
                })
                .unwrap();
            let window = handle.into();
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
            let rest = cx.capture_screenshot(window).unwrap();
            cx.update_window(window, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke: Keystroke::parse("tab").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                assert!(window.last_input_was_keyboard());
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
            let focus = cx.capture_screenshot(window).unwrap();
            let scale = focus.width() / 112;
            let pixel = |image: &image::RgbaImage, x, y| image.get_pixel(x * scale, y * scale).0;
            for (x, y) in [(54, 30), (23, 30)] {
                assert_eq!(
                    pixel(&focus, x, y),
                    pixel(&rest, x, y),
                    "focus changed {name} at ({x}, {y}) for light={}",
                    palette.light,
                );
            }
            let [_, red, green, blue] = palette.blue.to_be_bytes();
            assert_eq!(
                pixel(&focus, 24, 30),
                [red, green, blue, 255],
                "no blue focus edge on {name} for light={}",
                palette.light,
            );
        }
    }
    set_colors(&plait_client::theme::DEFAULT_DARK);
}
