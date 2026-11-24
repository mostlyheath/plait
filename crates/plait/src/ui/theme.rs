use gpui::{prelude::*, *};
use plait_client::host;
use std::borrow::Cow;

pub use super::a11y::{AuditA11y, Audited, text as a11y_text};
pub use plait_client::theme::{Palette, mix};

/// Interactive control element: audited in debug tests, zero-cost `Stateful<Div>` otherwise.
pub type Control = Audited<Stateful<Div>>;

thread_local! {
    /// The palette every view draws with. Per thread, so parallel UI tests
    /// with different themes do not see each other's colors.
    static CURRENT: std::cell::Cell<&'static Palette> =
        const { std::cell::Cell::new(&plait_client::theme::DEFAULT_DARK) };
}

/// Under a window modal, assistive actions reach only the modal and what
/// opens above it, as the pointer does through its scrim. GPUI runs an
/// element's accessibility action directly, without a hit test, so the
/// window's action shield and the scrim do not stop it.
#[derive(Default)]
pub(super) struct Covered(std::collections::HashMap<WindowId, Vec<FocusHandle>>);

impl Global for Covered {}

thread_local! {
    /// Set while controls in or above a window modal are built.
    static ABOVE_MODAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While held, built controls belong to a window modal or to something
/// opened over it, and stay reachable by assistive actions.
pub(super) struct AboveModal(bool);

impl AboveModal {
    pub(super) fn enter() -> Self {
        Self(ABOVE_MODAL.with(|above| above.replace(true)))
    }
}

impl Drop for AboveModal {
    fn drop(&mut self) {
        ABOVE_MODAL.with(|above| above.set(self.0));
    }
}

/// Record the focus handles of what covers the window's content, or `None`.
pub(super) fn cover(window: &Window, above: Option<Vec<FocusHandle>>, cx: &mut App) {
    let id = window.window_handle().window_id();
    if cx
        .try_global::<Covered>()
        .and_then(|covered| covered.0.get(&id))
        == above.as_ref()
    {
        return;
    }
    let covered = cx.default_global::<Covered>();
    match above {
        Some(above) => covered.0.insert(id, above),
        None => covered.0.remove(&id),
    };
}

/// An assistive action on an element beneath a window modal is refused.
/// `focus` is the element's own focus handle, when it has one.
pub(super) fn beneath_modal(focus: Option<&FocusHandle>, window: &Window, cx: &App) -> bool {
    let id = window.window_handle().window_id();
    cx.try_global::<Covered>()
        .and_then(|covered| covered.0.get(&id))
        .is_some_and(|above| {
            !focus.is_some_and(|focus| above.iter().any(|handle| handle.contains(focus, window)))
        })
}

/// The current theme's colors. Read during render; cheap.
pub fn colors() -> &'static Palette {
    CURRENT.with(std::cell::Cell::get)
}

/// Draw with `palette` from now on. False when it is already in use.
pub fn set_colors(palette: &Palette) -> bool {
    if colors() == palette {
        return false;
    }
    // A few hundred bytes per switch, kept so `colors()` can hand out a
    // plain reference without locking.
    CURRENT.with(|current| current.set(Box::leak(Box::new(*palette))));
    true
}

/// Readable column for conversation text and the composer.
pub const CONTENT_WIDTH: f32 = 760.;
/// Shared chrome row: the sidebar brand row and the workspace header align.
pub const CHROME_HEIGHT: f32 = 40.;
/// Compact row for navigation, changed files, and model choices.
pub const ROW_HEIGHT: f32 = 28.;

pub const CONTROL_CORNER: f32 = 7.;
pub const ITEM_CORNER: f32 = 6.;
pub const SURFACE_CORNER: f32 = 12.;
pub const PALETTE_CORNER: f32 = 14.;

/// Type scale from DESIGN.md: point sizes, line heights, and weights.
/// Reserved entries stay ready for later lanes without changing values here.
#[allow(dead_code)]
pub const TYPE_PAGE_TITLE_SIZE: f32 = 20.;
#[allow(dead_code)]
pub const TYPE_PAGE_TITLE_LINE: f32 = 26.;
#[allow(dead_code)]
pub const TYPE_PAGE_TITLE_WEIGHT: FontWeight = FontWeight::SEMIBOLD;

pub const TYPE_BODY_SIZE: f32 = 14.5;
pub const TYPE_BODY_LINE: f32 = 23.;
#[allow(dead_code)]
pub const TYPE_DRAFT_LINE: f32 = 21.;

pub const TYPE_HEADER_SIZE: f32 = 14.;
pub const TYPE_HEADER_LINE: f32 = 20.;
pub const TYPE_HEADER_WEIGHT: FontWeight = FontWeight::SEMIBOLD;

