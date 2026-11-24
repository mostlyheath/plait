//! Sidebar widths: their defaults and limits, and the edge handle that drags them.
//!
//! A drag changes only the width on screen. The width is saved to preferences
//! when the button is released, so a drag writes the file once, not per move.

use super::{Plait, jump::PreferenceGroup, theme::*};
use gpui::{prelude::*, *};

/// A sidebar whose width can be dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Panel {
    /// Projects and threads, on the left.
    Sidebar,
    /// The Git inspector, on the right.
    Review,
}

/// What a resize drag carries.
struct Resize(Panel);

/// The conversation keeps at least this much room when a panel is dragged wider.
const MIN_MAIN: f32 = 420.;
/// Wide enough to grab without covering the panel's content.
pub(super) const HANDLE: f32 = 6.;

impl Panel {
    fn limits(self) -> (f32, f32) {
        match self {
            Self::Sidebar => (200., 480.),
            Self::Review => (360., f32::MAX),
        }
    }

    fn default_width(self, viewport: f32) -> f32 {
        match self {
            Self::Sidebar if viewport < 760. => 236.,
            Self::Sidebar => 288.,
            Self::Review => viewport / 2.,
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Sidebar => Self::Review,
            Self::Review => Self::Sidebar,
        }
    }
}

/// Fit a width to its panel's limits and to `room`, the window width less the
/// other panel. The minimum wins when the window is too small for both.
fn fit(panel: Panel, width: f32, room: f32) -> f32 {
    let (min, max) = panel.limits();
    width.min(max).min(room - MIN_MAIN).max(min)
}

/// Layout state that is not saved until a drag ends.
#[derive(Default)]
pub(super) struct Panels {
    /// Window width at the last render; widths are fitted to it.
    pub viewport: f32,
    pub focused: Option<plait_client::panes::PaneKind>,
    /// The width being dragged, saved when the button is released.
    live: Option<(Panel, f32)>,
}

impl Plait {
    fn saved_width(&self, panel: Panel) -> Option<f32> {
        let value = &self.preferences.value;
        match panel {
            Panel::Sidebar => value.sidebar_width,
            Panel::Review => value.panel_width,
        }
        .map(f32::from)
    }

    /// The width a panel wants before it is fitted beside the other one.
    fn wanted_width(&self, panel: Panel) -> f32 {
        match self.panels.live {
            Some((live, width)) if live == panel => width,
            _ => self
                .saved_width(panel)
                .unwrap_or_else(|| panel.default_width(self.panels.viewport)),
        }
    }

    /// Room left for `panel`: the window less the other panel when it is shown.
    fn room_for(&self, panel: Panel) -> f32 {
        let other = panel.other();
        let shown = match other {
            Panel::Sidebar => self.visible_sidebar(),
            Panel::Review => false,
        };
        let taken = if shown {
            let (min, max) = other.limits();
            self.wanted_width(other).clamp(min, max)
        } else {
            0.
        };
        self.panels.viewport - taken
    }

    /// The width to draw a panel at.
    pub(super) fn panel_width(&self, panel: Panel) -> f32 {
        if let Panel::Review = panel
            && let Some(model) = self.tabs.model.panel()
        {
            let preferred = self
                .panels
                .live
                .filter(|(edge, _)| *edge == panel)
                .map(|(_, w)| w)
                .or_else(|| self.saved_width(panel));
            return model.width(self.sheet_width(), preferred);
        }
        fit(panel, self.wanted_width(panel), self.room_for(panel))
    }

    /// The grab area on a panel's inner edge. The panel must be `relative`, and
    /// the handle its last child so it sits above the panel's content.
    pub(super) fn resize_handle(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let (id, label) = match panel {
            Panel::Sidebar => ("resize-sidebar", "Resize sidebar"),
            Panel::Review => ("resize-panel", "Resize panel"),
        };
        div()
            .id(id)
            .role(Role::Splitter)
            .aria_label(label)
            .audited()
            .aria_description("Drag to resize. Double-click to restore the default width.")
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(HANDLE))
            .map(|el| match panel {
                Panel::Sidebar => el.right_0(),
                Panel::Review => el.left_0(),
            })
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|s| s.bg(rgba(colors().hover)))
            .on_drag(Resize(panel), |_, _, _, cx| cx.new(|_| Empty))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.reset_panel(panel, cx);
                }
            }))
    }

    /// Follow resize drags anywhere in the window, and save when one ends.
    pub(super) fn on_panel_resize(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_drag_move(cx.listener(|this, event: &DragMoveEvent<Resize>, _, cx| {
            let Resize(panel) = *event.drag(cx);
            let x = f32::from(event.event.position.x);
            let width = match panel {
                Panel::Sidebar => x,
                Panel::Review => this.panels.viewport - x,
            };
            this.panels.live = Some((panel, fit(panel, width, this.room_for(panel))));
            cx.notify();
        }))
        // Any release ends the drag. One released outside the window is saved
        // by the next click; the dragged width stays on screen until then.
        .capture_any_mouse_up(cx.listener(|this, _, _, cx| this.save_panel_width(cx)))
    }

    fn save_panel_width(&mut self, cx: &mut Context<Self>) {
        let Some((panel, width)) = self.panels.live.take() else {
            return;
        };
        let mut value = self.preferences.value.clone();
        let width = Some(width.round() as u16);
        match panel {
            Panel::Sidebar => value.sidebar_width = width,
            Panel::Review => value.panel_width = width,
        }
        self.set_preferences(value, PreferenceGroup::View, cx);
    }

    fn reset_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panels.live = None;
        let mut value = self.preferences.value.clone();
        match panel {
            Panel::Sidebar => value.sidebar_width = None,
            Panel::Review => value.panel_width = None,
        }
        self.set_preferences(value, PreferenceGroup::View, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{Panel, fit};

    #[test]
    fn sidebar_default_changes_below_760_points() {
        assert_eq!(Panel::Sidebar.default_width(759.), 236.);
        assert_eq!(Panel::Sidebar.default_width(760.), 288.);
    }

    #[test]
    fn widths_stay_within_limits_and_leave_the_conversation_room() {
        assert_eq!(fit(Panel::Sidebar, 320., 1400.), 320.);
        assert_eq!(fit(Panel::Sidebar, 40., 1400.), 200.);
        assert_eq!(fit(Panel::Sidebar, 900., 1400.), 480.);
        // 1000 points of room leave 580 for the panel, under its own limit.
        assert_eq!(fit(Panel::Review, 560., 1000.), 560.);
        assert_eq!(fit(Panel::Review, 560., 800.), 380.);
        // Too small a window for both: the panel keeps its minimum.
        assert_eq!(fit(Panel::Review, 300., 500.), 360.);
    }
}
