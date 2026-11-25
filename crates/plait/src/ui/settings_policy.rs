//! The open project's command policy, on its Project settings page. The host
//! owns the policy; an unread policy is never called sandboxed.
use super::{Plait, settings, theme::*};
use gpui::{prelude::*, *};
use plait_client::host::{
    policy::{self, CommandPolicy, PolicyReply, Request},
    providers,
};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct PolicyState {
    pub key: Option<(u64, PathBuf)>,
    pub value: Option<CommandPolicy>,
    pub busy: bool,
    pub confirm: bool,
    confirm_focus: Option<FocusHandle>,
    pub note: Option<String>,
}

impl Plait {
    pub(super) fn command_policy(&self) -> Option<CommandPolicy> {
        (self.host.connection.connected
            && self.policy.key.as_ref()
                == Some(&(
                    self.host.connection.epoch,
                    self.host.creation.checkout.clone(),
                )))
        .then_some(self.policy.value)
        .flatten()
    }

    pub(super) fn sync_policy(&mut self, cx: &mut Context<Self>) {
        if !self.host.connection.connected
            || self.state.repository.snapshot().ready().is_none()
            || cfg!(test)
        {
            return;
        }
        let key = (
            self.host.connection.epoch,
            self.host.creation.checkout.clone(),
        );
        if self.policy.key.as_ref() == Some(&key) {
            return;
        }
        self.policy = PolicyState {
            key: Some(key),
            ..Default::default()
        };
        self.request_policy(None, cx);
    }

    pub(super) fn prepare_project_policy(&mut self, cx: &mut Context<Self>) {
        let checkout = self.settings_checkout().to_owned();
        let key = (self.host.connection.epoch, checkout.clone());
        if self.project_policy().key.as_ref() != Some(&key) {
            *self.project_policy_mut() = PolicyState {
                key: Some(key),
                ..Default::default()
            };
            if !cfg!(test) {
                self.request_policy_at(checkout, None, cx);
            }
        }
    }

    fn request_project_policy(&mut self, value: Option<CommandPolicy>, cx: &mut Context<Self>) {
        self.request_policy_at(self.settings_checkout().to_owned(), value, cx);
    }

    fn project_command_policy(&self) -> Option<CommandPolicy> {
        let state = self.project_policy();
        (self.host.connection.connected
            && state.key.as_ref()
                == Some(&(
                    self.host.connection.epoch,
                    self.settings_checkout().to_owned(),
                )))
        .then_some(state.value)
        .flatten()
    }

    pub(super) fn request_policy(&mut self, value: Option<CommandPolicy>, cx: &mut Context<Self>) {
        self.request_policy_at(self.host.creation.checkout.clone(), value, cx);
    }

