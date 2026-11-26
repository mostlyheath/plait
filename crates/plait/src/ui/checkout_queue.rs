//! Checkout queue controls use the shared catalog; they own no watch or timer.
use super::{Plait, composer::merge_draft_images, theme::*, thread_controls::TextTooltip};
use gpui::*;
use plait_client::host::{Request, checkout_queue};
use plait_client::images::ImageRef;
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

    /// A Wait message that never started returns its exact text and images to
    /// an empty draft; when the draft already has content, it keeps the draft
    /// intact, copies any text to the clipboard, and attaches any images that
    /// fit within the message limits. Says where they went.
    fn restore_unstarted_wait(
        &mut self,
        id: &str,
        request_id: &str,
        cx: &mut Context<Self>,
    ) -> String {
        let Some(pending) = self
            .host
            .submissions
            .accepted(id)
            .iter()
            .find(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return String::new();
        };
        self.host.submissions.forget_accepted(id, request_id);
        let client = self.host_client();
        let outcome = match self.workspace.composers.get(id).cloned() {
            Some(composer) => composer.update(cx, |composer, cx| {
                let revision = composer.saved_draft().1;
                let draft = if composer.restore_cancelled(&pending.text, revision, cx) {
                    DraftState::Empty
                } else {
                    DraftState::Occupied(composer.saved_images())
                };
                let mut plan = plan_wait_recovery(&pending.text, &pending.images, draft);
                if let Some(references) = plan.references.take() {
                    composer.apply_recovered_images(references, id.to_owned(), client, cx);
                }
                plan
            }),
            None => plan_wait_recovery(&pending.text, &pending.images, DraftState::Unavailable),
        };
        if let Some(text) = outcome.clipboard_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        self.capture_session(cx);
        outcome.notice
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
                this.apply_checkout_receipts(&summary, asked_host.as_ref(), &reply, cx);
            },
        );
    }

    /// Settle this thread's saved Waits from a verified receipt list. Only a
    /// Wait accepted by the host verified now, the same one the request was
    /// asked of, is settled; any other is kept as it is (#132).
    pub(super) fn apply_checkout_receipts(
        &mut self,
        summary: &plait_client::host::ThreadSummary,
        asked_host: Option<&plait_client::tabs::HostKey>,
        reply: &checkout_queue::Reply,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.verified_host().cloned() else {
            return;
        };
        if asked_host != Some(&host) || !self.may_reconcile(summary) {
            return;
        }
        let id = summary.id.clone();
        let waits = self
            .host
            .submissions
            .accepted(&id)
            .iter()
            .filter(|pending| pending.checkout_wait)
            .filter(|pending| self.saved_work.accepted_by.get(&pending.request_id) == Some(&host))
            .map(|pending| pending.request_id.clone())
            .collect::<Vec<_>>();
        let mut notes = Vec::new();
        for request_id in waits {
            let receipt = reply.waiters.iter().find(|w| w.request_id == request_id);
            match receipt.map(|w| (w.state, w.error.as_ref())) {
                Some((checkout_queue::State::Waiting | checkout_queue::State::Held, _)) => {}
                Some((checkout_queue::State::Failed, reason)) => notes.push(format!(
                    "Not run. {}{}",
                    reason.map_or("", |reason| reason.message.as_str()),
                    self.restore_unstarted_wait(&id, &request_id, cx)
                )),
                Some((checkout_queue::State::Cancelled, _)) => notes.push(format!(
                    "Cancelled. Your waiting message will not run.{}",
                    self.restore_unstarted_wait(&id, &request_id, cx)
                )),
                // Started, or older than the receipts the host keeps: it is history now.
                _ => self.host.submissions.forget_accepted(&id, &request_id),
            }
        }
        if !notes.is_empty() {
            self.set_thread_notice(id, notes.join(" "), true);
        }
        self.capture_session(cx);
        cx.notify();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DraftState<'a> {
    Unavailable,
    Empty,
    Occupied(&'a [ImageRef]),
}

#[derive(Debug, PartialEq, Eq)]
struct WaitRecovery {
    clipboard_text: Option<String>,
    references: Option<Vec<ImageRef>>,
    notice: String,
}

fn image_noun(count: usize) -> &'static str {
    if count == 1 { "image" } else { "images" }
}

