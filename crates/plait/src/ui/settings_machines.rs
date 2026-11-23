//! Settings → Machines: which machine's host this window works with. A remote
//! machine is reached with `ssh <destination> plait-host bridge`, using the
//! person's own SSH keys; Plait never asks for or keeps a password.
use super::{Plait, jump::PreferenceGroup, settings, text_field::TextField, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    host::Client,
    machines::{MachineId, Transport},
    remote::{
        ssh::{self, SshDestination, SshOptions},
        tailnet,
    },
};
use std::path::PathBuf;

pub(super) struct MachineForm {
    pub name: Entity<TextField>,
    pub destination: Entity<TextField>,
    pub port: Entity<TextField>,
    pub store: Entity<TextField>,
    pub error: Option<String>,
    pub tailnet: bool,
}

#[derive(Default)]
pub(super) struct MachinesUi {
    pub form: Option<MachineForm>,
    pub confirm_remove: Option<MachineId>,
    /// The machine being connected to, while its SSH link starts.
    pub connecting: Option<MachineId>,
    /// What happened last, and whether it was a failure.
    pub note: Option<(String, bool)>,
    generation: u64,
    /// The saved choice was applied once at launch.
    restored: bool,
    /// The next reconnect follows a link that was just made, so it need not
    /// make another. Every other reconnect to a remote machine starts one.
    pub(super) fresh_link: bool,
    /// The machine this window is connected to; this Mac until a switch.
    pub(super) active: MachineId,
    /// Installing Plait on a machine that lacks it, while that is offered.
    pub(super) install: Option<super::machine_install::MachineInstall>,
    pub(super) update: Option<super::machine_update::MachineUpdate>,
}

fn field(
    name: &'static str,
    placeholder: &'static str,
    cx: &mut Context<Plait>,
) -> Entity<TextField> {
    let input = cx.new(|cx| TextField::new(name, placeholder, cx));
    cx.observe(&input, |_, _, cx| cx.notify()).detach();
    input
}

impl Plait {
    pub(super) fn selected_machine(&self) -> MachineId {
        let selected = &self.preferences.value.selected_machine;
        if self.preferences.value.machines.get(selected).is_some() {
            selected.clone()
        } else {
            MachineId::default()
        }
    }

    /// The machine this window is connected to, for headers. A remote host
    /// names itself; until it has, the name it was saved under is used.
    pub(super) fn machine_label(&self) -> String {
        if self.host_is_local() {
            return "This Mac".into();
        }
        if let Some(identity) = self.host_identity() {
            return identity.name.clone();
        }
        let id = self.selected_machine();
        match self.preferences.value.machines.get(&id) {
            Some(machine) => machine.name.clone(),
            None => "Another machine".into(),
        }
    }

    /// The window's title names another machine, so it is clear in the window
    /// list and the Dock which machine a window works on.
    pub(super) fn update_window_title(&self, window: &mut Window) {
        let title = if self.host_is_local() {
            "Plait".to_owned()
        } else {
            format!("Plait — {}", self.machine_label())
        };
        window.set_window_title(&title);
    }

    /// The connected machine, as projects and tabs name it.
    pub(super) fn current_machine(&self) -> plait_client::model::Machine {
        let local = self.host_is_local();
        plait_client::model::Machine {
            name: if local {
                plait_client::machine::local_name()
            } else {
                self.machine_label()
            },
            local,
        }
    }

