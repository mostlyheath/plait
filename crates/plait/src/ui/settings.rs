//! The application's Settings modal: host status first, then New threads,
//! Providers, Linear, Agents, Keyboard, Editor, Appearance, and About. Only what applies to the
//! whole app or host is here; a project's own settings have their own modal
//! (settings_project.rs), and a thread's are under its message box.

use super::{
    Plait,
    commands::Command,
    jump::PreferenceGroup,
    model_picker::{ModelPurpose, model_label},
    palette::{Item, Options, Palette},
    theme::*,
};
use gpui::{prelude::*, *};
use std::collections::HashSet;

/// Which settings surface is open over the workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SettingsPage {
    /// Settings (Cmd-comma): the whole app and host.
    #[default]
    Application,
    /// The active project's own settings, from its sidebar menu.
    Project,
}

/// The application page's tabs, in the order they are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SettingsTab {
    #[default]
    NewThreads,
    Providers,
    /// The host's Linear sign-in.
    Linear,
    /// Which machine's host this window works with.
    Machines,
    Agents,
    Keyboard,
    /// What Open in Editor and Open Terminal Here run.
    Editor,
    Appearance,
    About,
}

impl SettingsTab {
    pub(super) const ALL: [Self; 9] = [
        Self::NewThreads,
        Self::Providers,
        Self::Linear,
        Self::Machines,
        Self::Agents,
        Self::Keyboard,
        Self::Editor,
        Self::Appearance,
        Self::About,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NewThreads => "New threads",
            Self::Providers => "Providers",
            Self::Linear => "Linear",
            Self::Machines => "Machines",
            Self::Agents => "Agents",
            Self::Keyboard => "Keyboard",
            Self::Editor => "Editor",
            Self::Appearance => "Appearance",
            Self::About => "About",
        }
    }
}

/// Centered reading column for settings-style pages.
pub(super) fn page(id: &'static str) -> Stateful<Div> {
    column()
        .id(id)
        .overflow_y_scroll()
        .flex_1()
        .min_h_0()
        .w_full()
        .max_w(px(CONTENT_WIDTH))
        .mx_auto()
        .gap_4()
}

pub(super) fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(TYPE_HEADER_SIZE))
        .line_height(px(TYPE_HEADER_LINE))
        .text_color(rgb(colors().strong))
        .font_weight(TYPE_HEADER_WEIGHT)
        .child(text.into())
}

/// The workspace area around a settings page's column.
pub(super) fn framed(column: Div) -> Div {
    div()
        .flex()
        .flex_1()
        .min_h_0()
        .px(px(30.))
        .pb_5()
        .child(column)
}

fn note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(TYPE_CONTROL_SIZE))
        .line_height(px(TYPE_CONTROL_LINE))
        .text_color(rgb(colors().muted))
        .child(text.into())
}

/// One titled group of settings, ruled off from the group above it.
pub(super) fn section(title: &'static str) -> Div {
    column()
        .gap_3()
        .flex_shrink_0()
        .pt_4()
        .border_t_1()
        .border_color(rgb(colors().border))
        .child(
            div()
                .text_size(px(TYPE_ROW_SIZE))
                .line_height(px(TYPE_ROW_LINE))
                .font_weight(TYPE_MEDIUM)
                // Several section headings can appear in one accessibility tree.
                .child(a11y_text(
                    (
                        ElementId::from("settings-section-heading"),
                        SharedString::from(title),
                    ),
                    title,
                )),
        )
}

/// A setting: what it is and what it means on the left, its control on the right.
pub(super) fn setting(
    label: &'static str,
    detail: impl Into<SharedString>,
    control: impl IntoElement,
) -> Div {
    row()
        .items_start()
        .justify_between()
        .gap_4()
        .child(
            column()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(TYPE_CONTROL_SIZE))
                        .text_color(rgb(colors().muted))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(TYPE_META_SIZE))
                        .line_height(px(TYPE_META_LINE))
                        .text_color(rgb(colors().subtle))
                        .child(detail.into()),
                ),
        )
        .child(control)
}

