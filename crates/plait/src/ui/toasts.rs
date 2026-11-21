use super::*;
use plait_core::toasts::LIFETIME;

/// Clears the tab row and the workspace header.
const TOP: f32 = 84.;

impl Plait {
    /// Shows an in-app alert for another thread and wakes when it expires.
    pub(super) fn show_toast(
        &mut self,
        id: &str,
        title: String,
        detail: String,
        cx: &mut Context<Self>,
    ) {
        self.toasts.push(id, title, detail, Instant::now());
        let timer = cx.background_executor().timer(LIFETIME);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.toasts.expire(Instant::now()) {
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn dismiss_toast(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.toasts.dismiss(id) {
            cx.notify();
        }
    }

    fn open_toast(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.toasts.dismiss(&id);
        if self
            .host
            .connection
            .threads
            .iter()
            .any(|thread| thread.id == id)
        {
            self.select_host_thread(id, window, cx);
        }
        cx.notify();
    }

    /// The stack in the window's upper right, below the header, newest at the
    /// bottom. Lower down it would cover the composer's Send and Stop.
    pub(super) fn toast_stack(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.toasts.items().is_empty() {
            return None;
        }
        let cards = self.toasts.items().iter().map(|toast| {
            let color = self
                .host
                .connection
                .threads
                .iter()
                .find(|thread| thread.id == toast.thread)
                .map_or(colors().muted, |thread| host_status_color(thread.status));
            let open = toast.thread.clone();
            let close = toast.thread.clone();
            row()
                .id(SharedString::from(format!("toast:{}", toast.thread)))
                .role(Role::Status)
                .w_full()
                .gap_1()
                .p_1()
                .rounded_lg()
                .border_1()
                .border_color(rgb(colors().edge))
                .bg(rgb(colors().panel))
                .shadow_md()
                .child(
                    toned_button(
                        SharedString::from(format!("toast-open:{}", toast.thread)),
                        "",
                        Tone::Ghost,
                    )
                    .aria_label(format!("Open {}: {}", toast.title, toast.detail))
                    .flex_1()
                    .min_w_0()
                    .h_auto()
                    .py_1()
                    .items_start()
                    .child(dot(color).mt(px(5.)))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(colors().text))
                                    .child(toast.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(rgb(colors().muted))
                                    .child(toast.detail.clone()),
                            ),
                    )
                    .on_activate(cx, move |this, window, cx| {
                        this.open_toast(open.clone(), window, cx)
                    }),
                )
                .child(
                    icon_button(
                        SharedString::from(format!("toast-dismiss:{}", toast.thread)),
                        "close",
                        format!("Dismiss alert for {}", toast.title),
                    )
                    .on_activate(cx, move |this, _, cx| this.dismiss_toast(&close, cx)),
                )
        });
        Some(
            column()
                .id("toasts")
                .absolute()
                .right(px(16.))
                .top(px(TOP))
                .w(px(320.))
                .max_w(relative(0.9))
                .gap_2()
                .children(cards)
                .into_any_element(),
        )
    }
}
