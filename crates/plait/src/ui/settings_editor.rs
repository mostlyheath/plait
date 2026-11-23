//! Settings → Editor: what Open in Editor and Open Terminal Here run. Each is
//! a ready choice or a custom command line; a custom command that cannot be
//! used is not saved, and the line beneath its field says why.

use super::{
    Plait,
    jump::PreferenceGroup,
    settings::{section, setting},
    text_field::{TextField, TextFieldEvent},
    theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_client::handoff::{self, EditorChoice, Purpose, TerminalChoice};

/// Which of the two settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Opens {
    Files,
    Terminals,
}

/// The position of Custom among each setting's choices.
const CUSTOM: usize = 2;

/// Said of every custom command, on hover.
const QUOTING: &str = "Quotes and backslashes work as in a shell, but nothing runs through a shell, so nothing is expanded. Name a program outside /usr/bin and /bin by its full path.";

impl Opens {
    fn title(self) -> &'static str {
        match self {
            Self::Files => "Open files with",
            Self::Terminals => "Open terminals with",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Files => "open-files-with",
            Self::Terminals => "open-terminals-with",
        }
    }

    fn note_id(self) -> &'static str {
        match self {
            Self::Files => "open-files-note",
            Self::Terminals => "open-terminals-note",
        }
    }

    fn purpose(self) -> Purpose {
        match self {
            Self::Files => Purpose::Editor,
            Self::Terminals => Purpose::Terminal,
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Files => "zed {file}:{line}",
            Self::Terminals => "wezterm start --cwd {project}",
        }
    }

    /// Each choice's label and hover text, Custom last.
    fn choices(self) -> [(&'static str, String); 3] {
        match self {
            Self::Files => [
                (
                    EditorChoice::System.label(),
                    "Opens the file in the app macOS uses for its type. No line is given.".into(),
                ),
                (
                    EditorChoice::RexNeovim.label(),
                    "Runs nvim +line -- file in a new Rex tab at the project's checkout. Rex is found by its bundle ID, com.superlogical.rex, so Rex and Rex Beta both answer.".into(),
                ),
                (
                    "Custom",
                    format!("One line, such as zed {{file}}:{{line}}. {{file}} is the file's full path, {{line}} the line, and {{project}} the checkout. {QUOTING}"),
                ),
            ],
            Self::Terminals => [
                (
                    TerminalChoice::Terminal.label(),
                    "Opens a Terminal window at the project's checkout.".into(),
                ),
                (
                    TerminalChoice::Rex.label(),
                    "Opens a Rex tab at the project's checkout. Rex is found by its bundle ID, com.superlogical.rex.".into(),
                ),
                (
                    "Custom",
                    format!("One line, such as wezterm start --cwd {{project}}. {{project}} is the checkout's full path. {QUOTING}"),
                ),
            ],
        }
    }
}

/// A custom command's field and what was found in it.
#[derive(Default)]
pub(super) struct CommandField {
    pub field: Option<Entity<TextField>>,
    /// Custom is chosen on screen. It is saved once the field holds a
    /// command that can be used.
    pub custom: bool,
    /// Why the command in the field cannot be used, worked out when it changes.
    pub error: Option<String>,
    _changed: Option<Subscription>,
}

#[derive(Default)]
pub(super) struct EditorSettings {
    pub files: CommandField,
    pub terminals: CommandField,
}

impl EditorSettings {
    pub(super) fn get(&self, opens: Opens) -> &CommandField {
        match opens {
            Opens::Files => &self.files,
            Opens::Terminals => &self.terminals,
        }
    }

    fn get_mut(&mut self, opens: Opens) -> &mut CommandField {
        match opens {
            Opens::Files => &mut self.files,
            Opens::Terminals => &mut self.terminals,
        }
    }
}

impl Plait {
    /// Settings is opening: make the two fields, and show the saved choices.
    pub(super) fn prepare_editor_settings(&mut self, cx: &mut Context<Self>) {
        for opens in [Opens::Files, Opens::Terminals] {
            if self.editor_settings.get(opens).field.is_some() {
                continue;
            }
            let field = cx.new(|cx| TextField::new(opens.title(), opens.placeholder(), cx));
            let changed = cx.subscribe(&field, move |this, _, _: &TextFieldEvent, cx| {
                this.custom_command_typed(opens, cx)
            });
            let state = self.editor_settings.get_mut(opens);
            state.field = Some(field);
            state._changed = Some(changed);
        }
        self.show_editor_commands(cx);
    }

    /// The saved custom command, if Custom is the saved choice.
    fn saved_command(&self, opens: Opens) -> Option<&str> {
        let preferences = &self.preferences.value;
        match (
            opens,
            &preferences.open_files_with,
            &preferences.open_terminals_with,
        ) {
            (Opens::Files, EditorChoice::Custom(command), _)
            | (Opens::Terminals, _, TerminalChoice::Custom(command)) => Some(command),
            _ => None,
        }
    }

    /// Show the saved choices: Custom, with its command in the field, or a
    /// ready choice. Also run when the preferences finish loading.
    pub(super) fn show_editor_commands(&mut self, cx: &mut Context<Self>) {
        for opens in [Opens::Files, Opens::Terminals] {
            let saved = self.saved_command(opens).map(str::to_owned);
            let state = self.editor_settings.get_mut(opens);
            state.custom = saved.is_some();
            state.error = None;
            let (Some(field), Some(saved)) = (state.field.clone(), saved) else {
                continue;
            };
            if field.read(cx).text() != saved {
                field.update(cx, |field, cx| field.set_text(&saved, cx));
            }
        }
        cx.notify();
    }