pub const TYPE_ROW_SIZE: f32 = 13.5;
pub const TYPE_ROW_LINE: f32 = 19.;
#[allow(dead_code)]
pub const TYPE_CARD_TITLE_WEIGHT: FontWeight = FontWeight::SEMIBOLD;

pub const TYPE_CONTROL_SIZE: f32 = 12.;
pub const TYPE_CONTROL_LINE: f32 = 17.;
pub const TYPE_CONTROL_WEIGHT: FontWeight = FontWeight::MEDIUM;

pub const TYPE_META_SIZE: f32 = 11.;
pub const TYPE_META_LINE: f32 = 15.;
pub const TYPE_META_WEIGHT: FontWeight = FontWeight::MEDIUM;

pub const TYPE_KEY_HINT_SIZE: f32 = 10.5;

#[allow(dead_code)]
pub const TYPE_REGULAR: FontWeight = FontWeight::NORMAL;
pub const TYPE_MEDIUM: FontWeight = FontWeight::MEDIUM;
#[allow(dead_code)]
pub const TYPE_SEMIBOLD: FontWeight = FontWeight::SEMIBOLD;

/// OpenType tabular figures (`tnum=1`), shared so in-place numbers do not
/// allocate per frame and keep constant digit width in System UI font.
pub fn tabular_nums() -> FontFeatures {
    static TABULAR: std::sync::LazyLock<FontFeatures> =
        std::sync::LazyLock::new(|| FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])));
    TABULAR.clone()
}

fn shadow_layer(y: f32, blur: f32, spread: f32, tint: u32) -> BoxShadow {
    BoxShadow::new(px(0.), px(y), rgba(tint).into())
        .blur_radius(px(blur))
        .spread_radius(px(spread))
}

pub fn floating_shadow() -> Vec<BoxShadow> {
    if colors().light {
        vec![
            shadow_layer(14., 36., -12., 0x242a3e38),
            shadow_layer(2., 6., 0., 0x242a3e14),
        ]
    } else {
        vec![
            shadow_layer(18., 44., -12., 0x000000b3),
            shadow_layer(2., 8., 0., 0x00000066),
        ]
    }
}

pub fn box_shadow() -> Vec<BoxShadow> {
    vec![shadow_layer(
        8.,
        24.,
        -10.,
        if colors().light {
            0x242a3e24
        } else {
            0x00000073
        },
    )]
}

/// The system's own interface font family.
pub const SYSTEM_UI_FONT: &str = ".SystemUIFont";
/// The bundled interface font family (Regular, Medium, SemiBold).
pub const PLEX_UI_FONT: &str = "IBM Plex Sans";

const BUNDLED_UI_FONT_PATHS: [&str; 3] = [
    "fonts/IBMPlexSans-Regular.otf",
    "fonts/IBMPlexSans-Medium.otf",
    "fonts/IBMPlexSans-SemiBold.otf",
];

const BUNDLED_UI_WEIGHTS: [FontWeight; 3] =
    [FontWeight::NORMAL, FontWeight::MEDIUM, FontWeight::SEMIBOLD];

/// The system's own monospaced font: SF Mono on macOS, asked for by the
/// family name Core Text gives it, since it is not installed under a public one.
#[cfg(target_os = "macos")]
pub const SYSTEM_MONO: &str = ".AppleSystemUIFontMonospaced";
#[cfg(not(target_os = "macos"))]
pub const SYSTEM_MONO: &str = "DejaVu Sans Mono";

thread_local! {
    /// Whether every bundled IBM Plex Sans face loaded and resolved cleanly.
    static BUNDLED_UI_LOADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The family interface labels, controls, and prose are drawn in.
    static UI_FONT: std::cell::RefCell<SharedString> =
        const { std::cell::RefCell::new(SharedString::new_static(SYSTEM_UI_FONT)) };
    /// The family code, paths, and commands are drawn in. Per thread, like
    /// the palette, so parallel UI tests do not see each other's choice.
    static CODE_FONT: std::cell::RefCell<SharedString> =
        const { std::cell::RefCell::new(SharedString::new_static(SYSTEM_MONO)) };
    /// The point size code, paths, and commands are drawn at. Per thread, like
    /// the palette and code font, so parallel UI tests stay isolated.
    static CODE_SIZE: std::cell::Cell<f32> =
        const { std::cell::Cell::new(plait_client::preferences::CODE_SIZE_DEFAULT as f32) };
}

