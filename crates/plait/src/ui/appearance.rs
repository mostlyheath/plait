//! Light and dark themes. Preferences name a theme for each appearance; the
//! appearance follows macOS unless one is chosen. Scheme files in the themes
//! folder are read in the background at launch and on request.

use super::{
    Plait,
    jump::PreferenceGroup,
    text_field::{TextField, TextFieldEvent},
    theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_core::theme::{self as themes, Appearance, DEFAULT_DARK, DEFAULT_LIGHT, Theme};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

/// The code font choice: the families installed on this Mac, read in the
/// background, and the field in Settings → Appearance that names one.
#[derive(Default)]
pub(super) struct CodeFont {
    /// Installed family names, sorted. `None` until they have been read.
    pub installed: Option<Arc<Vec<String>>>,
    /// What loading and measuring a family found, by its installed name.
    checked: HashMap<String, Fit>,
    /// Counts reads of the installed families, so a late one is dropped.
    reads: u64,
    /// The newest read has not landed yet.
    reading: bool,
    pub field: Option<Entity<TextField>>,
    /// The field holds what was typed since Settings opened. Until then it
    /// shows the saved choice, and follows it when the preferences load.
    edited: bool,
    /// Installed families offered for what is typed in the field.
    offered: Vec<SharedString>,
    _changed: Option<Subscription>,
}

// Only the macOS font scenes measure installed families.
#[cfg(all(test, target_os = "macos"))]
impl CodeFont {
    /// Nothing is being read or measured in the background.
    pub fn settled(&self) -> bool {
        !self.reading && !self.checked.values().any(|fit| *fit == Fit::Checking)
    }

    /// What measuring the installed family `name` found.
    pub fn fit(&self, name: &str) -> Option<Fit> {
        self.checked.get(name).copied()
    }
}

/// Whether code can be drawn in an installed family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fit {
    /// Being loaded and measured in the background.
    Checking,
    Monospaced,
    /// Its characters differ in width. File and diff views size their
    /// scrolling from the line with the most characters, so they need one width.
    Uneven,
    /// Listed as installed, but the text system drew another font for it.
    Unavailable,
}

/// The installed family that `wanted` names, whatever its case.
fn installed_family<'a>(installed: &'a [String], wanted: &str) -> Option<&'a str> {
    installed
        .iter()
        .find(|name| name.eq_ignore_ascii_case(wanted))
        .map(String::as_str)
}

/// Up to `limit` installed families whose name contains `typed`, those that
/// start with it first. Nothing for an empty or an exactly matching name.
fn family_suggestions<'a>(installed: &'a [String], typed: &str, limit: usize) -> Vec<&'a str> {
    if typed.is_empty() || installed_family(installed, typed).is_some() {
        return Vec::new();
    }
    let typed = typed.as_bytes();
    let starts = |name: &str| {
        name.as_bytes()
            .get(..typed.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(typed))
    };
    let contains = |name: &str| {
        name.as_bytes()
            .windows(typed.len())
            .any(|part| part.eq_ignore_ascii_case(typed))
    };
    let matching = |start: bool| {
        installed.iter().map(String::as_str).filter(move |name| {
            // System families begin with a dot and are not for choosing.
            !name.starts_with('.') && contains(name) && starts(name) == start
        })
    };
    matching(true).chain(matching(false)).take(limit).collect()
}

/// Load `family` and measure it. Loading a font can take a moment, so this
/// runs on the background executor.
fn check_family(text_system: &TextSystem, family: &str) -> Fit {
    /// Narrow, wide, and plain characters: one width in a monospaced font.
    const MEASURED: [char; 6] = ['i', 'l', 'm', 'W', '0', ' '];
    let id = text_system.resolve_font(&font(SharedString::from(family.to_owned())));
    // A family that cannot be loaded resolves to a fallback font instead.
    let loaded = text_system
        .get_font_for_id(id)
        .is_some_and(|found| found.family.eq_ignore_ascii_case(family));
    if !loaded {
        return Fit::Unavailable;
    }
    let width = |character| {
        text_system
            .advance(id, px(100.), character)
            .map(|advance| advance.width)
            .ok()
    };
    let Some(first) = width(MEASURED[0]) else {
        return Fit::Uneven;
    };
    let even = MEASURED[1..]
        .iter()
        .all(|character| width(*character).is_some_and(|w| (w - first).abs() < px(0.01)));
    if even { Fit::Monospaced } else { Fit::Uneven }
}

