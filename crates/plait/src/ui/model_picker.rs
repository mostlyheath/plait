//! The model picker: the host's catalog as palette items, and what a choice does.
//!
//! The picker lists only models the host reports as signed in; Settings →
//! Providers lists the rest. Favorites and recently used models are desktop
//! preferences: identifiers only, never credentials.

use super::{
    Plait,
    commands::Command,
    composer::Composer,
    jump::PreferenceGroup,
    palette::{Item, Options, Palette},
    theme::*,
};
use gpui::{prelude::*, *};
use plait_client::{
    host::{Model, ModelChoice, harness::HarnessId},
    preferences::{ModelKey, Preferences},
};
use std::collections::{HashMap, HashSet};

/// What the chosen model is for. A per-thread switch would be a third purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelPurpose {
    /// The new-thread draft's model, from its harness's list.
    NewThread,
    /// Save the chosen model as the host's default for new threads.
    DefaultModel,
    ThreadModel,
}

/// What an open picker's rows and footer were built from. When it changes,
/// the picker is rebuilt, so a save or a reconnect shows without reopening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PickerStamp {
    connected: bool,
    models: u64,
    default: Option<ModelChoice>,
    choosing: Option<ModelChoice>,
    saving: bool,
    message: Option<String>,
    favorites: u64,
    /// The harness list generation, so a new list or state shows at once.
    harnesses: u64,
}

/// The identifier shown beside a model's display name.
pub(super) fn model_label(choice: &ModelChoice) -> String {
    format!("{}/{}", choice.provider, choice.model_id)
}

/// Identity of a model across sections. The separator cannot occur in an identifier
/// the way a slash can.
pub(super) fn model_key(model: &Model) -> String {
    format!("{}\u{1f}{}", model.provider, model.model_id)
}

fn find<'a>(models: &'a [Model], key: &ModelKey) -> Option<&'a Model> {
    models
        .iter()
        .find(|model| model.provider == key.provider && model.model_id == key.model_id)
}

/// Signed-in models from `keys`, in the order they are kept.
fn signed_in<'a>(
    models: &'a [Model],
    keys: &'a [ModelKey],
) -> impl Iterator<Item = &'a Model> + 'a {
    keys.iter()
        .filter_map(|key| find(models, key))
        .filter(|model| model.authenticated)
}

/// Signed-in favorites, in the order they were starred. Every harness's picker
/// starts with these; each lists only the ones in its own catalog.
pub(super) fn favorites<'a>(
    models: &'a [Model],
    preferences: &'a Preferences,
) -> impl Iterator<Item = &'a Model> + 'a {
    signed_in(models, &preferences.favorite_models)
}

/// Signed-in models in the order a person reaches for them: favorites, then
/// recently used, then the rest of the catalog. Each appears once.
pub(super) fn usable_models<'a>(models: &'a [Model], preferences: &Preferences) -> Vec<&'a Model> {
    let remembered = preferences
        .favorite_models
        .iter()
        .chain(&preferences.recent_models)
        .filter_map(|key| find(models, key));
    let mut seen = HashSet::new();
    remembered
        .chain(models)
        .filter(|model| model.authenticated)
        .filter(|model| seen.insert((&model.provider, &model.model_id)))
        .collect()
}

/// Picker rows before a search: Favorites, Recent, then each signed-in
/// provider. A favorite is also listed under its provider, so each provider's
/// group stays complete.
pub(super) fn picker_items(
    models: &[Model],
    preferences: &Preferences,
    default: Option<&ModelChoice>,
) -> Vec<Item> {
    let item = |model: &Model, section: &str| {
        let choice = model.choice();
        let item = Item::new(
            model_key(model),
            model.name.clone(),
            Command::Model(choice.clone()),
        )
        .detail(model_label(&choice))
        .section(section.to_owned())
        .marked(preferences.is_favorite(&choice));
        if default == Some(&choice) {
            item.badge("Default", colors().mint)
        } else {
            // One step from any row; the default row carries the marker instead.
            item.aside("Set default")
        }
    };
    let mut items = favorites(models, preferences)
        .map(|model| item(model, "Favorites"))
        .collect::<Vec<_>>();
    items.extend(signed_in(models, &preferences.recent_models).map(|model| item(model, "Recent")));

    // Providers keep the catalog's order.
    let mut groups: Vec<(&str, Vec<&Model>)> = Vec::new();
    let mut positions = HashMap::new();
    for model in models.iter().filter(|model| model.authenticated) {
        let position = *positions.entry(model.provider.as_str()).or_insert_with(|| {
            groups.push((model.provider.as_str(), Vec::new()));
            groups.len() - 1
        });
        groups[position].1.push(model);
    }
    for (provider, members) in groups {
        items.extend(members.into_iter().map(|model| item(model, provider)));
    }
    items
}

