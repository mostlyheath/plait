//! Installing Plait on a machine that does not have it, from Settings →
//! Machines. Offered after an SSH connection fails because the host is missing.
use super::{Plait, theme::*};
use gpui::*;
use plait_client::{
    machines::{MachineId, Transport},
    remote::{
        install::{self, Probe},
        ssh::{SshDestination, SshOptions},
    },
    update,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Checking,
    /// The machine answered and has no host.
    Missing(Probe),
    Installing,
    Failed(String),
}

#[derive(Clone, Debug)]
pub(super) struct MachineInstall {
    pub(super) id: MachineId,
    pub(super) name: String,
    pub(super) stage: Stage,
}

/// Why a machine without a host cannot get one from here, if it cannot.
pub(super) fn blocker(probe: &Probe) -> Option<String> {
    if probe.platform.is_none() {
        return Some(format!(
            "Plait has no host for {} yet; hosts are published for Linux on x86_64 and arm64.",
            if probe.system.is_empty() {
                "this system"
            } else {
                &probe.system
            }
        ));
    }
    (!probe.downloader).then(|| "That machine needs curl or wget to download Plait.".to_owned())
}

/// What the person is told once the host is installed.
pub(super) fn installed_note(name: &str, linger: bool) -> String {
    if linger {
        format!("Installed Plait on {name}.")
    } else {
        format!(
            "Installed Plait on {name}. It stops when you log out of {name} unless an administrator runs loginctl enable-linger for your account there."
        )
    }
}

fn ssh_target(transport: &Transport) -> Option<(SshDestination, SshOptions)> {
    match transport {
        Transport::Ssh {
            destination,
            port,
            store,
        } => Some((
            destination.clone(),
            SshOptions {
                port: *port,
                store: store.clone(),
                ..SshOptions::default()
            },
        )),
        _ => None,
    }
}

impl Plait {
    /// After an SSH connection fails, ask the machine whether Plait is there.
    pub(super) fn check_install(&mut self, id: MachineId, cx: &mut Context<Self>) {
        let Some(machine) = self.preferences.value.machines.get(&id).cloned() else {
            return;
        };
        let Some((destination, options)) = ssh_target(&machine.transport) else {
            return;
        };
        self.machines.install = Some(MachineInstall {
            id: id.clone(),
            name: machine.name.clone(),
            stage: Stage::Checking,
        });
        let task = cx
            .background_executor()
            .spawn(async move { install::probe(&destination, &options) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(state) = this.machines.install.as_mut().filter(|s| s.id == id) else {
                    return;
                };
                match result {
                    // SSH itself failed, or Plait is there: the connection error says why.
                    Ok(probe) if !probe.installed => state.stage = Stage::Missing(probe),
                    _ => this.machines.install = None,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Installs the newest host on the machine, then connects to it.
    pub(super) fn install_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.machines.install.clone() else {
            return;
        };
        let Stage::Missing(probe) = &state.stage else {
            return;
        };
        let (Some(platform), None) = (probe.platform.clone(), blocker(probe)) else {
            return;
        };
        let Some((destination, options)) = self
            .preferences
            .value
            .machines
            .get(&state.id)
            .and_then(|machine| ssh_target(&machine.transport))
        else {
            return;
        };
        self.machines.note = None;
        if let Some(install) = self.machines.install.as_mut() {
            install.stage = Stage::Installing;
        }
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            // This Mac checks the signed release; the machine checks the download.
            let release = update::host_release(&platform).map_err(|e| e.to_string())?;
            install::install(&destination, &options, &release).map_err(|e| e.to_string())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(current) = this.machines.install.as_mut().filter(|s| s.id == state.id)
                else {
                    return;
                };
                match result {
                    Ok(installed) => {
                        this.machines.install = None;
                        let note = installed_note(&state.name, installed.linger);
                        this.use_machine(state.id.clone(), window, cx);
                        this.machines.note = Some((note, false));
                    }
                    Err(error) => {
                        current.stage = Stage::Failed(format!(
                            "Plait was not installed on {}. {error}",
                            state.name
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The install offer under the Machines list, while there is one.
    pub(super) fn install_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let state = self.machines.install.as_ref()?;
        let name = state.name.clone();
        let line =
            |id: &'static str, text: String, tint: u32| tag(id, text, tint).line_height(px(18.));
        Some(match &state.stage {
            Stage::Checking => row().child(line(
                "machine-install-checking",
                format!("Checking whether Plait is on {name}…"),
                colors().muted,
            )),
            Stage::Installing => row().child(line(
                "machine-install-progress",
                format!(
                    "Installing Plait on {name}. The machine downloads the release, checks it against the signed one, and starts its host…"
                ),
                colors().muted,
            )),
            Stage::Failed(error) => row().child(line("machine-install-failed", error.clone(), colors().amber)),
            Stage::Missing(probe) => {
                let mut panel = column().gap_2().child(line(
                    "machine-install-missing",
                    format!("Plait is not installed on {name}."),
                    colors().text,
                ));
                match blocker(probe) {
                    Some(reason) => panel = panel.child(line("machine-install-blocked", reason, colors().amber)),
                    None => {
                        panel = panel.child(
                            row().child(
                                button("machine-install", format!("Install Plait on {name}"))
                                    .aria_description(
                                        "Downloads the newest Plait host on that machine, checks it, and starts it, as you, without administrator rights.",
                                    )
                                    .on_activate(cx, |this, window, cx| this.install_host(window, cx)),
                            ),
                        )
                    }
                }
                panel
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(system: &str, downloader: bool) -> Probe {
        Probe {
            platform: install::platform(system).map(str::to_owned),
            system: system.into(),
            installed: false,
            downloader,
        }
    }

    #[core::prelude::v1::test]
    fn an_install_is_offered_only_where_a_host_can_run() {
        assert_eq!(blocker(&probe("Linux x86_64", true)), None);
        assert!(
            blocker(&probe("Darwin arm64", true))
                .unwrap()
                .contains("Darwin arm64")
        );
        assert!(
            blocker(&probe("Linux aarch64", false))
                .unwrap()
                .contains("curl or wget")
        );
        assert!(installed_note("box", false).contains("enable-linger"));
        assert_eq!(installed_note("box", true), "Installed Plait on box.");
    }
}