pub(super) struct Themes {
    built_in: Vec<Theme>,
    /// Read from the themes folder; listed before the shipped ones.
    pub user: Vec<Theme>,
    pub errors: Vec<String>,
    pub folder: Option<PathBuf>,
    /// The appearance override last given to macOS, so it is set only on change.
    applied: Option<Option<WindowAppearance>>,
    generation: u64,
    /// Scenes pin light or dark instead of asking the test platform.
    #[cfg(test)]
    pub forced: Option<bool>,
}

impl Default for Themes {
    fn default() -> Self {
        Self {
            built_in: themes::built_in(),
            user: Vec::new(),
            errors: Vec::new(),
            folder: themes::folder(),
            applied: None,
            generation: 0,
            #[cfg(test)]
            forced: Some(false),
        }
    }
}

impl Themes {
    /// Themes of one appearance: the folder's first, then the shipped ones.
    pub fn of(&self, light: bool) -> impl Iterator<Item = &Theme> {
        self.user
            .iter()
            .chain(&self.built_in)
            .filter(move |theme| theme.palette.light == light)
    }

    /// The palette for `name`, or the default when it is gone.
    fn palette(&self, name: Option<&str>, light: bool) -> themes::Palette {
        name.and_then(|name| self.of(light).find(|theme| theme.name == name))
            .map(|theme| theme.palette)
            .unwrap_or(if light { DEFAULT_LIGHT } else { DEFAULT_DARK })
    }
}

impl Plait {
    /// Pick the palette for the current appearance and redraw if it changed.
    pub(super) fn apply_theme(&mut self, cx: &mut Context<Self>) {
        let preferences = &self.preferences.value;
        let appearance = preferences.appearance;
        // Native window chrome follows the chosen appearance too.
        let wanted = match appearance {
            Appearance::System => None,
            Appearance::Light => Some(WindowAppearance::Light),
            Appearance::Dark => Some(WindowAppearance::Dark),
        };
        if self.themes.applied != Some(wanted) {
            self.themes.applied = Some(wanted);
            cx.set_window_appearance(wanted);
        }
        let light = self.light_appearance(appearance, cx);
        let name = if light {
            preferences.light_theme.as_deref()
        } else {
            preferences.dark_theme.as_deref()
        };
        if set_colors(&self.themes.palette(name, light)) {
            cx.refresh_windows();
        }
    }

    fn light_appearance(&self, appearance: Appearance, cx: &App) -> bool {
        #[cfg(test)]
        if let Some(light) = self.themes.forced {
            return light;
        }
        match appearance {
            Appearance::Light => true,
            Appearance::Dark => false,
            Appearance::System => matches!(
                cx.window_appearance(),
                WindowAppearance::Light | WindowAppearance::VibrantLight
            ),
        }
    }

