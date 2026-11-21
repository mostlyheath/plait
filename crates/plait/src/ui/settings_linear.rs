//! Settings → Linear: the host's Linear sign-in. The key goes to the host once;
//! the field is cleared as it is sent, and nothing here keeps or shows it.
use super::{Plait, settings, text_field::TextField, theme::*};
use gpui::{prelude::*, *};
use plait_core::{
    host::{self, linear},
    linear_settings::{Account, Auth, Pending},
};

#[derive(Default)]
pub(super) struct LinearState {
    pub account: Account,
    pub field: Option<Entity<TextField>>,
}

/// A status reply, or the refusal already classified on the background thread.
type Reply = Result<linear::AuthStatus, linear::Rejection>;

impl Plait {
    /// Runs one Linear call on the background executor. A reply that lands
    /// after a reconnect is dropped here; `Account::reply` drops the rest.
    fn linear_call(
        &mut self,
        generation: u64,
        call: impl FnOnce(&host::Client) -> Reply + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let epoch = self.host.connection.epoch;
        let task = cx.background_executor().spawn(async move {
            match host::Client::from_environment() {
                Ok(client) => call(&client),
                Err(error) => Err(linear::classify(&error, false)),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch == epoch
                    && this.linear.account.reply(generation, result)
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Keeps the account in step with the connection, and loads it the first
    /// time the Linear tab is shown. The key field never outlives Settings.
    pub(super) fn sync_linear_settings(&mut self, cx: &mut Context<Self>) {
        let showing = self.settings_open && self.settings_tab == settings::SettingsTab::Linear;
        if self.linear.account.connect(self.host.connection.epoch) || !showing {
            self.clear_linear_field(cx);
        }
        if showing && self.linear.field.is_none() {
            let input = cx.new(|cx| TextField::new("Linear API key", "lin_api_…", cx).secret());
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            self.linear.field = Some(input);
        }
        // Test scenes inject account state and never reach a host.
        if cfg!(test) || !showing || !self.host.connection.connected {
            return;
        }
        if self.linear.account.needs_load() {
            let generation = self.linear.account.start_load();
            // An older host refuses linear_info, the capability probe.
            self.linear_call(
                generation,
                |client| {
                    linear::info(client).map_err(|error| linear::classify(&error, true))?;
                    linear::auth_status(client).map_err(|error| linear::classify(&error, false))
                },
                cx,
            );
        }
    }

    /// Sync runs on every notify, so clearing an empty field must not notify
    /// again: setting its text would re-run sync and loop.
    fn clear_linear_field(&mut self, cx: &mut Context<Self>) {
        if let Some(field) = &self.linear.field
            && !field.read(cx).text().is_empty()
        {
            field.update(cx, |field, cx| field.set_text("", cx));
        }
    }

    pub(super) fn save_linear_key(&mut self, cx: &mut Context<Self>) {
        let Some(field) = self.linear.field.clone() else {
            return;
        };
        let key = field.read(cx).text().trim().to_owned();
        let Some(generation) = self.linear.account.start_save(&key) else {
            return;
        };
        // Cleared as it is sent: a refusal asks for the key again.
        field.update(cx, |field, cx| field.set_text("", cx));
        if cfg!(test) {
            return;
        }
        self.linear_call(
            generation,
            move |client| {
                linear::set_key(client, key).map_err(|error| linear::classify(&error, false))
            },
            cx,
        );
        cx.notify();
    }

    pub(super) fn sign_out_of_linear(&mut self, cx: &mut Context<Self>) {
        let Some(generation) = self.linear.account.start_sign_out() else {
            return;
        };
        if !cfg!(test) {
            self.linear_call(
                generation,
                |client| linear::sign_out(client).map_err(|error| linear::classify(&error, false)),
                cx,
            );
        }
        cx.notify();
    }

    fn linear_key_form(&self, replacing: bool, cx: &mut Context<Self>) -> Div {
        let Some(field) = self.linear.field.clone() else {
            return div();
        };
        let busy = self.linear.account.pending != Pending::None;
        let ready =
            self.host.connection.connected && !busy && !field.read(cx).text().trim().is_empty();
        let reveal = field.read(cx).revealed();
        let toggle = field.clone();
        column()
            .gap_2()
            .child(
                row()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(field))
                    .child(
                        button("reveal-linear-key", if reveal { "Hide" } else { "Reveal" })
                            .on_activate(cx, move |_, _, cx| {
                                toggle.update(cx, |field, cx| field.reveal(cx))
                            }),
                    ),
            )
            .child(
                row()
                    .gap_2()
                    .child(
                        toned_button("save-linear-key", "Save key", Tone::Primary)
                            .disabled(!ready)
                            .when(ready, |el| {
                                el.on_activate(cx, |this, _, cx| this.save_linear_key(cx))
                            }),
                    )
                    .when(replacing, |el| {
                        el.child(
                            button("cancel-linear-key", "Cancel")
                                .disabled(busy)
                                .on_activate(cx, |this, _, cx| {
                                    this.linear.account.replace_key(false);
                                    this.clear_linear_field(cx);
                                    cx.notify();
                                }),
                        )
                    }),
            )
    }

    fn linear_signed_in(&self, workspace: &str, viewer: &str, cx: &mut Context<Self>) -> Div {
        let account = &self.linear.account;
        let enabled = self.host.connection.connected && account.pending == Pending::None;
        let summary = tag(
            "linear-signed-in",
            format!("Signed in to {workspace} as {viewer}."),
            colors().text,
        );
        if account.confirm_sign_out {
            return column()
                .gap_2()
                .child(summary)
                .child(tag(
                    "linear-sign-out-confirm",
                    "Sign out removes the saved key, cached issues, and project bindings. Thread links are kept.",
                    colors().amber,
                ).line_height(px(18.)))
                .child(
                    row()
                        .gap_2()
                        .child(
                            toned_button("confirm-linear-sign-out", "Sign out", Tone::Primary)
                                .disabled(!enabled)
                                .when(enabled, |el| {
                                    el.on_activate(cx, |this, _, cx| this.sign_out_of_linear(cx))
                                }),
                        )
                        .child(button("cancel-linear-sign-out", "Cancel").on_activate(
                            cx,
                            |this, _, cx| {
                                this.linear.account.cancel_sign_out();
                                cx.notify();
                            },
                        )),
                );
        }
        if account.replacing {
            return column()
                .gap_2()
                .child(summary)
                .child(self.linear_key_form(true, cx));
        }
        column().gap_2().child(summary).child(
            row()
                .gap_2()
                .child(
                    button("replace-linear-key", "Replace key")
                        .disabled(!enabled)
                        .when(enabled, |el| {
                            el.on_activate(cx, |this, _, cx| {
                                this.linear.account.replace_key(true);
                                cx.notify();
                            })
                        }),
                )
                .child(
                    button("sign-out-of-linear", "Sign out…")
                        .disabled(!enabled)
                        .when(enabled, |el| {
                            el.on_activate(cx, |this, _, cx| {
                                this.linear.account.ask_sign_out();
                                this.clear_linear_field(cx);
                                cx.notify();
                            })
                        }),
                ),
        )
    }

    pub(super) fn linear_settings(&self, cx: &mut Context<Self>) -> Div {
        let account = &self.linear.account;
        let connected = self.host.connection.connected;
        let body = match &account.auth {
            _ if !connected => tag(
                "linear-disconnected",
                "Connect to the host to manage Linear.",
                colors().amber,
            ),
            Auth::Loading => tag("linear-loading", "Checking Linear sign-in…", colors().muted),
            Auth::OlderHost => tag(
                "linear-older-host",
                "Linear needs a newer host. Update Plait's host to use it.",
                colors().amber,
            ),
            Auth::SignedOut => column()
                .gap_2()
                .child(
                    tag(
                        "linear-key-help",
                        "Create a personal API key in Linear under Settings → Security & access. A key with read access is enough to browse issues.",
                        colors().muted,
                    )
                    .line_height(px(18.)),
                )
                .child(self.linear_key_form(false, cx)),
            Auth::KeyRejected => column()
                .gap_2()
                .child(
                    tag(
                        "linear-key-rejected",
                        "Linear no longer accepts the saved key. Enter a new one.",
                        colors().amber,
                    )
                    .line_height(px(18.)),
                )
                .child(self.linear_key_form(false, cx)),
            Auth::SignedIn { workspace, viewer } => {
                self.linear_signed_in(workspace, viewer, cx)
            }
        };
        let progress = match account.pending {
            Pending::Saving => Some("Checking the key with Linear…"),
            Pending::SigningOut => Some("Signing out…"),
            Pending::None | Pending::Loading => None,
        };
        settings::section("Linear")
            .child(
                tag(
                    "linear-intro",
                    "Browse a project's Linear issues and start threads from them. The host keeps the key and makes every Linear request; Plait does not show the key again after you save it.",
                    colors().muted,
                )
                .line_height(px(18.)),
            )
            .child(body)
            .when_some(progress, |el, text| {
                el.child(tag("linear-progress", text, colors().muted))
            })
            .when_some(account.note.clone(), |el, note| {
                let color = if account.failed {
                    colors().amber
                } else {
                    colors().mint
                };
                el.child(tag("linear-note", note, color).line_height(px(18.)))
            })
    }
}