impl Plait {
    /// The harness whose models a picker lists: the open thread's for its own
    /// menu, the draft's for a new thread, else Pi.
    fn picker_harness(&self, purpose: Option<ModelPurpose>) -> HarnessId {
        match purpose {
            Some(ModelPurpose::ThreadModel) => self.thread_harness(),
            Some(ModelPurpose::NewThread) => self.draft_harness(),
            _ => HarnessId::Pi,
        }
    }

    /// Pi's catalog, or only the models of the harness the thread runs in.
    fn picker_items(&self, purpose: Option<ModelPurpose>) -> Vec<Item> {
        let Some(status) = self
            .host
            .connection
            .status
            .as_ref()
            .filter(|_| self.host.connection.connected)
        else {
            return Vec::new();
        };
        let harness = self.picker_harness(purpose);
        if harness != HarnessId::Pi {
            return self
                .harness_row(harness)
                .map(|row| super::harnesses::thread_items(row, &self.preferences.value))
                .unwrap_or_default();
        }
        picker_items(
            &status.models,
            &self.preferences.value,
            status.default_model.as_ref(),
        )
    }

    pub(super) fn picker_stamp(&self) -> PickerStamp {
        let status = self.host.connection.status.as_ref();
        PickerStamp {
            connected: self.host.connection.connected,
            models: self.host.connection.catalog_revision,
            default: status.and_then(|status| status.default_model.clone()),
            choosing: self.host.connection.settings_model.clone(),
            saving: self.host.connection.saving,
            message: self.host.connection.save_message.clone(),
            favorites: self.preferences.favorites_revision,
            harnesses: self.harnesses.generation,
        }
    }

    /// Rebuild an open picker when the host state or favorites behind it changed.
    pub(super) fn sync_picker(&mut self, cx: &mut Context<Self>) {
        if self
            .overlay
            .as_ref()
            .is_none_or(|overlay| overlay.stamp.is_none())
        {
            return;
        }
        let stamp = self.picker_stamp();
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        if overlay.stamp.is_none() || overlay.stamp.as_ref() == Some(&stamp) {
            return;
        }
        overlay.stamp = Some(stamp);
        let palette = overlay.palette.clone();
        let purpose = overlay.purpose;
        let items = self.picker_items(purpose);
        let note = self.picker_note(purpose);
        palette.update(cx, |palette, cx| {
            palette.set_items(items, cx);
            palette.set_note(note, cx);
        });
    }

    /// The model a new thread starts with unless another is chosen, in the
    /// default harness from Settings → New threads.
    pub(super) fn new_thread_model(&self) -> Option<&Model> {
        let harness = self.default_harness();
        if harness != HarnessId::Pi {
            let choice = self.default_model_for(harness)?;
            return self
                .harness_model(harness, &choice)
                .filter(|model| model.authenticated);
        }
        let status = self.host.connection.status.as_ref()?;
        let default = status.default_model.as_ref()?;
        let usable = self.host.available(Some(default));
        status
            .models
            .iter()
            .find(|model| usable && model.choice() == *default)
    }

    /// What the idle workspace says about the next thread's model.
    pub(super) fn new_thread_note(&self) -> Option<String> {
        self.new_thread_model()
            .map(|model| format!("New threads use {}", model.name))
    }

