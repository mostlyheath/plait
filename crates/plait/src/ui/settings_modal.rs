//! Window-owned settings surface; the workspace stays mounted underneath.
use super::{Plait, actions, settings::SettingsPage, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};

impl Plait {
    /// Application actions that may run without changing the covered workspace.
    fn settings_action_allowed(id: &str) -> bool {
        matches!(
            id,
            "app.settings"
                | "app.quit"
                | "settings.providers"
                | "project.settings"
                | "app.check_for_updates"
                | "window.minimize"
                | "focus.next"
                | "focus.previous"
        )
    }

    pub(super) fn settings_shortcuts(
        &self,
        mut root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if !self.settings_open {
            return root;
        }
        // Registered before the window's handlers, including menu dispatch.
        for action in actions::ACTIONS
            .iter()
            .filter(|action| !Self::settings_action_allowed(action.id))
        {
            root = root.on_boxed_action(action.action().as_ref(), |_, _, _| {});
        }
        // The recording interceptor may cancel before this same Escape reaches key handlers.
        let recording = self.keyboard.recording.is_some();
        root.on_action(cx.listener(|this, _: &super::Tab, window, cx| {
            this.settings_focus_step(true, window, cx)
        }))
        .on_action(cx.listener(|this, _: &super::BackTab, window, cx| {
            this.settings_focus_step(false, window, cx)
        }))
        .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" && this.overlay.is_none() {
                if recording || this.keyboard.recording.is_some() {
                    this.cancel_recording(cx);
                } else if this.harness_path_open() {
                    this.cancel_harness_path();
                    window.focus(&this.settings_focus, cx);
                    cx.notify();
                } else if this.providers.form.is_some() {
                    this.close_provider_form(cx);
                    window.focus(&this.settings_focus, cx);
                } else if this.settings_page == SettingsPage::Project
                    && this.project_policy().confirm
                {
                    // As Keep current policy does.
                    this.project_policy_mut().confirm = false;
                    window.focus(&this.settings_focus, cx);
                    cx.notify();
                } else {
                    this.show_settings(false, window, cx);
                }
                cx.stop_propagation();
            }
        }))
    }

    /// Tell assistive actions what covers the workspace this frame: the
    /// modal, and a picker opened over it.
    pub(super) fn cover_workspace(&self, window: &Window, cx: &mut Context<Self>) {
        let above = self.settings_open.then(|| {
            let mut above = vec![self.settings_focus.clone()];
            if let Some(overlay) = &self.overlay {
                above.push(overlay.palette.focus_handle(cx));
            }
            above
        });
        super::theme::cover(window, above, cx);
    }

    /// Focus moved beneath the modal, as an assistive Focus action can do
    /// without a click, goes back into it.
    pub(super) fn keep_focus_in_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.on_focus_out(&self.settings_focus, window, |this, _, window, cx| {
            // Leaving the app also reports focus out; a field keeps it then.
            if this.settings_open
                && this.overlay.is_none()
                && window.is_window_active()
                && !this.settings_focus.contains_focused(window, cx)
            {
                window.focus(&this.settings_focus, cx);
            }
        })
        .detach();
    }

    fn settings_focus_step(&self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        // A second key may arrive before the modal's first painted frame.
        if !self.settings_focus.is_rendered(window) {
            window.focus(&self.settings_focus, cx);
            return;
        }
        let start = window.focused(cx);
        loop {
            if forward {
                window.focus_next(cx);
            } else {
                window.focus_prev(cx);
            }
            if self.settings_focus.contains_focused(window, cx) {
                break;
            }
            if window.focused(cx) == start {
                window.focus(&self.settings_focus, cx);
                break;
            }
        }
    }

    pub(super) fn settings_modal(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let _above = super::theme::AboveModal::enter();
        let project = (self.settings_page == SettingsPage::Project)
            .then_some(self.settings_project.as_ref())
            .flatten();
        let title = project.map_or("Settings", |project| project.name.as_str());
        let body = match self.settings_page {
            SettingsPage::Application => self.settings(cx).into_any_element(),
            SettingsPage::Project => self.project_settings(cx).into_any_element(),
        };
        let viewport = window.viewport_size();
        let panel = column()
            .id("settings-modal")
            .role(Role::Dialog)
            .aria_label(if self.settings_page == SettingsPage::Project {
                "Project settings"
            } else {
                "Settings"
            })
            .audited()
            .track_focus(&self.settings_focus)
            .tab_group()
            .tab_stop(false)
            .w(px(820.).min(viewport.width - px(48.)))
            .h(px(720.).min(viewport.height - px(64.)))
            .overflow_hidden()
            .rounded(px(SURFACE_CORNER))
            .shadow(floating_shadow())
            .border_1()
            .border_color(rgb(colors().edge))
            .bg(rgb(colors().raised))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.overlay.is_none() && this.keyboard.recording.is_none() {
                    this.show_settings(false, window, cx);
                }
            }))
            .child(
                row()
                    .h(px(44.))
                    .flex_shrink_0()
                    .px(px(30.))
                    .gap_3()
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(super::settings::heading(title.to_owned()))
                            .when_some(project, |el, project| {
                                el.child(
                                    tag(
                                        "project-settings-checkout",
                                        project.checkout.to_string_lossy().into_owned(),
                                        colors().subtle,
                                    )
                                    .text_size(px(11.))
                                    .truncate(),
                                )
                            }),
                    )
                    .child(
                        toned_button("close-settings", "Done", Tone::Filled)
                            .tooltip(|_, cx| {
                                cx.new(|_| TextTooltip("Close settings (Escape)".into()))
                                    .into()
                            })
                            .on_activate(cx, |this, window, cx| {
                                this.show_settings(false, window, cx)
                            }),
                    ),
            )
            .child(body);
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., if colors().light { 0.25 } else { 0.45 }))
            .child(panel)
    }
}
