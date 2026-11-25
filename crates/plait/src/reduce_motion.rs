//! Bridge the macOS accessibility setting into GPUI's motion preference.

use block2::RcBlock;
use gpui::App;
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::NSOperationQueue;
use std::future::ready;

pub fn observe(cx: &mut App) {
    let workspace = NSWorkspace::sharedWorkspace();
    let center = workspace.notificationCenter();
    let (send, receive) = async_channel::bounded(1);
    // Only a signal crosses the callback boundary. GPUI reads the latest value
    // on its foreground executor, outside the AppKit notification callback.
    let handler = RcBlock::new(move |_| {
        let _ = send.try_send(());
    });
    // SAFETY: this notification comes from the workspace's center. The block
    // captures a thread-safe sender, and AppKit delivers it on the main queue.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &handler,
        )
    };
    cx.set_reduce_motion(workspace.accessibilityDisplayShouldReduceMotion());
    let task = cx.spawn(async move |cx| {
        while receive.recv().await.is_ok() {
            let reduce_motion = workspace.accessibilityDisplayShouldReduceMotion();
            cx.update(|cx| cx.set_reduce_motion(reduce_motion));
        }
    });
    let mut task = Some(task);
    cx.on_app_quit(move |_| {
        // SAFETY: this token was registered on this notification center above.
        unsafe { center.removeObserver((*observer).as_ref()) };
        task.take();
        ready(())
    })
    .detach();
}