/// A state in words beside its dot, so the color is never the only signal.
fn state_line(id: impl Into<ElementId>, text: impl Into<SharedString>, tint: u32) -> Div {
    row()
        .items_start()
        .child(dot(tint).mt(px(5.)))
        // Long host errors wrap; they are never cut off.
        .child(tag(id, text, tint).flex_1().min_w_0().line_height(px(17.)))
}

impl Plait {
    fn host_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, tint) = if self.host.connection.connecting {
            (
                format!("Connecting to {}…", self.machine_label()),
                colors().muted,
            )
        } else if self.host.connection.connected {
            (
                format!(
                    "{} connected · protocol {}",
                    self.machine_label(),
                    self.host.protocol()
                ),
                colors().mint,
            )
        } else {
            (
                format!("{} disconnected", self.machine_label()),
                colors().amber,
            )
        };
        column()
            .gap_2()
            .flex_shrink_0()
            .px_3()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors().border))
            .bg(rgb(colors().rail))
            .child(
                row()
                    .justify_between()
                    .child(row().child(dot(tint)).child(tag("settings-host-status", label, tint)))
                    .child(
                        row()
                            .gap_1()
                            .child(
                                button("open-setup-guide", "Setup and recovery guide").on_activate(
                                    cx,
                                    |_, _, cx| {
                                        cx.open_url(
                                            "https://github.com/mostlyheath/plait/blob/main/docs/GUIDE.md",
                                        );
                                    },
                                ),
                            )
                            .child(
                                toned_button("reconnect-host", "Reconnect", Tone::Filled)
                                    .disabled(self.host.connection.connecting)
                                    .when(!self.host.connection.connecting, |el| {
                                        el.on_activate(cx, |this, window, cx| {
                                            this.reconnect_host(window, cx)
                                        })
                                    }),
                            ),
                    ),
            )
            .children(self.server_setup_line(cx))
            .when_some(
                self.host
                    .connection
                    .error
                    .clone()
                    .filter(|_| !self.server_setup_visible()),
                |el, error| {
                    el.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(colors().red))
                            .child(a11y_text("settings-host-error", error)),
                    )
                },
            )
            .when(!self.host.connection.connected, |el| {
                el.child(note(
                    "The installed host normally starts at login. Reconnect does not restart it. Closing Plait does not stop accepted work.",
                ))
            })
    }

    /// How much of the host's catalog can be used, counted from its reply.
    fn catalog_summary(&self) -> Option<String> {
        let models = &self.host.connection.status.as_ref()?.models;
        if models.is_empty() {
            return Some(
                "The host returned no models. The setup guide explains how to configure it.".into(),
            );
        }
        let signed_in = models.iter().filter(|model| model.authenticated);
        let usable = signed_in.clone().count();
        if usable == 0 {
            return Some("No model is signed in. Open Providers to sign in; its models become available as soon as the host confirms.".into());
        }
        let providers = |models: &mut dyn Iterator<Item = &plait_client::host::Model>| {
            models
                .map(|model| model.provider.as_str())
                .collect::<HashSet<_>>()
                .len()
        };
        Some(format!(
            "{usable} of {} models are signed in, from {} of {} providers.",
            models.len(),
            providers(&mut signed_in.clone()),
            providers(&mut models.iter()),
        ))
    }

    /// Settings → New threads: what a new thread starts with, then what the
    /// host's catalog offers.
    fn new_thread_settings(&self, cx: &mut Context<Self>) -> Div {
        column()
            .gap_4()
            .child(
                section("Defaults")
                    .child(self.default_harness_setting(cx))
                    .when_some(
                        self.preference_note(Some(PreferenceGroup::Agent)),
                        |el, note| el.child(tag("defaults-agent-note", note, colors().subtle)),
                    )
                    .child(self.default_model_setting(cx))
                    .child(self.default_thinking_setting(cx))
                    .child(self.approval_setting(cx)),
            )
            .child(self.naming_settings(cx))
            .child(self.catalog_settings())
    }

    /// How many models the host can use, and where sign-in answers go.
    fn catalog_settings(&self) -> Div {
        section("Models")
            .when_some(self.catalog_summary(), |el, summary| {
                el.child(tag("catalog-summary", summary, colors().subtle))
            })
            .child(tag(
                "catalog-auth-note",
                "Authentication stays on the host. Keys and sign-in answers entered here are sent once and never saved in desktop preferences.",
                colors().subtle,
            ))
            .when_some(
                self.preference_note(Some(PreferenceGroup::Models)),
                |el, note| {
                    el.child(tag(
                        "catalog-preference-note",
                        format!("Favorites and recent models: {note}"),
                        colors().subtle,
                    ))
                },
            )
    }

    /// The default model. Choosing in the picker saves at once; the line
    /// beneath reports what the host answered.
    fn default_model_setting(&self, cx: &mut Context<Self>) -> Div {
        let saved = self
            .host
            .connection
            .status
            .as_ref()
            .and_then(|status| status.default_model.as_ref());
        let name = saved.map_or_else(
            || "No default model".to_owned(),
            |choice| self.model_name(choice),
        );
        let usable = saved.is_none_or(|choice| self.host.available(Some(choice)));
        let can_choose = self.host.connection.connected && !self.host.connection.saving;
        let control = toned_button("choose-default-model", "", Tone::Filled)
            .aria_label(format!("Default model: {name}. Choose another…"))
            .gap_2()
            .max_w(px(320.))
            .child(div().min_w_0().truncate().child(name))
            .child(icon("down").text_color(rgb(colors().faint)))
            .disabled(!can_choose)
            .when(can_choose, |el| {
                el.on_activate(cx, |this, window, cx| {
                    this.open_model_picker(ModelPurpose::DefaultModel, window, cx)
                })
            });
        column()
            .gap_3()
            .child(setting(
                "Default model for new threads",
                "New threads start with this model. Choosing another saves it on the host at once. Existing threads are unchanged; their own model control changes them between runs.",
                control,
            ))
            .when_some(saved, |el, choice| {
                el.child(tag("default-model-label", model_label(choice), colors().muted))
            })
            .when(!usable && self.host.connection.connected, |el| {
                el.child(state_line(
                    "default-model-unusable",
                    "The saved default is not signed in on this host. New threads ask for a model until it is.",
                    colors().amber,
                ))
            })
            .when_some(self.default_model_state(), |el, (state, tint)| {
                el.child(state_line("default-model-state", state, tint))
            })
    }

    /// Saved on this desktop and sent with each new thread. The host picks the
    /// nearest level the thread's model offers, so one setting fits all models.
    fn default_thinking_setting(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let level = self.preferences.value.default_thinking();
        let detail = if self.host.connection.connected && self.host.protocol() < 3 {
            "This host starts every thread with thinking off. Update the host to use this setting."
        } else {
            "New threads start at this level, or the closest one their model offers. A thread's own level is under its message box."
        };
        let control = toned_button("choose-default-thinking", "", Tone::Filled)
            .aria_label(format!(
                "Default thinking: {}. Choose another…",
                level.label()
            ))
            .gap_2()
            .child(level.label())
            .child(icon("down").text_color(rgb(colors().faint)))
            .on_activate(cx, |this, window, cx| {
                this.open_default_thinking(window, cx)
            });
        setting("Default thinking for new threads", detail, control)
    }

    fn open_default_thinking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.preferences.value.default_thinking();
        let items = plait_client::host::ThinkingLevel::ALL
            .iter()
            .map(|level| {
                Item::new(
                    format!("default-thinking-{level:?}"),
                    level.label(),
                    Command::DefaultThinking(*level),
                )
            })
            .collect();
        let palette = cx.new(|cx| {
            Palette::new(
                Options {
                    name: "Default thinking level".into(),
                    placeholder: "Choose the level new threads start at…".into(),
                    empty: "No levels match.".into(),
                    // Start on the saved level, so Return keeps it.
                    initial: Some(format!("default-thinking-{current:?}").into()),
                    mark: None,
                    aside: None,
                    note: Some("Existing threads keep their own level.".into()),
                },
                items,
                window,
                cx,
            )
            .picker()
        });
        self.present(palette, None, window, cx);
    }

    /// Both modal bodies share one content width; the modal owns their header.
    pub(super) fn settings_column(&self, title: Option<AnyElement>) -> Div {
        column()
            .flex_1()
            .min_h_0()
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .gap_3()
            .children(title)
            .when_some(self.preference_note(None), |el, note| {
                el.child(state_line(
                    "settings-preference-note",
                    format!("Desktop preferences: {note}"),
                    colors().amber,
                ))
            })
    }

    fn settings_tabs(&self, cx: &mut Context<Self>) -> Div {
        let mut tabs = row().gap_1().flex_wrap();
        for tab in SettingsTab::ALL {
            let selected = self.settings_tab == tab;
            tabs = tabs.child(
                toned_button(
                    ("settings-section", tab as usize),
                    tab.label(),
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Ghost
                    },
                )
                .role(Role::Tab)
                .aria_selected(selected)
                .on_activate(cx, move |this, _, cx| {
                    this.cancel_recording(cx);
                    this.cancel_harness_path();
                    if tab != SettingsTab::Providers {
                        this.close_provider_form(cx);
                    }
                    this.settings_tab = tab;
                    this.settings_scroll.set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }),
            );
        }
        tabs
    }

    pub(super) fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.settings_tab {
            SettingsTab::NewThreads => self.new_thread_settings(cx).into_any_element(),
            SettingsTab::Providers => self.provider_settings(cx).into_any_element(),
            SettingsTab::Linear => self.linear_settings(cx).into_any_element(),
            SettingsTab::Machines => self.machine_settings(cx).into_any_element(),
            SettingsTab::Agents => self.harness_settings(cx).into_any_element(),
            SettingsTab::Keyboard => self.keyboard_settings(cx).into_any_element(),
            SettingsTab::Editor => self.editor_settings(cx).into_any_element(),
            SettingsTab::Appearance => self.view_settings(cx).into_any_element(),
            SettingsTab::About => self.build_details(cx).into_any_element(),
        };
        framed(
            self.settings_column(None)
                .child(self.host_status(cx))
                .child(self.settings_tabs(cx))
                .child(
                    page("settings-page")
                        .track_scroll(&self.settings_scroll)
                        .child(body),
                ),
        )
    }

    /// Identifies the running build, so an installed update can be confirmed.
    fn build_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let revision = option_env!("PLAIT_BUILD_REVISION").unwrap_or("source build");
        let built = option_env!("PLAIT_BUILD_TIME")
            .map(|time| format!(" · built {time}"))
            .unwrap_or_default();
        let host = if self.host.connection.connected {
            format!("Host protocol {}", self.host.protocol())
        } else {
            "Host not connected".into()
        };
        section("About and updates")
            .child(
                column()
                    .gap_1()
                    .child(tag(
                        "about-desktop-version",
                        format!("Desktop {} · {revision}{built}", env!("CARGO_PKG_VERSION")),
                        colors().muted,
                    ))
                    .child(tag("about-host-protocol", host, colors().subtle)),
            )
            .child(
                button("third-party-notices", "Third-party notices")
                    .on_activate(cx, |this, window, cx| this.open_notices(window, cx)),
            )
            .child(self.update_controls(cx))
    }

    fn open_notices(&self, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async {
            crate::update::setup::running_bundle()
                .map(|bundle| bundle.join("Contents/Resources/THIRD_PARTY_NOTICES.txt"))
                .filter(|file| file.is_file())
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = task.await;
            let _ = this.update_in(cx, |_, window, cx| {
                if let Some(path) = path {
                    cx.open_with_system(&path);
                } else {
                    let _answer = window.prompt(
                        PromptLevel::Info,
                        "Third-party notices are available in the packaged app.",
                        None,
                        &["OK"],
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn view_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hidden = self.preferences.value.hide_changed_files;
        let mut choices = segmented();
        for (index, (label, value)) in [("Show", false), ("Hide", true)].into_iter().enumerate() {
            choices = choices.child(
                segment(("changed-files", index), label, value == hidden)
                    .role(Role::RadioButton)
                    .aria_toggled(if value == hidden {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .aria_label(format!("Changed files above the message box: {label}"))
                    .on_activate(cx, move |this, _, cx| {
                        this.set_changed_files_hidden(value, cx)
                    }),
            );
        }
        section("Appearance")
            .child(self.thread_tabs_setting(cx))
            .child(self.theme_settings(cx))
            .child(setting(
                "Changed files above the message box",
                "Files the thread wrote that are not committed yet, with lines added and removed. Click one to see it in Changes.",
                choices,
            ))
            .when_some(
                self.preference_note(Some(PreferenceGroup::View)),
                |el, note| el.child(tag("appearance-preference-note", note, colors().subtle)),
            )
    }

    pub(super) fn set_changed_files_hidden(&mut self, hidden: bool, cx: &mut Context<Self>) {
        let mut value = self.preferences.value.clone();
        value.hide_changed_files = hidden;
        self.set_preferences(value, PreferenceGroup::View, cx);
        // Lists were not fetched while hidden.
        if !hidden {
            self.workspace.thread_files.reset();
            self.thread_files_snapshot(cx);
        }
    }

    /// Pi's default approvals. The detail names what commands reach in the
    /// open project, since that is where the next thread will run.
    fn approval_setting(&self, cx: &mut Context<Self>) -> Div {
        let ask = self.preferences.value.ask_before_actions;
        let mut choices = segmented();
        for (index, (label, value)) in [("Automatic", false), ("Ask each time", true)]
            .into_iter()
            .enumerate()
        {
            choices = choices.child(
                segment(("approval-mode", index), label, value == ask)
                    .role(Role::RadioButton)
                    .aria_toggled(if value == ask {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .aria_label(format!("Edits and commands: {label}"))
                    .on_activate(cx, move |this, _, cx| {
                        let mut preferences = this.preferences.value.clone();
                        preferences.ask_before_actions = value;
                        this.set_preferences(preferences, PreferenceGroup::Approvals, cx);
                    }),
            );
        }
        let detail = if self.host.connection.connected && self.host.protocol() < 2 {
            "This host asks before every edit and command. Update the host to use Automatic."
        } else {
            // Settings describes no one project, the open one included.
            plait_client::host::policy::approval_detail(ask)
        };
        let harness_modes = self.harness_rows().iter().any(|row| !row.modes.is_empty());
        column()
            .gap_3()
            .child(setting("Default for edits and commands", detail, choices))
            .child(tag(
                "approval-thread-override-note",
                "A thread can choose its own from the run settings under its message box.",
                colors().subtle,
            ))
            .when(harness_modes, |el| {
                el.child(
                    tag(
                        "approval-harness-scope-note",
                        "These apply to Pi threads. A thread in another harness uses that harness's own permissions, chosen under its message box and in Settings → Agents.",
                        colors().subtle,
                    )
                    .line_height(px(16.)),
                )
            })
            .when_some(
                self.preference_note(Some(PreferenceGroup::Approvals)),
                |el, note| el.child(tag("approval-preference-note", note, colors().subtle)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsTab;

    /// A tab's element ID is its position, so the order and the IDs agree.
    #[test]
    fn each_tab_is_drawn_once_at_its_own_position() {
        for (index, tab) in SettingsTab::ALL.into_iter().enumerate() {
            assert_eq!(tab as usize, index, "{tab:?}");
        }
        let labels = SettingsTab::ALL
            .map(SettingsTab::label)
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(labels.len(), SettingsTab::ALL.len());
        assert_eq!(SettingsTab::default(), SettingsTab::NewThreads);
        assert_eq!(SettingsTab::NewThreads.label(), "New threads");
    }
}
