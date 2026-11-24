mod handoff;
mod ui;
mod update;

use gpui::{prelude::*, *};
use std::{path::PathBuf, time::Instant};

fn main() {
    let startup = Instant::now();
    let requested = std::env::args_os().nth(1).map(PathBuf::from);
    let path = requested.clone().unwrap_or_else(|| PathBuf::from("."));
    gpui_platform::application()
        .with_assets(ui::theme::Assets)
        .run(move |cx| {
            ui::theme::load_bundled_fonts(cx.text_system());
            ui::menus(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1380.), px(860.)), cx);
            if let Err(error) = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.), px(600.))),
                    // One bar: the lights sit over the rail's top strip, and
                    // the app draws the rest. The title is not drawn, but
                    // Mission Control and the Window menu still name it.
                    titlebar: Some(TitlebarOptions {
                        title: Some("Plait".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(ui::TRAFFIC_LIGHTS),
                    }),
                    // The bar moves the window itself, so AppKit does not
                    // drag from the titlebar area or delay clicks there.
                    app_owns_titlebar_drag: true,
                    app_id: Some("dev.plait.desktop".into()),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let mut view = ui::Plait::new(path, startup, window, cx);
                        view.restore_desktop(requested, window, cx);
                        view
                    })
                },
            ) {
                eprintln!("Cannot open Plait: {error}");
                cx.quit();
            }
            cx.activate(true);
        });
}
