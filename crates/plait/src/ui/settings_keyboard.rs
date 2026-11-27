//! Shortcut editing. Recording intercepts keys before action dispatch.

use super::{Plait, actions, jump::PreferenceGroup, text_field::TextField, theme::*};
use gpui::{prelude::*, *};

#[derive(Default)]
pub(super) struct KeyboardState {
    pub search: Option<Entity<TextField>>,
    pub recording: Option<usize>,
    interceptor: Option<Subscription>,
    candidate: Option<Vec<Keystroke>>,
    note: Option<String>,
}

impl Plait {
    pub(super) fn on_keyboard_controls(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            if this.settings_open
                && this.keyboard.recording.is_some()
                && event.keystroke.key == "escape"
            {
                cx.stop_propagation();
                this.cancel_recording(cx);
            }
        }))
    }

    pub(super) fn prepare_keyboard(&mut self, cx: &mut Context<Self>) {
        if self.keyboard.search.is_none() {
            let search = cx.new(|cx| {
                TextField::new("Find shortcut", "Find an action or shortcut", cx).search()
            });
            cx.observe(&search, |_, _, cx| cx.notify()).detach();
            self.keyboard.search = Some(search);
        }
    }

    pub(super) fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        self.keyboard.recording = None;
        self.keyboard.interceptor = None;
        self.keyboard.candidate = None;
        self.keyboard.note = None;
        cx.notify();
    }

    pub(super) fn record_shortcut(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_recording(cx);
        self.keyboard.recording = Some(index);
        window.focus(&self.settings_focus, cx);
        let entity = cx.weak_entity();
        let window_id = window.window_handle().window_id();
        self.keyboard.interceptor = Some(cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != window_id {
                return;
            }
            let _ = entity.update(cx, |this, cx| {
                if this.keyboard.recording.is_none() || !this.settings_open {
                    return;
                }
                cx.stop_propagation();
                if event.keystroke.key == "escape" {
                    this.cancel_recording(cx);
                } else if !event.keystroke.is_modifier_key() {
                    this.recorded_shortcut(&event.keystroke, cx);
                }
            });
        }));
    }

    fn recorded_shortcut(&mut self, pressed: &Keystroke, cx: &mut Context<Self>) {
        let Some(index) = self.keyboard.recording else {
            return;
        };
        let shortcuts = actions::shortcuts();
        let shortcut = &shortcuts[index];
        let keys = match shortcut.candidate(pressed) {
            Ok(keys) => keys,
            Err(note) => {
                self.keyboard.note = Some(note);
                cx.notify();
                return;
            }
        };
        if let Some(note) = actions::refusal(shortcut, &keys, actions::editing_keys(cx)) {
            self.keyboard.note = Some(note);
            cx.notify();
            return;
        }
        self.offer_shortcut(index, keys, cx);
    }

    fn offer_shortcut(&mut self, index: usize, keys: Vec<Keystroke>, cx: &mut Context<Self>) {
        let shortcuts = actions::shortcuts();
        let shortcut = &shortcuts[index];
        let conflicts = actions::conflicts(shortcut, &keys, &self.preferences.value.keymap);
        if conflicts.iter().any(|action| action.required()) {
            self.keyboard.note = Some(
                "That shortcut keeps keyboard focus navigation reachable. Choose another.".into(),
            );
            cx.notify();
            return;
        }
        if conflicts.is_empty() {
            self.apply_shortcut(index, keys, cx);
        } else {
            self.keyboard.note = Some(format!(
                "Already used by {}. Reassigning removes those shortcuts.",
                conflicts
                    .iter()
                    .map(|action| action.title)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            self.keyboard.candidate = Some(keys);
            // Buttons must be usable after recording the candidate.
            self.keyboard.interceptor = None;
            cx.notify();
        }
    }

    fn reset_shortcut(&mut self, index: usize, cx: &mut Context<Self>) {
        let shortcuts = actions::shortcuts();
        let shortcut = &shortcuts[index];
        let defaults = shortcut.defaults();
        self.cancel_recording(cx);
        if defaults.is_empty() {
            let mut value = self.preferences.value.clone();
            for action in shortcut.actions {
                value.keymap.remove(action.id);
            }
            self.set_preferences(value, PreferenceGroup::Keyboard, cx);
        } else {
            self.keyboard.recording = Some(index);
            self.offer_shortcut(index, defaults, cx);
        }
    }

    fn apply_shortcut(&mut self, index: usize, keys: Vec<Keystroke>, cx: &mut Context<Self>) {
        let shortcuts = actions::shortcuts();
        let shortcut = &shortcuts[index];
        let mut value = self.preferences.value.clone();
        let displaced = actions::conflicts(shortcut, &keys, &value.keymap);
        actions::assign(&mut value.keymap, shortcut, &keys, &displaced);
        self.cancel_recording(cx);
        self.set_preferences(value, PreferenceGroup::Keyboard, cx);
    }

    pub(super) fn shortcut_editor(&self, cx: &mut Context<Self>) -> Div {
        let mut list = column().gap_1();
        let query = self
            .keyboard
            .search
            .as_ref()
            .map_or("", |field| field.read(cx).text());
        let keymap = &self.preferences.value.keymap;
        let mut category = "";
        let mut count = 0;
        for (index, shortcut) in actions::shortcuts().into_iter().enumerate() {
            let mut title = shortcut.title().to_owned();
            if let Some(number) = shortcut.actions[0]
                .id
                .strip_prefix("machine_jump.")
                .and_then(|n| n.parse::<usize>().ok())
            {
                if let Some(machine) = self.preferences.value.machines.list().get(number - 1) {
                    title = format!("Switch to {} (machine {number})", machine.name);
                } else if number > 9 && !shortcut.changed(keymap) {
                    continue;
                }
            }
            if !shortcut.matches(keymap, query)
                && !title.to_lowercase().contains(&query.to_lowercase())
            {
                continue;
            }
            count += 1;
            if category != shortcut.category() {
                category = shortcut.category();
                list = list.child(
                    section_label(
                        (
                            ElementId::from("shortcut-category"),
                            SharedString::from(category),
                        ),
                        category,
                    )
                    .mt_3()
                    .mb_1(),
                );
            }
            let recording = self.keyboard.recording == Some(index);
            let binding = shortcut.binding(keymap);
            let label = if recording {
                self.keyboard
                    .candidate
                    .as_ref()
                    .map_or_else(|| "Press shortcut…".to_owned(), |keys| actions::label(keys))
            } else if binding.label().is_empty() {
                "Unbound".into()
            } else {
                binding.label()
            };
            let changed = shortcut.changed(keymap);
            let required = shortcut.actions.iter().any(|action| action.required());
            let mut item = column().gap_1().child(
                row()
                    .gap_2()
                    .justify_between()
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(12.)).child(title.clone()))
                            .when(changed, |el| {
                                el.child(tag(
                                    ("shortcut-customized", index),
                                    "Customized",
                                    colors().subtle,
                                ))
                            }),
                    )
                    .child(
                        toned_button(
                            ("record-shortcut", index),
                            label,
                            if recording {
                                Tone::Selected
                            } else {
                                Tone::Filled
                            },
                        )
                        .aria_label(format!("{}: {}. Record shortcut", title, binding.spoken()))
                        .on_activate(cx, move |this, window, cx| {
                            this.record_shortcut(index, window, cx)
                        }),
                    )
                    .child(
                        button(("clear-shortcut", index), "Clear")
                            .aria_label(format!("Clear {title} shortcut"))
                            .disabled(required)
                            .when(!required, |el| {
                                el.on_activate(cx, move |this, _, cx| {
                                    let mut value = this.preferences.value.clone();
                                    actions::unbind(
                                        &mut value.keymap,
                                        &actions::shortcuts()[index],
                                    );
                                    this.cancel_recording(cx);
                                    this.set_preferences(value, PreferenceGroup::Keyboard, cx);
                                })
                            }),
                    )
                    .child(
                        button(("reset-shortcut", index), "Reset")
                            .aria_label(format!("Reset {title} shortcut"))
                            .disabled(!changed)
                            .when(changed, |el| {
                                el.on_activate(cx, move |this, _, cx| {
                                    this.reset_shortcut(index, cx);
                                })
                            }),
                    ),
            );
            if let Some(saved) = shortcut.unusable(keymap) {
                item = item.child(tag(
                    ("shortcut-unusable", index),
                    format!("Cannot read saved shortcut {saved:?}; using the default."),
                    colors().amber,
                ));
            }
            if recording {
                item = item
                    .child(tag(
                        ("shortcut-recording-note", index),
                        self.keyboard
                            .note
                            .as_deref()
                            .unwrap_or("Press a key combination. Escape cancels."),
                        colors().amber,
                    ))
                    .child(
                        row()
                            .gap_2()
                            .when_some(self.keyboard.candidate.clone(), |el, keys| {
                                el.child(
                                    button(("reassign-shortcut", index), "Reassign").on_activate(
                                        cx,
                                        move |this, _, cx| {
                                            this.apply_shortcut(index, keys.clone(), cx)
                                        },
                                    ),
                                )
                            })
                            .child(
                                button(("cancel-shortcut", index), "Cancel")
                                    .on_activate(cx, |this, _, cx| this.cancel_recording(cx)),
                            ),
                    );
            }
            list = list.child(item);
        }
        if count == 0 {
            list = list.child(tag(
                "shortcuts-empty",
                "No shortcuts match.",
                colors().muted,
            ));
        }
        let unknown = actions::unknown(keymap);
        column().gap_2()
            .when_some(self.keyboard.search.clone(), |el, field| el.child(field))
            .child(list)
            .when(unknown > 0, |el| el.child(tag("shortcuts-unknown", format!("{unknown} shortcut entries belong to another app version and are preserved."), colors().subtle)))
            .child(row().child(button("reset-all-shortcuts", "Reset all shortcuts").on_activate(cx, |this, _, cx| {
                let mut value = this.preferences.value.clone();
                actions::reset_all(&mut value.keymap);
                this.cancel_recording(cx);
                this.set_preferences(value, PreferenceGroup::Keyboard, cx);
            })))
    }
}
