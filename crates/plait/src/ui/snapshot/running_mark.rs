use super::{Assets, Harness, text_system};
use crate::ui::theme::running_mark;
use gpui::{prelude::*, *};
use std::{cell::Cell, path::Path, rc::Rc, sync::Arc, time::Duration};

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.cx.update(|cx| cx.set_reduce_motion(true));
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.messages = vec![snapshot.messages[0].clone()];
        snapshot.tools.remove(0);
        snapshot.tools[0].after = Some(Some(snapshot.messages[0].id.clone()));
        snapshot.live_text.clear();
    });
    harness.capture(&format!("running-mark-{suffix}"));
    harness.cx.update(|cx| cx.set_reduce_motion(false));
}

#[core::prelude::v1::test]
fn running_mark_does_not_schedule_view_renders() {
    struct View(Rc<Cell<usize>>);

    impl Render for View {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            running_mark(px(24.))
        }
    }

    for reduced in [false, true] {
        let mut cx = HeadlessAppContext::with_asset_source(text_system(), Arc::new(Assets));
        cx.update(|cx| cx.set_reduce_motion(reduced));
        let renders = Rc::new(Cell::new(0));
        let handle = cx
            .open_window(size(px(100.), px(100.)), |_, cx| {
                cx.new(|_| View(renders.clone()))
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let initial = renders.get();
        assert!(initial > 0);
        for _ in 0..4 {
            cx.advance_clock(Duration::from_millis(300));
            cx.update_window(handle.into(), |_, window, cx| {
                assert_eq!(window.simulate_next_frame(cx), 0);
            })
            .unwrap();
            cx.run_until_parked();
            assert_eq!(renders.get(), initial);
        }
    }
}

#[cfg(target_os = "macos")]
#[core::prelude::v1::test]
fn reduced_motion_renders_the_unrotated_ring() {
    use crate::ui::theme::{colors, icon};

    struct Pose(bool);

    impl Render for Pose {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(white()).child(if self.0 {
                running_mark(px(32.))
            } else {
                icon("running-mark")
                    .size(px(32.))
                    .text_color(rgb(colors().blue))
            })
        }
    }

    // Compare the reduced-motion element with the same asset drawn without a
    // transform. This also checks the registered SVG actually paints pixels.
    let mut cx = HeadlessAppContext::with_platform(
        text_system(),
        Arc::new(Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| cx.set_reduce_motion(true));
    let draw = |cx: &mut HeadlessAppContext, reduced_mark| {
        let handle = cx
            .open_window(size(px(48.), px(48.)), |_, cx| {
                cx.new(|_| Pose(reduced_mark))
            })
            .unwrap();
        cx.advance_clock(Duration::from_millis(300));
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.capture_screenshot(handle.into())
            .expect("running mark screenshot")
    };
    let reduced = draw(&mut cx, true);
    assert!(reduced.pixels().any(|pixel| pixel.0[0] < 240));
    assert_eq!(reduced, draw(&mut cx, false));
}