    pub(super) fn model_name(&self, choice: &ModelChoice) -> String {
        let models = self
            .host
            .connection
            .status
            .as_ref()
            .map(|status| &status.models);
        models
            .and_then(|models| models.iter().find(|model| model.choice() == *choice))
            .or_else(|| self.harness_model(self.thread_harness(), choice))
            .map_or_else(|| model_label(choice), |model| model.name.clone())
    }

    /// Where a default-model change stands, in words and with its accent.
    /// The host's reply decides; nothing is reported saved before it.
    pub(super) fn default_model_state(&self) -> Option<(String, u32)> {
        let saved = self.host.connection.status.as_ref()?.default_model.as_ref();
        let choosing = self.host.connection.settings_model.as_ref();
        if self.host.connection.saving {
            let name = choosing.map(|choice| self.model_name(choice));
            return Some((
                format!("Saving {} as the default…", name.unwrap_or_default()),
                colors().blue,
            ));
        }
        if let Some(choice) = choosing.filter(|choice| Some(*choice) != saved) {
            let reason = self
                .host
                .connection
                .save_message
                .clone()
                .unwrap_or_default();
            return Some((
                format!(
                    "{} is not saved as the default. {reason} Choose it again to retry.",
                    self.model_name(choice)
                ),
                colors().amber,
            ));
        }
        self.host
            .connection
            .save_message
            .clone()
            .map(|message| (message, colors().muted))
    }

    fn picker_note(&self, purpose: Option<ModelPurpose>) -> Option<SharedString> {
        self.host
            .connection
            .status
            .as_ref()
            .filter(|_| self.host.connection.connected)?;
        if self.picker_harness(purpose) != HarnessId::Pi {
            return Some("Threads stay in the harness they started in.".into());
        }
        self.default_model_state().map(|(state, _)| state.into())
    }