    fn request_policy_at(
        &mut self,
        checkout: PathBuf,
        value: Option<CommandPolicy>,
        cx: &mut Context<Self>,
    ) {
        if !self.host.connection.connected {
            return;
        }
        let key = (self.host.connection.epoch, checkout.clone());
        let Some(state) = self.policy_for_mut(&checkout) else {
            return;
        };
        if state.busy {
            return;
        }
        state.busy = true;
        state.confirm = false;
        state.note = None;
        let checkout = if checkout == self.host.creation.checkout {
            self.state
                .repository
                .snapshot()
                .ready()
                .map_or(checkout, |snapshot| snapshot.root.clone())
        } else {
            checkout
        };
        let checkout_generation =
            (key.1 == self.host.creation.checkout).then_some(self.host.creation.generation);
        let expected = self.settings_checkout_key(&key.1).cloned();
        let client = self.host_client();
        let supported = self.host.connection.has_checkout_identity;
        let local = self.host_is_local();
        let task = cx.background_executor().spawn(async move {
            let identity =
                Self::identify_settings_checkout(&client, &checkout, supported, local, expected)?;
            let canonical = identity.root.clone();
            let request = match value {
                Some(policy) => Request::SetCommandPolicy {
                    checkout: canonical.clone(),
                    policy,
                },
                None => Request::CommandPolicy {
                    checkout: canonical.clone(),
                    identity: supported.then(|| plait_client::host::git::WireIdentity {
                        dev: identity.device.to_string(),
                        ino: identity.inode.to_string(),
                    }),
                },
            };
            let reply: PolicyReply = client.send(&request)?;
            if reply.checkout != canonical {
                return Err(plait_client::host::Error::Protocol(
                    "Host returned another checkout's policy.".into(),
                ));
            }
            Self::identify_settings_checkout(
                &client,
                &identity.root,
                supported,
                local,
                Some(identity.clone()),
            )?;
            Ok((reply, identity))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if checkout_generation
                    .is_some_and(|generation| this.host.creation.generation != generation)
                {
                    return;
                }
                let result = result.and_then(|(reply, identity)| {
                    this.remember_settings_checkout_key(&key, identity)?;
                    Ok(reply)
                });
                if this.accept_policy(&key, value, result, cx) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn accept_policy(
        &mut self,
        key: &(u64, PathBuf),
        value: Option<CommandPolicy>,
        result: Result<PolicyReply, plait_client::host::Error>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.host.connection.epoch != key.0 {
            return false;
        }
        let Some(state) = self.policy_for_mut(&key.1) else {
            return false;
        };
        if state.key.as_ref() != Some(key) {
            return false;
        }
        state.busy = false;
        match result {
            Ok(reply) => {
                state.value = Some(reply.policy);
                state.note = value.map(|_| "Saved. Active runs keep their original policy.".into());
                if value.is_some() {
                    if key.1 == self.host.creation.checkout {
                        self.forget_agent();
                    } else if let Some(agent) = self.agent_for_mut(&key.1) {
                        agent.key = None;
                        agent.revision = agent.revision.wrapping_add(1);
                    }
                    if self.settings_checkout() == key.1 {
                        self.refresh_project_agent(cx);
                    }
                }
            }
            Err(error) => {
                state.note = Some(if providers::needs_host_update(&error) {
                    "Update the host to manage command policy.".into()
                } else {
                    providers::refusal(&error)
                })
            }
        }
        true
    }

    pub(super) fn policy_settings(&self, cx: &mut Context<Self>) -> Div {
        let current = self.project_command_policy();
        let enabled = self.host.connection.connected && !self.project_policy().busy;
        let mut choices = segmented();
        for (index, (label, policy)) in [
            ("Sandboxed", CommandPolicy::Sandboxed),
            ("Trusted", CommandPolicy::Trusted),
        ]
        .into_iter()
        .enumerate()
        {
            choices = choices.child(
                segment(("command-policy", index), label, current == Some(policy))
                    .disabled(!enabled)
                    .when(enabled, |el| {
                        el.on_activate(cx, move |this, window, cx| {
                            if policy == CommandPolicy::Trusted
                                && this.project_command_policy() != Some(policy)
                            {
                                this.project_policy_mut().confirm = true;
                                let focus = cx.focus_handle().tab_stop(true);
                                window.focus(&focus, cx);
                                this.project_policy_mut().confirm_focus = Some(focus);
                                cx.notify();
                            } else {
                                this.request_project_policy(Some(policy), cx);
                            }
                        })
                    }),
            );
        }
        // The page's heading names the checkout this applies to.
        settings::section("Commands")
            .when(!self.project_policy().confirm, |el| {
                el.child(settings::setting(
                    "Commands in this project",
                    current.map_or(
                        "Policy has not loaded from the host.",
                        CommandPolicy::meaning,
                    ),
                    choices,
                ))
            })
            .when(self.project_policy().busy, |el| {
                el.child(tag(
                    "policy-loading",
                    "Loading command policy…",
                    colors().muted,
                ))
            })
            .when_some(self.project_policy().note.clone(), |el, note| {
                el.child(tag("policy-note", note, colors().amber))
            })
            .when(current.is_none() && enabled, |el| {
                el.child(
                    button("retry-policy", "Retry")
                        .on_activate(cx, |this, _, cx| this.request_project_policy(None, cx)),
                )
            })
            .when(self.project_policy().confirm, |el| {
                el.child(
                    column()
                        .gap_2()
                        .p_3()
                        .border_1()
                        .border_color(rgb(colors().amber))
                        .rounded_md()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Trust commands in this checkout?"),
                        )
                        .children(policy::TRUSTED_CONSEQUENCES.iter().enumerate().map(
                            |(index, line)| {
                                tag(("trusted-consequence", index), *line, colors().muted)
                                    .line_height(px(18.))
                            },
                        ))
                        .child(tag("trusted-scope", policy::TRUSTED_SCOPE, colors().subtle))
                        .child(
                            row()
                                .gap_2()
                                .child(
                                    button("cancel-trusted", "Keep current policy")
                                        .when_some(
                                            self.project_policy().confirm_focus.as_ref(),
                                            |el, focus| el.track_focus(focus),
                                        )
                                        .on_activate(cx, |this, window, cx| {
                                            this.project_policy_mut().confirm = false;
                                            window.focus(&this.settings_focus, cx);
                                            cx.notify();
                                        }),
                                )
                                .child(
                                    toned_button(
                                        "confirm-trusted",
                                        "Trust this checkout",
                                        Tone::Filled,
                                    )
                                    .on_activate(
                                        cx,
                                        |this, window, cx| {
                                            this.request_project_policy(
                                                Some(CommandPolicy::Trusted),
                                                cx,
                                            );
                                            window.focus(&this.settings_focus, cx);
                                        },
                                    ),
                                ),
                        ),
                )
            })
    }
}