/// Register the bundled IBM Plex Sans faces and verify that Regular, Medium,
/// and SemiBold each resolve directly to distinct faces before drawing with them.
pub fn load_bundled_fonts(text_system: &TextSystem) -> bool {
    let mut faces = Vec::with_capacity(BUNDLED_UI_FONT_PATHS.len());
    for path in BUNDLED_UI_FONT_PATHS {
        match Assets.load(path) {
            Ok(Some(bytes)) => faces.push(bytes),
            _ => {
                BUNDLED_UI_LOADED.with(|loaded| loaded.set(false));
                return false;
            }
        }
    }
    if text_system.add_fonts(faces).is_err() {
        BUNDLED_UI_LOADED.with(|loaded| loaded.set(false));
        return false;
    }
    let mut resolved = Vec::with_capacity(BUNDLED_UI_WEIGHTS.len());
    for weight in BUNDLED_UI_WEIGHTS {
        let wanted = Font {
            weight,
            ..font(SharedString::new_static(PLEX_UI_FONT))
        };
        let id = text_system.resolve_font(&wanted);
        let direct = text_system
            .get_font_for_id(id)
            .is_some_and(|found| found.family.eq_ignore_ascii_case(PLEX_UI_FONT));
        if !direct || resolved.contains(&id) {
            BUNDLED_UI_LOADED.with(|loaded| loaded.set(false));
            return false;
        }
        resolved.push(id);
    }
    BUNDLED_UI_LOADED.with(|loaded| loaded.set(true));
    true
}

/// True when all three bundled IBM Plex Sans weights loaded at startup.
pub fn bundled_ui_font_loaded() -> bool {
    BUNDLED_UI_LOADED.with(std::cell::Cell::get)
}

/// The interface font family. Read during render; cheap.
pub fn ui_font() -> SharedString {
    UI_FONT.with(|font| font.borrow().clone())
}

/// Draw the interface in `choice` (falling back to the system UI font if the
/// bundled faces did not load). False when already in use.
pub fn set_ui_font(choice: plait_client::preferences::InterfaceFont) -> bool {
    let family = match choice {
        plait_client::preferences::InterfaceFont::Plex if bundled_ui_font_loaded() => {
            SharedString::new_static(PLEX_UI_FONT)
        }
        _ => SharedString::new_static(SYSTEM_UI_FONT),
    };
    UI_FONT.with(|font| {
        if *font.borrow() == family {
            return false;
        }
        *font.borrow_mut() = family;
        true
    })
}

/// The code font's family. Read during render; cheap.
pub fn mono() -> SharedString {
    CODE_FONT.with(|font| font.borrow().clone())
}

/// Draw code in `family` from now on. False when it is already in use.
/// Callers pass only a family that is installed: GPUI draws a missing one in
/// a proportional fallback.
pub fn set_mono(family: SharedString) -> bool {
    CODE_FONT.with(|font| {
        if *font.borrow() == family {
            return false;
        }
        *font.borrow_mut() = family;
        true
    })
}

/// The code font's point size (12–14). Read during render; cheap.
pub fn code_size() -> f32 {
    CODE_SIZE.with(std::cell::Cell::get)
}

/// Draw code at `points` (clamped to 12–14) from now on. False when unchanged.
pub fn set_code_size(points: u8) -> bool {
    let points = plait_client::preferences::clamp_code_size(points) as f32;
    CODE_SIZE.with(|size| {
        if (size.get() - points).abs() < f32::EPSILON {
            return false;
        }
        size.set(points);
        true
    })
}

/// Line height for monospace code blocks, inline conversation diffs, and tool
/// command output: 7 points of leading over the code font size (19 at 12,
/// 20 at 13, 21 at 14) so 13- and 14-point faces never clip.
pub fn code_line_height() -> f32 {
    code_size() + 7.
}

pub fn row() -> Div {
    div().flex().items_center().gap_2()
}
pub fn column() -> Div {
    div().flex().flex_col()
}

/// How much a control asks for attention. GPUI accepts one hover style per
/// element, so the tone is chosen when the control is built.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Quiet until hovered: navigation, toolbars, secondary actions.
    Ghost,
    /// The current row or tab. Pair it with `aria_selected`.
    Selected,
    /// A raised secondary action.
    Filled,
    /// The one affirmative action in a view.
    Primary,
}

pub(super) fn hovered<E: Styled>(style: E, tone: Tone) -> E {
    match tone {
        Tone::Ghost => style
            .bg(rgba(colors().hover))
            .text_color(rgb(colors().text)),
        Tone::Filled => style
            .bg(rgb(colors().selected))
            .border_color(rgb(colors().faint)),
        Tone::Primary => style.opacity(0.88),
        Tone::Selected => style,
    }
}

pub(super) fn pressed<E: Styled>(style: E, tone: Tone) -> E {
    match tone {
        Tone::Ghost => style.opacity(0.6),
        Tone::Filled | Tone::Primary => style.opacity(0.7),
        Tone::Selected => style,
    }
}

