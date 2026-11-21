//! Provider configuration is requested from the host. Secrets live only in forms.
use super::{Plait, settings, text_field::TextField, theme::*};
use gpui::{prelude::*, *};
use plait_core::host::{
    self,
    providers::{self, Provider, ProviderAction, Request, Secret},
    sign_in::{self, SignIn, SignInReply},
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

#[derive(Default)]
pub(super) struct ProviderState {
    pub rows: Vec<Provider>,
    pub loaded: bool,
    pub busy: bool,
    pub unsupported: bool,
    pub note: Option<String>,
    search: Option<Entity<TextField>>,
    all: bool,
    generation: u64,
    epoch: Option<u64>,
    pub form: Option<Form>,
}

pub(super) enum Form {
    Key {
        provider: String,
        field: Entity<TextField>,
    },
    Flow {
        state: SignIn,
        field: Entity<TextField>,
    },
    Endpoint(EndpointForm),
    Remove {
        id: String,
        name: String,
    },
}

pub(super) struct EndpointForm {
    pub id: Option<String>,
    pub name: Entity<TextField>,
    pub url: Entity<TextField>,
    pub models: Entity<TextField>,
    pub key: Entity<TextField>,
    pub checked: Option<(String, providers::EndpointCheck)>,
}

impl EndpointForm {
    pub(super) fn current_check(&self, cx: &App) -> Option<&providers::EndpointCheck> {
        self.checked
            .as_ref()
            .and_then(|(url, check)| (self.url.read(cx).text().trim() == url).then_some(check))
    }
}

fn field(
    name: &'static str,
    placeholder: &'static str,
    value: &str,
    cx: &mut Context<Plait>,
) -> Entity<TextField> {
    let input = cx.new(|cx| {
        let mut input = TextField::new(name, placeholder, cx);
        input.set_text(value, cx);
        input
    });
    cx.observe(&input, |_, _, cx| cx.notify()).detach();
    input
}

fn secret(name: &'static str, cx: &mut Context<Plait>) -> Entity<TextField> {
    let input = cx.new(|cx| TextField::new(name, "Paste here", cx).secret());
    cx.observe(&input, |_, _, cx| cx.notify()).detach();
    input
}

impl Plait {
    pub(super) fn settings_request<
        R: Serialize + Send + 'static,
        T: DeserializeOwned + Send + 'static,
    >(
        &mut self,
        request: R,
        cx: &mut Context<Self>,
        complete: impl FnOnce(&mut Self, Result<T, host::Error>, &mut Context<Self>) + 'static,
    ) {
        let epoch = self.host.connection.epoch;
        let client = self.host_client();
        let task = cx
            .background_executor()
            .spawn(async move { client.send(&request) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.host.connection.epoch != epoch {
                    return;
                }
                complete(this, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn sync_provider_settings(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        if self.providers.epoch != Some(self.host.connection.epoch) {
            let generation = self.providers.generation + 1;
            self.providers = ProviderState {
                generation,
                epoch: Some(self.host.connection.epoch),
                ..Default::default()
            };
        }
        if self.host.connection.connected
            && !self.providers.loaded
            && !self.providers.busy
            && self.providers.note.is_none()
        {
            self.prepare_providers(cx);
        }
        if !self.settings_open && self.providers.form.is_some() {
            self.close_provider_form(cx);
        }
        if !self.settings_open && self.keyboard.recording.is_some() {
            self.cancel_recording(cx);
        }
    }

    pub(super) fn prepare_providers(&mut self, cx: &mut Context<Self>) {
        if self.providers.search.is_none() {
            let input =
                cx.new(|cx| TextField::new("Find provider", "Find a provider", cx).search());
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            self.providers.search = Some(input);
        }
        if !self.providers.loaded && !self.providers.busy {
            self.load_providers(false, cx);
        }
    }

    pub(super) fn close_provider_form(&mut self, cx: &mut Context<Self>) {
        self.providers.generation += 1;
        let form = self.providers.form.take();
        self.providers.busy = false;
        self.providers.note = None;
        if let Some(Form::Flow { state, .. }) = form
            && state.state.pending()
            && !cfg!(test)
        {
            self.settings_request::<_, SignInReply>(
                Request::CancelSignIn {
                    sign_in_id: state.id,
                },
                cx,
                |_, _, _| {},
            );
        }
        cx.notify();
    }

    pub(super) fn load_providers(&mut self, reload: bool, cx: &mut Context<Self>) {
        if !self.host.connection.connected || cfg!(test) {
            return;
        }
        self.providers.busy = true;
        let generation = self.providers.generation;
        let request = if reload {
            Request::ReloadProviders
        } else {
            Request::ListProviders
        };
        self.settings_request::<_, providers::ProvidersReply>(
            request,
            cx,
            move |this, result, cx| {
                if this.providers.generation != generation {
                    return;
                }
                this.providers.busy = false;
                match result {
                    Ok(reply) => {
                        this.providers.rows = reply.providers;
                        this.providers.loaded = true;
                        this.providers.unsupported = false;
                    }
                    Err(error) => {
                        this.providers.unsupported = providers::needs_host_update(&error);
                        this.providers.note = Some(if this.providers.unsupported {
                            "Update the host to manage providers here.".into()
                        } else {
                            providers::refusal(&error)
                        });
                    }
                }
                if reload {
                    this.refresh_provider_catalog(cx);
                }
            },
        );
    }

    fn refresh_provider_catalog(&mut self, cx: &mut Context<Self>) {
        self.settings_request::<_, host::Status>(
            host::Request::Status {
                protocol_version: Some(self.host.protocol()),
            },
            cx,
            |this, result, cx| match result {
                Ok(status) => {
                    this.host.apply_status(this.host.connection.epoch, status);
                    this.sync_composer(cx);
                }
                Err(error) => this.providers.note = Some(error.to_string()),
            },
        );
    }

    fn provider_mutation<T: DeserializeOwned + Send + 'static>(
        &mut self,
        request: Request,
        cx: &mut Context<Self>,
    ) {
        self.providers.busy = true;
        self.providers.note = None;
        let generation = self.providers.generation;
        self.settings_request::<_, T>(request, cx, move |this, result, cx| match result {
            Ok(_) => {
                if this.providers.generation == generation {
                    this.close_provider_form(cx);
                    this.providers.note = Some("Saved on the host.".into());
                }
                this.load_providers(false, cx);
                this.refresh_provider_catalog(cx);
            }
            Err(error) if this.providers.generation == generation => {
                this.providers.busy = false;
                this.providers.note = Some(providers::refusal(&error));
            }
            Err(_) => {}
        });
    }

    fn provider_action(
        &mut self,
        provider: Provider,
        action: ProviderAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_provider_form(cx);
        match action {
            ProviderAction::SignIn => self.start_provider_sign_in(provider.id, cx),
            ProviderAction::EnterKey => {
                let field = secret("Provider API key", cx);
                window.focus(&field.focus_handle(cx), cx);
                self.providers.form = Some(Form::Key {
                    provider: provider.id,
                    field,
                })
            }
            ProviderAction::SignOut => self.provider_mutation::<providers::ProviderReply>(
                Request::SignOutProvider {
                    provider: provider.id,
                },
                cx,
            ),
            ProviderAction::Edit => self.edit_endpoint(Some(provider), window, cx),
            ProviderAction::Check => self.check_provider_endpoint(provider.id, cx),
            ProviderAction::Remove => {
                self.providers.form = Some(Form::Remove {
                    id: provider.id,
                    name: provider.name,
                })
            }
        }
        cx.notify();
    }

    pub(super) fn start_provider_sign_in(&mut self, provider: String, cx: &mut Context<Self>) {
        self.providers.busy = true;
        let generation = self.providers.generation;
        self.settings_request::<_, SignInReply>(
            Request::StartSignIn {
                provider,
                method: None,
            },
            cx,
            move |this, result, cx| {
                if this.providers.generation == generation {
                    this.sign_in_reply(result, cx);
                } else if let Ok(reply) = result {
                    this.settings_request::<_, SignInReply>(
                        Request::CancelSignIn {
                            sign_in_id: reply.sign_in.id,
                        },
                        cx,
                        |_, _, _| {},
                    );
                }
            },
        );
    }

    fn sign_in_reply(&mut self, result: Result<SignInReply, host::Error>, cx: &mut Context<Self>) {
        self.providers.busy = false;
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                self.providers.note = Some(providers::refusal(&error));
                return;
            }
        };
        let view = sign_in::view(&reply.sign_in);
        let field = match self.providers.form.take() {
            Some(Form::Flow { field, .. }) => field,
            _ => secret("Sign-in answer", cx),
        };
        let id = reply.sign_in.id.clone();
        self.providers.form = Some(Form::Flow {
            state: reply.sign_in,
            field,
        });
        if view.signed_in {
            self.load_providers(false, cx);
            self.refresh_provider_catalog(cx);
        }
        if view.poll && self.settings_open {
            self.poll_sign_in(id, cx);
        }
    }

    fn poll_sign_in(&mut self, id: String, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let generation = self.providers.generation;
        let epoch = self.host.connection.epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update(cx, |this, cx| {
                if this.providers.generation != generation || this.host.connection.epoch != epoch || !this.settings_open { return; }
                if !matches!(&this.providers.form, Some(Form::Flow { state, .. }) if state.id == id && state.state.pending()) { return; }
                this.settings_request::<_, SignInReply>(Request::SignInStatus { sign_in_id: id }, cx, move |this, result, cx| {
                    if this.providers.generation == generation { this.sign_in_reply(result, cx); }
                });
            });
        }).detach();
    }

    fn answer_sign_in(&mut self, answer: Option<String>, cx: &mut Context<Self>) {
        let Some(Form::Flow { state, field }) = &self.providers.form else {
            return;
        };
        let id = state.id.clone();
        let input = answer
            .map(Secret::new)
            .unwrap_or_else(|| field.update(cx, |field, cx| field.take_secret(cx)));
        self.providers.busy = true;
        // Invalidate an in-flight status poll before answering this prompt.
        self.providers.generation += 1;
        let generation = self.providers.generation;
        self.settings_request::<_, SignInReply>(
            Request::AnswerSignIn {
                sign_in_id: id,
                input,
            },
            cx,
            move |this, result, cx| {
                if this.providers.generation == generation {
                    this.sign_in_reply(result, cx);
                }
            },
        );
    }

    fn edit_endpoint(
        &mut self,
        provider: Option<Provider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let endpoint = provider.as_ref().and_then(|p| p.endpoint.as_ref());
        self.providers.form = Some(Form::Endpoint(EndpointForm {
            id: provider.as_ref().map(|p| p.id.clone()),
            name: field(
                "Endpoint name",
                "Server name",
                provider.as_ref().map_or("", |p| p.name.as_str()),
                cx,
            ),
            url: field(
                "Endpoint base URL",
                "http://localhost:8000/v1",
                endpoint.and_then(|e| e.base_url.as_deref()).unwrap_or(""),
                cx,
            ),
            models: field(
                "Endpoint model IDs",
                "Model IDs, separated by commas",
                &endpoint.map(|e| e.model_ids.join(", ")).unwrap_or_default(),
                cx,
            ),
            key: secret("Endpoint API key", cx),
            checked: None,
        }));
        if let Some(Form::Endpoint(form)) = &self.providers.form {
            window.focus(&form.name.focus_handle(cx), cx);
        }
    }

    fn save_endpoint(&mut self, cx: &mut Context<Self>) {
        let Some(Form::Endpoint(form)) = &self.providers.form else {
            return;
        };
        let draft = providers::EndpointDraft {
            name: form.name.read(cx).text().trim().into(),
            base_url: form.url.read(cx).text().trim().into(),
            model_ids: providers::parse_model_ids(form.models.read(cx).text()),
        };
        if let Some(problem) = draft.problem() {
            self.providers.note = Some(problem.into());
            cx.notify();
            return;
        }
        let key = (!form.key.read(cx).text().is_empty())
            .then(|| form.key.update(cx, |field, cx| field.take_secret(cx)));
        let request = if let Some(id) = &form.id {
            Request::UpdateEndpoint {
                id: id.clone(),
                name: Some(draft.name),
                base_url: Some(draft.base_url),
                model_ids: Some(draft.model_ids),
                key,
            }
        } else {
            Request::AddEndpoint {
                id: providers::endpoint_id(&draft.name),
                name: draft.name,
                base_url: draft.base_url,
                model_ids: draft.model_ids,
                key,
            }
        };
        self.provider_mutation::<providers::ProviderReply>(request, cx);
    }

    fn check_provider_endpoint(&mut self, id: String, cx: &mut Context<Self>) {
        self.providers.busy = true;
        let generation = self.providers.generation;
        self.settings_request::<_, providers::EndpointCheck>(
            Request::CheckSaved { id },
            cx,
            move |this, result, _| {
                if this.providers.generation != generation {
                    return;
                }
                this.providers.busy = false;
                this.providers.note = Some(match result {
                    Ok(check) => check.summary(),
                    Err(error) => providers::refusal(&error),
                });
            },
        );
    }

    fn check_endpoint_form(&mut self, cx: &mut Context<Self>) {
        let Some(Form::Endpoint(form)) = &self.providers.form else {
            return;
        };
        let base_url = form.url.read(cx).text().trim().to_owned();
        if let Some(problem) = providers::url_problem(&base_url) {
            self.providers.note = Some(problem.into());
            cx.notify();
            return;
        }
        let key = (!form.key.read(cx).text().is_empty())
            .then(|| form.key.update(cx, |field, cx| field.take_secret(cx)));
        if let Some(Form::Endpoint(form)) = &mut self.providers.form {
            form.checked = None;
        }
        self.providers.busy = true;
        let generation = self.providers.generation;
        let tested_url = base_url.clone();
        self.settings_request::<_, providers::EndpointCheck>(
            Request::CheckAddress { base_url, key },
            cx,
            move |this, result, cx| {
                if this.providers.generation != generation {
                    return;
                }
                this.providers.busy = false;
                let Some(Form::Endpoint(form)) = &mut this.providers.form else {
                    return;
                };
                if form.url.read(cx).text().trim() != tested_url {
                    this.providers.note = Some("The address changed. Check it again.".into());
                    return;
                }
                match result {
                    Ok(check) => {
                        form.checked = Some((tested_url, check));
                    }
                    Err(error) => this.providers.note = Some(providers::refusal(&error)),
                }
            },
        );
    }

    fn secret_control(&self, input: &Entity<TextField>, cx: &mut Context<Self>) -> Div {
        let reveal = input.read(cx).revealed();
        let field = input.clone();
        row()
            .gap_2()
            .child(div().flex_1().min_w_0().child(input.clone()))
            .child(
                button(
                    "reveal-provider-secret",
                    if reveal { "Hide" } else { "Reveal" },
                )
                .on_activate(cx, move |_, _, cx| {
                    field.update(cx, |field, cx| field.reveal(cx))
                }),
            )
    }

    fn provider_form(&self, cx: &mut Context<Self>) -> Div {
        let mut panel = column()
            .gap_3()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(colors().border));
        let enabled = !self.providers.busy && self.host.connection.connected;
        match self.providers.form.as_ref() {
            Some(Form::Key { provider, field }) => {
                panel = panel.child(div().child(format!("API key for {provider}")))
                    .child(tag("provider-key-note", "An API key replaces this provider's subscription sign-in. A new sign-in replaces the key.", colors().muted))
                    .child(self.secret_control(field, cx))
                    .child(button("save-provider-key", "Save key").disabled(!enabled || field.read(cx).text().is_empty()).when(enabled, |el| el.on_activate(cx, |this, _, cx| {
                        let Some(Form::Key { provider, field }) = &this.providers.form else { return; };
                        let provider = provider.clone();
                        let key = field.update(cx, |field, cx| field.take_secret(cx));
                        if !key.is_empty() { this.provider_mutation::<providers::ProviderReply>(Request::SetProviderKey { provider, key }, cx); }
                    })));
            }
            Some(Form::Flow { state, field }) => {
                let view = sign_in::view(state);
                panel = panel
                    .child(div().font_weight(FontWeight::MEDIUM).child(view.status))
                    .when_some(view.detail, |el, detail| {
                        el.child(tag("provider-flow-detail", detail, colors().muted))
                    })
                    .when_some(view.remaining, |el, remaining| {
                        el.child(tag("provider-flow-remaining", remaining, colors().subtle))
                    });
                if let Some(destination) = view.destination {
                    let url = destination.url;
                    let copy = url.clone();
                    panel = panel
                        .child(tag(
                            "provider-flow-destination",
                            format!("Browser destination: {}", destination.host),
                            colors().muted,
                        ))
                        .child(
                            row()
                                .gap_2()
                                .child(
                                    button("open-provider-sign-in", "Open in browser")
                                        .on_activate(cx, move |_, _, cx| cx.open_url(&url)),
                                )
                                .child(button("copy-provider-url", "Copy link").on_activate(
                                    cx,
                                    move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    },
                                )),
                        );
                }
                if view.unusable_address {
                    panel = panel.child(tag(
                        "provider-flow-unusable-address",
                        "The host returned an unsupported sign-in address.",
                        colors().amber,
                    ));
                }
                if let Some(code) = view.user_code {
                    panel = panel.child(
                        row()
                            .gap_2()
                            .child(div().font_family("Menlo").child(code.clone()))
                            .child(button("copy-provider-code", "Copy code").on_activate(
                                cx,
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                                },
                            )),
                    );
                }
                if let Some(prompt) = view.prompt {
                    match prompt {
                        sign_in::Prompt::Field {
                            label,
                            optional,
                            submit,
                            ..
                        } => {
                            panel = panel
                                .child(tag(
                                    "provider-flow-prompt-label",
                                    if optional {
                                        format!("{label} (optional)")
                                    } else {
                                        label
                                    },
                                    colors().muted,
                                ))
                                .child(self.secret_control(field, cx))
                                .child(
                                    button("answer-provider-sign-in", submit)
                                        .disabled(!enabled)
                                        .when(enabled, |el| {
                                            el.on_activate(cx, |this, _, cx| {
                                                this.answer_sign_in(None, cx)
                                            })
                                        }),
                                );
                        }
                        sign_in::Prompt::Choice { label, choices } => {
                            panel = panel.child(tag(
                                "provider-flow-choice-label",
                                label,
                                colors().muted,
                            ));
                            for (index, choice) in choices.into_iter().enumerate() {
                                panel = panel.child(
                                    button(("provider-sign-in-choice", index), choice.label)
                                        .disabled(!enabled)
                                        .when(enabled, |el| {
                                            el.on_activate(cx, move |this, _, cx| {
                                                this.answer_sign_in(Some(choice.id.clone()), cx)
                                            })
                                        }),
                                );
                            }
                        }
                    }
                }
                if view.can_retry {
                    let provider = state.provider.clone();
                    panel = panel.child(button("retry-provider-sign-in", "Try again").on_activate(
                        cx,
                        move |this, _, cx| {
                            this.close_provider_form(cx);
                            this.start_provider_sign_in(provider.clone(), cx);
                        },
                    ));
                }
            }
            Some(Form::Endpoint(form)) => {
                panel = panel.child(div().font_weight(FontWeight::MEDIUM).child(if form.id.is_some() { "Edit endpoint" } else { "Add OpenAI-compatible endpoint" }))
                    .child(tag("endpoint-name-label", "Name", colors().muted)).child(form.name.clone())
                    .child(tag("endpoint-url-label", "Base URL", colors().muted)).child(form.url.clone())
                    .child(tag("endpoint-models-label", "Model IDs", colors().muted)).child(form.models.clone())
                    .child(tag("endpoint-key-label", "Optional API key. Leave blank to keep a saved key. A connection check uses the entered key once; paste it again before saving.", colors().subtle))
                    .child(self.secret_control(&form.key, cx))
                    .child(row().gap_2()
                        .child(button("check-endpoint", "Check connection").disabled(!enabled).when(enabled, |el| el.on_activate(cx, |this, _, cx| this.check_endpoint_form(cx))))
                        .child(button("save-endpoint", "Save endpoint").disabled(!enabled).when(enabled, |el| el.on_activate(cx, |this, _, cx| this.save_endpoint(cx)))));
                if let Some(check) = form.current_check(cx) {
                    panel = panel.child(tag(
                        "endpoint-check-summary",
                        check.summary(),
                        colors().muted,
                    ));
                    let selected = providers::parse_model_ids(form.models.read(cx).text());
                    for (index, id) in check.model_ids.iter().enumerate() {
                        let chosen = selected.contains(id);
                        let id = id.clone();
                        panel = panel.child(
                            button(
                                ("endpoint-model", index),
                                format!("{} {id}", if chosen { "Remove" } else { "Add" }),
                            )
                            .on_activate(cx, move |this, _, cx| {
                                let Some(Form::Endpoint(form)) = &this.providers.form else {
                                    return;
                                };
                                if !form
                                    .current_check(cx)
                                    .is_some_and(|check| check.model_ids.contains(&id))
                                {
                                    return;
                                }
                                let mut ids =
                                    providers::parse_model_ids(form.models.read(cx).text());
                                if ids.contains(&id) {
                                    ids.retain(|value| value != &id);
                                } else {
                                    ids.push(id.clone());
                                }
                                form.models
                                    .update(cx, |field, cx| field.set_text(&ids.join(", "), cx));
                            }),
                        );
                    }
                }
            }
            Some(Form::Remove { id, name }) => {
                let id = id.clone();
                panel = panel.child(tag("provider-remove-confirm-note", format!("Remove {name} and its saved key from this host? Existing threads remain."), colors().muted))
                    .child(button("remove-endpoint-confirm", "Remove endpoint").disabled(!enabled).when(enabled, |el| el.on_activate(cx, move |this, _, cx| this.provider_mutation::<providers::RemovedReply>(Request::RemoveEndpoint { id: id.clone() }, cx))));
            }
            None => return column(),
        }
        panel.child(row().child(
            button("close-provider-form", "Back to providers").on_activate(
                cx,
                |this, window, cx| {
                    this.close_provider_form(cx);
                    if let Some(search) = &this.providers.search {
                        window.focus(&search.focus_handle(cx), cx);
                    }
                },
            ),
        ))
    }

    pub(super) fn provider_settings(&self, cx: &mut Context<Self>) -> Div {
        if self.providers.form.is_some() {
            return settings::section("Providers")
                .when(self.providers.busy, |el| {
                    el.child(tag(
                        "providers-form-busy",
                        "Waiting for the host…",
                        colors().muted,
                    ))
                })
                .when_some(self.providers.note.clone(), |el, note| {
                    el.child(tag("providers-form-note", note, colors().amber).line_height(px(18.)))
                })
                .child(self.provider_form(cx));
        }
        let mut section = settings::section("Providers")
            .child(tag(
                "providers-intro",
                "Sign in with a subscription, use an API key, or connect your own model server. Credentials are stored by the host.",
                colors().muted,
            ))
            .when_some(self.providers.search.clone(), |el, search| el.child(search));
        let enabled =
            self.host.connection.connected && !self.providers.busy && !self.providers.unsupported;
        let query = self
            .providers
            .search
            .as_ref()
            .map_or("", |field| field.read(cx).text());
        let arranged = providers::arrange(&self.providers.rows, query, self.providers.all);
        for index in arranged.rows {
            let provider = &self.providers.rows[index];
            let mut controls = row().gap_1().flex_wrap();
            for (action_index, action) in provider.actions().into_iter().enumerate() {
                let label = match action {
                    ProviderAction::SignIn => "Sign in",
                    ProviderAction::EnterKey => "API key",
                    ProviderAction::SignOut => "Sign out",
                    ProviderAction::Edit => "Edit",
                    ProviderAction::Check => "Check",
                    ProviderAction::Remove => "Remove",
                };
                let provider = provider.clone();
                controls = controls.child(
                    button(("provider-action", index * 8 + action_index), label)
                        .aria_label(format!("{label}: {}", provider.name))
                        .disabled(!enabled)
                        .when(enabled, |el| {
                            el.on_activate(cx, move |this, window, cx| {
                                this.provider_action(provider.clone(), action, window, cx)
                            })
                        }),
                );
            }
            section = section.child(
                column().gap_1().py_2().child(
                    row()
                        .gap_3()
                        .justify_between()
                        .child(
                            column()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(px(13.)).child(provider.name.clone()))
                                .child(tag(
                                    ("provider-summary", index),
                                    format!(
                                        "{} · {}",
                                        provider.credential_label(),
                                        provider.model_label()
                                    ),
                                    colors().subtle,
                                )),
                        )
                        .child(controls),
                ),
            );
        }
        section
            .when(arranged.hidden > 0, |el| {
                el.child(
                    button(
                        "show-all-providers",
                        format!("Show {} more providers", arranged.hidden),
                    )
                    .on_activate(cx, |this, _, cx| {
                        this.providers.all = true;
                        cx.notify();
                    }),
                )
            })
            .child(
                row()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        button("reload-providers", "Reload providers")
                            .disabled(!enabled)
                            .when(enabled, |el| {
                                el.on_activate(cx, |this, _, cx| this.load_providers(true, cx))
                            }),
                    )
                    .child(
                        button("add-endpoint", "Add endpoint")
                            .disabled(!enabled)
                            .when(enabled, |el| {
                                el.on_activate(cx, |this, window, cx| {
                                    this.close_provider_form(cx);
                                    this.edit_endpoint(None, window, cx);
                                })
                            }),
                    ),
            )
            .when(!self.host.connection.connected, |el| {
                el.child(tag(
                    "providers-disconnected",
                    "Connect to the host to manage providers.",
                    colors().amber,
                ))
            })
            .when(self.providers.busy, |el| {
                el.child(tag(
                    "providers-busy",
                    "Waiting for the host…",
                    colors().muted,
                ))
            })
            .when_some(self.providers.note.clone(), |el, note| {
                el.child(tag("providers-note", note, colors().amber).line_height(px(18.)))
            })
            .when(self.providers.form.is_some(), |el| {
                el.child(self.provider_form(cx))
            })
    }
}