    /// Applies the saved machine once, after preferences load. A remote one
    /// connects in the background; launch never waits for it.
    pub(super) fn restore_machine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.machines.restored || !self.preferences.loaded || cfg!(test) {
            return;
        }
        self.machines.restored = true;
        let id = self.selected_machine();
        if id != MachineId::default() {
            self.connect_machine(id, true, window, cx);
        }
    }

    fn save_selected(&mut self, id: MachineId, cx: &mut Context<Self>) {
        let mut value = self.preferences.value.clone();
        value.selected_machine = id;
        self.set_preferences(value, PreferenceGroup::Machines, cx);
    }

    /// Switches the window to machine `id`. A remote machine connects over SSH
    /// off the UI thread; on failure the window stays where it was.
    pub(super) fn use_machine(
        &mut self,
        id: MachineId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connect_machine(id, false, window, cx);
    }

    fn connect_machine(
        &mut self,
        id: MachineId,
        restoring: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(machine) = self.preferences.value.machines.get(&id).cloned() else {
            return;
        };
        self.machines.generation += 1;
        self.machine_restore_notice = None;
        let generation = self.machines.generation;
        self.machines.confirm_remove = None;
        self.machines.install = None;
        self.machines.update = None;
        let transport = match machine.transport {
            Transport::Local => {
                self.machines.connecting = None;
                self.machines.note = None;
                self.save_selected(id.clone(), cx);
                self.use_machine_client(Client::local_or_unavailable(), id, true, window, cx);
                return;
            }
            remote => remote,
        };
        self.machines.connecting = Some(id.clone());
        self.machines.note = None;
        cx.notify();
        if cfg!(test) {
            return;
        }
        let task = cx.background_executor().spawn(async move {
            match transport {
                Transport::Ssh {
                    destination,
                    port,
                    store,
                } => ssh::connect(
                    &destination,
                    SshOptions {
                        port,
                        store,
                        ..Default::default()
                    },
                ),
                Transport::Tailnet { host, port } => tailnet::connect(&host, port),
                Transport::Local => unreachable!(),
            }
        });
        let name = machine.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // A newer choice was made while this one connected.
                if this.machines.generation != generation {
                    return;
                }
                this.machines.connecting = None;
                match result {
                    Ok(link) => {
                        if this.machines.note.is_none() {
                            this.machines.note = Some((format!("Using {name}."), false));
                        }
                        this.save_selected(id.clone(), cx);
                        this.use_machine_client(Client::over_link(link), id, false, window, cx);
                    }
                    Err(error) => {
                        this.machines.note =
                            Some((format!("Could not reach {name}. {error}"), true));
                        if restoring {
                            this.machine_restore_notice =
                                Some(format!("Could not reach {name}. {error}"));
                        } else {
                            this.settings_tab = super::settings::SettingsTab::Machines;
                            this.show_settings(true, window, cx);
                        }
                        // Perhaps it answers but has no Plait yet: offer to install it.
                        this.check_install(id.clone(), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Called at the start of a reconnect. A remote machine needs a new link
    /// first: the old one may be dead. Returns whether the reconnect should wait
    /// for that link instead of going ahead.
    pub(super) fn relink_before_reconnect(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.machines.update.as_ref().is_some_and(|state| {
            state.id == self.machines.active
                && matches!(state.stage, super::machine_update::Stage::Updating)
        }) {
            return true;
        }
        if self.host_is_local() || std::mem::take(&mut self.machines.fresh_link) {
            return false;
        }
        if self.machines.connecting.is_none() {
            self.use_machine(self.selected_machine(), window, cx);
        }
        true
    }

    fn open_machine_form(&mut self, cx: &mut Context<Self>) {
        self.machines.form = Some(MachineForm {
            name: field("Machine name", "workstation", cx),
            destination: field("SSH destination", "you@workstation", cx),
            port: field("SSH port", "22", cx),
            store: field("Plait store on that machine", "Default", cx),
            error: None,
            tailnet: false,
        });
        cx.notify();
    }

    fn add_machine(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.machines.form else {
            return;
        };
        let read = |field: &Entity<TextField>| field.read(cx).text().trim().to_owned();
        let (name, destination, port, store) = (
            read(&form.name),
            read(&form.destination),
            read(&form.port),
            read(&form.store),
        );
        let tailnet = form.tailnet;
        let transport = (|| {
            if tailnet {
                let port = if port.is_empty() {
                    7419
                } else {
                    port.parse::<u16>()
                        .ok()
                        .filter(|p| *p > 0)
                        .ok_or("The port is a number from 1 to 65535.")?
                };
                let transport = Transport::Tailnet {
                    host: destination,
                    port,
                };
                transport.validate()?;
                return Ok(transport);
            }
            let destination =
                SshDestination::new(destination).map_err(|error| error.to_string())?;
            let port = if port.is_empty() {
                None
            } else {
                Some(
                    port.parse::<u16>()
                        .ok()
                        .filter(|port| *port > 0)
                        .ok_or("The port is a number from 1 to 65535.")?,
                )
            };
            let store = if store.is_empty() {
                None
            } else {
                let path = PathBuf::from(&store);
                if !path.is_absolute() {
                    return Err("The store is an absolute path on that machine.".to_owned());
                }
                Some(path)
            };
            Ok::<_, String>(Transport::Ssh {
                destination,
                port,
                store,
            })
        })();
        let mut value = self.preferences.value.clone();
        let result = transport.and_then(|transport| value.machines.add(name, transport));
        match result {
            Ok(_) => {
                self.machines.form = None;
                self.machines.note = Some(("Machine added. Choose Use to connect.".into(), false));
                self.set_preferences(value, PreferenceGroup::Machines, cx);
            }
            Err(error) => {
                if let Some(form) = &mut self.machines.form {
                    form.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    fn remove_machine(&mut self, id: MachineId, window: &mut Window, cx: &mut Context<Self>) {
        // Leave a machine before forgetting it.
        if self.selected_machine() == id {
            self.use_machine(MachineId::default(), window, cx);
        }
        let mut value = self.preferences.value.clone();
        if value.machines.remove(&id).is_ok() {
            self.machines.confirm_remove = None;
            self.set_preferences(value, PreferenceGroup::Machines, cx);
        }
        cx.notify();
    }

    fn machine_form(&self, cx: &mut Context<Self>) -> Div {
        let Some(form) = &self.machines.form else {
            return div();
        };
        let labelled = |label: &'static str, input: &Entity<TextField>| {
            column()
                .gap_1()
                .child(tag(
                    (
                        ElementId::from("machine-form-label"),
                        SharedString::from(label),
                    ),
                    label,
                    colors().muted,
                ))
                .child(input.clone())
        };
        column()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(colors().border))
            .child(
                row()
                    .gap_2()
                    .child(
                        button(
                            "machine-kind-ssh",
                            if form.tailnet { "SSH" } else { "SSH ✓" },
                        )
                        .on_activate(cx, |this, _, cx| {
                            if let Some(form) = &mut this.machines.form {
                                form.tailnet = false;
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        button(
                            "machine-kind-tailnet",
                            if form.tailnet {
                                "Tailnet ✓"
                            } else {
                                "Tailnet"
                            },
                        )
                        .on_activate(cx, |this, _, cx| {
                            if let Some(form) = &mut this.machines.form {
                                form.tailnet = true;
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(labelled("Name", &form.name))
            .child(labelled(
                if form.tailnet {
                    "Tailnet host"
                } else {
                    "SSH destination"
                },
                &form.destination,
            ))
            .child(labelled(
                if form.tailnet {
                    "Port (default 7419)"
                } else {
                    "Port (optional)"
                },
                &form.port,
            ))
            .when(!form.tailnet, |el| {
                el.child(labelled(
                    "Plait store on that machine (optional)",
                    &form.store,
                ))
            })
            .child(
                tag(
                    "machine-form-help",
                    if form.tailnet {
                        "On the host, allow your login and run plait-host serve --tailnet."
                    } else {
                        "Plait runs SSH with your own keys. The host needs plait-host run."
                    },
                    colors().subtle,
                )
                .line_height(px(18.)),
            )
            .when_some(form.error.clone(), |el, error| {
                el.child(tag("machine-form-error", error, colors().amber))
            })
            .child(
                row()
                    .gap_2()
                    .child(
                        toned_button("add-machine-save", "Add", Tone::Primary)
                            .on_activate(cx, |this, _, cx| this.add_machine(cx)),
                    )
                    .child(button("add-machine-cancel", "Cancel").on_activate(
                        cx,
                        |this, _, cx| {
                            this.machines.form = None;
                            cx.notify();
                        },
                    )),
            )
    }

    pub(super) fn machine_settings(&self, cx: &mut Context<Self>) -> Div {
        let active = &self.machines.active;
        let busy = self.machines.connecting.is_some();
        let mut list = column().gap_1();
        for (index, machine) in self.preferences.value.machines.list().iter().enumerate() {
            let local = matches!(machine.transport, Transport::Local);
            let address = match &machine.transport {
                Transport::Local => "The host on this Mac".to_owned(),
                Transport::Ssh {
                    destination, port, ..
                } => match port {
                    Some(port) => format!("SSH {} · port {port}", destination.as_str()),
                    None => format!("SSH {}", destination.as_str()),
                },
                Transport::Tailnet { host, port } => format!("Tailnet {host}:{port}"),
            };
            let in_use = machine.id == *active;
            let connecting = self.machines.connecting.as_ref() == Some(&machine.id);
            let confirming = self.machines.confirm_remove.as_ref() == Some(&machine.id);
            let use_id = machine.id.clone();
            let remove_id = machine.id.clone();
            let confirm_id = machine.id.clone();
            let mut controls = row().gap_1();
            if confirming {
                controls = controls
                    .child(
                        toned_button(("machine-remove-confirm", index), "Remove", Tone::Primary)
                            .aria_label(format!("Remove {}", machine.name))
                            .on_activate(cx, move |this, window, cx| {
                                this.remove_machine(remove_id.clone(), window, cx)
                            }),
                    )
                    .child(
                        button(("machine-remove-cancel", index), "Cancel").on_activate(
                            cx,
                            |this, _, cx| {
                                this.machines.confirm_remove = None;
                                cx.notify();
                            },
                        ),
                    );
            } else {
                let usable = !connecting && (!in_use || busy);
                controls = controls.child(
                    button(
                        ("machine-use", index),
                        if connecting { "Connecting…" } else { "Use" },
                    )
                    .aria_label(format!("Use {}", machine.name))
                    .disabled(!usable)
                    .when(usable, |el| {
                        el.on_activate(cx, move |this, window, cx| {
                            this.use_machine(use_id.clone(), window, cx)
                        })
                    }),
                );
                if !local {
                    controls = controls.child(
                        button(("machine-remove", index), "Remove…")
                            .aria_label(format!("Remove {}…", machine.name))
                            .disabled(busy)
                            .when(!busy, |el| {
                                el.on_activate(cx, move |this, _, cx| {
                                    this.machines.confirm_remove = Some(confirm_id.clone());
                                    cx.notify();
                                })
                            }),
                    );
                }
            }
            list = list.child(
                row()
                    .gap_3()
                    .py_2()
                    .justify_between()
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(13.)).child(machine.name.clone()))
                            .child(tag(
                                ("machine-address", index),
                                if in_use {
                                    format!("{address} · in use")
                                } else {
                                    address
                                },
                                if in_use {
                                    colors().mint
                                } else {
                                    colors().subtle
                                },
                            )),
                    )
                    .child(controls),
            );
            if confirming {
                list = list.child(
                    tag(
                        ("machine-remove-note", index),
                        "Plait forgets this machine. Its host and threads stay on that machine.",
                        colors().amber,
                    )
                    .line_height(px(18.)),
                );
            }
        }
        let connecting = self.machines.connecting.as_ref().and_then(|id| {
            self.preferences
                .value
                .machines
                .get(id)
                .map(|m| m.name.clone())
        });
        settings::section("Machines")
            .child(
                tag(
                    "machines-intro",
                    "Work on another machine through its Plait host: its projects, threads, Changes, History, and Files. Each window works with one machine; switch from the top of the sidebar.",
                    colors().muted,
                )
                .line_height(px(18.)),
            )
            .child(list)
            .when_some(connecting, |el, name| {
                el.child(tag("machines-connecting", format!("Connecting to {name}…"), colors().muted))
            })
            .when_some(self.machines.note.clone(), |el, (note, failed)| {
                el.child(
                    tag("machines-note", note, if failed { colors().amber } else { colors().mint })
                        .line_height(px(18.)),
                )
            })
            .children(self.install_panel(cx))
            .children(self.update_panel(cx))
            .child(if self.machines.form.is_some() {
                self.machine_form(cx)
            } else {
                row().child(
                    button("add-machine", "Add machine…")
                        .on_activate(cx, |this, _, cx| this.open_machine_form(cx)),
                )
            })
    }

    #[cfg(test)]
    pub(super) fn open_machine_form_for_test(&mut self, cx: &mut Context<Self>) {
        self.open_machine_form(cx);
    }
    #[cfg(test)]
    pub(super) fn add_machine_for_test(&mut self, cx: &mut Context<Self>) {
        self.add_machine(cx);
    }
}
