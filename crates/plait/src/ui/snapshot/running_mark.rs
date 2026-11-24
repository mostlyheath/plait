use super::{Assets, Harness, capture_both, text_system};
use crate::ui::theme::running_mark;
use gpui::{prelude::*, *};
use std::{cell::Cell, path::Path, rc::Rc, sync::Arc, time::Duration};

pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.cx.update(|cx| cx.set_reduce_motion(true));
    install(harness, repository);
    // Keep this scene portable and focused on the four marks.
    let (machine, panel_was_open) = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let panel = plait.tabs.model.panel_mut().expect("active fixture thread");
            let panel_was_open = panel.open;
            panel.close();
            (
                std::mem::replace(
                    &mut plait.state.projects[0].machines[0].name,
                    "studio".into(),
                ),
                panel_was_open,
            )
        })
        .expect("window update");
    capture_both(harness, &format!("running-mark-{suffix}"));
    harness.update(move |plait, _, _| {
        plait.state.projects[0].machines[0].name = machine;
        plait
            .tabs
            .model
            .panel_mut()
            .expect("active fixture thread")
            .open = panel_was_open;
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

    harness.update(|plait, _, _| {
        let mut wrong_checkout = (**plait.host.thread.snapshot.as_ref().unwrap()).clone();
        wrong_checkout.checkout = repository.join("replaced");
        assert!(!plait.host.apply_snapshot(
            plait.host.connection.epoch,
            plait.host.thread.watch_epoch,
            wrong_checkout,
        ));
        assert!(!plait.host.connection.connected);
    });
    let lost_identity = animated_marks(&mut harness, &animated);
    harness.update(|plait, _, _| plait.tabs.model.disconnected(&plait.tabs.host));
    let lost_watch = animated_marks(&mut harness, &animated);
    assert_eq!(
        [lost_identity, lost_watch],
        [0, 0],
        "cached work is offline"
    );
    install(&mut harness, &repository);
    assert_eq!(animated_marks(&mut harness, &animated), 4, "work resumed");

    check_tool_states(&mut harness, &repository, &animated);
}

#[cfg(target_os = "macos")]
fn animated_marks(harness: &mut Harness, animated: &Cell<usize>) -> usize {
    harness.draw();
    harness
        .cx
        .capture_screenshot(harness.window.into())
        .unwrap();
    animated.get()
}

#[cfg(target_os = "macos")]
fn check_tool_states(harness: &mut Harness, repository: &Path, animated: &Cell<usize>) {
    use plait_client::host::{ThreadStatus, checkout_queue};
    // A call awaiting authorization can already be in the tool list as Running.
    let approval = super::thread_snapshot(repository, true).pending_approval;
    harness.change_snapshot(|s| {
        s.status = ThreadStatus::AwaitingApproval;
        s.pending_approval = approval;
    });
    let waiting = animated_marks(harness, animated);
    // Other calls in the same turn can still be working.
    harness.change_snapshot(|s| s.pending_approval.as_mut().unwrap().tool_id = "other".into());
    let parallel = animated_marks(harness, animated);
    // A terminal snapshot can retain a tool for which no result arrived.
    harness.change_snapshot(|s| {
        s.status = ThreadStatus::Idle;
        s.pending_approval = None;
    });
    let idle = animated_marks(harness, animated);
    harness.update(|plait, _, _| {
        plait.host.connection.threads_mut()[0].checkout_queue = Some(checkout_queue::Summary::new(
            "later".into(),
            checkout_queue::State::Waiting,
            1,
            1,
        ));
    });
    let queued = animated_marks(harness, animated);
    assert_eq!(
        [waiting, parallel, idle, queued],
        [0, 1, 0, 0],
        "animated sprites for approval, another call's approval, idle, and queued"
    );
}
