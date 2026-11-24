//! Thread tabs and the tab list. The core owns every transition and presentation fact.
use super::{Plait, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::tabs::{
    CheckoutKey, HostKey, TabError, TabId, TabTarget, WindowId, WindowTabs,
    row::{self as model, RowContext, RowWidths, TabLayout, TabMetadata, TabState},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) const HEADER_HEIGHT: f32 = 46.;
const LIGHTS_INSET: f32 = 84.;
const FULLSCREEN_INSET: f32 = 8.;
const NEW_TAB_WIDTH: f32 = 32.;
const DRAG_MIN: f32 = 32.;

pub(super) fn lights_inset(fullscreen: bool) -> f32 {
    if fullscreen {
        FULLSCREEN_INSET
    } else {
        LIGHTS_INSET
    }
}

static WINDOW_IDS: AtomicU64 = AtomicU64::new(1);
pub(super) fn fresh_window_id() -> WindowId {
    WindowId(WINDOW_IDS.fetch_add(1, Ordering::Relaxed))
}

actions!(
    plait,
    [
        NewTab,
        CloseTab,
        ReopenTab,
        NextTab,
        PreviousTab,
        MoveTabLeft,
        MoveTabRight,
        ListTabs,
        TabJump1,
        TabJump2,
        TabJump3,
        TabJump4,
        TabJump5,
        TabJump6,
        TabJump7,
        TabJump8,
        TabJump9
    ]
);

pub(super) struct Tabs {
    pub model: WindowTabs,
    pub lifecycle: super::tab_lifecycle::Lifecycle,
    pub identity: super::tab_identity::Identity,
    pub persistence: super::tab_persistence::Persistence,
    pub(super) strip: Option<Entity<TabStrip>>,
    strip_theme: u32,
    pub host: HostKey,
    pub rows: Vec<TabMetadata>,
    pub(super) draft_titles: std::collections::HashMap<String, String>,
    pub(super) draft_row: Option<(String, usize)>,
    layout: Option<TabLayout>,
    /// The most urgent state among hidden tabs, for the overflow summary's accent.
    overflow_state: Option<TabState>,
    /// Bounds of the flexible slot between the header's other controls.
    slot: Option<Bounds<Pixels>>,
    measured: Option<(f32, usize)>,
    overflow_width: f32,
    first_visible: usize,
    pub list_open: bool,
    list_cursor: Option<TabId>,
    list_scroll: ScrollHandle,
    large_list_scroll: UniformListScrollHandle,
    list_items: Vec<ListItem>,
    drag: Option<Drag>,
}
pub(super) struct TabStrip {
    owner: WeakEntity<Plait>,
}
impl Render for TabStrip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |p, cx| p.tab_strip(cx).into_any_element())
            .unwrap_or_else(|_| div().into_any_element())
    }
}

#[derive(Clone, Copy)]
enum ListItem {
    Heading(model::TabGroup),
    Tab(usize),
}

struct Drag {
    id: TabId,
    start: Pixels,
    destination: Option<usize>,
}
impl Default for Tabs {
    fn default() -> Self {
        let window = fresh_window_id().0;
        // Unverified hosts can use session tabs; this identity is never durable.
        Self {
            model: WindowTabs::new(WindowId(window)),
            lifecycle: Default::default(),
            identity: Default::default(),
            persistence: Default::default(),
            strip: None,
            strip_theme: 0,
            host: HostKey {
                id: format!("session-{window}"),
                store: PathBuf::from("/"),
            },
            rows: Vec::new(),
            draft_titles: Default::default(),
            draft_row: None,
            layout: None,
            overflow_state: None,
            slot: None,
            measured: None,
            overflow_width: 0.,
            first_visible: 0,
            list_open: false,
            list_cursor: None,
            list_scroll: ScrollHandle::new(),
            large_list_scroll: UniformListScrollHandle::new(),
            list_items: Vec::new(),
            drag: None,
        }
    }
}

