//! Cmd-Option-1 through Cmd-Option-9, and the desktop preferences file that backs them and
//! the other choices that belong to this desktop.

use super::{Plait, settings, theme::*};
use gpui::{prelude::*, *};
use plait_client::preferences::{self, JumpTarget, Preferences};

// Bound in the action table (actions.rs), like every application shortcut.
actions!(
    plait,
    [
        Jump1, Jump2, Jump3, Jump4, Jump5, Jump6, Jump7, Jump8, Jump9
    ]
);

/// Which part of the app a preference belongs to, so its saved state is shown
/// beside the control that changed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreferenceGroup {
    Models,
    Approvals,
    Keyboard,
    /// Settings → New threads: the harness new threads start in.
    Agent,
    /// Settings → Editor: what opens files and terminals.
    Editor,
    /// Layout choices made outside Settings, such as hiding the Git inspector.
    View,
    /// Settings → Machines: the machine list and the one in use.
    Machines,
}

/// Saved state of the preference file, shown beside the setting.
#[derive(Default)]
pub(super) struct PreferenceState {
    pub value: Preferences,
    pub favorites_revision: u64,
    pub loaded: bool,
    writable: bool,
    writes: preferences::SaveQueue,
    /// `None` when the platform gives no home directory; changes then last one session.
    path: Option<std::path::PathBuf>,
    /// Only the newest save may report its result.
    revision: u64,
    pub note: Option<String>,
    /// The group of the newest change. `None` while `note` is about loading.
    group: Option<PreferenceGroup>,
    /// `note` reports a problem, not progress.
    failed: bool,
}

impl PreferenceState {
    fn replace_value(&mut self, value: Preferences) {
        if self.value.favorite_models != value.favorite_models {
            self.favorites_revision = self.favorites_revision.wrapping_add(1);
        }
        self.value = value;
    }
}