    /// A choice was made: a ready one is saved now; Custom is saved once its
    /// field holds a command that can be used.
    pub(super) fn choose_command(&mut self, opens: Opens, index: usize, cx: &mut Context<Self>) {
        let state = self.editor_settings.get_mut(opens);
        state.custom = index == CUSTOM;
        state.error = None;
        if index == CUSTOM {
            return self.custom_command_typed(opens, cx);
        }
        let mut value = self.preferences.value.clone();
        let saved = match opens {
            Opens::Files => value.set_open_files_with(if index == 0 {
                EditorChoice::System
            } else {
                EditorChoice::RexNeovim
            }),
            Opens::Terminals => value.set_open_terminals_with(if index == 0 {
                TerminalChoice::Terminal
            } else {
                TerminalChoice::Rex
            }),
        };
        if saved.is_ok() {
            self.set_preferences(value, PreferenceGroup::Editor, cx);
        }
        cx.notify();
    }

    /// The custom command changed: save it if it can be used, or say why not.
    fn custom_command_typed(&mut self, opens: Opens, cx: &mut Context<Self>) {
        let state = self.editor_settings.get(opens);
        let (Some(field), true) = (state.field.clone(), state.custom) else {
            return;
        };
        let command = field.read(cx).text().to_owned();
        let mut value = self.preferences.value.clone();
        let checked = match opens {
            Opens::Files => value.set_open_files_with(EditorChoice::Custom(command)),
            Opens::Terminals => value.set_open_terminals_with(TerminalChoice::Custom(command)),
        };
        self.editor_settings.get_mut(opens).error = checked.err();
        if self.editor_settings.get(opens).error.is_none() && value != self.preferences.value {
            self.set_preferences(value, PreferenceGroup::Editor, cx);
        }
        cx.notify();
    }

    /// The choice shown as selected: Custom while it is being written,
    /// otherwise the saved one.
    fn chosen_command(&self, opens: Opens) -> usize {
        if self.editor_settings.get(opens).custom {
            return CUSTOM;
        }
        let preferences = &self.preferences.value;
        match opens {
            Opens::Files => match preferences.open_files_with {
                EditorChoice::System => 0,
                EditorChoice::RexNeovim => 1,
                EditorChoice::Custom(_) => CUSTOM,
            },
            Opens::Terminals => match preferences.open_terminals_with {
                TerminalChoice::Terminal => 0,
                TerminalChoice::Rex => 1,
                TerminalChoice::Custom(_) => CUSTOM,
            },
        }
    }

    /// What the field's command comes to, as a tint and a sentence.
    fn command_note(&self, opens: Opens, cx: &App) -> (u32, String) {
        let state = self.editor_settings.get(opens);
        let empty = state
            .field
            .as_ref()
            .is_none_or(|field| field.read(cx).text().trim().is_empty());
        match &state.error {
            // Nothing typed yet is not a mistake.
            Some(error) if empty => (colors().subtle, error.clone()),
            Some(error) => (colors().amber, format!("Not saved. {error}")),
            None => {
                let program = self
                    .saved_command(opens)
                    .and_then(|command| handoff::template(command, opens.purpose()).ok())
                    .map(|template| template.program().to_owned())
                    .unwrap_or_default();
                (
                    colors().mint,
                    format!("Runs {program} directly, not through a shell."),
                )
            }
        }
    }

    /// One setting: its choices, and the custom command's field when chosen.
    fn command_setting(&self, opens: Opens, detail: String, cx: &mut Context<Self>) -> Div {
        let chosen = self.chosen_command(opens);
        let mut choices = segmented();
        for (index, (label, hover)) in opens.choices().into_iter().enumerate() {
            let selected = index == chosen;
            choices = choices.child(
                segment((opens.id(), index), label, selected)
                    .role(Role::RadioButton)
                    .aria_toggled(if selected {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .aria_label(format!("{}: {label}", opens.title()))
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(hover.clone())).into())
                    .on_activate(cx, move |this, _, cx| this.choose_command(opens, index, cx)),
            );
        }
        let state = self.editor_settings.get(opens);
        let custom = state.custom.then(|| state.field.clone()).flatten();
        let control = column()
            .w(px(300.))
            .flex_shrink_0()
            .gap(px(6.))
            // At the right edge, as every other setting's control is.
            .child(choices.self_end())
            .when_some(custom, |el, field| {
                let (tint, note) = self.command_note(opens, cx);
                el.child(field).child(
                    row()
                        .gap(px(6.))
                        .items_start()
                        .child(div().h(px(16.)).flex().items_center().child(dot(tint)))
                        // A long program path wraps rather than running off.
                        .child(
                            tag(opens.note_id(), note, colors().subtle)
                                .flex_1()
                                .min_w_0()
                                .line_height(px(16.)),
                        ),
                )
            });
        setting(opens.title(), detail, control)
    }

    /// `title`, then its shortcut in parentheses when it has one.
    fn with_shortcut(&self, title: &str, id: &str) -> String {
        match self.shortcut_label(id) {
            label if label.is_empty() => title.to_owned(),
            label => format!("{title} ({label})"),
        }
    }

    /// Settings → Editor.
    pub(super) fn editor_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.with_shortcut("Open in Editor", "file.open_in_editor");
        let terminal = self.with_shortcut("Open Terminal Here", "project.open_terminal");
        section("Editor and terminal")
            .child(self.command_setting(
                Opens::Files,
                format!("{editor} opens the file in Files or Changes, at the line on screen."),
                cx,
            ))
            .child(self.command_setting(
                Opens::Terminals,
                format!("{terminal} opens the project's checkout."),
                cx,
            ))
            .when_some(
                self.preference_note(Some(PreferenceGroup::Editor)),
                |el, note| el.child(tag("editor-preference-note", note, colors().subtle)),
            )
    }
}