pub(super) fn focused_control<E: Styled>(style: E) -> E {
    // A hollow inset keeps the surface visible with the pinned GPUI styles.
    style.shadow(vec![
        BoxShadow::new(px(0.), px(0.), rgb(colors().blue).into())
            .spread_radius(px(1.5))
            .inset(),
    ])
}

/// Focusable, keyboard-reachable base for every original control.
fn control(id: impl Into<ElementId>, name: SharedString, tone: Tone) -> Control {
    let base = row()
        .id(id)
        .role(Role::Button)
        .aria_label(name)
        .focusable()
        .tab_stop(true)
        .cursor_pointer()
        .rounded(px(CONTROL_CORNER))
        .border_1()
        .border_color(gpui::transparent_black())
        .focus_visible(focused_control)
        .when(tone != Tone::Selected, |el| {
            el.hover(move |s| hovered(s, tone))
                .active(move |s| pressed(s, tone))
        })
        .audited();
    match tone {
        Tone::Ghost => base.text_color(rgb(colors().muted)),
        Tone::Selected => base
            .bg(rgb(colors().selected))
            .text_color(rgb(colors().text)),
        Tone::Filled => base
            .bg(rgb(colors().raised))
            .border_color(rgb(colors().edge))
            .text_color(rgb(colors().text)),
        Tone::Primary => base
            .bg(rgb(colors().strong))
            .text_color(rgb(colors().bg))
            .font_weight(TYPE_CONTROL_WEIGHT),
    }
}

pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Control {
    toned_button(id, label, Tone::Ghost)
}

pub fn toned_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
) -> Control {
    let label = label.into();
    control(id, label.clone(), tone)
        .h(px(ROW_HEIGHT))
        .flex_shrink_0()
        .text_size(px(TYPE_CONTROL_SIZE))
        .px(px(11.))
        .font_weight(TYPE_CONTROL_WEIGHT)
        // Callers that pass no label supply their own children.
        .when(!label.is_empty(), |el| el.child(label))
}

/// Secondary action that stays out of sight until the pointer is over its
/// `group` or it takes keyboard focus, so reading is not interrupted by chrome.
pub fn quiet_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    group: impl Into<SharedString>,
) -> Control {
    button(id, label)
        .h(px(24.))
        .opacity(0.)
        .group_hover(group, |s| s.opacity(1.))
        .focus_visible(|s| s.border_color(rgb(colors().blue)).opacity(1.))
}

/// Square glyph control. The name is the accessible label; no text is drawn.
pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    name: impl Into<SharedString>,
) -> Control {
    toned_icon_button(id, glyph, name, Tone::Ghost)
}

pub fn toned_icon_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    name: impl Into<SharedString>,
    tone: Tone,
) -> Control {
    // GPUI tints a glyph with its own color, not the control's text color.
    let tint = match tone {
        Tone::Ghost => colors().muted,
        Tone::Selected | Tone::Filled => colors().text,
        Tone::Primary => colors().bg,
    };
    control(id, name.into(), tone)
        .size(px(ROW_HEIGHT))
        .flex_shrink_0()
        .justify_center()
        .child(icon(glyph).size(px(16.)).text_color(rgb(tint)))
}

/// Full-width row for the navigation rail and lists. Callers add the content,
/// and `aria_selected` where the row's role supports selection.
pub fn nav_row(id: impl Into<ElementId>, name: impl Into<SharedString>, selected: bool) -> Control {
    let tone = if selected {
        Tone::Selected
    } else {
        Tone::Ghost
    };
    control(id, name.into(), tone)
        .w_full()
        .min_w_0()
        .h(px(ROW_HEIGHT))
        .px_2()
        .gap(px(6.))
        .text_size(px(TYPE_ROW_SIZE))
}

pub trait ButtonAction {
    fn on_activate<T: 'static>(
        self,
        cx: &Context<T>,
        action: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Self;
}

impl<E: StatefulInteractiveElement> ButtonAction for E {
    fn on_activate<T: 'static>(
        self,
        cx: &Context<T>,
        action: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
    ) -> Self {
        let action = std::rc::Rc::new(action);
        let click = action.clone();
        let entity = cx.weak_entity();
        let above = ABOVE_MODAL.with(std::cell::Cell::get);
        self.on_click(cx.listener(move |this, _, window, cx| click(this, window, cx)))
            // Bind identity directly; GPUI's fallback simulates a click at cached bounds.
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
                if !above && beneath_modal(None, window, cx) {
                    return;
                }
                let _ = entity.update(cx, |this, cx| action(this, window, cx));
            })
    }
}

pub trait DisabledControl {
    fn disabled(self, disabled: bool) -> Self;
}