fn plan_wait_recovery(text: &str, images: &[ImageRef], draft: DraftState<'_>) -> WaitRecovery {
    let has_text = !text.trim().is_empty();
    let has_images = !images.is_empty();
    match draft {
        DraftState::Empty => {
            let notice = match (has_text, has_images) {
                (true, false) => " Its text is back in the draft.".into(),
                (true, true) => format!(
                    " Its text and {} are back in the draft.",
                    image_noun(images.len())
                ),
                (false, true) if images.len() == 1 => " Its image is back in the draft.".into(),
                (false, true) => " Its images are back in the draft.".into(),
                (false, false) => String::new(),
            };
            WaitRecovery {
                clipboard_text: None,
                references: has_images.then(|| images.to_vec()),
                notice,
            }
        }
        DraftState::Occupied(existing) => {
            let merged = merge_draft_images(existing, images);
            let clipboard_text = has_text.then(|| text.to_owned());
            let overflow = merged.overflow;
            let in_draft = merged.added + merged.already_attached;
            let notice = occupied_notice(has_text, in_draft, overflow);
            WaitRecovery {
                clipboard_text,
                references: (merged.added > 0).then_some(merged.references),
                notice,
            }
        }
        DraftState::Unavailable => {
            let clipboard_text = has_text.then(|| text.to_owned());
            let notice = match (has_text, has_images) {
                (true, false) => " Its text was copied to the clipboard.".into(),
                (true, true) => format!(
                    " Its text was copied to the clipboard, but its {} could not be attached because the draft is closed.",
                    image_noun(images.len())
                ),
                (false, true) => format!(
                    " Its {} could not be attached because the draft is closed.",
                    image_noun(images.len())
                ),
                (false, false) => String::new(),
            };
            WaitRecovery {
                clipboard_text,
                references: None,
                notice,
            }
        }
    }
}

fn occupied_notice(has_text: bool, in_draft: usize, overflow: usize) -> String {
    let mut notice = String::new();
    if has_text {
        notice.push_str(" Its text was copied to the clipboard.");
    }
    match in_draft {
        0 => {}
        1 => notice.push_str(" 1 recovered image is in the draft."),
        n => notice.push_str(&format!(" {n} recovered images are in the draft.")),
    }
    if overflow > 0 {
        notice.push_str(&format!(
            " {overflow} {} could not be attached because of the 4-image / 5 MiB limit.",
            image_noun(overflow)
        ));
    }
    notice
}

#[cfg(test)]
mod tests {
    use super::{DraftState, ImageRef, merge_draft_images, plan_wait_recovery};
    use plait_client::images::MESSAGE_BYTES;

    fn image(tag: char, size: usize) -> ImageRef {
        ImageRef {
            id: std::iter::repeat_n(tag, 64).collect(),
            media_type: "image/png".into(),
            size,
            width: 100,
            height: 100,
        }
    }

    #[test]
    fn image_only_wait_attaches_to_nonempty_draft_without_touching_clipboard() {
        let shot = image('a', 1024);
        let plan = plan_wait_recovery("", std::slice::from_ref(&shot), DraftState::Occupied(&[]));
        assert_eq!(plan.clipboard_text, None);
        assert!(plan.references.is_some());
        assert_eq!(plan.notice, " 1 recovered image is in the draft.");

        let blank_text = plan_wait_recovery(
            "   ",
            std::slice::from_ref(&shot),
            DraftState::Occupied(&[]),
        );
        assert_eq!(blank_text, plan);
    }

    #[test]
    fn text_and_images_wait_copies_text_and_attaches_images_to_nonempty_draft() {
        let images = [image('a', 1024), image('b', 2048)];
        let plan = plan_wait_recovery("Check these shots", &images, DraftState::Occupied(&[]));
        assert_eq!(plan.clipboard_text.as_deref(), Some("Check these shots"));
        assert!(plan.references.is_some());
        assert_eq!(
            plan.notice,
            " Its text was copied to the clipboard. 2 recovered images are in the draft."
        );

        let text_only = plan_wait_recovery("Just text", &[], DraftState::Occupied(&[]));
        assert_eq!(text_only.clipboard_text.as_deref(), Some("Just text"));
        assert!(text_only.references.is_none());
        assert_eq!(text_only.notice, " Its text was copied to the clipboard.");
    }