    /// Follow macOS switching between light and dark.
    pub(super) fn observe_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.observe_window_appearance(window, |this, _, cx| this.apply_theme(cx))
            .detach();
    }

    /// Read the themes folder in the background, then apply the choice again,
    /// since a chosen theme may live there.
    pub(super) fn load_themes(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.themes.folder.clone() else {
            return;
        };
        self.themes.generation += 1;
        let generation = self.themes.generation;
        let task = cx
            .background_executor()
            .spawn(async move { themes::load_folder(&folder) });
        cx.spawn(async move |this, cx| {
            let (user, errors) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.themes.generation != generation {
                    return;
                }
                this.themes.user = user;
                this.themes.errors = errors;
                this.apply_theme(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Read the installed font families in the background and check the
    /// saved choice against them, then draw code in it if it can be used.
    /// Runs when the preferences load and each time Settings opens, since
    /// fonts are installed and removed while Plait runs.
    pub(super) fn load_fonts(&mut self, cx: &mut Context<Self>) {
        self.code_font.reads += 1;
        self.code_font.reading = true;
        let read = self.code_font.reads;
        let wanted = self.preferences.value.code_font().map(str::to_owned);
        let text_system = cx.text_system().clone();
        let task = cx.background_executor().spawn(async move {
            let installed = text_system.all_font_names();
            let checked = wanted.and_then(|wanted| {
                let name = installed_family(&installed, &wanted)?.to_owned();
                let fit = check_family(&text_system, &name);
                Some((name, fit))
            });
            (installed, checked)
        });
        cx.spawn(async move |this, cx| {
            let (installed, checked) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.code_font.reads != read {
                    return;
                }
                this.code_font.reading = false;
                // What was learned about a family holds while it is installed.
                this.code_font
                    .checked
                    .retain(|name, _| installed.binary_search(name).is_ok());
                this.code_font.checked.extend(checked);
                this.code_font.installed = Some(Arc::new(installed));
                this.apply_code_font(cx);
                // A name typed while the families were being read counts now.
                this.code_font_typed(cx);
            });
        })
        .detach();
    }

    /// Draw code in the saved family when it is installed and monospaced,
    /// else in the system's monospaced font.
    pub(super) fn apply_code_font(&mut self, cx: &mut Context<Self>) {
        let known = &self.code_font;
        let chosen = self.preferences.value.code_font().and_then(|wanted| {
            let name = installed_family(known.installed.as_deref()?, wanted)?;
            (known.checked.get(name) == Some(&Fit::Monospaced)).then_some(name)
        });
        let family = match chosen {
            Some(name) => SharedString::from(name.to_owned()),
            None => SharedString::new_static(SYSTEM_MONO),
        };
        if set_mono(family) {
            // Another font has other widths and heights.
            self.workspace.live_scroll.remeasure();
            cx.refresh_windows();
        }
    }

    /// Settings is opening: make the field that names a family, show the
    /// saved choice in it, and learn which families are installed now.
    pub(super) fn prepare_code_font(&mut self, cx: &mut Context<Self>) {
        if self.code_font.field.is_none() {
            let field = cx.new(|cx| TextField::new("Code font", "System monospaced", cx));
            self.code_font._changed =
                Some(cx.subscribe(&field, |this, _, _: &TextFieldEvent, cx| {
                    this.code_font_changed(cx)
                }));
            self.code_font.field = Some(field);
        }
        // A name left half typed last time is gone: the field shows the choice.
        self.code_font.edited = false;
        self.show_code_font(cx);
        self.load_fonts(cx);
    }

    /// Put the saved choice in the field, unless a name is being typed
    /// there. A field made before the preferences loaded shows the default
    /// until this runs again.
    pub(super) fn show_code_font(&mut self, cx: &mut Context<Self>) {
        let Some(field) = self.code_font.field.clone() else {
            return;
        };
        if self.code_font.edited {
            return;
        }
        let chosen = self.preferences.value.code_font().unwrap_or_default();
        if field.read(cx).text() != chosen {
            let chosen = chosen.to_owned();
            field.update(cx, |field, cx| field.set_text(&chosen, cx));
        }
        self.offer_code_fonts(cx);
    }

    /// What is in the field, without the space around it. `None` until
    /// Settings has been opened: there is no field to read.
    fn typed_code_font(&self, cx: &App) -> Option<String> {
        let field = self.code_font.field.as_ref()?;
        Some(field.read(cx).text().trim().to_owned())
    }

    /// Work out which installed families to offer for what is in the field,
    /// when it or the families change, so drawing Settings does not search.
    fn offer_code_fonts(&mut self, cx: &mut Context<Self>) {
        /// Installed families offered while a name is being typed.
        const OFFERED: usize = 5;
        let typed = self.typed_code_font(cx).unwrap_or_default();
        self.code_font.offered = self
            .code_font
            .installed
            .as_deref()
            .map(|names| family_suggestions(names, &typed, OFFERED))
            .unwrap_or_default()
            .into_iter()
            .map(|name| SharedString::from(name.to_owned()))
            .collect();
    }

    /// The field's text changed. Showing the saved choice there is not
    /// typing, so it never changes the choice: only other text does.
    fn code_font_changed(&mut self, cx: &mut Context<Self>) {
        let saved = self.preferences.value.code_font().unwrap_or_default();
        if self.typed_code_font(cx).is_some_and(|typed| typed != saved) {
            self.code_font.edited = true;
        }
        self.code_font_typed(cx);
    }

    /// Act on what was typed, now or once its family has been read and
    /// measured. An empty field means the system's font; the name of an
    /// installed monospaced family is saved and drawn; anything else leaves
    /// the choice as it was.
    fn code_font_typed(&mut self, cx: &mut Context<Self>) {
        self.offer_code_fonts(cx);
        cx.notify();
        let Some(typed) = self.typed_code_font(cx).filter(|_| self.code_font.edited) else {
            return;
        };
        if typed.is_empty() {
            self.choose_code_font(None, cx);
            return;
        }
        let Some(installed) = self.code_font.installed.clone() else {
            return;
        };
        let Some(name) = installed_family(&installed, &typed) else {
            return;
        };
        match self.code_font.checked.get(name) {
            Some(Fit::Monospaced) => self.choose_code_font(Some(name.to_owned()), cx),
            Some(_) => {}
            None => self.check_code_font(name.to_owned(), cx),
        }
    }

    /// Load and measure an installed family in the background, then read
    /// the field again: the name in it may still be this one.
    fn check_code_font(&mut self, name: String, cx: &mut Context<Self>) {
        self.code_font.checked.insert(name.clone(), Fit::Checking);
        let text_system = cx.text_system().clone();
        let task = cx.background_executor().spawn({
            let name = name.clone();
            async move { check_family(&text_system, &name) }
        });
        cx.spawn(async move |this, cx| {
            let fit = task.await;
            let _ = this.update(cx, |this, cx| {
                this.code_font.checked.insert(name, fit);
                this.code_font_typed(cx);
            });
        })
        .detach();
    }

    /// Save the choice and draw code in it.
    fn choose_code_font(&mut self, chosen: Option<String>, cx: &mut Context<Self>) {
        if chosen.as_deref() != self.preferences.value.code_font() {
            let mut value = self.preferences.value.clone();
            value.code_font = chosen;
            self.set_preferences(value, PreferenceGroup::View, cx);
        }
        self.apply_code_font(cx);
    }

    /// What the field's name comes to, as a tint and a sentence.
    fn code_font_note(&self, typed: &str) -> (u32, String) {
        if typed.is_empty() {
            return (colors().mint, "Using the system's monospaced font.".into());
        }
        let reading = || (colors().subtle, "Reading the installed fonts…".to_owned());
        let Some(installed) = self.code_font.installed.as_deref() else {
            return reading();
        };
        // A name that cannot be used changes nothing; say what is drawn.
        let drawn = mono();
        let stays = if drawn.as_ref() == SYSTEM_MONO {
            "Code stays in the system's monospaced font.".to_owned()
        } else {
            format!("Code stays in {drawn}.")
        };
        let Some(name) = installed_family(installed, typed) else {
            // The families read last time may lack one installed since.
            if self.code_font.reading {
                return reading();
            }
            return (
                colors().amber,
                format!("No installed font is named “{typed}”. {stays}"),
            );
        };
        match self.code_font.checked.get(name) {
            Some(Fit::Monospaced) => (colors().mint, format!("Using {name}.")),
            Some(Fit::Uneven) => (
                colors().amber,
                format!("{name} is not a monospaced font. {stays}"),
            ),
            Some(Fit::Unavailable) => (
                colors().amber,
                format!("{name} could not be loaded. {stays}"),
            ),
            Some(Fit::Checking) | None => (colors().subtle, format!("Checking {name}…")),
        }
    }

    /// Settings → Appearance: the font code, paths, and commands are drawn in.
    fn code_font_setting(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (tint, note) = self.code_font_note(&self.typed_code_font(cx).unwrap_or_default());
        let offered = self.code_font.offered.clone();
        let control = column()
            .w(px(260.))
            .flex_shrink_0()
            .gap(px(6.))
            .when_some(self.code_font.field.clone(), |el, field| el.child(field))
            .child(
                row()
                    .gap(px(6.))
                    .items_start()
                    .child(div().h(px(16.)).flex().items_center().child(dot(tint)))
                    .child(tag("code-font-note", note, colors().subtle).line_height(px(16.))),
            )
            .when(!offered.is_empty(), |el| {
                el.child(row().flex_wrap().gap(px(4.)).children(
                    offered.into_iter().enumerate().map(|(index, name)| {
                        let family = name.clone();
                        button(("code-font-offer", index), name.clone())
                            .aria_label(format!("Use {name} for code"))
                            .on_activate(cx, move |this, _, cx| {
                                if let Some(field) = this.code_font.field.clone() {
                                    field.update(cx, |field, cx| field.set_text(&family, cx));
                                }
                            })
                    }),
                ))
            });
        super::settings::setting(
            "Code font",
            "Code, file paths, and commands. Type the name of an installed monospaced font, or leave it empty for the system's.",
            control,
        )
    }

    fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        let mut value = self.preferences.value.clone();
        value.appearance = appearance;
        self.set_preferences(value, PreferenceGroup::View, cx);
        self.apply_theme(cx);
    }

    fn set_theme(&mut self, light: bool, name: String, cx: &mut Context<Self>) {
        let mut value = self.preferences.value.clone();
        // The default is stored as unset, so it follows future defaults.
        let default = if light {
            themes::DEFAULT_LIGHT_NAME
        } else {
            themes::DEFAULT_DARK_NAME
        };
        let name = (name != default).then_some(name);
        if light {
            value.light_theme = name;
        } else {
            value.dark_theme = name;
        }
        self.set_preferences(value, PreferenceGroup::View, cx);
        self.apply_theme(cx);
    }

    /// Open the themes folder in Finder, creating it first so there is
    /// somewhere to put scheme files.
    fn open_themes_folder(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.themes.folder.clone() else {
            return;
        };
        let task = cx.background_executor().spawn({
            let folder = folder.clone();
            async move { std::fs::create_dir_all(&folder) }
        });
        cx.spawn(async move |this, cx| {
            let created = task.await;
            let _ = this.update(cx, |this, cx| match created {
                Ok(()) => cx.open_with_system(&folder),
                Err(error) => {
                    this.themes.errors = vec![format!("Cannot create the themes folder: {error}")];
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Settings → Appearance: light or dark, a theme for each, the themes
    /// folder, and the code font.
    pub(super) fn theme_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.preferences.value.appearance;
        let mut choices = segmented();
        for (index, (label, value)) in [
            ("System", Appearance::System),
            ("Light", Appearance::Light),
            ("Dark", Appearance::Dark),
        ]
        .into_iter()
        .enumerate()
        {
            choices = choices.child(
                segment(("appearance", index), label, value == chosen)
                    .role(Role::RadioButton)
                    .aria_toggled(if value == chosen {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .aria_label(format!("Light or dark: {label}"))
                    .on_activate(cx, move |this, _, cx| this.set_appearance(value, cx)),
            );
        }
        let folder_note = match &self.themes.folder {
            Some(folder) => format!(
                "Add Base16 scheme files (.yaml) to {} and choose Reload. Hundreds are published at github.com/tinted-theming/schemes.",
                super::theme::home_relative(folder)
            ),
            None => "No themes folder: the home directory is unknown.".into(),
        };
        column()
            .gap_4()
            .child(super::settings::setting(
                "Light or dark",
                "System follows macOS between light and dark.",
                choices,
            ))
            .child(
                row()
                    .gap_4()
                    .items_start()
                    .child(self.theme_list(true, cx))
                    .child(self.theme_list(false, cx)),
            )
            .child(
                column()
                    .gap_2()
                    .child(
                        tag("themes-folder-note", folder_note, colors().subtle)
                            .line_height(px(16.)),
                    )
                    .child(
                        row()
                            .gap_2()
                            .child(
                                button("themes-open-folder", "Open themes folder")
                                    .on_activate(cx, |this, _, cx| this.open_themes_folder(cx)),
                            )
                            .child(
                                button("themes-reload", "Reload")
                                    .on_activate(cx, |this, _, cx| this.load_themes(cx)),
                            ),
                    )
                    .children(self.themes.errors.iter().enumerate().map(|(index, error)| {
                        tag(("theme-error", index), error.clone(), colors().amber)
                            .line_height(px(16.))
                    })),
            )
            .child(self.code_font_setting(cx))
    }

    fn theme_list(&self, light: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let preferences = &self.preferences.value;
        let chosen = if light {
            preferences
                .light_theme
                .as_deref()
                .unwrap_or(themes::DEFAULT_LIGHT_NAME)
        } else {
            preferences
                .dark_theme
                .as_deref()
                .unwrap_or(themes::DEFAULT_DARK_NAME)
        };
        let label = if light { "Light theme" } else { "Dark theme" };
        column()
            .id(if light { "themes-light" } else { "themes-dark" })
            .flex_1()
            .min_w_0()
            .gap_1()
            .role(Role::RadioGroup)
            .aria_label(label)
            .audited()
            .child(section_label(
                if light {
                    "themes-light-heading"
                } else {
                    "themes-dark-heading"
                },
                label,
            ))
            .children(self.themes.of(light).enumerate().map(|(index, theme)| {
                let selected = theme.name == chosen;
                let name = theme.name.clone();
                let tip = match &theme.source {
                    themes::Source::BuiltIn => "Plait's own theme".to_owned(),
                    themes::Source::Included { credit } => (*credit).to_owned(),
                    themes::Source::Bundled { author } if !author.is_empty() => {
                        format!("Base16 scheme by {author}")
                    }
                    themes::Source::Bundled { .. } => "Base16 scheme".to_owned(),
                    themes::Source::File(path) => path.display().to_string(),
                };
                nav_row(
                    (if light { "theme-light" } else { "theme-dark" }, index),
                    format!("{label}: {}", theme.name),
                    selected,
                )
                .role(Role::RadioButton)
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .gap_2()
                .px_2()
                .tooltip(move |_, cx| cx.new(|_| TextTooltip(tip.clone())).into())
                .child(swatch(&theme.palette))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .child(theme.name.clone()),
                )
                .on_activate(cx, move |this, _, cx| {
                    this.set_theme(light, name.clone(), cx)
                })
            }))
    }
}

/// A theme's background with its text and four accents on it.
fn swatch(palette: &themes::Palette) -> Div {
    let mark = |color: u32| div().size(px(6.)).rounded_full().bg(rgb(color));
    row()
        .flex_shrink_0()
        .gap(px(2.))
        .px(px(5.))
        .h(px(16.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(colors().border))
        .bg(rgb(palette.bg))
        .child(mark(palette.text))
        .child(mark(palette.blue))
        .child(mark(palette.mint))
        .child(mark(palette.amber))
        .child(mark(palette.red))
}

#[cfg(test)]
mod tests {
    use super::{family_suggestions, installed_family};

    fn installed() -> Vec<String> {
        [
            ".AppleSystemUIFontMonospaced",
            "Courier New",
            "JetBrains Mono",
            "JetBrains Mono NL",
            "Menlo",
            "SF Compact Rounded",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn a_family_is_found_by_its_name_whatever_the_case() {
        let installed = installed();
        assert_eq!(installed_family(&installed, "menlo"), Some("Menlo"));
        assert_eq!(installed_family(&installed, "Menlo Bold"), None);
        assert_eq!(installed_family(&installed, ""), None);
    }

    #[test]
    fn suggestions_lead_with_names_that_start_with_what_was_typed() {
        let installed = installed();
        assert_eq!(
            family_suggestions(&installed, "mono", 5),
            ["JetBrains Mono", "JetBrains Mono NL"]
        );
        // Names that start with the text come before those that only contain it.
        assert_eq!(
            family_suggestions(&installed, "co", 5),
            ["Courier New", "SF Compact Rounded"]
        );
        assert_eq!(family_suggestions(&installed, "jet", 1), ["JetBrains Mono"]);
        // Nothing to offer for an empty field or a name that is already whole,
        // and system families are not offered.
        assert!(family_suggestions(&installed, "", 5).is_empty());
        assert!(family_suggestions(&installed, "Menlo", 5).is_empty());
        assert!(family_suggestions(&installed, "apple", 5).is_empty());
    }
}