fn tint(state: TabState, active: bool) -> u32 {
    match state {
        TabState::Approve | TabState::Interrupted | TabState::Held => colors().amber,
        TabState::Failed => colors().red,
        TabState::Working | TabState::Waiting => colors().blue,
        TabState::Finished => colors().mint,
        TabState::Idle if active => colors().mint,
        _ => colors().subtle,
    }
}
/// Accents follow the urgent-count order: needs me, then failed, then working.
fn urgent_hidden(hidden: impl Iterator<Item = TabState>) -> Option<TabState> {
    let rank = |state: TabState| match state {
        TabState::Approve | TabState::Interrupted => Some(0),
        TabState::Failed => Some(1),
        TabState::Working => Some(2),
        _ => None,
    };
    hidden
        .filter_map(|state| rank(state).map(|r| (r, state)))
        .min_by_key(|(r, _)| *r)
        .map(|(_, state)| state)
}
fn close_name(title: &str) -> String {
    format!("Close {title}")
}
fn tooltip(value: String, cx: &mut App) -> AnyView {
    cx.new(|_| TextTooltip(value)).into()
}
/// Measures at the overflow button's drawn size.
fn measure(text: &str, window: &Window) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: Font {
            weight: TYPE_CONTROL_WEIGHT,
            features: tabular_nums(),
            ..font(ui_font())
        },
        color: rgb(colors().muted).into(),
        ..Default::default()
    };
    window
        .text_system()
        .shape_line(text.to_owned().into(), px(TYPE_CONTROL_SIZE), &[run], None)
        .width
        .into()
}

impl Plait {
    pub(super) fn invalidate_tab_measurement(&mut self) {
        self.tabs.measured = None;
    }

    pub(super) fn dismiss_tab_controls(&mut self) {
        self.tabs.list_open = false;
        self.tabs.drag = None;
        // Cached controls must rebuild their focus actions when the modal changes.
        self.tabs.measured = None;
        self.window_drag = false;
    }

    pub(super) fn covered_tab_control<E: StatefulInteractiveElement + FluentBuilder>(
        &self,
        control: E,
    ) -> E {
        control.when(self.settings_open, |el| {
            el.tab_stop(false)
                // GPUI otherwise handles assistive Focus without the modal's hit test.
                .on_a11y_action(AccessibleAction::Focus, |_, _, _| {})
        })
    }

    pub(super) fn refresh_tab_rows(&mut self) {
        let mut rows = model::metadata(
            &self.tabs.model,
            &RowContext {
                host: &self.tabs.host,
                threads: self.host.connection.threads(),
                navigation: self.lists(),
                machine: &self.active_project().machines[0].name,
            },
        );
        self.name_draft_rows(&mut rows);
        if rows != self.tabs.rows {
            self.tabs.rows = rows;
            self.tabs.measured = None;
        }
    }