impl Plait {
    pub(super) fn on_jump_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        macro_rules! jumps {
            ($root:expr, $($action:ident => $number:literal),*) => {
                $root$(.on_action(cx.listener(|this, _: &$action, window, cx| {
                    this.jump_to($number, window, cx)
                })))*
            };
        }
        jumps!(root, Jump1 => 1, Jump2 => 2, Jump3 => 3, Jump4 => 4, Jump5 => 5,
            Jump6 => 6, Jump7 => 7, Jump8 => 8, Jump9 => 9)
    }

    /// Select the Nth thread, machine, or project in sidebar order.
    pub(super) fn jump_to(&mut self, number: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(jump) = plait_client::navigation::jump(
            self.lists(),
            &self.state.projects,
            self.host.connection.threads(),
            &self.filtered_threads(cx),
            self.preferences.value.jump_target,
            number,
        ) else {
            return;
        };
        self.navigation
            .collapsed
            .remove(&self.state.projects[jump.project].checkout);
        self.state.collapsed_projects.remove(&jump.project);
        self.state
            .collapsed_machines
            .remove(&(jump.project, jump.machine));
        let id = self.host.connection.threads()[jump.thread].id.clone();
        self.select_host_thread(id, window, cx);
    }

    /// Read the file in the background. Until it loads, the default applies.
    pub(super) fn load_preferences(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            let path = preferences::default_path();
            let loaded = path.as_deref().map(preferences::load);
            (path, loaded)
        });
        cx.spawn(async move |this, cx| {
            let (path, loaded) = task.await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.preferences;
                state.path = path;
                state.loaded = true;
                state.writable = matches!(loaded, Some(Ok(_)));
                let edited = state.value.clone();
                state.failed = !state.writable;
                match loaded {
                    Some(Ok(value)) => match preferences::merge_startup(value.clone(), &edited) {
                        Ok(merged) => {
                            if merged != value {
                                state.revision += 1;
                                state.writes.queue(state.revision, merged.clone());
                            }
                            state.replace_value(merged);
                        }
                        Err(error) => {
                            state.writable = false;
                            state.failed = true;
                            state.note = Some(error);
                        }
                    },
                    Some(Err(error)) => state.note = Some(error),
                    None => {
                        state.note =
                            Some("No home directory; preferences last until Plait quits.".into())
                    }
                }
                super::actions::install(cx, &this.preferences.value.keymap);
                this.apply_theme(cx);
                this.apply_code_size(cx);
                this.reconcile_live_folding();
                // Settings opened before this read shows the default font.
                this.show_code_font(cx);
                this.show_editor_commands(cx);
                // The system's font needs no check; a chosen one is drawn
                // only after it has been found installed and measured.
                if this.preferences.value.code_font().is_some() {
                    this.load_fonts(cx);
                }
                this.pump_preferences(cx);
                let threads = this.host.connection.threads().to_vec();
                this.update_lists(
                    |lists| {
                        lists.observe(&threads, None, true);
                    },
                    cx,
                );
                this.rebuild_projects(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn set_jump_target(&mut self, target: JumpTarget, cx: &mut Context<Self>) {
        let mut value = self.preferences.value.clone();
        value.jump_target = target;
        self.set_preferences(value, PreferenceGroup::Keyboard, cx);
    }

    /// Apply now; report "Saved" only once the file write succeeds.
    pub(super) fn set_preferences(
        &mut self,
        value: Preferences,
        group: PreferenceGroup,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.preferences;
        if state.value == value && state.revision > 0 {
            return;
        }
        super::actions::install(cx, &value.keymap);
        state.replace_value(value.clone());
        state.revision += 1;
        state.group = Some(group);
        state.failed = state.path.is_none();
        if !state.loaded {
            state.note = Some("Waiting for saved preferences…".into());
        } else if !state.writable {
            state.failed = true;
            state.note = Some("Preferences were not loaded safely. Changes last until Plait quits; the saved file was left intact.".into());
        } else {
            state.note = Some("Saving…".into());
            state.writes.queue(state.revision, value);
            self.pump_preferences(cx);
        }
        cx.notify();
    }

    /// Background activity may update memory before loading, but cannot replace
    /// an unreadable file or suppress its error message.
    pub(super) fn persist_navigation(&mut self, value: Preferences, cx: &mut Context<Self>) {
        if self.preferences.loaded && self.preferences.writable {
            self.set_preferences(value, PreferenceGroup::View, cx);
        } else {
            self.preferences.value.navigation = value.navigation;
            cx.notify();
        }
    }

    fn pump_preferences(&mut self, cx: &mut Context<Self>) {
        if !self.preferences.writable {
            return;
        }
        let Some(path) = self.preferences.path.clone() else {
            return;
        };
        let Some((revision, value)) = self.preferences.writes.next_write() else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { preferences::save(&path, &value) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.preferences.writes.finish(revision);
                if this.preferences.revision == revision {
                    this.preferences.failed = result.is_err();
                    this.preferences.note = Some(match result {
                        Ok(()) => "Saved.".into(),
                        Err(error) => format!("{error} The choice applies until Plait quits."),
                    });
                    if this.preferences.failed
                        && this.preferences.group == Some(PreferenceGroup::Models)
                    {
                        this.overlay_note(this.preferences.note.clone().map(Into::into), cx);
                    }
                }
                this.pump_preferences(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The preference file's state, for the section whose control changed it.
    /// Loading problems and changes made outside Settings belong to no section.
    pub(super) fn preference_note(&self, group: Option<PreferenceGroup>) -> Option<String> {
        let state = &self.preferences;
        let shown = match state.group {
            // Outside a section only a problem is worth a line, never "Saved."
            None | Some(PreferenceGroup::View) => state.failed.then_some(None),
            changed => Some(changed),
        };
        state.note.clone().filter(|_| shown == Some(group))
    }

    pub(super) fn keyboard_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.preferences.value.jump_target;
        let mut choices = segmented();
        for (index, target) in JumpTarget::ALL.into_iter().enumerate() {
            choices = choices.child(
                segment(("jump-target", index), target.label(), target == current)
                    .role(Role::RadioButton)
                    .aria_toggled(if target == current {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .aria_label(format!("Sidebar shortcuts jump to {}", target.label()))
                    .on_activate(cx, move |this, _, cx| this.set_jump_target(target, cx)),
            );
        }
        settings::section("Keyboard")
            .child(settings::setting(
                "Sidebar jumps select",
                "Counted in sidebar order. A machine or project opens its first thread.",
                choices,
            ))
            .child(self.shortcut_editor(cx))
            .when_some(
                self.preference_note(Some(PreferenceGroup::Keyboard)),
                |el, note| el.child(tag("keyboard-preference-note", note, colors().subtle)),
            )
    }
}

#[cfg(test)]
mod favorite_revision_tests {
    use super::PreferenceState;
    use plait_client::host::ModelChoice;

    #[test]
    fn revision_changes_only_when_favorites_change() {
        let mut state = PreferenceState::default();
        state.replace_value(state.value.clone());
        assert_eq!(state.favorites_revision, 0);
        let mut value = state.value.clone();
        value.toggle_favorite(&ModelChoice {
            provider: "p".into(),
            model_id: "m".into(),
        });
        state.replace_value(value.clone());
        assert_eq!(state.favorites_revision, 1);
        state.replace_value(value);
        assert_eq!(state.favorites_revision, 1);
    }
}
