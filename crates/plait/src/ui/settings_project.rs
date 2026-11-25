//! Project settings has a fixed target independent of the selected thread.
use super::theme::ButtonAction;
use super::{Plait, settings, settings_agent::AgentState, settings_policy::PolicyState};
use gpui::{prelude::*, *};
use std::path::{Path, PathBuf};

pub(super) struct ProjectSettings {
    pub checkout: PathBuf,
    pub name: String,
    policy: PolicyState,
    agent: AgentState,
    identity: Option<plait_client::ownership::CheckoutKey>,
}

impl Plait {
    pub(super) fn settings_checkout_key(
        &self,
        checkout: &Path,
    ) -> Option<&plait_client::ownership::CheckoutKey> {
        if checkout == self.host.creation.checkout {
            return self.state.checkout_key();
        }
        self.settings_project
            .as_ref()
            .filter(|project| project.checkout == checkout)
            .and_then(|project| project.identity.as_ref())
    }

    /// Resolve and verify the checkout on the host that owns it.
    pub(super) fn identify_settings_checkout(
        client: &plait_client::host::Client,
        checkout: &Path,
        supported: bool,
        _local: bool,
        expected: Option<plait_client::ownership::CheckoutKey>,
    ) -> Result<plait_client::ownership::CheckoutKey, plait_client::host::Error> {
        if !supported {
            return Err(plait_client::host::Error::Protocol(
                "Update the host to verify the checkout.".into(),
            ));
        }
        let id = expected
            .as_ref()
            .map(|key| plait_protocol::lease::DirIdentity {
                dev: key.device,
                ino: key.inode,
            });
        let read = plait_client::host::git::identity(client, checkout, id)?;
        Ok(plait_client::ownership::CheckoutKey {
            host: expected.and_then(|key| key.host),
            root: read.result.root,
            device: read.identity.dev,
            inode: read.identity.ino,
        })
    }

    pub(super) fn remember_settings_checkout_key(
        &mut self,
        key: &(u64, PathBuf),
        identity: plait_client::ownership::CheckoutKey,
    ) -> Result<(), plait_client::host::Error> {
        if self.host.connection.epoch != key.0
            || self
                .settings_checkout_key(&key.1)
                .is_some_and(|expected| expected != &identity)
        {
            return Err(plait_client::host::Error::Protocol(
                "Checkout identity changed.".into(),
            ));
        }
        if let Some(project) = self
            .settings_project
            .as_mut()
            .filter(|project| project.checkout == key.1)
        {
            project.identity = Some(identity);
        }
        Ok(())
    }

    pub(super) fn sync_project_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open
            || self.settings_page != settings::SettingsPage::Project
            || !self.host.connection.connected
        {
            return;
        }
        self.prepare_project_policy(cx);
        self.prepare_isolation_settings(cx);
        self.prepare_build_settings(cx);
        let key = (
            self.host.connection.epoch,
            self.settings_checkout().to_owned(),
        );
        if self.project_agent().key.as_ref() != Some(&key) {
            self.refresh_project_agent(cx);
        }
    }

    pub(super) fn project_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        settings::framed(
            self.settings_column(None).child(
                settings::page("project-settings-page")
                    .track_scroll(&self.settings_scroll)
                    .child(self.policy_settings(cx))
                    .child(self.agent_settings(cx))
                    .child(self.isolation_settings(cx))
                    .child(self.project_linear_settings(cx))
                    .child(
                        super::theme::button("project-settings-worktrees", "Worktrees")
                            .on_activate(cx, |this, window, cx| {
                                this.open_worktrees(this.settings_checkout().to_owned(), window, cx)
                            }),
                    ),
            ),
        )
    }

    pub(super) fn settings_checkout(&self) -> &Path {
        self.settings_project
            .as_ref()
            .map_or(&self.host.creation.checkout, |project| &project.checkout)
    }

    pub(super) fn policy_for(&self, checkout: &Path) -> Option<&PolicyState> {
        if checkout == self.host.creation.checkout {
            return Some(&self.policy);
        }
        self.settings_project
            .as_ref()
            .filter(|project| project.checkout == checkout)
            .map(|project| &project.policy)
    }
    pub(super) fn policy_for_mut(&mut self, checkout: &Path) -> Option<&mut PolicyState> {
        if checkout == self.host.creation.checkout {
            return Some(&mut self.policy);
        }
        self.settings_project
            .as_mut()
            .filter(|project| project.checkout == checkout)
            .map(|project| &mut project.policy)
    }
    pub(super) fn project_policy(&self) -> &PolicyState {
        self.policy_for(self.settings_checkout())
            .expect("settings target")
    }
    pub(super) fn project_policy_mut(&mut self) -> &mut PolicyState {
        let checkout = self.settings_checkout().to_owned();
        self.policy_for_mut(&checkout).expect("settings target")
    }
    pub(super) fn agent_for_mut(&mut self, checkout: &Path) -> Option<&mut AgentState> {
        if checkout == self.host.creation.checkout {
            return Some(&mut self.agent);
        }
        self.settings_project
            .as_mut()
            .filter(|project| project.checkout == checkout)
            .map(|project| &mut project.agent)
    }
    pub(super) fn project_agent(&self) -> &AgentState {
        self.settings_project
            .as_ref()
            .filter(|project| project.checkout != self.host.creation.checkout)
            .map_or(&self.agent, |project| &project.agent)
    }
    pub(super) fn project_agent_mut(&mut self) -> &mut AgentState {
        let checkout = self.settings_checkout().to_owned();
        self.agent_for_mut(&checkout).expect("settings target")
    }

    pub(super) fn show_settings_for_project(
        &mut self,
        checkout: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A pending write keeps its named target and its result surface.
        if self.settings_open
            && self.settings_page == settings::SettingsPage::Project
            && self.settings_checkout() != checkout
            && (self.project_policy().busy
                || self.project_agent().busy
                || self.admission.settings_busy
                || self.build_output.busy)
        {
            return;
        }
        if self
            .settings_project
            .as_ref()
            .is_none_or(|project| project.checkout != checkout)
        {
            let name = self
                .state
                .projects
                .iter()
                .find(|project| project.checkout == checkout)
                .map(|project| project.name.clone())
                .unwrap_or_else(|| {
                    checkout
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
            self.settings_project = Some(ProjectSettings {
                checkout: checkout.clone(),
                name,
                policy: Default::default(),
                agent: Default::default(),
                identity: None,
            });
        }
        self.prepare_project_policy(cx);
        self.open_page(Some(settings::SettingsPage::Project), window, cx);
    }
}
