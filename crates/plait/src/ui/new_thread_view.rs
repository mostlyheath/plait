//! The new-thread draft on screen: a header row of choices above its composer
//! (project, model, thinking, harness, permissions), and the menus those chips open.
//! Every value here is read from state that replies already filled in; the
//! menus are built when opened, never per frame.

use super::{
    OpenRepository, Plait,
    commands::Command,
    composer,
    model_picker::ModelPurpose,
    palette::{Item, Options, Palette},
    permissions::{Permissions, menu_chip},
    theme::*,
    thread_controls::{approval_label, thinking_value},
};
use gpui::{prelude::*, *};
use plait_core::host::{ApprovalMode, harness::HarnessId};
use std::rc::Rc;

impl Plait {
    /// The conversation area while the draft is open. Nothing here runs an agent.
    pub(super) fn draft_view(&self, wide: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.active_project();
        let detail = format!(
            "It starts in {} on {} when you send its first message. Nothing is created before then.",
            project.name, project.machines[0].name
        );
        let composer = self.draft.composer.clone();
        let main = column()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(empty("New thread", detail))
            .child(
                column()
                    .flex_shrink_0()
                    .px_6()
                    .pt_2()
                    .pb_3()
                    .gap_2()
                    .when_some(composer, |el, composer| {
                        el.child(
                            div()
                                .w_full()
                                .max_w(px(CONTENT_WIDTH))
                                .mx_auto()
                                // Escape reaches here when the composer has nothing else to close.
                                .on_action(cx.listener(|this, _: &composer::Cancel, window, cx| {
                                    this.escape_draft(window, cx)
                                }))
                                .child(composer),
                        )
                    })
                    // Under the box, where an open thread's controls are, so
                    // the box stays where it is when the first message is sent.
                    .child(self.draft_controls(cx))
                    .children(self.isolation_choice(cx))
                    .when(!self.draft_isolation_choice(), |el| {
                        el.children(self.draft_status(cx))
                    }),
            );
        row()
            .gap_0()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(main)
            .when(self.shows_inspector(wide), |el| {
                el.child(self.inspector(cx))
            })
    }

