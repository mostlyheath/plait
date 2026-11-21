use super::*;

/// Clears the tab row and the workspace header.
const TOP: f32 = 84.;

impl Plait {
    /// Whether a toast would be seen and usable. Settings and an enlarged image
    /// cover the workspace, so alerts then go to the system instead.
    pub(super) fn toasts_visible(&self) -> bool {
        !self.settings_open && self.workspace.images.enlarged.is_none()
    }

    /// Shows an in-app alert for another thread.
    pub(super) fn show_toast(
        &mut self,
        id: &str,
        title: String,
        detail: String,
        cx: &mut Context<Self>,
    ) {
        self.toasts.push(id, title, detail, Instant::now());
        self.schedule_toast_expiry(cx);
        cx.notify();
    }

    /// One timer for the earliest expiry. It reschedules itself rather than
    /// trusting a single wake to land after every toast's deadline.
    fn schedule_toast_expiry(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.toasts.next_expiry() else {
            self.toast_timer = None;
            return;
        };
        let timer = cx
            .background_executor()
            .timer(at.saturating_duration_since(Instant::now()));
        self.toast_timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.toasts.expire(Instant::now()) {
                    cx.notify();
                }
                this.schedule_toast_expiry(cx);
            });
        }));
    }

    pub(super) fn dismiss_toast(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.toasts.dismiss(id) {
            self.schedule_toast_expiry(cx);
            cx.notify();
        }
    }

    /// Drops toasts for threads that are gone or archived, which never alert.
    pub(super) fn retain_toasts(&mut self, cx: &mut Context<Self>) {
        let threads = &self.host.connection.threads;
        let before = self.toasts.items().len();
        self.toasts.retain_threads(|id| {
            threads
                .iter()
                .any(|thread| thread.id == id && thread.archived != Some(true))
        });
        if self.toasts.items().len() != before {
            self.schedule_toast_expiry(cx);
            cx.notify();
        }
    }

    pub(super) fn clear_toasts(&mut self) {
        self.toasts = Default::default();
        self.toast_timer = None;
    }

    /// Opens the thread's conversation, whichever view its tab last showed.
    fn open_toast(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_toast(&id, cx);
        if !self
            .host
            .connection
            .threads
            .iter()
            .any(|thread| thread.id == id)
        {
            return;
        }
        self.select_host_thread(id.clone(), window, cx);
        if self.host.thread.selected.as_deref() == Some(id.as_str()) {
            self.show_tab(Workspace::Conversation, window, cx);
        }
        cx.notify();
    }

    /// The stack in the window's upper right, below the header, newest at the
    /// bottom. Lower down it would cover the composer's Send and Stop.
    pub(super) fn toast_stack(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cards =
            self.toasts
                .items()
                .iter()
                .filter_map(|toast| {
                    // Render only live threads; `retain_toasts` removes the rest.
                    let thread = self.host.connection.threads.iter().find(|thread| {
                        thread.id == toast.thread && thread.archived != Some(true)
                    })?;
                    Some((toast, host_status_color(thread.status)))
                })
                .map(|(toast, color)| {
                    let open = toast.thread.clone();
                    let close = toast.thread.clone();
                    row()
                        .id(SharedString::from(format!("toast:{}", toast.thread)))
                        .role(Role::Status)
                        .aria_label(format!("{}: {}", toast.title, toast.detail))
                        .audited()
                        // Clicks and scrolls on the card stay on the card.
                        .occlude()
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
                })
                .collect::<Vec<_>>();
        if cards.is_empty() {
            return None;
        }
        Some(
            column()
                .id("toasts")
                .absolute()
                .right(px(16.))
                .top(px(TOP))
                .w(px(300.))
                .max_w(relative(0.9))
                .gap_2()
                .children(cards)
                .into_any_element(),
        )
    }
}
