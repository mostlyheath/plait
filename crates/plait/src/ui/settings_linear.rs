//! Settings → Linear: the host's Linear sign-in. The key goes to the host once;
//! the field is cleared as it is sent, and nothing here keeps or shows it.
use super::{Plait, settings, text_field::TextField, theme::*};
use gpui::{prelude::*, *};
use plait_core::{
    host::{self, linear},
    linear_settings::{Account, Auth, Pending, ProjectTeam, TeamPending},
};

#[derive(Default)]
pub(super) struct LinearState {
    pub account: Account,
    pub field: Option<Entity<TextField>>,
    /// The Project settings page's Linear team.
    pub team: ProjectTeam,
}

/// A status reply, or the refusal already classified on the background thread.
type Reply = Result<linear::AuthStatus, linear::Rejection>;

impl Plait {
    /// Runs one Linear call on the background executor and hands its result
    /// to `apply`, which returns whether it was current. A reply that lands
    /// after a reconnect is dropped here; the models drop the rest.
    pub(super) fn linear_task<T: Send + 'static>(
        &mut self,
        call: impl FnOnce(&host::Client) -> Result<T, linear::Rejection> + Send + 'static,
        apply: impl FnOnce(&mut Self, Result<T, linear::Rejection>, &mut Context<Self>) -> bool
        + 'static,
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
                if this.host.connection.epoch == epoch && apply(this, result, cx) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn linear_call(
        &mut self,
        generation: u64,
        call: impl FnOnce(&host::Client) -> Reply + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.linear_task(
            call,
            move |this, result, _| this.linear.account.reply(generation, result),
            cx,
        );
    }

    fn linear_team_page(&self) -> bool {
        self.settings_open && self.settings_page == settings::SettingsPage::Project
    }