    /// Project | model | thinking | harness | permissions, then Discard.
    fn draft_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = self.draft_locked();
        let project = self.active_project();
        let harness = self.draft_harness();
        let harness_name = self.harness_row(harness).map_or_else(
            || SharedString::from(harness.label()),
            |row| row.name.clone(),
        );
        let model = self
            .draft_model_name()
            .unwrap_or_else(|| "Choose a model".into());
        let mut chips = row()
            .flex_wrap()
            .flex_1()
            .min_w_0()
            .gap_x_1()
            .gap_y_1()
            .child(
                menu_chip(
                    "draft-project",
                    None,
                    Some(icon("folder").size(px(13.)).text_color(rgb(colors().mint))),
                    project.name.clone().into(),
                    format!("Project: {}. Choose where the thread runs.", project.name).into(),
                    locked,
                )
                .aria_label(format!("Project: {}", project.name))
                .relative()
                .child(self.anchor_probe("draft-project"))
                .on_activate(cx, |this, window, cx| {
                    this.anchor_next("draft-project");
                    this.open_draft_projects(window, cx)
                }),
            )
            .child(
                menu_chip(
                    "draft-model",
                    None,
                    None,
                    model.clone(),
                    format!("Model: {model}. Choose among {harness_name}'s models.").into(),
                    locked,
                )
                .max_w(px(240.))
                .aria_label(format!("Model: {model}"))
                .relative()
                .child(self.anchor_probe("draft-model"))
                .on_activate(cx, |this, window, cx| {
                    this.anchor_next("draft-model");
                    this.open_model_picker(ModelPurpose::NewThread, window, cx)
                }),
            )
            .children(self.draft_thinking_chip(cx))
            .child(
                menu_chip(
                    "draft-harness",
                    None,
                    harness_logo(harness).map(|logo| logo.text_color(rgb(colors().muted))),
                    harness_name.clone(),
                    "The agent harness that runs the thread. A thread stays in the harness it starts in."
                        .into(),
                    locked,
                )
                .aria_label(format!("Harness: {harness_name}"))
                .relative()
                .child(self.anchor_probe("draft-harness"))
                .on_activate(cx, |this, window, cx| {
                    this.anchor_next("draft-harness");
                    this.open_draft_harnesses(window, cx)
                }),
            );
        chips = chips.children(self.draft_permissions(cx));
        row()
            .w_full()
            .max_w(px(CONTENT_WIDTH))
            .mx_auto()
            .items_start()
            .gap_2()
            .text_size(px(11.))
            .text_color(rgb(colors().muted))
            .child(chips)
            .child(
                button("draft-discard", "Discard")
                    .aria_description("Discard this draft and close its tab.")
                    .tooltip(|_, cx| {
                        cx.new(|_| {
                            super::thread_controls::TextTooltip(
                                "Discard this draft and close its tab. It cannot be reopened."
                                    .into(),
                            )
                        })
                        .into()
                    })
                    .on_activate(cx, |this, window, cx| this.discard_draft(window, cx)),
            )
    }

    /// The level the thread starts at; none for a model without thinking levels.
    fn draft_thinking_chip(&self, cx: &mut Context<Self>) -> Option<Control> {
        let level = self.draft_thinking()?;
        let source = if self.draft.thinking.is_some() {
            "Chosen for this thread"
        } else {
            "The default from Settings → New threads"
        };
        Some(
            menu_chip(
                "draft-thinking",
                None,
                None,
                thinking_value(level).into(),
                format!(
                    "Thinking: {}. {source}, at the nearest level this model has.",
                    level.label()
                )
                .into(),
                self.draft_locked(),
            )
            .aria_label(format!("Thinking level: {}", level.label()))
            .relative()
            .child(self.anchor_probe("draft-thinking"))
            .on_activate(cx, |this, window, cx| {
                this.anchor_next("draft-thinking");
                this.open_draft_thinking(window, cx)
            }),
        )
    }

    /// The thread's thinking menu, listing the draft model's levels.
    pub(super) fn open_draft_thinking(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft_locked().is_some() {
            return;
        }
        let levels = self.draft_thinking_levels().to_vec();
        let current = self.draft_thinking();
        self.present_thinking(
            &levels,
            current,
            "Applies from the thread's first turn. Change it any time after.",
            Command::DraftThinking,
            window,
            cx,
        );
    }

    /// The Permissions chip and menu of a draft in a harness with its own modes.
    fn draft_harness_permissions(&self) -> Option<Permissions> {
        let harness = self.draft_harness();
        let row = self.harness_row(harness)?;
        let current = self.draft_mode()?;
        // The chip names the mode; hover says what it allows and what
        // protects the checkout.
        let allows = row
            .mode(&current)
            .map(|mode| mode.detail.trim().to_owned())
            .unwrap_or_default();
        let lead = [allows.as_str(), row.protection.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let tip = format!(
            "{lead}{}Choose what {} may do in the new thread.",
            if lead.is_empty() { "" } else { " " },
            row.name
        );
        Some(Permissions {
            harness,
            current,
            blocked: self.draft_locked(),
            tip: tip.into(),
            note: "Applies from the thread's first turn. Change it any time after.".into(),
            on_select: Rc::new(move |this, mode, _, cx| this.choose_draft_mode(harness, mode, cx)),
        })
    }

    /// Opens the draft's harness Permissions menu, as its chip does. For scenes.
    #[cfg(test)]
    pub(super) fn open_draft_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(permissions) = self.draft_harness_permissions() {
            self.open_permissions(&permissions, window, cx);
        }
    }

    /// A harness's own modes, or Plait's Ask and Auto for Pi, in the same place.
    fn draft_permissions(&self, cx: &mut Context<Self>) -> Option<Control> {
        let harness = self.draft_harness();
        let locked = self.draft_locked();
        if harness != HarnessId::Pi {
            let permissions = self.draft_harness_permissions()?;
            return self.permissions_chip("draft-permissions", permissions, cx);
        }
        let mode = self.draft_approval();
        let value = match mode {
            ApprovalMode::Ask => "Ask",
            ApprovalMode::Auto => "Auto",
        };
        let source = if self.draft.approval.is_some() {
            "Chosen for this thread."
        } else {
            "The default from Settings → New threads."
        };
        let blocked = locked.or_else(|| {
            (self.host.connection.connected && self.host.protocol() < 2)
                .then_some("This host asks before every edit and command.")
        });
        Some(
            menu_chip(
                "draft-permissions",
                Some("Permissions"),
                None,
                value.into(),
                format!("{}. {source}", approval_label(mode)).into(),
                blocked,
            )
            .on_activate(cx, |this, window, cx| this.open_draft_approvals(window, cx)),
        )
    }

    /// Under the composer: the creation under way or its failure, else what
    /// the chosen permissions allow.
    fn draft_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let line = || {
            row()
                .w_full()
                .max_w(px(CONTENT_WIDTH))
                .mx_auto()
                .gap_2()
                .text_size(px(11.))
        };
        // First launch: setting up the server explains the missing connection.
        if let Some(setup) = self.server_setup_line(cx) {
            return Some(line().child(setup).into_any_element());
        }
        if self.draft_pending() {
            if self.draft_creating() {
                let model = self.draft_model_name().unwrap_or_default();
                return Some(
                    line()
                        .child(dot(colors().blue))
                        .child(div().text_color(rgb(colors().muted)).child(a11y_text(
                            "draft-creating",
                            format!("Creating thread with {model}…"),
                        )))
                        .into_any_element(),
                );
            }
            let detail = self
                .draft
                .error
                .clone()
                .unwrap_or_else(|| "The host did not answer.".into());
            let connected = self.host.connection.connected;
            return Some(
                line()
                    .child(dot(colors().amber))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(colors().amber))
                            .child(a11y_text(
                                "draft-unconfirmed",
                                format!("Thread not confirmed. {detail}"),
                            )),
                    )
                    .child(if connected {
                        toned_button("draft-retry", "Retry", Tone::Filled)
                            .on_activate(cx, |this, window, cx| this.retry_draft(window, cx))
                    } else {
                        toned_button("draft-reconnect", "Reconnect", Tone::Filled)
                            .disabled(self.host.connection.connecting)
                            .on_activate(cx, |this, window, cx| this.reconnect_host(window, cx))
                    })
                    .into_any_element(),
            );
        }
        if let Some(error) = &self.draft.error {
            return Some(
                line()
                    .child(dot(colors().red))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(colors().red))
                            .child(a11y_text(
                                "draft-error",
                                self.identity_recovery_message(error),
                            )),
                    )
                    .when(self.has_identity_recovery(error), |el| {
                        el.child(self.identity_recovery_button(cx))
                    })
                    .into_any_element(),
            );
        }
        // A harness that lists no modes has no Permissions chip to carry
        // what protects the checkout, so it is said here.
        let harness = self.draft_harness();
        let row = self
            .harness_row(harness)
            .filter(|row| harness != HarnessId::Pi && row.modes.is_empty())?;
        let protection = row.protection.clone();
        (!protection.is_empty()).then(|| {
            line()
                .child(tag("draft-protection", protection, colors().subtle).line_height(px(16.)))
                .into_any_element()
        })
    }

    /// The sidebar's projects, most recently active first, then Open project….
    pub(super) fn open_draft_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft_locked().is_some() {
            return;
        }
        let current = self.active_project().checkout.clone();
        let mut items = self
            .state
            .projects
            .iter()
            .map(|project| {
                let missing = self.navigation.available.get(&project.checkout) == Some(&false);
                let item = Item::new(
                    format!("draft-project:{}", project.checkout.display()),
                    project.name.clone(),
                    Command::DraftProject(project.checkout.clone()),
                )
                .detail(home_relative(&project.checkout))
                .section("Projects")
                .glyph(
                    "folder",
                    if missing {
                        colors().amber
                    } else {
                        colors().mint
                    },
                );
                if missing {
                    item.badge("Missing", colors().amber).enabled(false)
                } else if project.checkout == current {
                    item.badge("Current", colors().blue)
                } else {
                    item
                }
            })
            .collect::<Vec<_>>();
        items.push(
            Item::new(
                "draft-open-repository",
                "Open project…",
                Command::Action(Box::new(OpenRepository)),
            )
            .detail("Choose a folder on this Mac")
            .section("Another folder")
            .glyph("folder", colors().subtle)
            .hint(self.shortcut_label("repository.open")),
        );
        let options = Options {
            name: "Project for the new thread".into(),
            placeholder: "Search projects…".into(),
            empty: "No project matches.".into(),
            initial: Some(format!("draft-project:{}", current.display()).into()),
            mark: None,
            aside: None,
            note: Some("The thread starts in the project you choose.".into()),
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
    }

    /// Move the draft to project `checkout`. The repository work is what
    /// opening a thread there does; the draft stays open.
    pub(super) fn choose_draft_project(
        &mut self,
        checkout: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.draft_locked().is_some() {
            return;
        }
        if checkout == self.host.creation.checkout {
            self.close_overlay(true, window, cx);
            return;
        }
        let Some(tab) = self.tabs.model.active() else {
            return;
        };
        self.capture_session(cx);
        let result = self
            .tabs
            .model
            .move_draft(tab, self.tab_checkout(checkout.clone()));
        if !self.tab_error(result) {
            return;
        }
        let draft = std::mem::take(&mut self.draft);
        if let Some(id) = &draft.id
            && let Some(data) = &mut self.session.data
        {
            data.clear_draft_tab(id);
        }
        self.tabs.lifecycle.presenting = true;
        self.change_checkout(checkout, window, cx);
        self.tabs.lifecycle.presenting = false;
        self.draft = draft;
        self.open_draft_content(window, cx);
        self.refresh_tab_rows();
        self.capture_session(cx);
    }

    pub(super) fn open_draft_harnesses(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft_locked().is_some() {
            return;
        }
        let items = super::harnesses::harness_items(self.harness_rows(), self.draft_harness());
        let note = if self.host.connection.connected && self.host.protocol() < 4 {
            "This host runs every thread in Pi. Update the host to choose another harness."
        } else {
            "A thread stays in the harness it starts in. Settings → Agents lists each one."
        };
        let options = Options {
            name: "Harness for the new thread".into(),
            placeholder: "Search harnesses…".into(),
            empty: "No harness matches.".into(),
            initial: Some(format!("harness\u{1f}{}", self.draft_harness().label()).into()),
            mark: None,
            aside: None,
            note: Some(note.into()),
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
    }

    fn open_draft_approvals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft_locked().is_some() {
            return;
        }
        let chosen = self.draft.approval;
        let default = self.default_approval();
        let section = "Approvals for the new thread";
        let item = |key: &str, title: String, detail: &str, mode: Option<ApprovalMode>| {
            let item = Item::new(key.to_owned(), title, Command::DraftApproval(mode))
                .section(section)
                .detail(detail.to_owned());
            if chosen == mode {
                item.badge("Current", colors().blue)
            } else {
                item
            }
        };
        let items = vec![
            item(
                "draft-approval-default",
                format!("Use default ({})", approval_label(default)),
                "Follow Settings → New threads.",
                None,
            ),
            item(
                "draft-approval-ask",
                approval_label(ApprovalMode::Ask).into(),
                "Each edit and command waits for Allow once.",
                Some(ApprovalMode::Ask),
            ),
            item(
                "draft-approval-auto",
                approval_label(ApprovalMode::Auto).into(),
                "Edits and commands run without asking.",
                Some(ApprovalMode::Auto),
            ),
        ];
        let initial = match chosen {
            None => "draft-approval-default",
            Some(ApprovalMode::Ask) => "draft-approval-ask",
            Some(ApprovalMode::Auto) => "draft-approval-auto",
        };
        let options = Options {
            name: "Permissions".into(),
            placeholder: "Choose how the new thread runs…".into(),
            empty: "No choices.".into(),
            initial: Some(initial.into()),
            mark: None,
            aside: None,
            note: Some("Pi asks before actions, or not, as Plait's approvals say. Change it any time after.".into()),
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
    }
}
