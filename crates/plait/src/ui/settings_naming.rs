//! Naming uses the host's provider catalog, independently of the coding harness.
use super::{
    Plait,
    commands::Command,
    model_picker,
    palette::{Options, Palette},
    settings,
    theme::*,
};
use gpui::{prelude::*, *};
use plait_client::host::{
    ModelChoice,
    thread_naming::{Request, Settings},
};

impl Plait {
    pub(super) fn sync_naming_settings(&mut self, cx: &mut Context<Self>) {
        self.naming.sync(self.host.connection.epoch);
        if !cfg!(test)
            && self.settings_open
            && self.host.connection.connected
            && self.naming.saved.is_none()
            && !self.naming.busy
            && self.naming.note.is_none()
        {
            self.load_naming_settings(cx);
        }
    }

    pub(super) fn load_naming_settings(&mut self, cx: &mut Context<Self>) {
        self.naming.sync(self.host.connection.epoch);
        if !self.host.connection.connected || self.naming.busy || cfg!(test) {
            return;
        }
        self.naming.busy = true;
        self.naming.note = None;
        let epoch = self.host.connection.epoch;
        self.settings_request(Request::GetThreadNaming, cx, move |this, result, _| {
            this.naming.complete(epoch, result);
        });
    }

    fn save_naming_settings(&mut self, value: Settings, cx: &mut Context<Self>) {
        if !self.host.connection.connected || self.naming.busy || self.naming.unsupported {
            return;
        }
        self.naming.sync(self.host.connection.epoch);
        self.naming.busy = true;
        self.naming.note = None;
        let epoch = self.host.connection.epoch;
        self.settings_request(
            Request::SetThreadNaming {
                enabled: value.enabled,
                model: value.model,
            },
            cx,
            move |this, result, _| {
                this.naming.complete(epoch, result);
            },
        );
    }

    pub(super) fn choose_naming_model(
        &mut self,
        choice: ModelChoice,
        epoch: u64,
        cx: &mut Context<Self>,
    ) {
        if epoch != self.host.connection.epoch || self.naming.saved.is_none() {
            return;
        }
        self.save_naming_settings(
            Settings {
                enabled: true,
                model: Some(choice),
            },
            cx,
        );
    }

    fn open_naming_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = self
            .host
            .connection
            .status
            .as_ref()
            .filter(|_| self.host.connection.connected)
        else {
            return;
        };
        let selected = self
            .naming
            .saved
            .as_ref()
            .and_then(|value| value.model.as_ref());
        let epoch = self.host.connection.epoch;
        let items = model_picker::picker_items(&status.models, &self.preferences.value, selected)
            .into_iter()
            .map(|mut item| {
                if let Command::Model(choice) = item.command {
                    item.command = Command::NamingModel { choice, epoch };
                }
                item.aside = None;
                item.marked = None;
                if item
                    .badge
                    .as_ref()
                    .is_some_and(|(label, _)| label == "Default")
                {
                    item.badge = Some(("Selected".into(), colors().mint));
                }
                item
            })
            .collect();
        let initial =
            selected.map(|model| format!("{}\u{1f}{}", model.provider, model.model_id).into());
        let palette = cx.new(|cx| {
            Palette::new(
                Options {
                    name: "Thread naming model".into(),
                    placeholder: "Find a naming model…".into(),
                    empty: "Add a model in Settings → Providers.".into(),
                    initial,
                    mark: None,
                    aside: None,
                    note: Some(
                        "Choosing a model enables naming for new threads on this host.".into(),
                    ),
                },
                items,
                window,
                cx,
            )
        });
        self.present(palette, None, window, cx);
    }

    pub(super) fn naming_settings(&self, cx: &mut Context<Self>) -> Div {
        let saved = self.naming.saved.as_ref();
        let enabled = saved.is_some_and(|value| value.enabled);
        let model = saved.and_then(|value| value.model.as_ref());
        let available = model.is_none_or(|choice| self.host.available(Some(choice)));
        let can_edit = self.host.connection.connected
            && !self.naming.busy
            && !self.naming.unsupported
            && saved.is_some();
        let label = model.map_or_else(
            || "Choose model…".to_owned(),
            |choice| self.model_name(choice),
        );
        let reason = if !self.host.connection.connected {
            "Connect to this host to change naming."
        } else if self.naming.unsupported {
            "Update this host to configure naming."
        } else {
            "Wait for naming settings to load or save."
        };
        let toggle = button("thread-naming-toggle", if enabled { "On" } else { "Off" })
            .aria_label(format!(
                "Automatic thread naming: {}",
                if enabled { "On" } else { "Off" }
            ))
            .disabled(!can_edit)
            .tooltip(move |_, cx| {
                cx.new(|_| {
                    super::thread_controls::TextTooltip(
                        if can_edit {
                            "Enable or disable automatic thread names"
                        } else {
                            reason
                        }
                        .into(),
                    )
                })
                .into()
            })
            .on_activate(cx, |this, window, cx| {
                let Some(mut value) = this.naming.saved.clone() else {
                    return;
                };
                if value.model.is_none() {
                    this.open_naming_models(window, cx);
                } else {
                    value.enabled = !value.enabled;
                    this.save_naming_settings(value, cx);
                }
            });
        settings::section("Thread naming")
            .child(settings::setting("Automatic names", "Summarize the first message with one extra model call. Existing threads and names you set stay unchanged.", toggle))
            .child(settings::setting("Naming model", "Used for Pi, Codex, and Claude Code threads on this host. Add a local endpoint in Providers.",
                toned_button("choose-naming-model", "", Tone::Filled)
                    .aria_label(format!("Thread naming model: {label}"))
                    .max_w(px(280.)).gap_2()
                    .child(div().min_w_0().truncate().child(label))
                    .child(icon("down"))
                    .disabled(!can_edit)
                    .tooltip(move |_, cx| cx.new(|_| super::thread_controls::TextTooltip(if can_edit { "Choose a model and enable thread naming" } else { reason }.into())).into())
                    .on_activate(cx, |this, window, cx| this.open_naming_models(window, cx))))
            .when_some(model, |el, choice| el.child(tag("naming-model-id", model_picker::model_label(choice), colors().muted)))
            .child(tag("naming-privacy-note", "Only the first message text is sent—no files, tools, or images. If naming fails, the prompt-based title stays; no other provider is used.", colors().subtle))
            .when(self.naming.busy, |el| el.child(tag("naming-busy", "Loading or saving naming settings…", colors().muted)))
            .when(!available && self.host.connection.connected, |el| el.child(tag("naming-unavailable", "The saved model is unavailable. Check Providers or choose another model.", colors().amber)))
            .when_some(self.naming.note.as_ref(), |el, note| el.child(tag("naming-note", note.clone(), colors().amber)))
            .when(self.naming.note.is_some() && !self.naming.unsupported, |el| el.child(
                button("reload-naming-settings", "Reload naming settings")
                    .disabled(!self.host.connection.connected || self.naming.busy)
                    .on_activate(cx, |this, _, cx| this.load_naming_settings(cx))))
    }
}