    /// Keeps the account in step with the connection, and loads it the first
    /// time the Linear tab is shown. The key field never outlives Settings.
    pub(super) fn sync_linear_settings(&mut self, cx: &mut Context<Self>) {
        let showing = self.settings_open
            && self.settings_page == settings::SettingsPage::Application
            && self.settings_tab == settings::SettingsTab::Linear;
        let project = self.linear_team_page();
        if self.linear.account.connect(self.host.connection.epoch) || !showing {
            self.clear_linear_field(cx);
        }
        if showing && self.linear.field.is_none() {
            let input = cx.new(|cx| TextField::new("Linear API key", "lin_api_…", cx).secret());
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            self.linear.field = Some(input);
        }
        if project {
            let checkout = self.settings_checkout().to_owned();
            self.linear
                .team
                .target(self.host.connection.epoch, &checkout);
        }
        // Test scenes inject state and never reach a host.
        // The account loads on every connection: issue labels on threads and
        // the Issues tab need to know who is signed in. It is a local read.
        if cfg!(test) || !self.host.connection.connected {
            return;
        }
        if project
            && matches!(self.linear.account.auth, Auth::SignedIn { .. })
            && self.linear.team.needs_load()
        {
            let generation = self.linear.team.start_load();
            let checkout = self.settings_checkout().to_owned();
            self.linear_task(
                move |client| {
                    let classify = |error: host::Error| linear::classify(&error, false);
                    let binding =
                        linear::get_project_binding(client, checkout).map_err(classify)?;
                    let teams = linear::list_teams(client, false).map_err(classify)?;
                    Ok((binding, teams))
                },
                move |this, result, _| this.linear.team.loaded(generation, result),
                cx,
            );
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

    pub(super) fn bind_linear_team(&mut self, team: Option<linear::Team>, cx: &mut Context<Self>) {
        let Some(generation) = self
            .linear
            .team
            .start_bind(team.as_ref().map(|team| team.id.as_str()))
        else {
            return;
        };
        cx.notify();
        if cfg!(test) {
            return;
        }
        let checkout = self.settings_checkout().to_owned();
        self.linear_task(
            move |client| {
                let (id, key, name) = match team {
                    Some(team) => (Some(team.id), Some(team.key), Some(team.name)),
                    None => (None, None, None),
                };
                linear::bind_project(client, checkout, id, key, name)
                    .map_err(|error| linear::classify(&error, false))
            },
            move |this, result, _| this.linear.team.bound(generation, result),
            cx,
        );
    }

    fn refresh_linear_teams(&mut self, cx: &mut Context<Self>) {
        let Some(generation) = self.linear.team.start_refresh() else {
            return;
        };
        cx.notify();
        if cfg!(test) {
            return;
        }
        self.linear_task(
            |client| {
                linear::list_teams(client, true).map_err(|error| linear::classify(&error, false))
            },
            move |this, result, _| this.linear.team.refreshed(generation, result),
            cx,
        );
    }

    fn open_linear_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_page(Some(settings::SettingsPage::Application), window, cx);
        self.settings_tab = settings::SettingsTab::Linear;
        cx.notify();
    }

    /// Project settings → Linear: which team's issues this project shows.
    pub(super) fn project_linear_settings(&self, cx: &mut Context<Self>) -> Div {
        let section = settings::section("Linear");
        let connected = self.host.connection.connected;
        match &self.linear.account.auth {
            Auth::OlderHost => {
                return section.child(tag(
                    "project-linear-older-host",
                    "Linear needs a newer host.",
                    colors().muted,
                ));
            }
            Auth::Loading => {
                return section.child(tag(
                    "project-linear-loading",
                    if connected {
                        "Checking Linear sign-in…"
                    } else {
                        "Connect to the host to link this project to Linear."
                    },
                    colors().muted,
                ));
            }
            Auth::SignedOut | Auth::KeyRejected => {
                return section
                    .child(tag(
                        "project-linear-signed-out",
                        "Sign in to Linear to show a team's issues for this project.",
                        colors().muted,
                    ))
                    .child(
                        button("project-linear-open-account", "Linear settings…")
                            .on_activate(cx, |this, window, cx| {
                                this.open_linear_account(window, cx)
                            }),
                    );
            }
            Auth::SignedIn { .. } => {}
        }
        let team = &self.linear.team;
        let idle = connected && team.pending == TeamPending::None;
        let bound = team.binding.as_ref().map(|binding| binding.team_id.clone());
        let current = match &team.binding {
            Some(binding) => format!(
                "Issues from {}{}.",
                binding.team_name.as_deref().unwrap_or("the linked team"),
                binding
                    .team_key
                    .as_deref()
                    .map(|key| format!(" ({key})"))
                    .unwrap_or_default()
            ),
            None => "Not linked to a Linear team.".into(),
        };
        let mut rows = column().gap_1();
        for (index, choice) in team.teams.iter().flat_map(|t| t.teams.iter()).enumerate() {
            let selected = bound.as_deref() == Some(choice.id.as_str());
            let pick = choice.clone();
            rows = rows.child(
                toned_button(
                    ("project-linear-team", index),
                    "",
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Ghost
                    },
                )
                .aria_label(format!("{} ({})", choice.name, choice.key))
                .aria_selected(selected)
                .role(Role::RadioButton)
                .w_full()
                .px_2()
                .disabled(!idle)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(choice.name.clone()),
                )
                .child(tag(
                    ("project-linear-team-key", index),
                    choice.key.clone(),
                    colors().subtle,
                ))
                .when(idle && !selected, |el| {
                    el.on_activate(cx, move |this, _, cx| {
                        this.bind_linear_team(Some(pick.clone()), cx)
                    })
                }),
            );
        }
        let notes = team.teams.as_ref().map(|teams| {
            let mut notes = Vec::new();
            if teams.teams_complete == Some(false) {
                notes.push(
                    "Linear lists more teams than this shows; the list may be incomplete."
                        .to_owned(),
                );
            }
            if teams.cache.stale {
                notes.push(format!(
                    "Showing teams saved {} ago; Linear could not be reached.",
                    age(teams.cache.cache_age_seconds)
                ));
            }
            if let Some(warning) = &teams.cache.warning {
                notes.push(warning.clone());
            }
            notes
        });
        let progress = match team.pending {
            TeamPending::Loading => Some("Loading teams…"),
            TeamPending::Saving => Some("Saving the link…"),
            TeamPending::Refreshing => Some("Refreshing teams…"),
            TeamPending::None => None,
        };
        section
            .child(tag("project-linear-current", current, colors().text))
            .when(
                team.teams.as_ref().is_some_and(|t| t.teams.is_empty()),
                |el| {
                    el.child(tag(
                        "project-linear-no-teams",
                        "This workspace has no teams you can see.",
                        colors().muted,
                    ))
                },
            )
            .child(rows)
            .children(
                notes
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, note)| {
                        tag(("project-linear-team-note", index), note, colors().amber)
                            .line_height(px(18.))
                    }),
            )
            .child(
                row()
                    .gap_2()
                    .flex_wrap()
                    .when(bound.is_some(), |el| {
                        el.child(
                            button("project-linear-unlink", "Unlink")
                                .disabled(!idle)
                                .when(idle, |el| {
                                    el.on_activate(cx, |this, _, cx| {
                                        this.bind_linear_team(None, cx)
                                    })
                                }),
                        )
                    })
                    .child(
                        button("project-linear-refresh", "Refresh teams")
                            .disabled(!idle)
                            .when(idle, |el| {
                                el.on_activate(cx, |this, _, cx| this.refresh_linear_teams(cx))
                            }),
                    ),
            )
            .when_some(progress, |el, text| {
                el.child(tag("project-linear-progress", text, colors().muted))
            })
            .when_some(team.note.clone(), |el, note| {
                let color = if team.failed {
                    colors().amber
                } else {
                    colors().mint
                };
                el.child(tag("project-linear-note", note, color).line_height(px(18.)))
            })
    }
}

/// A cache age in the largest whole unit: "40 seconds", "3 minutes", "2 days".
fn age(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..60 => (seconds, "second"),
        60..3600 => (seconds / 60, "minute"),
        3600..86400 => (seconds / 3600, "hour"),
        _ => (seconds / 86400, "day"),
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}