impl<E: StatefulInteractiveElement + Styled + FluentBuilder> DisabledControl for E {
    fn disabled(self, disabled: bool) -> Self {
        self.when(disabled, |el| {
            el.opacity(0.5)
                .cursor_default()
                .a11y_synthetic_children(|builder| {
                    builder.parent_node().set_disabled();
                })
        })
    }
}

/// Accessible secondary label with a caller-supplied, place-based element ID.
/// Never derives its ID from `text`, so identical strings at the same level
/// cannot collide in the accessibility tree.
pub fn tag(id: impl Into<ElementId>, text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .text_size(px(TYPE_META_SIZE))
        .text_color(rgb(color))
        .child(a11y_text(id, text))
}

pub fn key_hint(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Div {
    tag(id, text, colors().subtle)
        .h(px(17.))
        .px(px(4.))
        .rounded(px(4.))
        .bg(rgba(colors().hover))
        .text_size(px(TYPE_KEY_HINT_SIZE))
        .font_family(mono())
}

pub fn host_status_color(status: host::ThreadStatus) -> u32 {
    match status {
        host::ThreadStatus::Idle => colors().mint,
        host::ThreadStatus::Running => colors().blue,
        host::ThreadStatus::AwaitingApproval | host::ThreadStatus::Interrupted => colors().amber,
        host::ThreadStatus::Stopped => colors().muted,
        host::ThreadStatus::Error => colors().red,
    }
}

/// Small uppercase-free heading above a group, in the manner of a list label.
pub fn section_label(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Div {
    tag(id, text, colors().subtle).font_weight(TYPE_META_WEIGHT)
}

/// Two-option switch. The caller supplies tab buttons with `aria_selected`.
pub fn segmented() -> Div {
    row()
        .gap(px(2.))
        .p(px(2.))
        .flex_shrink_0()
        .rounded(px(CONTROL_CORNER))
        .bg(rgb(colors().rail))
        .border_1()
        .border_color(rgb(colors().border))
}

pub fn segment(id: impl Into<ElementId>, label: &'static str, selected: bool) -> Control {
    let tone = if selected {
        Tone::Selected
    } else {
        Tone::Ghost
    };
    toned_button(id, label, tone)
        .h(px(22.))
        .rounded(px(ITEM_CORNER))
        .role(Role::Tab)
        .aria_selected(selected)
}

/// Compact on/off filter: outlined when off, filled when on.
pub fn chip(id: impl Into<ElementId>, label: &'static str, on: bool) -> Control {
    let tone = if on { Tone::Selected } else { Tone::Ghost };
    control(id, label.into(), tone)
        .h(px(22.))
        .px(px(9.))
        .flex_shrink_0()
        .rounded(px(ITEM_CORNER))
        .text_size(px(TYPE_META_SIZE))
        .when(!on, |el| el.border_color(rgb(colors().border)))
        .when(on, |el| el.border_color(rgb(colors().edge)))
        .aria_toggled(if on { Toggled::True } else { Toggled::False })
        .child(label)
}

/// Open assistant text on the conversation surface.
pub fn prose() -> Div {
    div()
        .text_size(px(TYPE_BODY_SIZE))
        .line_height(px(TYPE_BODY_LINE))
}

/// Pulls a disclosure row left so its chevron sits on the text edge.
pub const TOOL_INSET: f32 = 5.;

/// Indented monospace block for tool input, output, and exact arguments.
pub fn code_block() -> Div {
    column()
        .gap_1()
        .ml(px(6.))
        .pl_4()
        .py_1()
        .border_l_1()
        .border_color(rgb(colors().border))
        .font_family(mono())
        .text_size(px(code_size()))
        .line_height(px(code_line_height()))
        .text_color(rgb(colors().muted))
}

pub fn thread_row_colors(
    state: plait_client::navigation::ThreadRowState,
    selected: bool,
) -> (u32, u32, u32) {
    use plait_client::navigation::ThreadRowState::*;
    let c = colors();
    let (node, title, detail) = match state {
        Working => (c.blue, c.muted, c.blue),
        Approval | Interrupted => (c.amber, c.text, c.amber),
        Failed => (c.red, c.text, c.red),
        Queued(_) | Other => (c.subtle, c.muted, c.subtle),
        FinishedUnread => (c.mint, c.text, c.subtle),
    };
    (node, if selected { c.strong } else { title }, detail)
}

pub fn thread_mark(color: u32) -> Div {
    // Interrupt the line so hover and selection show through the three-point gap.
    let line = || {
        div()
            .w(px(1.))
            .h(px((ROW_HEIGHT - 13.) / 2.))
            .bg(rgb(colors().edge))
    };
    column()
        .flex_shrink_0()
        .items_center()
        .gap(px(3.))
        .child(line())
        .child(dot(color).size(px(7.)))
        .child(line())
}

pub fn dot(color: u32) -> Div {
    div()
        .size(px(7.))
        .flex_shrink_0()
        .rounded_full()
        .bg(rgb(color))
}

pub fn icon(name: &'static str) -> Svg {
    svg()
        .path(name)
        .size(px(14.))
        .flex_shrink_0()
        .text_color(rgb(if matches!(name, "right" | "up" | "down") {
            colors().faint
        } else {
            colors().muted
        }))
}

/// The logo of the harness that runs a thread, tinted like other icons. A
/// harness without one in `assets/harnesses` has none, and callers show its name.
pub fn harness_logo(id: plait_client::host::harness::HarnessId) -> Option<Svg> {
    harness_glyph(id).map(icon)
}

/// The asset name of a harness's logo, for lists that take a glyph name.
pub fn harness_glyph(id: plait_client::host::harness::HarnessId) -> Option<&'static str> {
    use plait_client::host::harness::HarnessId;
    match id {
        HarnessId::Pi => Some("harness-pi"),
        HarnessId::Codex => Some("harness-openai"),
        HarnessId::Claude => Some("harness-claude"),
        HarnessId::Other => None,
    }
}

pub fn empty(title: &'static str, detail: impl Into<SharedString>) -> Control {
    let detail = detail.into();
    column()
        .id(title)
        .role(Role::Group)
        .aria_label(format!("{title}. {detail}"))
        .audited()
        .flex_1()
        .min_h_0()
        .justify_center()
        .items_center()
        .p_6()
        .gap_2()
        .child(
            div()
                .text_size(px(TYPE_HEADER_SIZE))
                .line_height(px(TYPE_HEADER_LINE))
                .font_weight(TYPE_HEADER_WEIGHT)
                .text_color(rgb(colors().strong))
                .child(title),
        )
        .child(
            div()
                .max_w(px(380.))
                .text_center()
                .text_size(px(TYPE_CONTROL_SIZE))
                .line_height(px(TYPE_CONTROL_LINE))
                .text_color(rgb(colors().muted))
                .child(detail),
        )
}

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let font = match path {
            "fonts/IBMPlexSans-Regular.otf" => {
                Some(include_bytes!("../../../../assets/fonts/IBMPlexSans-Regular.otf").as_slice())
            }
            "fonts/IBMPlexSans-Medium.otf" => {
                Some(include_bytes!("../../../../assets/fonts/IBMPlexSans-Medium.otf").as_slice())
            }
            "fonts/IBMPlexSans-SemiBold.otf" => {
                Some(include_bytes!("../../../../assets/fonts/IBMPlexSans-SemiBold.otf").as_slice())
            }
            _ => None,
        };
        if let Some(bytes) = font {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        let svg = match path {
            "plait-mark.svg" => {
                include_str!("../../../../assets/branding/marks/plait-mark-gradient.svg")
            }
            "plait-wordmark.svg" => {
                include_str!("../../../../assets/branding/wordmarks/plait-wordmark-light.svg")
            }
            // Harness makers' marks, only to name the harness; see assets/harnesses/README.md.
            "harness-claude" => include_str!("../../../../assets/harnesses/claude.svg"),
            "harness-openai" => include_str!("../../../../assets/harnesses/openai.svg"),
            "harness-pi" => include_str!("../../../../assets/harnesses/pi.svg"),
            "more" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><circle cx="3" cy="8" r="1"/><circle cx="8" cy="8" r="1"/><circle cx="13" cy="8" r="1"/></svg>"#
            }
            // An original mark for an issue: a ring around a point.
            "issue" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><circle cx="8" cy="8" r="5.5" fill="none" stroke="black" stroke-width="1.4"/><circle cx="8" cy="8" r="1.6"/></svg>"#
            }
            "open-external" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M6.5 4H4v8h8V9.5m-5.5 0 6-6M9 3.5h3.5V7" fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
            }
            "close" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path d="m4 4 8 8m0-8-8 8" fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round"/></svg>"#
            }
            "down" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="m4 6 4 4 4-4" fill="none" stroke="black" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
            }
            "up" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="m4 10 4-4 4 4" fill="none" stroke="black" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
            }
            "right" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="m6 4 4 4-4 4" fill="none" stroke="black" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
            }
            // Original stroke glyphs for controls; category glyphs come from the brand kit.
            "search" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.5"><circle cx="6.75" cy="6.75" r="4.25"/><path d="m10 10 3.5 3.5" stroke-linecap="round"/></g></svg>"#
            }
            "plus" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 3.5v9M3.5 8h9" fill="none" stroke="black" stroke-width="1.5" stroke-linecap="round"/></svg>"#
            }
            "sliders" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round"><path d="M2.5 5h5.2M11.3 5h2.2M2.5 11h2.2M8.3 11h5.2"/><circle cx="9.5" cy="5" r="1.8"/><circle cx="6.5" cy="11" r="1.8"/></g></svg>"#
            }
            "panel" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.4" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="10" rx="2"/><path d="M6.5 3v10"/></g></svg>"#
            }
            "panel-right" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.4" stroke-linejoin="round"><rect x="2.5" y="3" width="11" height="10" rx="2"/><path d="M9.5 3v10"/></g></svg>"#
            }
            // Outline and filled forms: the shape says whether a favorite is set.
            "star" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 2.8 9.41 6.46 13.33 6.67 10.28 9.14 11.29 12.93 8 10.8 4.71 12.93 5.72 9.14 2.67 6.67 6.59 6.46Z" fill="none" stroke="black" stroke-width="1.3" stroke-linejoin="round"/></svg>"#
            }
            "star-filled" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 2.8 9.41 6.46 13.33 6.67 10.28 9.14 11.29 12.93 8 10.8 4.71 12.93 5.72 9.14 2.67 6.67 6.59 6.46Z" fill="black" stroke="black" stroke-width="1.3" stroke-linejoin="round"/></svg>"#
            }
            "send" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path d="M8 12.5v-9M4.5 7 8 3.5 11.5 7" fill="none" stroke="black" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"#
            }
            "stop" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="4.5" y="4.5" width="7" height="7" rx="1.5" fill="black"/></svg>"#
            }
            "refresh" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M13 8a5 5 0 1 1-1.6-3.7"/><path d="M13 2.8v3.4H9.6"/></g></svg>"#
            }
            // A bell, and the same bell struck through, for alerts on and off.
            "bell" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 10.8V7.4a3.5 3.5 0 0 1 7 0v3.4l1 1.2h-9Z"/><path d="M6.8 13.6a1.3 1.3 0 0 0 2.4 0"/></g></svg>"#
            }
            "bell-off" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 10.8V7.4a3.5 3.5 0 0 1 7 0v3.4l1 1.2h-9Z"/><path d="M6.8 13.6a1.3 1.3 0 0 0 2.4 0"/><path d="m2.5 2.5 11 11"/></g></svg>"#
            }
            "eye" => {
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><g fill="none" stroke="black" stroke-width="1.4" stroke-linejoin="round"><path d="M1.8 8S4.1 4 8 4s6.2 4 6.2 4-2.3 4-6.2 4S1.8 8 1.8 8Z"/><circle cx="8" cy="8" r="1.8"/></g></svg>"#
            }
            "folder" => include_str!("../../../../assets/branding/icons/plait-projects.svg"),
            "machine" => include_str!("../../../../assets/branding/icons/plait-machines.svg"),
            "thread" => include_str!("../../../../assets/branding/icons/plait-threads.svg"),
            "branch" => include_str!("../../../../assets/branding/icons/plait-git.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(svg.as_bytes())))
    }
    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        if path == "fonts" {
            return Ok(BUNDLED_UI_FONT_PATHS
                .into_iter()
                .map(SharedString::new_static)
                .collect());
        }
        Ok(vec![])
    }
}

