//! Explicit checks and updates for the connected SSH host.
use super::{Plait, theme::*};
use gpui::*;
use plait_client::{
    host::release,
    machines::{MachineId, Transport},
    remote::{install, ssh::SshOptions},
    update::{self, HostRelease},
};

#[derive(Clone, Debug)]
pub(super) enum Stage {
    Checking,
    Current,
    Unknown,
    Available(String),
    Updating,
    Failed(String),
}

#[derive(Clone, Debug)]
pub(super) struct MachineUpdate {
    pub id: MachineId,
    pub name: String,
    pub stage: Stage,
}

fn update_line(running: Option<&str>, release: &HostRelease, name: &str) -> (String, bool) {
    match running {
        None => ("This machine's host can't report its build; update it on the machine with plait-host update.".into(), false),
        Some(build) if build == release.build_id => ("This machine's host is current.".into(), false),
        Some(_) => (format!("Plait {} is available for {name}.", release.tag), true),
    }
}

impl Plait {
    fn update_target(
        &self,
    ) -> Option<(
        MachineId,
        String,
        plait_client::remote::ssh::SshDestination,
        SshOptions,
    )> {
        if !self.host.connection.connected || self.machines.connecting.is_some() {
            return None;
        }
        let id = self.machines.active.clone();
        let machine = self.preferences.value.machines.get(&id)?;
        let Transport::Ssh {
            destination,
            port,
            store,
        } = &machine.transport
        else {
            return None;
        };
        Some((
            id,
            machine.name.clone(),
            destination.clone(),
            SshOptions {
                port: *port,
                store: store.clone(),
                ..Default::default()
            },
        ))
    }

    fn check_host_update(&mut self, cx: &mut Context<Self>) {
        if self
            .machines
            .update
            .as_ref()
            .is_some_and(|s| matches!(s.stage, Stage::Checking | Stage::Updating))
        {
            return;
        }
        let Some((id, name, destination, options)) = self.update_target() else {
            return;
        };
        let client = self.host_client();
        self.machines.update = Some(MachineUpdate {
            id: id.clone(),
            name: name.clone(),
            stage: Stage::Checking,
        });
        let task = cx.background_executor().spawn(async move {
            let probe = install::probe(&destination, &options).map_err(|e| e.to_string())?;
            let info = release::host_info(&client).map_err(|e| e.to_string())?;
            let Some(build) = info.and_then(|info| info.build_id) else {
                return Ok(Stage::Unknown);
            };
            let platform = probe
                .platform
                .ok_or_else(|| format!("No host update is available for {}.", probe.system))?;
            let release = update::host_release(&platform).map_err(|e| e.to_string())?;
            let (line, available) = update_line(Some(&build), &release, &name);
            Ok::<_, String>(if available {
                Stage::Available(line)
            } else {
                Stage::Current
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.machines.active != id {
                    return;
                }
                let Some(state) = this
                    .machines
                    .update
                    .as_mut()
                    .filter(|s| s.id == id && matches!(s.stage, Stage::Checking))
                else {
                    return;
                };
                state.stage = result.unwrap_or_else(Stage::Failed);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn update_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, _, destination, options)) = self.update_target() else {
            return;
        };
        let Some(state) = self
            .machines
            .update
            .as_mut()
            .filter(|s| s.id == id && matches!(s.stage, Stage::Available(_)))
        else {
            return;
        };
        state.stage = Stage::Updating;
        let task = cx.background_executor().spawn(async move {
            install::update(&destination, &options).map_err(|e| e.to_string())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_host_update(id, result, window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn finish_host_update(
        &mut self,
        id: MachineId,
        result: Result<install::Updated, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.machines.active != id {
            return;
        }
        let Some(state) = self
            .machines
            .update
            .as_mut()
            .filter(|s| s.id == id && matches!(s.stage, Stage::Updating))
        else {
            return;
        };
        let name = &state.name;
        match result {
            Ok(result) => {
                let note = match result {
                    install::Updated::Current => "This machine's host is current.".to_owned(),
                    install::Updated::Active { version } => {
                        format!("Updated the host on {name} to {version}.")
                    }
                };
                state.stage = Stage::Current;
                if self.machines.connecting.is_none() {
                    self.use_machine(id, window, cx);
                    self.machines.note = Some((note, false));
                }
            }
            Err(error) => state.stage = Stage::Failed(error),
        }
        cx.notify();
    }

    pub(super) fn update_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let id = self.machines.active.clone();
        let state = self.machines.update.as_ref().filter(|s| s.id == id);
        let reachable = self.update_target().is_some();
        if !reachable
            && !state.is_some_and(|s| matches!(s.stage, Stage::Available(_) | Stage::Updating))
        {
            return None;
        }
        let mut panel = column().gap_2();
        if let Some(state) = state {
            let (text, tint) = match &state.stage {
                Stage::Checking => ("Checking for a host update…".into(), colors().muted),
                Stage::Current => ("This machine's host is current.".into(), colors().text),
                Stage::Unknown => ("This machine's host can't report its build; update it on the machine with plait-host update.".into(), colors().text),
                Stage::Available(line) => (line.clone(), colors().text),
                Stage::Updating => (format!("Updating the host on {}. Running threads must finish first; the connection drops while it restarts…", state.name), colors().muted),
                Stage::Failed(error) => (error.clone(), colors().amber),
            };
            panel = panel.child(tag("machine-update-line", text, tint).line_height(px(18.)));
            if matches!(state.stage, Stage::Available(_)) && !reachable {
                panel = panel.child(tag(
                    "machine-update-offline",
                    "Connect to this machine to update its host.",
                    colors().muted,
                ));
            }
            if reachable && matches!(state.stage, Stage::Available(_)) {
                panel = panel.child(
                    row().child(
                        button("machine-update", "Update host")
                            .on_activate(cx, |this, window, cx| this.update_host(window, cx)),
                    ),
                );
            }
        }
        if reachable && !state.is_some_and(|s| matches!(s.stage, Stage::Checking | Stage::Updating))
        {
            panel = panel.child(
                row().child(
                    button("machine-update-check", "Check for host update")
                        .on_activate(cx, |this, _, cx| this.check_host_update(cx)),
                ),
            );
        }
        Some(panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[core::prelude::v1::test]
    fn update_sentences_follow_build_identity() {
        let release = HostRelease {
            tag: "v1".into(),
            build_id: "build".into(),
            url: String::new(),
            sha256: String::new(),
            size: 0,
        };
        assert!(
            update_line(None, &release, "box")
                .0
                .contains("can't report")
        );
        assert_eq!(
            update_line(Some("build"), &release, "box").0,
            "This machine's host is current."
        );
        assert_eq!(
            update_line(Some("old"), &release, "box"),
            ("Plait v1 is available for box.".into(), true)
        );
    }
}
