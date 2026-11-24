use super::{Assets, Harness, capture_both, text_system};
use crate::ui::theme::running_mark;
use gpui::{prelude::*, *};
use std::{cell::Cell, path::Path, rc::Rc, sync::Arc, time::Duration};

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.cx.update(|cx| cx.set_reduce_motion(true));
    install(harness, repository);
    // Keep this scene portable and focused on the four marks.
    let (machine, hide_inspector) = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            (
                std::mem::replace(
                    &mut plait.state.projects[0].machines[0].name,
                    "studio".into(),
                ),
                std::mem::replace(&mut plait.preferences.value.hide_inspector, true),
            )
        })
        .expect("window update");
    capture_both(harness, &format!("running-mark-{suffix}"));
    harness.update(move |plait, _, _| {
        plait.state.projects[0].machines[0].name = machine;
        plait.preferences.value.hide_inspector = hide_inspector;
    });
    harness.cx.update(|cx| cx.set_reduce_motion(false));
}

fn install(harness: &mut Harness, repository: &Path) {
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.messages = vec![snapshot.messages[0].clone()];
        snapshot.tools.remove(0);
        snapshot.tools[0].after = Some(Some(snapshot.messages[0].id.clone()));
        snapshot.live_text.clear();
    });
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
fn plait_uses_animated_sprites_until_motion_is_reduced() {
    use super::{fixture_repository, isolated};
    use crate::ui::Plait;
    use std::time::Instant;

    if !isolated("running_mark::plait_uses_animated_sprites_until_motion_is_reduced") {
        return;
    }

    struct Meter {
        renderer: Option<Box<dyn PlatformHeadlessRenderer>>,
        animated: Rc<Cell<usize>>,
    }

    impl PlatformHeadlessRenderer for Meter {
        fn render_scene_to_image(
            &mut self,
            scene: &Scene,
            size: Size<DevicePixels>,
        ) -> gpui::Result<image::RgbaImage> {
            self.animated.set(
                scene
                    .monochrome_sprites
                    .iter()
                    .filter(|sprite| sprite.animation != SpriteAnimationId::NONE)
                    .count(),
            );
            // A device-free draw still exercises GPUI layout, paint and sprite selection.
            match &mut self.renderer {
                Some(renderer) => {
                    let image = renderer.render_scene_to_image(scene, size)?;
                    assert_eq!(image.dimensions(), (size.width.into(), size.height.into()));
                    Ok(image)
                }
                None => Ok(image::RgbaImage::default()),
            }
        }

        fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> gpui::Result<()> {
            match &mut self.renderer {
                Some(renderer) => renderer.render_scene(scene, size),
                None => Ok(()),
            }
        }

        fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
            match &self.renderer {
                Some(renderer) => renderer.sprite_atlas(),
                None => Arc::new(HeadlessAtlas::default()),
            }
        }
    }

    let home = std::env::var_os(super::SCENE_HOME).expect("scene home");
    let repository = fixture_repository(&Path::new(&home).join("running-mark"));
    let animated = Rc::new(Cell::new(0));
    let mut cx = HeadlessAppContext::with_platform(text_system(), Arc::new(Assets), {
        let animated = animated.clone();
        move || {
            let renderer = match gpui_platform::current_headless_renderer() {
                Ok(renderer) => renderer,
                // The pinned Metal backend reports a missing device as an error.
                Err(error) if error.to_string() == "accessing a compatible Metal device" => {
                    eprintln!("offscreen Metal unavailable; checking scene sprites only");
                    None
                }
                Err(error) => return Err(error),
            };
            Ok(Some(Box::new(Meter {
                renderer,
                animated: animated.clone(),
            })))
        }
    });
    cx.allow_parking();
    cx.update(crate::ui::menus);
    let window = cx
        .open_window(size(px(1380.), px(860.)), |window, cx| {
            let client = plait_client::host::Client::with_test_git_host(
                None,
                repository.join("absent.sock"),
            );
            cx.new(|cx| {
                Plait::with_host_client(repository.clone(), Instant::now(), client, window, cx)
            })
        })
        .unwrap();
    let mut harness = Harness {
        cx,
        window,
        output: None,
        _host: None,
    };
    harness.settle();
    install(&mut harness, &repository);

    let mut starting_pose = None;
    for (reduced, expected) in [(false, 4), (true, 0), (false, 4)] {
        harness.cx.update(|cx| cx.set_reduce_motion(reduced));
        harness.draw();
        let frame = harness.cx.capture_screenshot(window.into()).unwrap();
        assert_eq!(animated.get(), expected, "reduce motion = {reduced}");
        // Offscreen captures show each sprite before presentation advances it.
        // Reduced motion must preserve those same pixels.
        if frame.width() > 0 {
            let reference = starting_pose.get_or_insert_with(|| frame.clone());
            assert!(frame == *reference, "the starting pose changed");
        }
    }
}