/// `~/…` for a path under the home directory, so paths stay short. The home
/// directory is read and resolved once; this runs during render.
pub fn home_relative(path: &std::path::Path) -> String {
    static HOME: std::sync::OnceLock<Vec<std::path::PathBuf>> = std::sync::OnceLock::new();
    let homes = HOME.get_or_init(|| {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return Vec::new();
        };
        // Checkouts are canonical, and HOME may not be (`/tmp` is `/private/tmp`).
        let resolved = home
            .canonicalize()
            .ok()
            .filter(|resolved| *resolved != home);
        std::iter::once(home).chain(resolved).collect()
    });
    homes
        .iter()
        .find_map(|home| path.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AssetSource, SvgRenderer};
    use std::sync::Arc;

    #[core::prelude::v1::test]
    fn type_scale_constants_match_design() {
        assert_eq!(TYPE_PAGE_TITLE_SIZE, 20.);
        assert_eq!(TYPE_PAGE_TITLE_LINE, 26.);
        assert_eq!(TYPE_PAGE_TITLE_WEIGHT, FontWeight::SEMIBOLD);
        assert_eq!(TYPE_BODY_SIZE, 14.5);
        assert_eq!(TYPE_BODY_LINE, 23.);
        assert_eq!(TYPE_DRAFT_LINE, 21.);
        assert_eq!(TYPE_HEADER_SIZE, 14.);
        assert_eq!(TYPE_HEADER_LINE, 20.);
        assert_eq!(TYPE_HEADER_WEIGHT, FontWeight::SEMIBOLD);
        assert_eq!(TYPE_ROW_SIZE, 13.5);
        assert_eq!(TYPE_ROW_LINE, 19.);
        assert_eq!(TYPE_CARD_TITLE_WEIGHT, FontWeight::SEMIBOLD);
        assert_eq!(TYPE_CONTROL_SIZE, 12.);
        assert_eq!(TYPE_CONTROL_LINE, 17.);
        assert_eq!(TYPE_CONTROL_WEIGHT, FontWeight::MEDIUM);
        assert_eq!(TYPE_META_SIZE, 11.);
        assert_eq!(TYPE_META_LINE, 15.);
        assert_eq!(TYPE_META_WEIGHT, FontWeight::MEDIUM);
        assert_eq!(TYPE_KEY_HINT_SIZE, 10.5);
        assert_eq!(TYPE_REGULAR, FontWeight::NORMAL);
        assert_eq!(TYPE_MEDIUM, FontWeight::MEDIUM);
        assert_eq!(TYPE_SEMIBOLD, FontWeight::SEMIBOLD);
    }

    #[core::prelude::v1::test]
    fn tabular_nums_enables_tnum_feature() {
        assert_eq!(tabular_nums().tag_value_list(), &[("tnum".to_owned(), 1)]);
        #[cfg(target_os = "macos")]
        {
            let text_system = TextSystem::new(Arc::new(gpui_apple::AppleTextSystem::new()));
            let id = text_system.resolve_font(&Font {
                features: tabular_nums(),
                ..font(SharedString::new_static(SYSTEM_UI_FONT))
            });
            let narrow = text_system.layout_width(id, px(TYPE_META_SIZE), '1');
            let wide = text_system.layout_width(id, px(TYPE_META_SIZE), '8');
            assert!(
                (narrow - wide).abs() < px(0.05),
                "tabular figures differed: 1={narrow:?}, 8={wide:?}"
            );
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[core::prelude::v1::test]
    fn bundled_ui_fonts_load_all_three_weights() {
        #[cfg(target_os = "macos")]
        let platform: Arc<dyn PlatformTextSystem> = Arc::new(gpui_apple::AppleTextSystem::new());
        #[cfg(target_os = "linux")]
        let platform: Arc<dyn PlatformTextSystem> =
            Arc::new(gpui_wgpu::CosmicTextSystem::new("DejaVu Sans"));

        let text_system = TextSystem::new(platform);
        assert_eq!(Assets.list("fonts").unwrap().len(), 3);
        assert!(load_bundled_fonts(&text_system));
        assert!(bundled_ui_font_loaded());

        let ids: Vec<FontId> = BUNDLED_UI_WEIGHTS
            .into_iter()
            .map(|weight| {
                let id = text_system.resolve_font(&Font {
                    weight,
                    ..font(SharedString::new_static(PLEX_UI_FONT))
                });
                let found = text_system
                    .get_font_for_id(id)
                    .expect("bundled weight resolved directly");
                assert!(found.family.eq_ignore_ascii_case(PLEX_UI_FONT));
                id
            })
            .collect();
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
        assert_ne!(ids[0], ids[2]);

        set_ui_font(plait_client::preferences::InterfaceFont::Plex);
        assert_eq!(ui_font().as_ref(), PLEX_UI_FONT);
        set_ui_font(plait_client::preferences::InterfaceFont::System);
        assert_eq!(ui_font().as_ref(), SYSTEM_UI_FONT);
        set_ui_font(plait_client::preferences::InterfaceFont::Plex);
    }

    #[core::prelude::v1::test]
    fn embedded_icons_render_and_brand_keeps_color() {
        let renderer = SvgRenderer::new(Arc::new(Assets));
        for name in [
            "plait-mark.svg",
            "plait-wordmark.svg",
            "folder",
            "open-external",
            "machine",
            "thread",
            "branch",
            "down",
            "right",
            "plus",
            "sliders",
            "panel",
            "panel-right",
            "star",
            "star-filled",
            "send",
            "stop",
            "refresh",
            "bell",
            "bell-off",
            "up",
            "eye",
            "harness-claude",
            "harness-openai",
            "harness-pi",
        ] {
            let bytes = Assets.load(name).unwrap().expect("embedded icon");
            let image = renderer.render_single_frame(&bytes, 1.0).unwrap();
            let (pixels, remainder) = image.as_bytes(0).unwrap().as_chunks::<4>();
            assert!(remainder.is_empty());
            assert!(pixels.iter().any(|pixel| pixel[3] > 0), "{name}");
            if name == "plait-mark.svg" {
                assert!(pixels.iter().any(|pixel| pixel[0] != pixel[1]));
            }
        }
        assert!(Assets.load("missing.svg").unwrap().is_none());
    }
}
