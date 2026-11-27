//! Machine choices are derived from saved transports and the one live connection.
use super::{Plait, machine_update::Stage, settings::SettingsTab, theme::*};
use gpui::{prelude::*, *};
use plait_client::machines::{MachineId, Transport};

actions!(machines, [SwitchMachine, ReconnectMachine, AddMachine]);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = machines, no_json)]
pub(super) struct MachineNumber(pub usize);

pub(super) fn address(transport: &Transport) -> String {
    match transport {
        Transport::Local => "Local".into(),
        Transport::Ssh { destination, .. } => format!("SSH {}", destination.as_str()),
        Transport::Tailnet { host, port } => format!("Tailnet {host}:{port}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Connected,
    Connecting,
    Offline,
}

fn state(current: bool, connected: bool, connecting: bool) -> Option<State> {
    current.then_some(if connecting {
        State::Connecting
    } else if connected {
        State::Connected
    } else {
        State::Offline
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    Machine(MachineId),
    Reconnect,
    Update,
    Retry,
    Settings,
    Add,
    Manage,
}

fn cursor_key(key: &str, index: usize, count: usize) -> Option<usize> {
    match key {
        "up" => Some(index.saturating_sub(1)),
        "down" => Some((index + 1).min(count.saturating_sub(1))),
        _ => None,
    }
}

impl Plait {
    pub(super) fn machine_picker_actions<E: InteractiveElement>(
        &self,
        root: E,
        cx: &mut Context<Self>,
    ) -> E {
        let root = root
            .on_action(cx.listener(|this, _: &SwitchMachine, window, cx| {
                this.open_machine_picker(window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddMachine, window, cx| {
                this.machine_settings_page(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReconnectMachine, window, cx| {
                if !this.settings_open && this.machine_offline() {
                    this.activate_choice(Choice::Reconnect, window, cx);
                }
            }))
            // GPUI reports focus-out after drawing. A composer binding can dispatch
            // between the focus change and that frame.
            .capture_action(cx.listener(|this, _: &super::composer::Submit, _, cx| {
                this.dismiss_picker_before_composer(cx)
            }))
            .capture_action(cx.listener(|this, _: &super::composer::Cancel, _, cx| {
                this.dismiss_picker_before_composer(cx)
            }))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.machine_picker_key(&event.keystroke, window, cx) {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }));
        root.on_action(cx.listener(|this, action: &MachineNumber, window, cx| {
            this.pick_machine_number(action.0, window, cx)
        }))
    }

    fn dismiss_picker_before_composer(&mut self, cx: &mut Context<Self>) {
        if self.navigation.menu.as_deref() == Some("machines") {
            self.navigation.menu = None;
            cx.stop_propagation();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    fn picker_state(&self, id: &MachineId) -> Option<State> {
        state(
            id == &self.machines.active,
            self.host.connection.connected,
            self.host.connection.connecting || self.machines.connecting.as_ref() == Some(id),
        )
    }

    pub(super) fn machine_offline(&self) -> bool {
        self.machines.connecting.is_none()
            && self.picker_state(&self.machines.active) == Some(State::Offline)
    }

    pub(super) fn open_machine_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.show_settings(false, window, cx);
        }
        self.close_overlay(true, window, cx);
        self.sidebar = true;
        self.navigation.menu = Some("machines".into());
        self.machines.cursor = Some(Choice::Machine(self.machines.active.clone()));
        let index = self
            .preferences
            .value
            .machines
            .list()
            .iter()
            .position(|m| m.id == self.machines.active)
            .unwrap_or(0);
        self.machines.picker_scroll.scroll_to_item(index + 1);
        window.focus(&self.machine_trigger, cx);
        cx.notify();
    }

    pub(super) fn machine_settings_page(
        &mut self,
        add: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation.menu = None;
        self.settings_tab = SettingsTab::Machines;
        self.show_settings(true, window, cx);
        if add {
            if self.machines.form.is_none() {
                self.open_machine_form(cx);
            }
            if let Some(form) = &self.machines.form {
                window.focus(&form.name.focus_handle(cx), cx);
            }
            self.settings_scroll.scroll_to_bottom();
        }
    }

    pub(super) fn pick_machine_number(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_open
            && let Some(machine) = self.preferences.value.machines.list().get(number - 1)
        {
            self.pick_machine(machine.id.clone(), window, cx);
        }
    }

    pub(super) fn pick_machine(
        &mut self,
        id: MachineId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation.menu = None;
        if self.machines.active == id
            && self
                .machines
                .connecting
                .as_ref()
                .is_some_and(|pending| *pending != id)
        {
            self.cancel_machine_switch();
            if self.preferences.value.selected_machine != id {
                self.save_selected(id, cx);
            }
        } else if self.machines.active != id && self.machines.connecting.as_ref() != Some(&id) {
            self.use_machine(id, window, cx);
        }
        cx.notify();
    }

    fn machine_choices(&self) -> Vec<Choice> {
        let mut choices = Vec::new();
        for m in self.preferences.value.machines.list() {
            choices.push(Choice::Machine(m.id.clone()));
            if m.id == self.machines.active {
                if self.machine_offline() {
                    choices.push(Choice::Reconnect);
                }
                if self
                    .machines
                    .update
                    .as_ref()
                    .is_some_and(|u| u.id == m.id && matches!(u.stage, Stage::Available(_)))
                {
                    choices.push(Choice::Update);
                }
            }
        }
        if self
            .machines
            .failed_switch
            .as_ref()
            .and_then(|id| self.preferences.value.machines.get(id))
            .is_some()
        {
            choices.extend([Choice::Retry, Choice::Settings]);
        }
        choices.extend([Choice::Add, Choice::Manage]);
        choices
    }

    pub(super) fn activate_choice(
        &mut self,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation.menu = None;
        match choice {
            Choice::Machine(id) => self.pick_machine(id, window, cx),
            Choice::Reconnect => {
                if self.machine_offline() {
                    self.reconnect_host(window, cx);
                }
            }
            Choice::Retry => {
                if let Some(id) = self.machines.failed_switch.clone() {
                    self.use_machine(id, window, cx);
                }
            }
            Choice::Settings => {
                self.machine_settings_page(false, window, cx);
                if self.machines.install.is_none()
                    && let Some(id) = self.machines.failed_switch.clone()
                {
                    self.check_install(id, cx);
                }
            }
            Choice::Manage | Choice::Update | Choice::Add => {
                self.machine_settings_page(choice == Choice::Add, window, cx)
            }
        }
        cx.notify();
    }

    pub(super) fn machine_picker_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.navigation.menu.as_deref() != Some("machines")
            || self.settings_open
            || self.overlay.is_some()
            || key.modifiers != Modifiers::default()
        {
            return false;
        }
        let choices = self.machine_choices();
        let index = choices
            .iter()
            .position(|c| Some(c) == self.machines.cursor.as_ref());
        match key.key.as_str() {
            "escape" => self.navigation.menu = None,
            "enter" => {
                if let Some(index) = index {
                    self.activate_choice(choices[index].clone(), window, cx);
                }
            }
            key => {
                let Some(next) = cursor_key(key, index.unwrap_or(0), choices.len()) else {
                    return false;
                };
                self.machines.cursor = Some(choices[next].clone());
                self.machines.picker_scroll.scroll_to_item(
                    choices[..=next]
                        .iter()
                        .filter(|choice| {
                            matches!(
                                choice,
                                Choice::Machine(_) | Choice::Retry | Choice::Add | Choice::Manage
                            )
                        })
                        .count(),
                );
            }
        }
        cx.notify();
        true
    }

    pub(super) fn machine_menu(&self, height: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = column()
            .id("sidebar-machines-menu")
            .track_focus(&self.machine_menu_focus)
            .absolute()
            .bottom(px(38.))
            .left(px(8.))
            .w(px(
                (self.panel_width(super::panels::Panel::Sidebar) - 16.).max(320.)
            ))
            .max_h(height * 0.8)
            .overflow_y_scroll()
            .track_scroll(&self.machines.picker_scroll)
            .p_2()
            .gap_1()
            .border_1()
            .border_color(rgb(colors().edge))
            .rounded(px(SURFACE_CORNER))
            .shadow(floating_shadow())
            .bg(rgb(colors().raised))
            .occlude()
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if !this
                    .machines
                    .trigger_bounds
                    .get()
                    .is_some_and(|bounds| bounds.contains(&event.position))
                {
                    this.navigation.menu = None;
                    cx.notify();
                }
            }))
            .child(tag("machine-menu-heading", "Machines", colors().muted));
        let choices = self.machine_choices();
        let mut entries = choices.into_iter().peekable();
        while let Some(choice) = entries.next() {
            match choice {
                Choice::Machine(id) => {
                    let mut line = row()
                        .gap_1()
                        .flex_shrink_0()
                        .child(self.machine_row(&id, cx));
                    while entries
                        .peek()
                        .is_some_and(|c| matches!(c, Choice::Reconnect | Choice::Update))
                    {
                        if let Some(action) = entries.next() {
                            line = line.child(self.picker_button(action, cx));
                        }
                    }
                    panel = panel.child(line);
                }
                Choice::Retry => {
                    let name = self
                        .machines
                        .failed_switch
                        .as_ref()
                        .and_then(|id| self.preferences.value.machines.get(id))
                        .map(|m| m.name.as_str())
                        .unwrap_or_default();
                    let mut line = row()
                        .gap_1()
                        .flex_shrink_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TYPE_META_SIZE))
                                .text_color(rgb(colors().amber))
                                .child(format!("Switch failed: {name}")),
                        )
                        .child(self.picker_button(choice, cx));
                    if let Some(action) = entries.next_if(|c| *c == Choice::Settings) {
                        line = line.child(self.picker_button(action, cx));
                    }
                    panel = panel.child(line);
                }
                action => panel = panel.child(self.picker_button(action, cx)),
            }
        }
        panel
    }

    fn machine_row(&self, id: &MachineId, cx: &Context<Self>) -> Control {
        let machine = self
            .preferences
            .value
            .machines
            .get(id)
            .expect("listed machine");
        let id = machine.id.clone();
        let state = self.picker_state(&id);
        let hint = address(&machine.transport);
        let spoken = match state {
            Some(s) => format!("{}, current, {s:?}, {hint}", machine.name),
            None => format!("{}, {hint}", machine.name),
        };
        nav_row(
            SharedString::from(format!("machine-use-{}", id.as_str())),
            spoken,
            state.is_some(),
        )
        .flex_1()
        .h(px(42.))
        .when(
            self.machines.cursor.as_ref() == Some(&Choice::Machine(id.clone())),
            |el| el.border_color(rgb(colors().focus_edge)),
        )
        .child(
            div()
                .w(px(12.))
                .flex_shrink_0()
                .when(state.is_some(), |el| el.child(icon("check").size(px(12.)))),
        )
        .child(div().w(px(8.)).flex_shrink_0().when_some(state, |el, s| {
            el.child(dot(match s {
                State::Connected => colors().mint,
                State::Offline => colors().amber,
                State::Connecting => colors().muted,
            }))
        }))
        .child(
            column()
                .min_w_0()
                .child(div().truncate().child(machine.name.clone()))
                .child(
                    div()
                        .truncate()
                        .text_size(px(TYPE_META_SIZE))
                        .text_color(rgb(colors().subtle))
                        .child(hint),
                ),
        )
        .on_activate(cx, move |this, window, cx| {
            this.pick_machine(id.clone(), window, cx)
        })
    }

    fn picker_button(&self, choice: Choice, cx: &Context<Self>) -> Control {
        let (id, label) = match choice {
            Choice::Reconnect => ("machine-reconnect", "Reconnect"),
            Choice::Update => ("machine-host-update", "Host update"),
            Choice::Retry => ("machine-retry", "Retry"),
            Choice::Settings => ("machine-error-settings", "Open settings…"),
            Choice::Add => ("machine-add", "Add machine…"),
            Choice::Manage => ("machine-manage", "Manage machines…"),
            Choice::Machine(_) => unreachable!("machine rows have a dedicated control"),
        };
        button(id, label)
            .flex_shrink_0()
            .when(self.machines.cursor.as_ref() == Some(&choice), |el| {
                el.border_color(rgb(colors().focus_edge))
            })
            .on_activate(cx, move |this, window, cx| {
                this.activate_choice(choice.clone(), window, cx)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{State, cursor_key, state};
    #[test]
    fn only_current_has_link_state() {
        for connected in [true, false] {
            for connecting in [true, false] {
                assert_eq!(state(false, connected, connecting), None);
            }
        }
        assert_eq!(state(true, true, false), Some(State::Connected));
        assert_eq!(state(true, false, true), Some(State::Connecting));
        assert_eq!(state(true, false, false), Some(State::Offline));
    }
    #[test]
    fn arrow_keys_stay_in_list() {
        assert_eq!(cursor_key("up", 0, 3), Some(0));
        assert_eq!(cursor_key("down", 0, 3), Some(1));
        assert_eq!(cursor_key("down", 2, 3), Some(2));
        assert_eq!(cursor_key("m", 1, 3), None);
    }
}
