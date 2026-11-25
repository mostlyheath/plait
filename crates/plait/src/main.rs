mod handoff;
#[cfg(target_os = "macos")]
mod reduce_motion;
mod ui;
mod update;

use gpui::{prelude::*, *};
use std::{path::PathBuf, time::Instant};

/// Creates an empty, uniquely named directory in the system temp dir.
///
/// `create_dir` (not `create_dir_all`) fails if the name exists, so a reused
/// PID or a clock tie can never land in an old scratch directory; we retry
/// with a new suffix instead. The directory is not removed on exit.
///
/// Returns the new directory's path, or the first unexpected I/O error.
fn scratch_directory() -> std::io::Result<PathBuf> {
    for _ in 0..8 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let path = std::env::temp_dir().join(format!(
            "plait-scratch-{}-{nanos:09}",
            std::process::id()
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::ErrorKind::AlreadyExists.into())
}

fn main() {
    let startup = Instant::now();
    // Parse once: `--scratch` is a flag, not a project path.
    let mut scratch = false;
    let mut requested = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--scratch" {
            scratch = true;
        } else if requested.is_none() {
            requested = Some(PathBuf::from(arg));
        }
    }
    let path = if scratch {
        match scratch_directory() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("Cannot create scratch directory: {error}");
                std::process::exit(1);
            }
        }
    } else {
        requested.clone().unwrap_or_else(|| PathBuf::from("."))
    };
    // Scratch mode must not restore a saved checkout over its own directory.
    let requested = if scratch { Some(path.clone()) } else { requested };
    gpui_platform::application()
        .with_assets(ui::theme::Assets)
        .run(move |cx| {
            ui::theme::load_bundled_fonts(cx.text_system());
            #[cfg(target_os = "macos")]
            reduce_motion::observe(cx);
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
