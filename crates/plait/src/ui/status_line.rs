use super::{Plait, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::navigation::ThreadRowState;

impl Plait {
    pub(super) fn next_needing_me(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((index, _)) = self.activity().next {
            let id = self.host.connection.threads()[index].id.clone();
            self.select_host_thread(id, window, cx);
        }
    }

    fn machine_control(&self, cx: &mut Context<Self>) -> Control {
        let machine = self
            .machines
            .connecting
            .as_ref()
            .and_then(|id| self.preferences.value.machines.get(id))
            .map(|m| m.name.clone())
            .unwrap_or_else(|| self.machine_label());
        let (state, tint) = if self.host.connection.connecting || self.machines.connecting.is_some()
        {
            ("Connecting…", colors().muted)
        } else if self.host.connection.connected {
            ("Connected", colors().mint)
        } else {
            ("Offline", colors().amber)
        };
        let hover = format!("{machine}, {state}. Switch machine…");
        button("machine-switcher", "")
            .h(px(24.))
            // A long name gives way to Next and Settings; its hover text has it whole.
            .min_w_0()
            .max_w(px(260.))
            .flex_shrink_1()
            .aria_label(hover.clone())
            .aria_expanded(self.navigation.menu.as_deref() == Some("machines"))
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(hover.clone())).into())
            .child(dot(tint))
            .child(div().min_w_0().truncate().child(machine))
            .when(state != "Connected", |el| {
                el.child(div().flex_shrink_0().child(state))
            })
            .on_activate(cx, |this, _, cx| {
                this.navigation.menu = (this.navigation.menu.as_deref() != Some("machines"))
                    .then(|| "machines".into());
                cx.notify();
            })
    }

    /// The machines this device knows, and where to manage them.
    fn machine_menu(&self, panel: Div, cx: &mut Context<Self>) -> Div {
        let mut panel = panel.child(tag("machine-menu-heading", "Machines", colors().muted));
        for (index, machine) in self.preferences.value.machines.list().iter().enumerate() {
            let id = &machine.id;
            let label = if self.machines.connecting.as_ref() == Some(id) {
                format!("{} · Connecting…", machine.name)
            } else if id == &self.machines.active {
                format!("{} ✓", machine.name)
            } else {
                machine.name.clone()
            };
            let enabled = self.machines.connecting.as_ref() != Some(id)
                && (id != &self.machines.active || self.machines.connecting.is_some());
            let id = id.clone();
            panel = panel.child(
                button(("machine-use", index), label)
                    .disabled(!enabled)
                    .when(enabled, |el| {
                        el.on_activate(cx, move |this, window, cx| {
                            this.navigation.menu = None;
                            this.use_machine(id.clone(), window, cx);
                        })
                    }),
            );
        }
        panel.child(button("machine-manage", "Manage machines…").on_activate(
            cx,
            |this, window, cx| {
                this.navigation.menu = None;
                this.settings_tab = super::settings::SettingsTab::Machines;
                this.show_settings(true, window, cx);
            },
        ))
    }

    fn next_control(&self, index: usize, state: ThreadRowState, cx: &mut Context<Self>) -> Control {
        let title = self.summary_title(&self.host.connection.threads()[index]);
        let state = match state {
            ThreadRowState::Approval => "Waiting for approval",
            ThreadRowState::Failed => "Failed",
            ThreadRowState::Interrupted => "Interrupted",
            ThreadRowState::FinishedUnread => "Finished",
            // Next names only threads that need the person; no other state reaches here.
            other => other.label(""),
        };
        let label = format!("Next: {title} · {state}");
        button("status-next", "")
            .aria_label(label.clone())
            .aria_keyshortcuts(self.shortcut_aria("thread.next_needing_me"))
            .min_w_0()
            .flex_shrink_1()
            .h(px(24.))
            .gap_1()
            .child("Next:")
            .child(div().min_w_0().truncate().child(title))
            .child(div().flex_shrink_0().child(format!("· {state}")))
            .child(
                div()
                    .flex_shrink_0()
                    .child(self.shortcut_label("thread.next_needing_me")),
            )
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(label.clone())).into())
            .on_activate(cx, |this, window, cx| this.next_needing_me(window, cx))
    }

    pub(super) fn status_line(&self, wide: bool, cx: &mut Context<Self>) -> Div {
        let activity = self.activity();
        let mut line = row()
            .relative()
            .w_full()
            .h(px(26.))
            .flex_shrink_0()
            .px_2()
            .border_t_1()
            .border_color(rgb(colors().border))
            .bg(rgb(colors().rail))
            .text_size(px(12.))
            .text_color(rgb(colors().muted))
            .child(self.machine_control(cx));
        if wide {
            for (count, label, color) in [
                (activity.working, "working", colors().blue),
                (activity.finished, "finished", colors().mint),
                (activity.failed, "failed", colors().red),
                (activity.approval, "waiting for approval", colors().amber),
            ] {
                if count > 0 {
                    line = line.child(row().flex_shrink_0().gap_1().child(dot(color)).child(
                        tag(label, format!("{count} {label}"), colors().subtle).text_size(px(12.)),
                    ));
                }
            }
        }
        line.child(div().flex_1())
            .when_some(activity.next, |el, (index, state)| {
                el.child(self.next_control(index, state, cx))
            })
            .when(self.update_available(), |el| {
                el.child(
                    toned_button("status-update", "Update", Tone::Primary)
                        .h(px(24.))
                        .aria_label("Update available. Open Settings to install.")
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                TextTooltip("Update available. Open Settings to install.".into())
                            })
                            .into()
                        })
                        .on_activate(cx, |this, window, cx| this.show_settings(true, window, cx)),
                )
            })
            .child(
                icon_button("settings", "sliders", "Settings…")
                    .size(px(24.))
                    .aria_keyshortcuts(self.shortcut_aria("app.settings"))
                    .on_activate(cx, |this, window, cx| this.show_settings(true, window, cx)),
            )
            .when(self.navigation.menu.as_deref() == Some("machines"), |el| {
                el.child(
                    self.machine_menu(
                        column()
                            .absolute()
                            .bottom(px(26.))
                            .left(px(8.))
                            .p_2()
                            .gap_1()
                            .border_1()
                            .border_color(rgb(colors().edge))
                            .rounded(px(SURFACE_CORNER))
                            .shadow(floating_shadow())
                            .bg(rgb(colors().raised))
                            .occlude(),
                        cx,
                    ),
                )
            })
    }
}