    /// Why the picker has nothing to list. Only real host state is described.
    fn picker_empty(&self, purpose: ModelPurpose) -> &'static str {
        if self.host.connection.connecting {
            "Connecting to the local host…"
        } else if !self.host.connection.connected {
            "Not connected to the local host, so there is no model catalog. Reconnect from Settings (⌘,)."
        } else if self.picker_harness(Some(purpose)) == HarnessId::Pi {
            // The picker lists signed-in models only, so an empty Pi list means none is.
            "No model is signed in. Open Settings (⌘,) → Providers to sign in."
        } else {
            "The host returned no models. Settings (⌘,) links to the setup guide."
        }
    }

    /// Reload the model catalog when the host published providers after it was loaded, such as
    /// keys from the login shell that the host read after it started. An open picker follows
    /// the new catalog through `sync_picker`.
    pub(super) fn refresh_stale_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.host.connection.begin_catalog_refresh() else {
            return;
        };
        let epoch = self.host.connection.epoch;
        self.settings_request::<_, plait_client::host::Status>(
            plait_client::host::Request::Status {
                protocol_version: Some(self.host.protocol()),
            },
            cx,
            move |this, result, cx| {
                let loaded = match result {
                    Ok(status) => this.host.apply_status(epoch, status),
                    Err(_) => false,
                };
                if loaded {
                    this.sync_composer(cx);
                }
                this.host.connection.finish_catalog_refresh(at, loaded);
                // A newer registry may have been published meanwhile. After a failure, the
                // next workspace change or picker opening tries again; nothing polls.
                if loaded {
                    this.refresh_stale_catalog(cx);
                }
            },
        );
    }

    pub(super) fn open_model_picker(
        &mut self,
        purpose: ModelPurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_stale_catalog(cx);
        let (name, placeholder) = match purpose {
            ModelPurpose::NewThread => ("Model for the new thread", "Search models…"),
            ModelPurpose::DefaultModel => {
                ("Choose the default model", "Search models for the default…")
            }
            ModelPurpose::ThreadModel => ("Change thread model", "Search models for this thread…"),
        };
        let status = self
            .host
            .connection
            .status
            .as_ref()
            .filter(|_| self.host.connection.connected);
        let drafted = self.draft_model();
        let preferred = match purpose {
            ModelPurpose::ThreadModel => self
                .host
                .thread
                .snapshot
                .as_ref()
                .map(|snapshot| &snapshot.model),
            ModelPurpose::NewThread => drafted.as_ref(),
            ModelPurpose::DefaultModel => status.and_then(|status| status.default_model.as_ref()),
        };
        let harness = self.picker_harness(Some(purpose));
        let default = status.and_then(|status| {
            let default = preferred?;
            let model = if harness == HarnessId::Pi {
                status
                    .models
                    .iter()
                    .find(|model| model.choice() == *default)?
            } else {
                self.harness_model(harness, default)?
            };
            Some(model_key(model))
        });
        let options = Options {
            name: name.into(),
            placeholder: placeholder.into(),
            empty: self.picker_empty(purpose).into(),
            initial: default.map(Into::into),
            mark: Some("Favorite".into()),
            aside: Some("Set default".into()),
            note: self.picker_note(Some(purpose)),
        };
        let items = self.picker_items(Some(purpose));
        let palette = cx.new(|cx| Palette::new(options, items, window, cx).picker());
        self.present(palette, Some(purpose), window, cx);
    }

    pub(super) fn toggle_favorite_model(&mut self, choice: &ModelChoice, cx: &mut Context<Self>) {
        let mut value = self.preferences.value.clone();
        value.toggle_favorite(choice);
        // The open picker follows through `sync_picker`.
        self.set_preferences(value, PreferenceGroup::Models, cx);
    }

    /// Called once the host confirms a thread was created with `choice`, in any
    /// harness. Each list shows only its own models, and a new thread in a
    /// harness other than Pi starts with the one used most recently.
    pub(super) fn record_recent_model(&mut self, choice: &ModelChoice, cx: &mut Context<Self>) {
        let mut value = self.preferences.value.clone();
        value.record_recent(choice);
        self.set_preferences(value, PreferenceGroup::Models, cx);
    }

    /// Put the keyboard in the selected thread's composer, or the draft's.
    pub(super) fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(composer) = self.visible_composer() {
            window.focus(&composer.focus_handle(cx), cx);
        }
    }

    /// The composer the conversation tab shows: the selected thread's, or the draft's.
    pub(super) fn visible_composer(&self) -> Option<&Entity<Composer>> {
        match &self.host.thread.selected {
            Some(id) => self.workspace.composers.get(id),
            None => self.draft.composer.as_ref().filter(|_| self.draft.open),
        }
    }

    /// Save the picked model as the host's default. The Settings row reports the result.
    pub(super) fn choose_default_model(
        &mut self,
        choice: ModelChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.connection.saving || !self.host.available(Some(&choice)) {
            return;
        }
        let saved = self
            .host
            .connection
            .status
            .as_ref()
            .and_then(|s| s.default_model.as_ref());
        if saved == Some(&choice) && self.host.connection.settings_model.as_ref() == Some(&choice) {
            return;
        }
        self.host.connection.settings_model = Some(choice);
        self.save_default_model(window, cx);
    }

    /// Thread creation in progress or unconfirmed. There is no page for it, so
    /// the workspace says so wherever the user is.
    pub(super) fn creation_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = || {
            row()
                .flex_shrink_0()
                .px_4()
                .py_2()
                .gap_3()
                .border_b_1()
                .border_color(rgb(colors().border))
                .text_size(px(12.))
        };
        if self.worktree_busy {
            return Some(
                bar()
                    .child(dot(colors().blue))
                    .child(
                        div()
                            .text_color(rgb(colors().muted))
                            .child("Preparing worktree…"),
                    )
                    .into_any_element(),
            );
        }
        if self.draft_isolation_choice() {
            if self.draft_visible() {
                return None;
            }
            return Some(
                bar()
                    .child(a11y_text(
                        "admission-choice-notice",
                        "Project busy. Choose where the new thread starts.",
                    ))
                    .child(
                        button("admission-return", "Choose checkout")
                            .on_activate(cx, |this, window, cx| {
                                this.open_draft_content(window, cx)
                            }),
                    )
                    .into_any_element(),
            );
        }
        // The draft says how its own creation stands, under its composer.
        if self.draft_visible() && self.draft_pending() {
            return None;
        }
        if self.host.creation.creating {
            let model = self
                .host
                .creation
                .pending_create
                .as_ref()
                .map(|(_, model)| model);
            let label = model.map_or_else(
                || "Creating thread…".to_owned(),
                |model| format!("Creating thread with {}…", model_label(model)),
            );
            return Some(
                bar()
                    .child(dot(colors().blue))
                    .child(
                        div()
                            .text_color(rgb(colors().muted))
                            .child(a11y_text("creation-bar-status", label)),
                    )
                    .into_any_element(),
            );
        }
        let failure = self.host.connection.error.clone();
        if let Some((_, model)) = &self.host.creation.pending_create {
            let detail = failure.unwrap_or_else(|| "The host did not answer.".into());
            let connected = self.host.connection.connected;
            return Some(
                bar()
                    .child(dot(colors().amber))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(colors().amber))
                            .child(a11y_text(
                                "creation-bar-unconfirmed",
                                format!(
                                    "Thread with {} is not confirmed. {detail}",
                                    model_label(model)
                                ),
                            )),
                    )
                    .child(if connected {
                        toned_button("retry-create-thread", "Retry thread creation", Tone::Filled)
                            .on_activate(cx, |this, window, cx| this.retry_create(window, cx))
                    } else {
                        toned_button("reconnect-to-create", "Reconnect", Tone::Filled)
                            .disabled(self.host.connection.connecting)
                            .on_activate(cx, |this, window, cx| this.reconnect_host(window, cx))
                    })
                    .into_any_element(),
            );
        }
        // A rejected request. The conversation view shows host errors itself.
        let idle = self.new_thread_open && self.host.thread.selected.is_none();
        let error = failure.filter(|_| idle)?;
        Some(
            bar()
                .child(dot(colors().red))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(rgb(colors().red))
                        .child(a11y_text(
                            "creation-bar-error",
                            format!("Thread not created. {error}"),
                        )),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Named imports: a glob would bring in GPUI's own `test` attribute.
    use super::{Item, Model, ModelChoice, PickerStamp, Preferences, picker_items, usable_models};

    #[test]
    fn picker_stamp_tracks_catalog_and_favorite_revisions() {
        let stamp = PickerStamp {
            connected: true,
            models: 1,
            default: None,
            choosing: None,
            saving: false,
            message: None,
            favorites: 1,
            harnesses: 0,
        };
        assert_eq!(stamp, stamp.clone());
        let mut changed = stamp.clone();
        changed.models += 1;
        assert_ne!(stamp, changed);
        changed = stamp.clone();
        changed.favorites += 1;
        assert_ne!(stamp, changed);
    }

    fn model(provider: &str, model_id: &str, authenticated: bool) -> Model {
        Model {
            provider: provider.into(),
            model_id: model_id.into(),
            name: model_id.to_uppercase(),
            authenticated,
            ..Default::default()
        }
    }

    /// Signed-in models first, as the host state sorts the catalog.
    fn catalog() -> Vec<Model> {
        vec![
            model("openai", "luna", true),
            model("openai", "sol", true),
            model("router", "vendor/opus", true),
            model("google", "gemini", false),
            model("router", "vendor/preview", false),
            model("google", "flash", false),
        ]
    }

    fn choice(provider: &str, model_id: &str) -> ModelChoice {
        ModelChoice {
            provider: provider.into(),
            model_id: model_id.into(),
        }
    }

    fn listed(items: &[Item]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|item| (item.section.to_string(), item.detail.to_string()))
            .collect()
    }

    fn pair(section: &str, detail: &str) -> (String, String) {
        (section.to_owned(), detail.to_owned())
    }

    #[test]
    fn only_signed_in_models_are_listed_grouped_by_provider() {
        let items = picker_items(&catalog(), &Preferences::default(), None);
        assert_eq!(
            listed(&items),
            vec![
                pair("openai", "openai/luna"),
                pair("openai", "openai/sol"),
                pair("router", "router/vendor/opus"),
            ]
        );
        assert_eq!(items[0].title.as_ref(), "LUNA");
        assert!(items.iter().all(|item| item.enabled));
    }

    #[test]
    fn the_default_is_badged_and_a_signed_out_default_is_not_listed() {
        let default = choice("openai", "sol");
        let items = picker_items(&catalog(), &Preferences::default(), Some(&default));
        let badge = |detail: &str| {
            let item = items
                .iter()
                .find(|item| item.detail.as_ref() == detail)
                .unwrap();
            item.badge.as_ref().map(|(text, _)| text.to_string())
        };
        assert_eq!(badge("openai/luna"), None);
        assert_eq!(badge("openai/sol"), Some("Default".into()));
        let stale = choice("google", "flash");
        let items = picker_items(&catalog(), &Preferences::default(), Some(&stale));
        assert!(
            items
                .iter()
                .all(|item| item.detail.as_ref() != "google/flash")
        );
        assert!(items.iter().all(|item| item.badge.is_none()));
    }

    #[test]
    fn favorites_and_recents_lead_and_stay_listed_under_their_provider() {
        let mut preferences = Preferences::default();
        preferences.toggle_favorite(&choice("router", "vendor/opus"));
        // Not signed in: kept, and shown again once its provider signs in.
        preferences.toggle_favorite(&choice("google", "gemini"));
        // No longer in the catalog: remembered, but not shown.
        preferences.toggle_favorite(&choice("gone", "model"));
        preferences.toggle_favorite(&choice("openai", "luna"));
        preferences.record_recent(&choice("openai", "sol"));
        preferences.record_recent(&choice("google", "flash"));
        preferences.record_recent(&choice("gone", "model"));
        let items = picker_items(&catalog(), &preferences, None);
        assert_eq!(
            listed(&items),
            vec![
                pair("Favorites", "router/vendor/opus"),
                pair("Favorites", "openai/luna"),
                pair("Recent", "openai/sol"),
                pair("openai", "openai/luna"),
                pair("openai", "openai/sol"),
                pair("router", "router/vendor/opus"),
            ]
        );
        // The star shows the same state in every section a model appears in.
        for item in &items {
            let favorite = ["router/vendor/opus", "openai/luna"].contains(&item.detail.as_ref());
            assert_eq!(item.marked, Some(favorite), "{}", item.detail);
        }
        // One model has one key wherever it is listed, so a search shows it once.
        let opus = items
            .iter()
            .filter(|item| item.detail.as_ref() == "router/vendor/opus")
            .map(|item| item.key.clone())
            .collect::<Vec<_>>();
        assert_eq!(opus.len(), 2);
        assert_eq!(opus[0], opus[1]);

        let mut signed_in = catalog();
        signed_in[3].authenticated = true;
        let items = picker_items(&signed_in, &preferences, None);
        assert_eq!(listed(&items)[1], pair("Favorites", "google/gemini"));
    }

    #[test]
    fn a_slash_in_a_model_id_cannot_collide_with_another_provider() {
        let models = vec![model("a/b", "c", true), model("a", "b/c", true)];
        let items = picker_items(&models, &Preferences::default(), None);
        assert_eq!(items[0].detail, items[1].detail);
        assert_ne!(items[0].key, items[1].key);
    }

    #[test]
    fn usable_models_are_signed_in_and_ordered_by_how_they_were_used() {
        let catalog = catalog();
        let mut preferences = Preferences::default();
        let order = |preferences: &Preferences| {
            usable_models(&catalog, preferences)
                .into_iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&preferences), vec!["luna", "sol", "vendor/opus"]);
        preferences.record_recent(&choice("openai", "sol"));
        preferences.toggle_favorite(&choice("router", "vendor/opus"));
        preferences.toggle_favorite(&choice("google", "gemini"));
        preferences.record_recent(&choice("router", "vendor/opus"));
        assert_eq!(order(&preferences), vec!["vendor/opus", "sol", "luna"]);
    }
}
