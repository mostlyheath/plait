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