    #[test]
    fn empty_draft_restores_text_images_or_both_without_clipboard() {
        let shot = image('a', 1024);
        let text_only = plan_wait_recovery("Run this", &[], DraftState::Empty);
        assert_eq!(text_only.clipboard_text, None);
        assert!(text_only.references.is_none());
        assert_eq!(text_only.notice, " Its text is back in the draft.");

        let image_only = plan_wait_recovery("", std::slice::from_ref(&shot), DraftState::Empty);
        assert_eq!(image_only.clipboard_text, None);
        assert!(image_only.references.is_some());
        assert_eq!(image_only.notice, " Its image is back in the draft.");

        let both = plan_wait_recovery("Run this", std::slice::from_ref(&shot), DraftState::Empty);
        assert_eq!(both.clipboard_text, None);
        assert!(both.references.is_some());
        assert_eq!(both.notice, " Its text and image are back in the draft.");
    }

    #[test]
    fn duplicate_and_limit_overflow_images_respect_message_bounds() {
        let a = image('a', 1024);
        let b = image('b', 1024);
        let c = image('c', 1024);
        let d = image('d', 1024);
        let e = image('e', 1024);

        let dup = plan_wait_recovery(
            "",
            std::slice::from_ref(&a),
            DraftState::Occupied(std::slice::from_ref(&a)),
        );
        assert_eq!(dup.clipboard_text, None);
        assert!(dup.references.is_none());
        assert_eq!(dup.notice, " 1 recovered image is in the draft.");

        let existing_three = [a.clone(), b.clone(), c.clone()];
        let incoming_two = [d.clone(), e.clone()];
        let merged = merge_draft_images(&existing_three, &incoming_two);
        assert_eq!(
            merged.references,
            vec![a.clone(), b.clone(), c.clone(), d.clone()]
        );
        assert_eq!(merged.added, 1);
        assert_eq!(merged.already_attached, 0);
        assert_eq!(merged.overflow, 1);

        let partial = plan_wait_recovery(
            "Prompt",
            &incoming_two,
            DraftState::Occupied(&existing_three),
        );
        assert_eq!(partial.clipboard_text.as_deref(), Some("Prompt"));
        assert!(partial.references.is_some());
        assert_eq!(
            partial.notice,
            " Its text was copied to the clipboard. 1 recovered image is in the draft. 1 image could not be attached because of the 4-image / 5 MiB limit."
        );

        let existing_four = [a, b, c, d];
        let full = plan_wait_recovery(
            "",
            std::slice::from_ref(&e),
            DraftState::Occupied(&existing_four),
        );
        assert_eq!(full.clipboard_text, None);
        assert!(full.references.is_none());
        assert_eq!(
            full.notice,
            " 1 image could not be attached because of the 4-image / 5 MiB limit."
        );

        let big_existing = [image('1', MESSAGE_BYTES - 1024)];
        let too_big = image('2', 2048);
        let small_fit = image('3', 512);
        let by_size = merge_draft_images(&big_existing, &[too_big, small_fit.clone()]);
        assert_eq!(by_size.references, vec![big_existing[0].clone(), small_fit]);
        assert_eq!(by_size.added, 1);
        assert_eq!(by_size.overflow, 1);
    }

    #[test]
    fn unavailable_draft_copies_text_and_reports_closed_draft() {
        let shot = image('a', 1024);
        let both = plan_wait_recovery(
            "Saved prompt",
            std::slice::from_ref(&shot),
            DraftState::Unavailable,
        );
        assert_eq!(both.clipboard_text.as_deref(), Some("Saved prompt"));
        assert!(both.references.is_none());
        assert!(both.notice.contains("copied to the clipboard"));
        assert!(both.notice.contains("draft is closed"));

        let image_only =
            plan_wait_recovery("", std::slice::from_ref(&shot), DraftState::Unavailable);
        assert_eq!(image_only.clipboard_text, None);
        assert!(image_only.references.is_none());
        assert!(image_only.notice.contains("draft is closed"));
    }
}
