//! One checkout-scoped control. Disk and socket work stay off render.
use super::{Plait, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{host, ownership::CheckoutKey};
use plait_protocol::lease::cleanup::Cleanup;
use std::{path::PathBuf, time::Duration};

type Key = (u64, u64, PathBuf);
#[derive(Default)]
pub(super) struct CleanupState {
    pub key: Option<Key>,
    pub value: Option<Cleanup>,
    pub identity: Option<CheckoutKey>,
    pub busy: bool,
    pub confirm: bool,
    pub note: Option<String>,
}
impl Plait {
    fn cleanup_key(&self) -> Key {
        (
            self.host.connection.epoch,
            self.host.creation.generation,
            self.host.creation.checkout.clone(),
        )
    }
    pub(super) fn sync_cleanup(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) || !self.host.connection.connected {
            return;
        }
        let key = self.cleanup_key();
        if self.cleanup.key.as_ref() != Some(&key) {
            self.cleanup = CleanupState {
                key: Some(key),
                ..Default::default()
            };
            self.refresh_cleanup(cx);
        }
    }
    pub(super) fn refresh_cleanup(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        self.request_cleanup(None, cx);
    }
    fn request_cleanup(&mut self, confirm: Option<(String, bool)>, cx: &mut Context<Self>) {
        if !self.host.connection.connected || self.cleanup.busy {
            return;
        }
        let key = self.cleanup_key();
        self.cleanup.key = Some(key.clone());
        self.cleanup.busy = true;
        let root = key.2.clone();
        let expected = confirm.as_ref().and(self.cleanup.identity.clone());
        let client = self.host_client();
        let supported = self.host.connection.has_checkout_identity;
        let local = self.host_is_local();
        let task = cx.background_executor().spawn(async move {
            let identity = match expected {
                Some(value) => value,
                None if !supported => {
                    Self::identify_settings_checkout(&client, &root, supported, local, None)?
                }
                None => {
                    let read = host::git::identity(&client, &root, None)?;
                    CheckoutKey {
                        host: None,
                        root: read.result.root,
                        device: read.identity.dev,
                        inode: read.identity.ino,
                    }
                }
            };
            let value = host::cleanup::read(
                &client,
                &identity,
                confirm
                    .as_ref()
                    .map(|(token, stop)| (token.as_str(), *stop)),
                supported,
                local,
            )?;
            Ok::<_, host::Error>((identity, value))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let mut poll = false;
            let _ = this.update(cx, |this, cx| {
                if this.cleanup_key() != key || this.cleanup.key.as_ref() != Some(&key) {
                    return;
                }
                this.cleanup.busy = false;
                match result {
                    Ok((identity, value)) => {
                        // A refreshed token disarms an older confirmation.
                        if this.cleanup.value != value {
                            this.cleanup.confirm = false;
                        }
                        this.cleanup.identity = Some(identity);
                        this.cleanup.value = value;
                        this.cleanup.note = None;
                    }
                    Err(error) => this.cleanup.note = Some(error.to_string()),
                }
                poll = this.cleanup.value.is_some();
                cx.notify();
            });
            if poll {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.cleanup_key() == key {
                        this.refresh_cleanup(cx);
                    }
                });
            }
        })
        .detach();
    }
    pub(super) fn cleanup_control(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.cleanup.key.as_ref() != Some(&self.cleanup_key()) {
            return None;
        }
        let state = self.cleanup.value.as_ref()?;
        let token = state.token.clone();
        let stop = state
            .process
            .as_ref()
            .is_some_and(|process| process.can_stop);
        let action = if stop {
            format!(
                "Stop {} and its child processes in this checkout (process group {}).",
                state.process.as_ref().unwrap().command,
                state.process.as_ref().unwrap().group
            )
        } else {
            "Check for commands, servers, or builds still using this folder. Confirm only after they have stopped.".into()
        };
        let tip = format!(
            "{}\nCheckout: {}\n{}{}",
            state.reason,
            self.host.creation.checkout.display(),
            action,
            self.cleanup
                .note
                .as_ref()
                .map(|note| format!("\n{note}"))
                .unwrap_or_default()
        );
        Some(
            button(
                "checkout-cleanup",
                if self.cleanup.confirm {
                    if stop {
                        "Stop processes"
                    } else {
                        "Confirm stopped"
                    }
                } else {
                    "Stop unconfirmed"
                },
            )
            .text_color(rgb(colors().amber))
            .disabled(self.cleanup.busy || !self.host.connection.connected)
            .aria_description(tip.clone())
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(tip.clone())).into())
            .on_activate(cx, move |this, _, cx| {
                if this.cleanup.confirm {
                    this.request_cleanup(Some((token.clone(), stop)), cx);
                } else {
                    this.cleanup.confirm = true;
                    cx.notify();
                }
            })
            .into_any_element(),
        )
    }
}
