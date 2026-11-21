//! The host stores isolation authorization against the main repository identity.
use super::{Plait, settings, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::FluentBuilder, *};
use plait_core::host::{
    self,
    admission::{self, Request},
};
impl Plait {
    pub(super) fn prepare_isolation_settings(&mut self, cx: &mut Context<Self>) {
        let key = (
            self.host.connection.epoch,
            self.settings_checkout().to_owned(),
        );
        if self.admission.settings_key.as_ref() == Some(&key) || self.admission.settings_busy {
            return;
        }
        self.admission.settings_key = Some(key);
        self.admission.info = None;
        self.admission.note = None;
        if !cfg!(test) {
            self.request_isolation_setting(None, cx);
        }
    }
    fn request_isolation_setting(&mut self, value: Option<bool>, cx: &mut Context<Self>) {
        if self.admission.settings_busy || !self.host.connection.connected {
            return;
        }
        let key = (
            self.host.connection.epoch,
            self.settings_checkout().to_owned(),
        );
        let expected = self.settings_checkout_key(&key.1).cloned();
        let previous = self.admission.info.clone();
        self.admission.settings_busy = true;
        let checkout = key.1.clone();
        let client = self.host_client();
        let task = cx.background_executor().spawn(async move {
            let identity = Self::identify_settings_checkout(&checkout, expected)?;

            let info = if let Some(value) = value {
                let info = previous.ok_or_else(|| {
                    host::Error::Protocol("Load project isolation before changing it.".into())
                })?;
                Some(admission::set(
                    &client,
                    &Request::SetProjectIsolation {
                        checkout: identity.root.clone(),
                        identity: info.project_identity,
                        auto_worktree: value,
                    },
                )?)
            } else {
                admission::info(&client, identity.root.clone())?
            };
            identity.verify().map_err(host::Error::Protocol)?;
            if info
                .as_ref()
                .is_some_and(|info| info.checkout != identity.root)
            {
                return Err(host::Error::Protocol(
                    "Host returned another project's isolation setting.".into(),
                ));
            }
            Ok::<_, host::Error>((info, identity))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.admission.settings_key.as_ref() != Some(&key) {
                    return;
                }
                this.admission.settings_busy = false;
                let result = result.and_then(|(info, identity)| {
                    this.remember_settings_checkout_key(&key, identity)?;
                    Ok(info)
                });
                match result {
                    Ok(info) => {
                        this.admission.info = info;
                        this.admission.note = this
                            .admission
                            .info
                            .is_none()
                            .then(|| "Project isolation needs a newer host.".into());
                    }
                    Err(error) => {
                        this.admission.info = None;
                        this.admission.note = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn isolation_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.admission.info.as_ref().map(|info| info.auto_worktree);
        let enabled =
            current.is_some() && !self.admission.settings_busy && self.host.connection.connected;
        let mut control = segmented();
        for (i,(label,value,tip)) in [("Ask",false,"When this project is busy, choose whether a new thread gets its own worktree."), ("Own worktree",true,"When this project is busy, new threads automatically get a worktree from HEAD. Uncommitted files stay in the original checkout. Only approved build output can be cloned.")].into_iter().enumerate() {
            control = control.child(segment(("isolation-setting",i),label,current==Some(value)).disabled(!enabled)
                .tooltip(move |_,cx| cx.new(|_| TextTooltip(tip.into())).into())
                .when(enabled,|el| el.on_activate(cx,move |this,_,cx| this.request_isolation_setting(Some(value),cx))));
        }
        let control = if current.is_some() {
            control.into_any_element()
        } else {
            tag(
                "isolation-setting-unavailable",
                "Unavailable",
                colors().muted,
            )
            .into_any_element()
        };
        settings::section("New threads")
            .child(self.build_settings(cx))
            .child(settings::setting("When this project is busy", "Applies only to new threads in this project. Worktrees start from HEAD; cleanup is manual.", control))
            .when(self.admission.settings_busy, |el| el.child(tag("isolation-setting-loading", "Loading isolation setting…",colors().muted)))
            .when_some(self.admission.note.clone(),|el,note| el.child(tag("isolation-setting-note", note,colors().amber)))
            .when(current.is_none() && !self.admission.settings_busy, |el| el.child(button("isolation-settings-retry","Retry").on_activate(cx,|this,_,cx| this.request_isolation_setting(None,cx))))
    }
}
