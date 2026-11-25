//! Checkout queue controls use the shared catalog; they own no watch or timer.
use super::{Plait, theme::*, thread_controls::TextTooltip};
use gpui::*;
use plait_client::host::{Request, checkout_queue};
impl Plait {
    pub(super) fn wait_in_checkout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.admission.queue || !self.can_submit() {
            return;
        }
        let Some(id) = self.host.thread.selected.clone() else {
            return;
        };
        let Some(composer) = self.workspace.composers.get(&id) else {
            return;
        };
        let composer = composer.read(cx);
        let (text, revision, _) = composer.saved_draft();
        let images = composer.saved_images().to_vec();
        match self.host.submissions.begin_images(
            &id,
            text.into(),
            self.next_approval_mode(),
            None,
            Some(revision),
            images,
        ) {
            Ok(pending) => {
                self.host
                    .submissions
                    .wait_for_checkout(&id, &pending.request_id);
                self.retry_submission_id(&id, &pending.request_id, window, cx);
            }
            Err(error) => self.set_thread_notice(id, error, true),
        }
    }
    pub(super) fn checkout_wait_control(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.host.thread.selected.as_ref()?;
        let thread = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| &thread.id == id)?;
        let queue = thread.checkout_queue.as_ref()?;
        let cancel = Request::CancelCheckoutWait {
            thread_id: id.clone(),
            request_id: queue.request_id.clone(),
        };
        let resume = Request::ResumeCheckoutQueue {
            thread_id: id.clone(),
        };
        let detail = SharedString::from(queue.detail().to_owned());
        let enabled = self.admission.queue && self.host.connection.connected;
        let mut controls = row()
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .gap_2()
            .child(
                // A state, not a control: drawn as a status with its place in line on hover.
                div()
                    .id("checkout-wait-state")
                    .role(Role::Status)
                    .aria_label(queue.label())
                    .aria_description(detail.clone())
                    .audited()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(rgb(if queue.state == checkout_queue::State::Held {
                        colors().amber
                    } else {
                        colors().blue
                    }))
                    .child(queue.label())
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.to_string())).into()),
            );
        if queue.state == checkout_queue::State::Held {
            controls = controls.child(
                button("checkout-wait-resume", "Resume")
                    .disabled(!enabled)
                    .aria_description(
                        "Resume this checkout's held Wait messages in their original order.",
                    )
                    .tooltip(|_, cx| cx.new(|_| TextTooltip("Resume this checkout's held Wait messages in their original order.".into())).into())
                    .on_activate(cx, move |this, window, cx| {
                        this.change_checkout_wait(resume.clone(), window, cx)
                    }),
            );
        }
        Some(
            controls
                .child(
                    button("checkout-wait-cancel", "Cancel")
                        .disabled(!enabled)
                        .aria_description(
                            "Cancel this waiting message. If it just started, stop its run.",
                        )
                        .tooltip(|_, cx| cx.new(|_| TextTooltip("Cancel this waiting message. If it just started, stop its run.".into())).into())
                        .on_activate(cx, move |this, window, cx| {
                            this.change_checkout_wait(cancel.clone(), window, cx)
                        }),
                )
                .into_any_element(),
        )
    }
    pub(super) fn change_checkout_wait(
        &mut self,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, cancelled) = match &request {
            Request::CancelCheckoutWait {
                thread_id,
                request_id,
            } => (thread_id.clone(), Some(request_id.clone())),
            Request::ResumeCheckoutQueue { thread_id } => (thread_id.clone(), None),
            _ => return,
        };
        let Some(checkout) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .map(|thread| thread.checkout.clone())
        else {
            return;
        };
        self.host_request::<checkout_queue::Reply>(
            request,
            window,
            cx,
            move |this, result, _, cx| {
                match result {
                    Ok(reply) if reply.verified(&id, &checkout) => {
                        if let Some(request_id) = cancelled {
                            let Some(waiter) = reply.waiters.iter().find(|waiter| waiter.request_id == request_id) else {
                                this.set_thread_notice(id, "Queue change was not confirmed. Reconnect to inspect it.".into(), true);
                                cx.notify();
                                return;
                            };
                            let message = match waiter.state {
                                checkout_queue::State::Started => {
                                    this.host.submissions.forget_accepted(&id, &request_id);
                                    "Started. If this message was still running, it was asked to stop.".into()
                                }
                                checkout_queue::State::Cancelled => format!("Cancelled. Your waiting message will not run.{}", this.restore_unstarted_wait(&id, &request_id, cx)),
                                checkout_queue::State::Failed => format!("Not run. {}{}", waiter.error.as_ref().unwrap().message, this.restore_unstarted_wait(&id, &request_id, cx)),
                                _ => {
                                    this.set_thread_notice(id, "Queue change was not confirmed. Reconnect to inspect it.".into(), true);
                                    cx.notify();
                                    return;
                                }
                            };
                            this.set_thread_notice(id, message, false);
                            this.capture_session(cx);
                        }
                    }
                    Ok(_) => this.set_thread_notice(
                        id,
                        "Queue change was not confirmed. Reconnect to inspect it.".into(),
                        true,
                    ),
                    Err(error) => this.set_thread_notice(id, error.to_string(), true),
                }
                cx.notify();
            },
        );
    }

    /// A Wait message that never started gives its exact text and images back
    /// to an empty draft, or its text to the clipboard. Says where it went.
    fn restore_unstarted_wait(
        &mut self,
        id: &str,
        request_id: &str,
        cx: &mut Context<Self>,
    ) -> &'static str {
        let Some(pending) = self
            .host
            .submissions
            .accepted(id)
            .iter()
            .find(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return "";
        };
        self.host.submissions.forget_accepted(id, request_id);
        let client = self.host_client();
        let restored = self
            .workspace
            .composers
            .get(id)
            .cloned()
            .is_some_and(|composer| {
                composer.update(cx, |composer, cx| {
                    let revision = composer.saved_draft().1;
                    let restored = composer.restore_cancelled(&pending.text, revision, cx);
                    if restored {
                        composer.restore_thread_images_with_client(
                            pending.images.clone(),
                            id.to_owned(),
                            client,
                            cx,
                        );
                    }
                    restored
                })
            });
        self.capture_session(cx);
        if restored {
            " Its text is back in the draft."
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(pending.text));
            " The draft has other text, so its text was copied to the clipboard."
        }
    }
    /// When a thread's queue summary ends, read its receipts once, so a Wait
    /// message that failed or was cancelled while waiting is not lost. Runs on
    /// summary changes and when a thread opens, never per frame.
    pub(super) fn reconcile_checkout_waits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.admission.queue || !self.host.connection.connected {
            return;
        }
        let ended = self
            .host
            .submissions
            .checkout_waits()
            .filter(|id| {
                !self.admission.inspecting.contains(*id)
                    && self.workspace.composers.contains_key(*id)
                    && self
                        .host
                        .connection
                        .threads()
                        .iter()
                        .any(|thread| thread.id == *id && thread.checkout_queue.is_none())
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for id in ended {
            self.inspect_checkout_waits(id, window, cx);
        }
    }
    fn inspect_checkout_waits(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(summary) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .cloned()
        else {
            return;
        };
        // Receipts settle saved Waits only from this machine's verified host,
        // for this folder's sends (#132).
        if !self.may_reconcile(&summary) {
            return;
        }
        let checkout = summary.checkout.clone();
        let asked_host = self.verified_host().cloned();
        self.admission.inspecting.insert(id.clone());
        let request = Request::CheckoutQueue {
            thread_id: id.clone(),
        };
        self.host_request::<checkout_queue::Reply>(
            request,
            window,
            cx,
            move |this, result, _, cx| {
                this.admission.inspecting.remove(&id);
                // An unread or unverified reply keeps the messages; the next summary asks again.
                let Ok(reply) = result else { return };
                if !reply.verified(&id, &checkout) {
                    return;
                }
                // The reply settles nothing if the machine or its host changed meanwhile.
                if this.verified_host().cloned() != asked_host || !this.may_reconcile(&summary) {
                    return;
                }
                let waits = this
                    .host
                    .submissions
                    .accepted(&id)
                    .iter()
                    .filter(|pending| pending.checkout_wait)
                    .map(|pending| pending.request_id.clone())
                    .collect::<Vec<_>>();
                let mut notes = Vec::new();
                for request_id in waits {
                    let receipt = reply.waiters.iter().find(|w| w.request_id == request_id);
                    match receipt.map(|w| (w.state, w.error.as_ref())) {
                        Some((checkout_queue::State::Waiting | checkout_queue::State::Held, _)) => {
                        }
                        Some((checkout_queue::State::Failed, reason)) => notes.push(format!(
                            "Not run. {}{}",
                            reason.map_or("", |reason| reason.message.as_str()),
                            this.restore_unstarted_wait(&id, &request_id, cx)
                        )),
                        Some((checkout_queue::State::Cancelled, _)) => notes.push(format!(
                            "Cancelled. Your waiting message will not run.{}",
                            this.restore_unstarted_wait(&id, &request_id, cx)
                        )),
                        // Started, or older than the receipts the host keeps: it is history now.
                        _ => this.host.submissions.forget_accepted(&id, &request_id),
                    }
                }
                if !notes.is_empty() {
                    this.set_thread_notice(id, notes.join(" "), true);
                }
                this.capture_session(cx);
                cx.notify();
            },
        );
    }
}