    pub(super) fn on_tab_actions<E: InteractiveElement>(
        &self,
        root: E,
        cx: &mut Context<Self>,
    ) -> E {
        macro_rules! jumps { ($root:expr, $( $action:ty => $n:expr ),*) => {
            $root$(.on_action(cx.listener(|this, _: &$action, window, cx| {
                this.navigate_tab(TabNavigation::Jump($n), window, cx);
            })))*
        }; }
        let root = root
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                if !this.settings_open {
                    this.open_draft(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.settings_open {
                    return;
                }
                if let Some(id) = this.tabs.model.active() {
                    this.close_tab(id, window, cx);
                } else {
                    this.close_desktop(false, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ReopenTab, window, cx| {
                this.navigate_tab(TabNavigation::Reopen, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                this.navigate_tab(TabNavigation::Next, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| {
                this.navigate_tab(TabNavigation::Previous, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveTabLeft, _, cx| this.move_active_tab(false, cx)))
            .on_action(cx.listener(|this, _: &MoveTabRight, _, cx| this.move_active_tab(true, cx)))
            .on_action(cx.listener(|this, _: &ListTabs, window, cx| {
                this.toggle_tab_list(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.settings_open || !this.tabs.list_open {
                    return;
                }
                let order = this.tab_list_order();
                let current = order
                    .iter()
                    .position(|(id, _)| Some(*id) == this.tabs.list_cursor)
                    .unwrap_or(0);
                let next = match event.keystroke.key.as_str() {
                    "escape" => {
                        this.tabs.list_open = false;
                        None
                    }
                    "up" => Some(current.saturating_sub(1)),
                    "down" => Some((current + 1).min(order.len().saturating_sub(1))),
                    "home" => Some(0),
                    "end" => Some(order.len().saturating_sub(1)),
                    "enter" | "space" => {
                        if let Some((id, _)) = order.get(current) {
                            this.activate_tab(*id, window, cx);
                        }
                        None
                    }
                    _ => return,
                };
                if let Some(index) = next
                    && let Some((id, item)) = order.get(index)
                {
                    this.tabs.list_cursor = Some(*id);
                    this.tabs.list_scroll.scroll_to_item(*item);
                    this.tabs
                        .large_list_scroll
                        .scroll_to_item(*item, ScrollStrategy::Nearest);
                }
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }));
        jumps!(root, TabJump1 => 1, TabJump2 => 2, TabJump3 => 3, TabJump4 => 4,
            TabJump5 => 5, TabJump6 => 6, TabJump7 => 7, TabJump8 => 8, TabJump9 => 9)
    }

    pub(super) fn tab_error(&mut self, result: Result<(), TabError>) -> bool {
        match result {
            Ok(()) => true,
            Err(TabError::IdentityMismatch) => {
                self.notice = Some("This checkout has changed identity. Close its existing tab, then open the thread again.".into());
                false
            }
            Err(error) => {
                self.notice = Some(error.to_string());
                false
            }
        }
    }

    #[cfg(test)]
    pub(super) fn open_thread_tab(
        &mut self,
        id: String,
        checkout: PathBuf,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.save_tab_view(cx) {
            return false;
        }
        let target = TabTarget::Existing {
            id,
            checkout: self.tab_checkout(checkout),
        };
        let result = self.tabs.model.open(
            target,
            self.host
                .connection
                .connected
                .then_some(self.host.connection.epoch),
        );
        self.tab_error(result.map(|_| ()))
    }

    /// A checkout view opened before any thread belongs to its unsent draft tab.
    pub(super) fn ensure_view_tab(&mut self) -> bool {
        if self.tabs.model.active().is_some() {
            return true;
        }
        let target = TabTarget::Draft {
            id: plait_client::host_state::request_id(),
            checkout: self.tab_checkout(self.host.creation.checkout.clone()),
        };
        if let TabTarget::Draft { id, .. } = &target {
            self.draft.id = Some(id.clone());
        }
        let result = self.tabs.model.open(
            target,
            self.host
                .connection
                .connected
                .then_some(self.host.connection.epoch),
        );
        if !self.tab_error(result.map(|_| ())) {
            return false;
        }
        self.draft.open = true;
        self.tabs.persistence.edited = true;
        self.refresh_tab_rows();
        true
    }

    pub(super) fn tab_checkout(&self, root: PathBuf) -> CheckoutKey {
        CheckoutKey {
            host: self.tabs.host.clone(),
            directory: self.tabs.identity.checkouts.get(&root).copied(),
            root,
        }
    }

    #[cfg(test)]
    fn save_tab_view(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.tabs.model.active() else {
            return true;
        };
        if self.worktrees_visible() {
            return true;
        }
        self.capture_session(cx);
        let bookmark = self.durable_tab_bookmark(None, cx);
        match plait_client::tabs::bookmark::BookmarkRecord::capture(&bookmark) {
            Ok(bookmark) => {
                let result = self.tabs.model.save_bookmark(id, bookmark);
                self.tab_error(result)
            }
            Err(_) => {
                self.notice =
                    Some("The tab could not save its view. Try again before closing it.".into());
                false
            }
        }
    }

    fn activate_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        self.request_tab(super::tab_lifecycle::Intent::Activate(id), window, cx);
    }

    pub(super) fn present_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.list_open = false;
        let target = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .map(|tab| {
                (
                    tab.session().target().clone(),
                    tab.session().bookmark().view(),
                )
            });
        if target
            .as_ref()
            .is_some_and(|(target, _)| target.checkout().host != self.tabs.host)
        {
            self.notice = Some("This tab belongs to a different host store. Open the thread from the current sidebar.".into());
            self.refresh_tab_rows();
            cx.notify();
            return;
        }
        match target {
            Some((TabTarget::Issues { .. }, _)) => {
                self.host.select(None);
                self.draft.open = false;
                window.focus(&self.focus, cx);
                self.restore_issues_bookmark();
                cx.notify();
            }
            Some((TabTarget::Worktrees { .. }, _)) => {
                self.host.select(None);
                self.draft.open = false;
                window.focus(&self.focus, cx);
                self.restore_worktrees_bookmark();
                self.refresh_worktrees_view(false, cx);
            }
            Some((TabTarget::Existing { id, .. }, _)) => {
                self.select_host_thread_content(id, window, cx);
                self.present_workspace(super::Workspace::Conversation, window, cx);
            }
            Some((TabTarget::Draft { id, checkout }, _)) => {
                if checkout.root != self.host.creation.checkout {
                    self.change_checkout(checkout.root, window, cx);
                }
                self.restore_draft_tab(id, cx);
                self.open_draft_content(window, cx);
                self.present_workspace(super::Workspace::Conversation, window, cx);
            }
            None => {
                self.workspace.reset_for_thread_selection();
                self.host.select(None);
                self.draft.open = false;
                self.refresh_live_list(None);
                window.focus(&self.focus, cx);
            }
        }
        self.hydrate_tab(window, cx);
        self.refresh_tab_rows();
        cx.notify();
    }

    fn close_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        self.request_tab(super::tab_lifecycle::Intent::Close(id), window, cx);
    }

    fn navigate_tab(&mut self, action: TabNavigation, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        self.request_tab(super::tab_lifecycle::Intent::Navigate(action), window, cx);
    }

    fn move_active_tab(&mut self, right: bool, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        let Some(id) = self.tabs.model.active() else {
            return;
        };
        let Some(index) = self.tabs.model.tabs().iter().position(|t| t.id() == id) else {
            return;
        };
        let to = if right {
            index + 1
        } else {
            index.saturating_sub(1)
        };
        let result = self.tabs.model.move_tab(id, to);
        self.tab_error(result);
        self.refresh_tab_rows();
        self.queue_tab_save(cx);
        cx.notify();
    }

    pub(super) fn sidebar_toggle(&self, cx: &mut Context<Self>) -> Control {
        let keys = self.shortcut_label("view.toggle_sidebar");
        let tip = format!("Toggle sidebar  {keys}").trim_end().to_owned();
        icon_button("toggle-sidebar", "panel", "Toggle sidebar")
            .map(|el| self.covered_tab_control(el))
            .aria_keyshortcuts(self.shortcut_aria("view.toggle_sidebar"))
            .tooltip(move |_, cx| tooltip(tip.clone(), cx))
            .on_activate(cx, |this, window, cx| this.toggle_sidebar(window, cx))
    }

    pub(super) fn measure_tabs(&mut self, window: &Window) {
        let width = self
            .tabs
            .slot
            .map_or(self.sheet_width(), |bounds| bounds.size.width.into());
        let limit = if width > 760. { 2 } else { 1 };
        let key = (width, limit);
        if self.tabs.measured == Some(key) {
            return;
        }
        let available = (width - NEW_TAB_WIDTH - DRAG_MIN).max(0.);
        let widths = |overflow| RowWidths {
            available,
            minimum_tab: 128.,
            maximum_tab: 240.,
            overflow,
        };
        let initial = model::layout(&self.tabs.rows, widths(44.), self.tabs.first_visible, limit);
        let overflow = (measure(&initial.overflow_label, window) + 16.)
            .clamp(44., if limit == 2 { 164. } else { 140. })
            .min((available - 48.).max(44.));
        self.tabs.overflow_width = overflow;
        let layout = model::layout(
            &self.tabs.rows,
            widths(overflow),
            self.tabs.first_visible,
            limit,
        );
        self.tabs.first_visible = layout.first_visible;
        self.tabs.overflow_state = urgent_hidden(
            self.tabs
                .rows
                .iter()
                .filter(|row| layout.hidden.contains(&row.id))
                .map(|row| row.state),
        );
        let indices: std::collections::HashMap<_, _> = self
            .tabs
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id, i))
            .collect();
        self.tabs.list_items.clear();
        for group in &layout.groups {
            self.tabs.list_items.push(ListItem::Heading(group.group));
            for id in &group.tabs {
                self.tabs.list_items.push(ListItem::Tab(indices[id]));
            }
        }
        self.tabs.layout = Some(layout);
        self.tabs.measured = Some(key);
    }

    pub(super) fn tab_row(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let last = self.tabs.measured;
        self.measure_tabs(window);
        let redraw = self.tabs.strip_theme != colors().bg
            || last != self.tabs.measured
            || self.tabs.drag.is_some();
        self.tabs.strip_theme = colors().bg;
        let layout = self.tabs.layout.as_ref().expect("measured tab row");
        let strip = self
            .tabs
            .strip
            .get_or_insert_with(|| {
                let owner = cx.weak_entity();
                cx.new(|_| TabStrip { owner })
            })
            .clone();
        if redraw {
            strip.update(cx, |_, cx| cx.notify());
        }
        // The strip contributes accessibility nodes on every frame.
        let strip = div()
            .h_full()
            .w(px(layout.visible.len() as f32 * layout.tab_width))
            .min_w_0()
            .overflow_hidden()
            .child(strip);
        let owner = cx.weak_entity();
        let probe = canvas(
            move |bounds, _, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if this.tabs.slot != Some(bounds) {
                        this.tabs.slot = Some(bounds);
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let bar = row()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_0()
            .overflow_hidden()
            .child(probe)
            .child(strip)
            .when(!layout.hidden.is_empty(), |bar| {
                let hover = layout.overflow_hover.clone();
                let accent = self.tabs.overflow_state.map(|state| tint(state, false));
                bar.child(
                    button("tab-overflow", layout.overflow_label.clone())
                        .font_features(tabular_nums())
                        .w(px(self.tabs.overflow_width))
                        .px_1()
                        .justify_center()
                        .when_some(accent, |el, accent| el.text_color(rgb(accent)))
                        .truncate()
                        .overflow_hidden()
                        .aria_label("Show all tabs")
                        .map(|el| self.covered_tab_control(el))
                        .tooltip(move |_, cx| tooltip(hover.clone(), cx))
                        .on_activate(cx, |this, window, cx| {
                            this.toggle_tab_list(window, cx);
                        }),
                )
            })
            .child(
                icon_button("new-tab", "plus", "New thread tab")
                    .ml_1()
                    .map(|el| self.covered_tab_control(el))
                    .tooltip(|_, cx| tooltip("New thread tab (⌘T)".into(), cx))
                    .on_activate(cx, |this, window, cx| this.open_draft(window, cx)),
            )
            .child(div().flex_1().h_full());
        bar.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            let Some(bounds) = this.tabs.slot else { return };
            let left = f32::from(bounds.origin.x);
            let Some(drag) = this.tabs.drag.as_mut() else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                this.tabs.drag = None;
                return;
            }
            if (event.position.x - drag.start).abs() < px(4.) && drag.destination.is_none() {
                return;
            }
            this.window_drag = false;
            if let Some(layout) = &this.tabs.layout {
                let index =
                    ((f32::from(event.position.x) - left) / layout.tab_width).max(0.) as usize;
                drag.destination = Some(
                    (layout.first_visible + index).min(this.tabs.rows.len().saturating_sub(1)),
                );
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                if let Some(Drag {
                    id,
                    destination: Some(to),
                    ..
                }) = this.tabs.drag.take()
                {
                    this.tabs.measured = None;
                    let result = this.tabs.model.move_tab(id, to);
                    this.tab_error(result);
                    this.refresh_tab_rows();
                    this.queue_tab_save(cx);
                    cx.notify();
                }
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.tabs.drag = None;
                this.tabs.measured = None;
                cx.notify();
            }),
        )
    }

    fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = self.tabs.layout.as_ref().expect("measured strip");
        let mut strip = row()
            .id("tab-row")
            .role(Role::TabList)
            .aria_label("Open tabs")
            .audited()
            .gap_0()
            .min_w_0()
            .h_full()
            .items_center();
        for (offset, meta) in self.tabs.rows[layout.first_visible..]
            .iter()
            .take(layout.visible.len())
            .enumerate()
        {
            strip = strip.child(self.tab_control(
                meta,
                layout.first_visible + offset,
                layout.tab_width,
                cx,
            ));
        }
        strip
    }

    fn tab_control(
        &self,
        meta: &TabMetadata,
        index: usize,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = meta.id;
        let group = format!("thread-tab-{}", id.get());
        let hover = meta.hover.clone();
        let close_hover = meta.close_hover;
        let close = meta.active;
        let attention = matches!(
            meta.state,
            TabState::Approve | TabState::Interrupted | TabState::Failed
        );
        let at = Some(index);
        let insertion = self
            .tabs
            .drag
            .as_ref()
            .is_some_and(|drag| drag.destination == at);
        row()
            .id(SharedString::from(group.clone()))
            .group(SharedString::from(group.clone()))
            .role(Role::Tab)
            .aria_label(format!("{} — {}", meta.title, meta.state.word(true)))
            .aria_selected(meta.active)
            .audited()
            .focusable()
            .tab_stop(true)
            .map(|el| self.covered_tab_control(el))
            .cursor_pointer()
            .w(px((width - 3.).max(0.)))
            .mr(px(3.))
            .h(px(30.))
            .relative()
            .flex_shrink_0()
            .min_w_0()
            .gap(px(6.))
            .pl(px(8.))
            .pr(px(6.))
            .rounded(px(CONTROL_CORNER))
            .border_1()
            .border_color(if meta.active {
                rgb(colors().edge).into()
            } else {
                transparent_black()
            })
            .when(meta.active, |el| el.bg(rgb(colors().selected)))
            .when(!meta.active, |el| el.active(|s| pressed(s, Tone::Ghost)))
            .hover(|s| s.bg(rgba(colors().hover)))
            .focus_visible(|s| s.border_color(rgb(colors().blue)))
            .text_size(px(TYPE_CONTROL_SIZE))
            .line_height(px(TYPE_CONTROL_LINE))
            .font_weight(if meta.active || meta.unread || attention {
                TYPE_CONTROL_WEIGHT
            } else {
                TYPE_REGULAR
            })
            .tooltip(move |_, cx| tooltip(hover.clone(), cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, _| {
                    if this.settings_open {
                        return;
                    }
                    window.prevent_default();
                    this.window_drag = false;
                    this.tabs.drag = Some(Drag {
                        id,
                        start: event.position.x,
                        destination: None,
                    });
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.close_tab(id, window, cx);
                }),
            )
            .on_activate(cx, move |this, window, cx| {
                this.activate_tab(id, window, cx)
            })
            .when(insertion, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(rgb(colors().blue)),
                )
            })
            .child(
                div()
                    .w(px(12.))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .child(if meta.worktrees || meta.issues {
                        icon(if meta.issues { "issue" } else { "branch" })
                            .text_color(rgb(colors().muted))
                            .into_any_element()
                    } else {
                        dot(tint(meta.state, meta.active || meta.word == "Ready"))
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(meta.active, |el| el.text_color(rgb(colors().strong)))
                    .child(meta.title.replace(['\r', '\n'], " ")),
            )
            .child(self.tab_close_control(id, &meta.title, close_hover, close, group, cx))
    }

    fn tab_close_control(
        &self,
        id: TabId,
        title: &str,
        hover: &'static str,
        visible: bool,
        group: String,
        cx: &mut Context<Self>,
    ) -> Control {
        // Twenty identical "Close tab" names would not say which tab closes.
        icon_button(("close-tab", id.get()), "close", close_name(title))
            .size(px(18.))
            .map(|el| self.covered_tab_control(el))
            .when(!visible, |el| {
                el.w_0()
                    .opacity(0.)
                    .group_hover(SharedString::from(group), |s| s.w(px(18.)).opacity(1.))
                    .focus_visible(|s| s.w(px(18.)).opacity(1.).border_color(rgb(colors().blue)))
            })
            .tooltip(move |_, cx| tooltip(hover.into(), cx))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_activate(cx, move |this, window, cx| {
                cx.stop_propagation();
                this.close_tab(id, window, cx);
            })
    }

    fn toggle_tab_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            return;
        }
        self.tabs.list_open = !self.tabs.list_open;
        self.tabs.list_cursor = self.tabs.model.active();
        window.focus(&self.focus, cx);
        cx.notify();
    }
    fn tab_list_order(&self) -> Vec<(TabId, usize)> {
        let mut order = Vec::new();
        let mut item = 0;
        if let Some(layout) = &self.tabs.layout {
            for group in &layout.groups {
                item += 1;
                for &id in &group.tabs {
                    order.push((id, item));
                    item += 1;
                }
            }
        }
        order
    }

    pub(super) fn tab_list(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.settings_open || !self.tabs.list_open {
            return None;
        }
        let layout = self.tabs.layout.as_ref()?;
        let mut list = column()
            .id("all-tabs-list")
            .role(Role::Menu)
            .aria_label("All open tabs")
            .audited()
            .absolute()
            .top(px(
                HEADER_HEIGHT + if self.visible_sidebar() { 9. } else { 0. }
            ))
            .right(px(48.))
            .w(px(320.))
            .max_h(px(336.))
            .overflow_y_scroll()
            .track_scroll(&self.tabs.list_scroll)
            .p_1()
            .bg(rgb(colors().raised))
            .border_1()
            .border_color(rgb(colors().edge))
            .rounded(px(SURFACE_CORNER))
            .shadow(floating_shadow())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.tabs.list_open = false;
                cx.notify();
            }));
        if self.tabs.rows.len() > 32 {
            let owner = cx.entity().downgrade();
            return Some(
                list.child(
                    uniform_list(
                        "virtual-tabs",
                        self.tabs.list_items.len(),
                        move |range, _, cx| {
                            owner
                                .update(cx, |this, cx| {
                                    range
                                        .map(|index| match this.tabs.list_items[index] {
                                            ListItem::Heading(group) => div()
                                                .h(px(28.))
                                                .px_2()
                                                .pt_2()
                                                .text_size(px(11.))
                                                .text_color(rgb(colors().subtle))
                                                .child(group.title())
                                                .into_any_element(),
                                            ListItem::Tab(index) => this
                                                .tab_list_entry(&this.tabs.rows[index], cx)
                                                .into_any_element(),
                                        })
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        },
                    )
                    .track_scroll(&self.tabs.large_list_scroll)
                    .h(px(328.))
                    .w_full(),
                )
                .into_any_element(),
            );
        }
        for group in &layout.groups {
            list = list.child(
                div()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_size(px(11.))
                    .text_color(rgb(colors().subtle))
                    .child(group.group.title()),
            );
            for id in &group.tabs {
                let Some(meta) = self.tabs.rows.iter().find(|t| t.id == *id) else {
                    continue;
                };
                list = list.child(self.tab_list_entry(meta, cx));
            }
        }
        Some(list.into_any_element())
    }
    fn tab_list_entry(&self, meta: &TabMetadata, cx: &mut Context<Self>) -> impl IntoElement {
        let id = meta.id;
        let hover = meta.hover.clone();
        let group = format!("list-tab-{}", id.get());

        nav_row(("tab-list", id.get()), meta.title.clone(), meta.active)
            .role(Role::MenuItem)
            .when(self.tabs.list_cursor == Some(id), |el| {
                el.bg(rgba(colors().hover))
            })
            .group(SharedString::from(group.clone()))
            .text_size(px(12.))
            .tooltip(move |_, cx| tooltip(hover.clone(), cx))
            .child(dot(tint(meta.state, meta.active || meta.word == "Ready")))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .when(meta.active, |el| el.text_color(rgb(colors().strong)))
                    .child(meta.title.clone()),
            )
            .child(
                tag("tab-list-project", meta.project.clone(), colors().subtle)
                    .max_w(px(70.))
                    .truncate(),
            )
            .child(
                tag(
                    "tab-list-state",
                    meta.state.word(true),
                    tint(meta.state, meta.active || meta.word == "Ready"),
                )
                .text_size(px(11.)),
            )
            .when_some(meta.shortcut, |el, n| {
                el.child(key_hint("tab-list-shortcut", format!("⌘{n}")))
            })
            .child(self.tab_close_control(id, &meta.title, meta.close_hover, false, group, cx))
            .on_activate(cx, move |this, window, cx| {
                this.activate_tab(id, window, cx)
            })
    }
}

#[derive(Clone, Copy)]
pub(super) enum TabNavigation {
    Next,
    Previous,
    Jump(usize),
    Reopen,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn overflow_accent_takes_the_most_urgent_hidden_state() {
        use TabState::*;
        let urgent = |states: &[TabState]| urgent_hidden(states.iter().copied());
        assert_eq!(urgent(&[Working, Failed, Idle]), Some(Failed));
        assert_eq!(urgent(&[Working, Failed, Interrupted]), Some(Interrupted));
        assert_eq!(urgent(&[Idle, Working, Finished]), Some(Working));
        assert_eq!(urgent(&[Idle, Stopped, Finished]), None);
    }

    #[core::prelude::v1::test]
    fn a_hidden_sidebar_clears_the_native_lights() {
        assert_eq!(lights_inset(false), 84.);
        assert_eq!(lights_inset(true), 8.);
    }

    #[core::prelude::v1::test]
    fn close_controls_name_their_tab() {
        assert_eq!(
            close_name("Run the parser tests"),
            "Close Run the parser tests"
        );
    }
}
