//! GPUI-free pane tree model for a thread tab's Work view.
//!
//! Manages split layout, focus, kind uniqueness, and resize constraints without
//! UI types, serialization, or I/O. The desktop crate presents `PaneLayout`.

use std::fmt;

/// Minimum window width in points required to enter or draw Work view.
pub const WORK_VIEW_MIN_WINDOW_WIDTH: f32 = 800.0;
/// Initial width in points of the Thread pane on first entry into Work view.
pub const THREAD_INITIAL_WIDTH: f32 = 420.0;
/// Minimum width in points of any pane.
pub const MIN_PANE_WIDTH: f32 = 320.0;
/// Minimum height in points of any pane.
pub const MIN_PANE_HEIGHT: f32 = 160.0;
/// Height in points of a pane header bar.
pub const PANE_HEADER_HEIGHT: f32 = 30.0;
/// Visual thickness in points of a pane divider hairline.
pub const DIVIDER_THICKNESS: f32 = 1.0;
/// Pointer drag hit-area width/height in points centered on a divider hairline.
pub const DIVIDER_DRAG_EXTENT: f32 = 6.0;
/// Maximum number of panes allowed in a single thread tab.
pub const MAX_PANES: usize = 4;

/// Preference order when choosing a kind for a newly split pane.
const SPLIT_KIND_ORDER: [PaneKind; 3] = [PaneKind::Changes, PaneKind::Files, PaneKind::History];

/// Display order for the pane kind menu (non-thread kinds).
const MENU_KIND_ORDER: [PaneKind; 3] = [PaneKind::Changes, PaneKind::History, PaneKind::Files];

/// Stable identifier for a leaf pane within a `PaneTree`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Stable identifier for a split divider within a `PaneTree`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DividerId(u64);

impl DividerId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Content kind displayed by a Work view pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneKind {
    Thread,
    Changes,
    History,
    Files,
}

impl PaneKind {
    pub fn is_thread(self) -> bool {
        matches!(self, Self::Thread)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Thread => "Thread",
            Self::Changes => "Changes",
            Self::History => "History",
            Self::Files => "Files",
        }
    }
}

/// Orientation of a split node's two children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SplitDirection {
    /// Children sit left and right; the divider is vertical.
    SideBySide,
    /// Children sit top and bottom; the divider is horizontal.
    Stacked,
}

/// Explicit reason a pane operation was refused.
///
/// Each variant provides plain-language text suitable for disabled-control hover text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneRefusal {
    /// Window is narrower than `WORK_VIEW_MIN_WINDOW_WIDTH`.
    WindowTooNarrow,
    /// The tab already holds `MAX_PANES` panes.
    AtPaneLimit,
    /// Every non-thread pane kind is already open in this tab.
    NoKindAvailable,
    /// Splitting would make at least one half smaller than the minimum pane size.
    TooSmallToSplit { direction: SplitDirection },
    /// The requested kind is already open in this tab.
    KindAlreadyOpen(PaneKind),
    /// The Thread pane cannot be closed.
    CannotCloseThread,
    /// The Thread pane cannot change kind.
    CannotChangeThreadKind,
    /// No pane with this ID exists in the layout.
    PaneNotFound(PaneId),
    /// No divider with this ID exists in the layout.
    DividerNotFound(DividerId),
    /// The divider's two sides cannot both keep their minimum size in the space there is.
    TooSmallToResize,
    /// Available dimensions or drag position were non-finite or non-positive.
    InvalidGeometry,
}

impl PaneRefusal {
    /// Human-readable explanation for hover text or error reporting.
    pub fn reason(self) -> &'static str {
        match self {
            Self::WindowTooNarrow => "Work view needs a window at least 800 points wide.",
            Self::AtPaneLimit => "A tab holds at most four panes.",
            Self::NoKindAvailable => "Every pane kind is already open.",
            Self::TooSmallToSplit {
                direction: SplitDirection::SideBySide,
            } => "Not enough width to split this pane.",
            Self::TooSmallToSplit {
                direction: SplitDirection::Stacked,
            } => "Not enough height to split this pane.",
            Self::KindAlreadyOpen(PaneKind::Thread) => "Thread is already open.",
            Self::KindAlreadyOpen(PaneKind::Changes) => "Changes is already open.",
            Self::KindAlreadyOpen(PaneKind::History) => "History is already open.",
            Self::KindAlreadyOpen(PaneKind::Files) => "Files is already open.",
            Self::CannotCloseThread => "The Thread pane cannot be closed.",
            Self::CannotChangeThreadKind => "The Thread pane cannot change kind.",
            Self::PaneNotFound(_) => "That pane is no longer open.",
            Self::DividerNotFound(_) => "That divider is no longer present.",
            Self::TooSmallToResize => "Not enough room to move this divider.",
            Self::InvalidGeometry => "Invalid layout dimensions.",
        }
    }
}

impl fmt::Display for PaneRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for PaneRefusal {}

/// Check whether a window width in points is wide enough for Work view.
pub fn check_work_view_width(window_width: f32) -> Result<(), PaneRefusal> {
    if window_width.is_finite() && window_width >= WORK_VIEW_MIN_WINDOW_WIDTH {
        Ok(())
    } else {
        Err(PaneRefusal::WindowTooNarrow)
    }
}

/// Result of closing a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum CloseOutcome {
    /// The pane was removed and its sibling took its space.
    Closed { focused: PaneId },
    /// That was the last pane beside the Thread pane; the tab should return to Chat view.
    ReturnToChat,
}

/// Result of focusing a pane by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusKindOutcome {
    /// That kind was already open; focus moved to it.
    Focused(PaneId),
    /// That kind was not open; the last focused side pane switched to it and took focus.
    SwitchedKind { pane: PaneId, previous: PaneKind },
}

impl FocusKindOutcome {
    pub fn pane(self) -> PaneId {
        match self {
            Self::Focused(pane) | Self::SwitchedKind { pane, .. } => pane,
        }
    }
}

/// Axis-aligned rectangle in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }
}

/// Laid-out pane rectangle for presentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneRect {
    pub id: PaneId,
    pub kind: PaneKind,
    pub focused: bool,
    pub rect: Rect,
}

/// Laid-out divider between two split children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DividerRect {
    pub id: DividerId,
    pub direction: SplitDirection,
    /// 1-point hairline drawn along the seam.
    pub hairline: Rect,
    /// 6-point drag target centered on the seam.
    pub drag: Rect,
}

/// Complete geometry for one frame of Work view panes.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneLayout {
    pub panes: Vec<PaneRect>,
    pub dividers: Vec<DividerRect>,
}

/// How a split allocates space to its first child.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SplitSizing {
    /// Fixed initial size in points (used so the Thread pane starts at 420pt
    /// without requiring window dimensions at construction). Replaced by `Ratio`
    /// once the user drags or evens the divider.
    FixedFirst(f32),
    /// Proportional share of the split's span in `(0.0, 1.0)`.
    Ratio(f32),
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Leaf {
        id: PaneId,
        kind: PaneKind,
    },
    Split {
        id: DividerId,
        direction: SplitDirection,
        sizing: SplitSizing,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// Split-tree state for a thread tab's Work view panes.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneTree {
    root: Node,
    thread_id: PaneId,
    focused: PaneId,
    /// Most recently focused non-thread pane, used when switching to an unopened kind.
    last_focused_side: PaneId,
    next_pane_id: u64,
    next_divider_id: u64,
}

impl PaneTree {
    /// First visit to Work view: the Thread pane (420pt wide) beside one pane of `side_kind`.
    pub fn new(side_kind: PaneKind) -> Result<Self, PaneRefusal> {
        if side_kind.is_thread() {
            return Err(PaneRefusal::KindAlreadyOpen(PaneKind::Thread));
        }
        let thread_id = PaneId(1);
        let side_id = PaneId(2);
        let divider_id = DividerId(1);
        let root = Node::Split {
            id: divider_id,
            direction: SplitDirection::SideBySide,
            sizing: SplitSizing::FixedFirst(THREAD_INITIAL_WIDTH),
            first: Box::new(Node::Leaf {
                id: thread_id,
                kind: PaneKind::Thread,
            }),
            second: Box::new(Node::Leaf {
                id: side_id,
                kind: side_kind,
            }),
        };
        Ok(Self {
            root,
            thread_id,
            focused: thread_id,
            last_focused_side: side_id,
            next_pane_id: 3,
            next_divider_id: 2,
        })
    }

    pub fn pane_count(&self) -> usize {
        self.root.leaf_count()
    }

    pub fn focused(&self) -> PaneId {
        self.focused
    }

    pub fn last_focused_side(&self) -> PaneId {
        self.last_focused_side
    }

    pub fn thread_pane(&self) -> PaneId {
        self.thread_id
    }

    pub fn pane_kind(&self, pane: PaneId) -> Option<PaneKind> {
        self.root.find_kind(pane)
    }

    pub fn pane_of_kind(&self, kind: PaneKind) -> Option<PaneId> {
        self.root.find_by_kind(kind)
    }

    /// Panes in focus order: a split's first side (left or top) before its
    /// second, so each column or row is finished before the next begins.
    pub fn panes_in_order(&self) -> Vec<(PaneId, PaneKind)> {
        let mut out = Vec::with_capacity(self.pane_count());
        self.root.collect_leaves(&mut out);
        out
    }

    /// The smallest area in points that holds every pane at its minimum size.
    pub fn min_size(&self) -> (f32, f32) {
        (self.root.min_width(), self.root.min_height())
    }

    /// Whether the area holds every pane at its minimum size. A view that
    /// does not fit draws Chat view rather than panes under their minimum.
    pub fn fits(&self, available_width: f32, available_height: f32) -> bool {
        let (width, height) = self.min_size();
        valid_extent(available_width, available_height)
            && available_width >= width
            && available_height >= height
    }

    /// Kinds listed in `pane`'s kind menu (non-thread kinds not already open).
    /// Returns an empty list for the Thread pane or an unknown pane ID.
    pub fn available_kinds_for(&self, pane: PaneId) -> Vec<PaneKind> {
        let Some(kind) = self.pane_kind(pane) else {
            return Vec::new();
        };
        if kind.is_thread() {
            return Vec::new();
        }
        MENU_KIND_ORDER
            .into_iter()
            .filter(|candidate| self.pane_of_kind(*candidate).is_none())
            .collect()
    }

    /// First of Changes, Files, History that is not currently open.
    pub fn next_split_kind(&self) -> Option<PaneKind> {
        SPLIT_KIND_ORDER
            .into_iter()
            .find(|kind| self.pane_of_kind(*kind).is_none())
    }

    /// Check whether `pane` can be split in `direction` within `(available_width, available_height)`.
    pub fn can_split(
        &self,
        pane: PaneId,
        direction: SplitDirection,
        available_width: f32,
        available_height: f32,
    ) -> Result<(), PaneRefusal> {
        self.can_split_with_limit(
            pane,
            direction,
            available_width,
            available_height,
            MAX_PANES,
        )
        .map(|_| ())
    }

    /// Split `pane` side by side; the new pane takes the right half.
    pub fn split_right(
        &mut self,
        pane: PaneId,
        available_width: f32,
        available_height: f32,
    ) -> Result<PaneId, PaneRefusal> {
        self.split(
            pane,
            SplitDirection::SideBySide,
            available_width,
            available_height,
        )
    }

    /// Split `pane` vertically; the new pane takes the bottom half.
    pub fn split_down(
        &mut self,
        pane: PaneId,
        available_width: f32,
        available_height: f32,
    ) -> Result<PaneId, PaneRefusal> {
        self.split(
            pane,
            SplitDirection::Stacked,
            available_width,
            available_height,
        )
    }

    /// Split `pane` in `direction`, giving the new pane half its width or height
    /// and the first unopened kind in `Changes`, `Files`, `History`.
    pub fn split(
        &mut self,
        pane: PaneId,
        direction: SplitDirection,
        available_width: f32,
        available_height: f32,
    ) -> Result<PaneId, PaneRefusal> {
        self.split_with_limit(
            pane,
            direction,
            available_width,
            available_height,
            MAX_PANES,
        )
    }

    fn can_split_with_limit(
        &self,
        pane: PaneId,
        direction: SplitDirection,
        available_width: f32,
        available_height: f32,
        max_panes: usize,
    ) -> Result<PaneKind, PaneRefusal> {
        if self.pane_kind(pane).is_none() {
            return Err(PaneRefusal::PaneNotFound(pane));
        }
        if self.pane_count() >= max_panes {
            return Err(PaneRefusal::AtPaneLimit);
        }
        let Some(new_kind) = self.next_split_kind() else {
            return Err(PaneRefusal::NoKindAvailable);
        };
        if !valid_extent(available_width, available_height) {
            return Err(PaneRefusal::InvalidGeometry);
        }
        let layout = self.layout(available_width, available_height);
        let rect = layout
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.rect)
            .ok_or(PaneRefusal::PaneNotFound(pane))?;
        let (half_w, half_h) = match direction {
            SplitDirection::SideBySide => (rect.width * 0.5, rect.height),
            SplitDirection::Stacked => (rect.width, rect.height * 0.5),
        };
        if !(half_w >= MIN_PANE_WIDTH && half_h >= MIN_PANE_HEIGHT) {
            return Err(PaneRefusal::TooSmallToSplit { direction });
        }
        Ok(new_kind)
    }

    fn split_with_limit(
        &mut self,
        pane: PaneId,
        direction: SplitDirection,
        available_width: f32,
        available_height: f32,
        max_panes: usize,
    ) -> Result<PaneId, PaneRefusal> {
        let new_kind = self.can_split_with_limit(
            pane,
            direction,
            available_width,
            available_height,
            max_panes,
        )?;
        let new_pane = self.alloc_pane_id();
        let new_divider = self.alloc_divider_id();
        let replaced = self
            .root
            .split_leaf(pane, new_pane, new_kind, new_divider, direction);
        debug_assert!(replaced, "validated pane must exist in tree");
        self.focused = new_pane;
        self.last_focused_side = new_pane;
        Ok(new_pane)
    }

    /// Close `pane`, giving its space to its sibling.
    ///
    /// Refuses the Thread pane. Closing the last pane beside the Thread pane
    /// reports `CloseOutcome::ReturnToChat` without leaving the tree invalid.
    pub fn close(&mut self, pane: PaneId) -> Result<CloseOutcome, PaneRefusal> {
        let Some(kind) = self.pane_kind(pane) else {
            return Err(PaneRefusal::PaneNotFound(pane));
        };
        if kind.is_thread() {
            return Err(PaneRefusal::CannotCloseThread);
        }
        if self.pane_count() <= 2 {
            return Ok(CloseOutcome::ReturnToChat);
        }
        let sibling_focus = self
            .root
            .remove_leaf(pane)
            .ok_or(PaneRefusal::PaneNotFound(pane))?;
        if self.focused == pane {
            self.focused = sibling_focus;
        }
        self.repair_last_focused_side(pane, sibling_focus);
        Ok(CloseOutcome::Closed {
            focused: self.focused,
        })
    }

    /// Change `pane`'s kind to an unopened non-thread kind.
    pub fn set_kind(&mut self, pane: PaneId, new_kind: PaneKind) -> Result<(), PaneRefusal> {
        let Some(current) = self.pane_kind(pane) else {
            return Err(PaneRefusal::PaneNotFound(pane));
        };
        if current.is_thread() {
            return Err(PaneRefusal::CannotChangeThreadKind);
        }
        if self.pane_of_kind(new_kind).is_some() {
            return Err(PaneRefusal::KindAlreadyOpen(new_kind));
        }
        self.root.set_leaf_kind(pane, new_kind);
        Ok(())
    }

    /// Drag `divider` to `position` (X for side-by-side, Y for stacked) within
    /// `(available_width, available_height)`, clamped so neither side violates minimums.
    pub fn resize_divider(
        &mut self,
        divider: DividerId,
        position: f32,
        available_width: f32,
        available_height: f32,
    ) -> Result<(), PaneRefusal> {
        if !valid_extent(available_width, available_height) || !position.is_finite() {
            return Err(PaneRefusal::InvalidGeometry);
        }
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: available_width,
            height: available_height,
        };
        let Some(info) = self.root.find_divider_geometry(divider, bounds) else {
            return Err(PaneRefusal::DividerNotFound(divider));
        };
        if info.span <= 0.0 {
            return Err(PaneRefusal::InvalidGeometry);
        }
        // A drag in a space too small for both minimums would store a ratio
        // the person never chose; keep the one they had.
        if info.first_min + info.second_min > info.span {
            return Err(PaneRefusal::TooSmallToResize);
        }
        let desired = position - info.origin;
        let clamped = desired.clamp(info.first_min, info.span - info.second_min);
        let ratio = clamped / info.span;
        self.root
            .set_divider_sizing(divider, SplitSizing::Ratio(ratio));
        Ok(())
    }

    /// Even the two sides of `divider` (50/50 split, e.g. on double-click).
    pub fn even_divider(&mut self, divider: DividerId) -> Result<(), PaneRefusal> {
        if self
            .root
            .set_divider_sizing(divider, SplitSizing::Ratio(0.5))
        {
            Ok(())
        } else {
            Err(PaneRefusal::DividerNotFound(divider))
        }
    }

    /// Move focus directly to `pane`.
    pub fn focus(&mut self, pane: PaneId) -> Result<(), PaneRefusal> {
        let Some(kind) = self.pane_kind(pane) else {
            return Err(PaneRefusal::PaneNotFound(pane));
        };
        self.focused = pane;
        if !kind.is_thread() {
            self.last_focused_side = pane;
        }
        Ok(())
    }

    /// Focus the next pane in `panes_in_order`, wrapping.
    pub fn focus_next(&mut self) -> PaneId {
        self.step_focus(1)
    }

    /// Focus the previous pane in `panes_in_order`, wrapping.
    pub fn focus_previous(&mut self) -> PaneId {
        self.step_focus(-1)
    }

    /// Focus the pane showing `kind`. If `kind` is not open, the last focused pane
    /// beside the Thread pane switches to `kind` and takes focus.
    pub fn focus_kind(&mut self, kind: PaneKind) -> FocusKindOutcome {
        if let Some(id) = self.pane_of_kind(kind) {
            self.focused = id;
            if !kind.is_thread() {
                self.last_focused_side = id;
            }
            return FocusKindOutcome::Focused(id);
        }
        let Some((target, previous)) = self.side_pane_to_switch() else {
            // Unreachable while a side pane exists; focus stays where it is.
            return FocusKindOutcome::Focused(self.focused);
        };
        self.root.set_leaf_kind(target, kind);
        self.last_focused_side = target;
        self.focused = target;
        FocusKindOutcome::SwitchedKind {
            pane: target,
            previous,
        }
    }

    /// Compute pane and divider rectangles for the given area in points.
    ///
    /// Pure read: stored ratios survive window resizes while minimums are respected.
    pub fn layout(&self, available_width: f32, available_height: f32) -> PaneLayout {
        let width = sanitize_dimension(available_width);
        let height = sanitize_dimension(available_height);
        let mut panes = Vec::with_capacity(self.pane_count());
        let mut dividers = Vec::with_capacity(self.pane_count().saturating_sub(1));
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        self.root
            .layout(bounds, self.focused, &mut panes, &mut dividers);
        PaneLayout { panes, dividers }
    }

    /// The side pane a view action changes: the last focused one, or the
    /// first side pane when that one is gone.
    fn side_pane_to_switch(&self) -> Option<(PaneId, PaneKind)> {
        let side = |kind: PaneKind| !kind.is_thread();
        self.pane_kind(self.last_focused_side)
            .filter(|kind| side(*kind))
            .map(|kind| (self.last_focused_side, kind))
            .or_else(|| {
                self.panes_in_order()
                    .into_iter()
                    .find(|(_, kind)| side(*kind))
            })
    }

    fn step_focus(&mut self, delta: isize) -> PaneId {
        let order = self.panes_in_order();
        let len = order.len() as isize;
        let current = order
            .iter()
            .position(|(id, _)| *id == self.focused)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        let (id, kind) = order[next];
        self.focused = id;
        if !kind.is_thread() {
            self.last_focused_side = id;
        }
        id
    }

    fn repair_last_focused_side(&mut self, closed: PaneId, sibling_focus: PaneId) {
        if let Some(kind) = self.pane_kind(self.focused)
            && !kind.is_thread()
        {
            self.last_focused_side = self.focused;
            return;
        }
        if self.last_focused_side != closed {
            return;
        }
        if let Some(kind) = self.pane_kind(sibling_focus)
            && !kind.is_thread()
        {
            self.last_focused_side = sibling_focus;
            return;
        }
        if let Some((id, _)) = self
            .panes_in_order()
            .into_iter()
            .find(|(_, kind)| !kind.is_thread())
        {
            self.last_focused_side = id;
        }
    }

    fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }

    fn alloc_divider_id(&mut self) -> DividerId {
        let id = DividerId(self.next_divider_id);
        self.next_divider_id += 1;
        id
    }
}

struct DividerGeometry {
    origin: f32,
    span: f32,
    first_min: f32,
    second_min: f32,
}

impl Node {
    fn leaf_count(&self) -> usize {
        match self {
            Self::Leaf { .. } => 1,
            Self::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    fn first_leaf(&self) -> PaneId {
        match self {
            Self::Leaf { id, .. } => *id,
            Self::Split { first, .. } => first.first_leaf(),
        }
    }

    fn find_kind(&self, target: PaneId) -> Option<PaneKind> {
        match self {
            Self::Leaf { id, kind } if *id == target => Some(*kind),
            Self::Leaf { .. } => None,
            Self::Split { first, second, .. } => {
                first.find_kind(target).or_else(|| second.find_kind(target))
            }
        }
    }

    fn find_by_kind(&self, target: PaneKind) -> Option<PaneId> {
        match self {
            Self::Leaf { id, kind } if *kind == target => Some(*id),
            Self::Leaf { .. } => None,
            Self::Split { first, second, .. } => first
                .find_by_kind(target)
                .or_else(|| second.find_by_kind(target)),
        }
    }

    fn collect_leaves(&self, out: &mut Vec<(PaneId, PaneKind)>) {
        match self {
            Self::Leaf { id, kind } => out.push((*id, *kind)),
            Self::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    fn min_width(&self) -> f32 {
        match self {
            Self::Leaf { .. } => MIN_PANE_WIDTH,
            Self::Split {
                direction: SplitDirection::SideBySide,
                first,
                second,
                ..
            } => first.min_width() + second.min_width(),
            Self::Split {
                direction: SplitDirection::Stacked,
                first,
                second,
                ..
            } => first.min_width().max(second.min_width()),
        }
    }

    fn min_height(&self) -> f32 {
        match self {
            Self::Leaf { .. } => MIN_PANE_HEIGHT,
            Self::Split {
                direction: SplitDirection::SideBySide,
                first,
                second,
                ..
            } => first.min_height().max(second.min_height()),
            Self::Split {
                direction: SplitDirection::Stacked,
                first,
                second,
                ..
            } => first.min_height() + second.min_height(),
        }
    }

    fn split_leaf(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        new_kind: PaneKind,
        new_divider: DividerId,
        direction: SplitDirection,
    ) -> bool {
        match self {
            Self::Leaf { id, kind } if *id == target => {
                let existing = Self::Leaf {
                    id: *id,
                    kind: *kind,
                };
                let added = Self::Leaf {
                    id: new_pane,
                    kind: new_kind,
                };
                *self = Self::Split {
                    id: new_divider,
                    direction,
                    sizing: SplitSizing::Ratio(0.5),
                    first: Box::new(existing),
                    second: Box::new(added),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target, new_pane, new_kind, new_divider, direction)
                    || second.split_leaf(target, new_pane, new_kind, new_divider, direction)
            }
        }
    }

    /// Removes `target` leaf and promotes its sibling. Returns a neighbour pane ID
    /// from the promoted sibling for focus handoff.
    fn remove_leaf(&mut self, target: PaneId) -> Option<PaneId> {
        let Self::Split { first, second, .. } = self else {
            return None;
        };
        if matches!(first.as_ref(), Self::Leaf { id, .. } if *id == target) {
            let promoted = std::mem::replace(
                second.as_mut(),
                Self::Leaf {
                    id: PaneId(0),
                    kind: PaneKind::Thread,
                },
            );
            let neighbour = promoted.first_leaf();
            *self = promoted;
            return Some(neighbour);
        }
        if matches!(second.as_ref(), Self::Leaf { id, .. } if *id == target) {
            let promoted = std::mem::replace(
                first.as_mut(),
                Self::Leaf {
                    id: PaneId(0),
                    kind: PaneKind::Thread,
                },
            );
            let neighbour = promoted.first_leaf();
            *self = promoted;
            return Some(neighbour);
        }
        first
            .remove_leaf(target)
            .or_else(|| second.remove_leaf(target))
    }

    fn set_leaf_kind(&mut self, target: PaneId, new_kind: PaneKind) -> bool {
        match self {
            Self::Leaf { id, kind } if *id == target => {
                *kind = new_kind;
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.set_leaf_kind(target, new_kind) || second.set_leaf_kind(target, new_kind)
            }
        }
    }

    fn set_divider_sizing(&mut self, target: DividerId, new_sizing: SplitSizing) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                id,
                sizing,
                first,
                second,
                ..
            } => {
                if *id == target {
                    *sizing = new_sizing;
                    return true;
                }
                first.set_divider_sizing(target, new_sizing)
                    || second.set_divider_sizing(target, new_sizing)
            }
        }
    }

    fn find_divider_geometry(&self, target: DividerId, bounds: Rect) -> Option<DividerGeometry> {
        let Self::Split {
            id,
            direction,
            sizing,
            first,
            second,
        } = self
        else {
            return None;
        };
        let (first_rect, second_rect, geometry) =
            split_rect(bounds, *direction, *sizing, first, second);
        if *id == target {
            return Some(geometry);
        }
        first
            .find_divider_geometry(target, first_rect)
            .or_else(|| second.find_divider_geometry(target, second_rect))
    }

    fn layout(
        &self,
        bounds: Rect,
        focused: PaneId,
        panes: &mut Vec<PaneRect>,
        dividers: &mut Vec<DividerRect>,
    ) {
        match self {
            Self::Leaf { id, kind } => {
                panes.push(PaneRect {
                    id: *id,
                    kind: *kind,
                    focused: *id == focused,
                    rect: bounds,
                });
            }
            Self::Split {
                id,
                direction,
                sizing,
                first,
                second,
            } => {
                let (first_rect, second_rect, _) =
                    split_rect(bounds, *direction, *sizing, first, second);
                dividers.push(make_divider_rect(*id, *direction, bounds, first_rect));
                first.layout(first_rect, focused, panes, dividers);
                second.layout(second_rect, focused, panes, dividers);
            }
        }
    }
}

fn split_rect(
    bounds: Rect,
    direction: SplitDirection,
    sizing: SplitSizing,
    first: &Node,
    second: &Node,
) -> (Rect, Rect, DividerGeometry) {
    match direction {
        SplitDirection::SideBySide => {
            let first_min = first.min_width();
            let second_min = second.min_width();
            let first_w = resolve_first_size(sizing, bounds.width, first_min, second_min);
            let first_rect = Rect {
                x: bounds.x,
                y: bounds.y,
                width: first_w,
                height: bounds.height,
            };
            let second_rect = Rect {
                x: bounds.x + first_w,
                y: bounds.y,
                width: (bounds.width - first_w).max(0.0),
                height: bounds.height,
            };
            let geometry = DividerGeometry {
                origin: bounds.x,
                span: bounds.width,
                first_min,
                second_min,
            };
            (first_rect, second_rect, geometry)
        }
        SplitDirection::Stacked => {
            let first_min = first.min_height();
            let second_min = second.min_height();
            let first_h = resolve_first_size(sizing, bounds.height, first_min, second_min);
            let first_rect = Rect {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: first_h,
            };
            let second_rect = Rect {
                x: bounds.x,
                y: bounds.y + first_h,
                width: bounds.width,
                height: (bounds.height - first_h).max(0.0),
            };
            let geometry = DividerGeometry {
                origin: bounds.y,
                span: bounds.height,
                first_min,
                second_min,
            };
            (first_rect, second_rect, geometry)
        }
    }
}

fn resolve_first_size(sizing: SplitSizing, total: f32, first_min: f32, second_min: f32) -> f32 {
    if total <= 0.0 {
        return 0.0;
    }
    let desired = match sizing {
        SplitSizing::FixedFirst(size) => size,
        SplitSizing::Ratio(ratio) => total * ratio,
    };
    let min_sum = first_min + second_min;
    if min_sum <= total {
        desired.clamp(first_min, total - second_min)
    } else {
        // Available space is smaller than the combined minimums; share proportionally.
        total * (first_min / min_sum)
    }
}

/// The part of `rect` inside `bounds`, so a divider never draws or takes
/// drags outside the area its split was given.
fn clip(rect: Rect, bounds: Rect) -> Rect {
    let x = rect.x.max(bounds.x);
    let y = rect.y.max(bounds.y);
    Rect {
        x,
        y,
        width: (rect.right().min(bounds.right()) - x).max(0.0),
        height: (rect.bottom().min(bounds.bottom()) - y).max(0.0),
    }
}

fn make_divider_rect(
    id: DividerId,
    direction: SplitDirection,
    bounds: Rect,
    first_rect: Rect,
) -> DividerRect {
    let divider = unclipped_divider_rect(id, direction, bounds, first_rect);
    DividerRect {
        hairline: clip(divider.hairline, bounds),
        drag: clip(divider.drag, bounds),
        ..divider
    }
}

fn unclipped_divider_rect(
    id: DividerId,
    direction: SplitDirection,
    bounds: Rect,
    first_rect: Rect,
) -> DividerRect {
    match direction {
        SplitDirection::SideBySide => {
            let seam = first_rect.right();
            DividerRect {
                id,
                direction,
                hairline: Rect {
                    x: seam - DIVIDER_THICKNESS * 0.5,
                    y: bounds.y,
                    width: DIVIDER_THICKNESS,
                    height: bounds.height,
                },
                drag: Rect {
                    x: seam - DIVIDER_DRAG_EXTENT * 0.5,
                    y: bounds.y,
                    width: DIVIDER_DRAG_EXTENT,
                    height: bounds.height,
                },
            }
        }
        SplitDirection::Stacked => {
            let seam = first_rect.bottom();
            DividerRect {
                id,
                direction,
                hairline: Rect {
                    x: bounds.x,
                    y: seam - DIVIDER_THICKNESS * 0.5,
                    width: bounds.width,
                    height: DIVIDER_THICKNESS,
                },
                drag: Rect {
                    x: bounds.x,
                    y: seam - DIVIDER_DRAG_EXTENT * 0.5,
                    width: bounds.width,
                    height: DIVIDER_DRAG_EXTENT,
                },
            }
        }
    }
}

fn valid_extent(width: f32, height: f32) -> bool {
    width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0
}

fn sanitize_dimension(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-3;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= EPS,
            "expected {expected}, got {actual}"
        );
    }

    /// Verify pane rectangles have no interior overlap and tile `[0, w] x [0, h]`.
    fn assert_tiles_without_overlap(layout: &PaneLayout, width: f32, height: f32) {
        let mut total_area = 0.0_f32;
        for (i, a) in layout.panes.iter().enumerate() {
            assert!(a.rect.x >= -EPS && a.rect.y >= -EPS);
            assert!(a.rect.width >= 0.0 && a.rect.height >= 0.0);
            assert!(a.rect.right() <= width + EPS);
            assert!(a.rect.bottom() <= height + EPS);
            total_area += a.rect.width * a.rect.height;

            for b in &layout.panes[i + 1..] {
                let x_overlap = a.rect.right().min(b.rect.right()) - a.rect.x.max(b.rect.x);
                let y_overlap = a.rect.bottom().min(b.rect.bottom()) - a.rect.y.max(b.rect.y);
                assert!(
                    x_overlap <= EPS || y_overlap <= EPS,
                    "panes {:?} and {:?} overlap: {:?} vs {:?}",
                    a.id,
                    b.id,
                    a.rect,
                    b.rect
                );
            }
        }
        assert_close(total_area, width * height);
    }

    #[test]
    fn first_visit_starts_with_thread_at_420_and_side_pane() {
        assert_eq!(
            PaneTree::new(PaneKind::Thread),
            Err(PaneRefusal::KindAlreadyOpen(PaneKind::Thread))
        );

        let panes = PaneTree::new(PaneKind::Changes).unwrap();
        assert_eq!(panes.pane_count(), 2);
        assert_eq!(panes.focused(), panes.thread_pane());
        assert_eq!(
            panes.panes_in_order(),
            vec![
                (panes.thread_pane(), PaneKind::Thread),
                (
                    panes.pane_of_kind(PaneKind::Changes).unwrap(),
                    PaneKind::Changes
                ),
            ]
        );

        let layout = panes.layout(1200.0, 800.0);
        assert_eq!(layout.panes.len(), 2);
        assert_eq!(layout.dividers.len(), 1);
        assert_close(layout.panes[0].rect.width, THREAD_INITIAL_WIDTH);
        assert_close(layout.panes[1].rect.width, 1200.0 - THREAD_INITIAL_WIDTH);
        assert_close(layout.dividers[0].hairline.width, DIVIDER_THICKNESS);
        assert_close(layout.dividers[0].drag.width, DIVIDER_DRAG_EXTENT);
        assert_tiles_without_overlap(&layout, 1200.0, 800.0);
    }

    #[test]
    fn work_view_width_check_enforces_800_point_minimum() {
        assert_eq!(check_work_view_width(800.0), Ok(()));
        assert_eq!(check_work_view_width(1440.0), Ok(()));
        let err = check_work_view_width(799.5).unwrap_err();
        assert_eq!(err, PaneRefusal::WindowTooNarrow);
        assert!(err.reason().contains("800"));
        assert_eq!(
            check_work_view_width(f32::NAN),
            Err(PaneRefusal::WindowTooNarrow)
        );
    }

    #[test]
    fn split_chooses_first_unopened_kind_and_refuses_fifth_pane() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();

        // First split picks Files (Changes is open; order is Changes, Files, History).
        let files = panes.split_down(changes, 1200.0, 800.0).unwrap();
        assert_eq!(panes.pane_kind(files), Some(PaneKind::Files));
        assert_eq!(panes.focused(), files);

        // Second split picks History.
        let history = panes.split_right(changes, 1200.0, 800.0).unwrap();
        assert_eq!(panes.pane_kind(history), Some(PaneKind::History));
        assert_eq!(panes.pane_count(), 4);
        assert_eq!(panes.next_split_kind(), None);

        // Fifth pane is refused with the four-pane limit reason.
        let err = panes.split_down(files, 1600.0, 1000.0).unwrap_err();
        assert_eq!(err, PaneRefusal::AtPaneLimit);
        assert!(err.reason().contains("four panes"));

        // With a higher synthetic cap, running out of kinds is also refused explicitly.
        let err = panes
            .split_with_limit(files, SplitDirection::Stacked, 1600.0, 1000.0, 5)
            .unwrap_err();
        assert_eq!(err, PaneRefusal::NoKindAvailable);
        assert!(!err.reason().is_empty());
    }

    #[test]
    fn split_refuses_when_either_half_would_be_under_minimum() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let thread = panes.thread_pane();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();

        // Thread is 420 wide -> half is 210 < 320.
        assert_eq!(
            panes.split_right(thread, 1200.0, 800.0),
            Err(PaneRefusal::TooSmallToSplit {
                direction: SplitDirection::SideBySide,
            })
        );
        // Changes is 480 wide at 900 total -> half is 240 < 320.
        assert_eq!(
            panes.split_right(changes, 900.0, 800.0),
            Err(PaneRefusal::TooSmallToSplit {
                direction: SplitDirection::SideBySide,
            })
        );
        // Height 300 -> half is 150 < 160.
        assert_eq!(
            panes.split_down(changes, 1200.0, 300.0),
            Err(PaneRefusal::TooSmallToSplit {
                direction: SplitDirection::Stacked,
            })
        );
        // Exact threshold 320 high -> two 160-high halves succeed.
        assert!(panes.split_down(changes, 1200.0, 320.0).is_ok());
    }

    #[test]
    fn thread_pane_cannot_close_or_change_kind_and_each_kind_opens_once() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let thread = panes.thread_pane();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();

        assert_eq!(panes.close(thread), Err(PaneRefusal::CannotCloseThread));
        assert_eq!(
            panes.set_kind(thread, PaneKind::Files),
            Err(PaneRefusal::CannotChangeThreadKind)
        );
        assert!(panes.available_kinds_for(thread).is_empty());

        // Kind menu for Changes lists History and Files (not Thread or Changes).
        assert_eq!(
            panes.available_kinds_for(changes),
            vec![PaneKind::History, PaneKind::Files]
        );

        // Cannot change to an already-open kind.
        assert_eq!(
            panes.set_kind(changes, PaneKind::Thread),
            Err(PaneRefusal::KindAlreadyOpen(PaneKind::Thread))
        );
        assert_eq!(
            panes.set_kind(changes, PaneKind::Changes),
            Err(PaneRefusal::KindAlreadyOpen(PaneKind::Changes))
        );

        // Can change to History; then menu lists Changes and Files.
        panes.set_kind(changes, PaneKind::History).unwrap();
        assert_eq!(panes.pane_kind(changes), Some(PaneKind::History));
        assert_eq!(
            panes.available_kinds_for(changes),
            vec![PaneKind::Changes, PaneKind::Files]
        );
    }

    #[test]
    fn closing_last_side_pane_reports_return_to_chat_and_sibling_takes_space() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();

        // Only one side pane exists -> closing reports ReturnToChat.
        assert_eq!(panes.close(changes), Ok(CloseOutcome::ReturnToChat));
        assert_eq!(panes.pane_count(), 2);

        // Add Files below Changes, then close Files -> Changes reclaims full height and focus.
        let files = panes.split_down(changes, 1200.0, 800.0).unwrap();
        assert_eq!(panes.focused(), files);
        assert_eq!(
            panes.close(files),
            Ok(CloseOutcome::Closed { focused: changes })
        );
        assert_eq!(panes.pane_count(), 2);
        assert_eq!(panes.focused(), changes);
        let layout = panes.layout(1200.0, 800.0);
        assert_close(layout.panes[1].rect.height, 800.0);
        assert_tiles_without_overlap(&layout, 1200.0, 800.0);
    }

    #[test]
    fn focus_next_and_previous_wrap_in_reading_order_with_nested_splits() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let thread = panes.thread_pane();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();
        let files = panes.split_down(changes, 1200.0, 800.0).unwrap();
        let history = panes.split_right(changes, 1200.0, 800.0).unwrap();

        // Order: Thread (left), then on the right top-left Changes, top-right History, bottom Files.
        let expected = [thread, changes, history, files];
        assert_eq!(
            panes
                .panes_in_order()
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            expected
        );

        panes.focus(thread).unwrap();
        assert_eq!(panes.focus_next(), changes);
        assert_eq!(panes.focus_next(), history);
        assert_eq!(panes.focus_next(), files);
        assert_eq!(panes.focus_next(), thread); // wraps

        assert_eq!(panes.focus_previous(), files); // wraps backwards
        assert_eq!(panes.focus_previous(), history);
        assert_eq!(panes.focus_previous(), changes);
        assert_eq!(panes.focus_previous(), thread);
    }

    #[test]
    fn focus_kind_focuses_open_pane_or_switches_last_focused_side_pane() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let thread = panes.thread_pane();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();
        let files = panes.split_down(changes, 1200.0, 800.0).unwrap();

        // Focus Changes, then Thread: last_focused_side remains Changes.
        panes.focus(changes).unwrap();
        panes.focus(thread).unwrap();
        assert_eq!(panes.last_focused_side(), changes);

        // Files is already open -> focuses Files without changing kinds.
        assert_eq!(
            panes.focus_kind(PaneKind::Files),
            FocusKindOutcome::Focused(files)
        );
        assert_eq!(panes.focused(), files);
        assert_eq!(panes.last_focused_side(), files);

        // Move focus to Thread; requesting unopened History switches Files (last side), never Thread.
        panes.focus(thread).unwrap();
        assert_eq!(
            panes.focus_kind(PaneKind::History),
            FocusKindOutcome::SwitchedKind {
                pane: files,
                previous: PaneKind::Files,
            }
        );
        assert_eq!(panes.pane_kind(thread), Some(PaneKind::Thread));
        assert_eq!(panes.pane_kind(files), Some(PaneKind::History));
        assert_eq!(panes.focused(), files);

        // Requesting Thread always focuses the Thread pane.
        assert_eq!(
            panes.focus_kind(PaneKind::Thread),
            FocusKindOutcome::Focused(thread)
        );
        assert_eq!(panes.focused(), thread);
    }

    #[test]
    fn resize_clamps_to_minimums_evens_and_survives_window_resize() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let divider = panes.layout(1200.0, 800.0).dividers[0].id;

        // Dragging past left minimum clamps Thread to 320.
        panes.resize_divider(divider, 100.0, 1200.0, 800.0).unwrap();
        let layout = panes.layout(1200.0, 800.0);
        assert_close(layout.panes[0].rect.width, MIN_PANE_WIDTH);
        assert_close(layout.panes[1].rect.width, 1200.0 - MIN_PANE_WIDTH);

        // Dragging past right minimum clamps Changes to 320.
        panes
            .resize_divider(divider, 1100.0, 1200.0, 800.0)
            .unwrap();
        let layout = panes.layout(1200.0, 800.0);
        assert_close(layout.panes[0].rect.width, 1200.0 - MIN_PANE_WIDTH);
        assert_close(layout.panes[1].rect.width, MIN_PANE_WIDTH);

        // Shrink window to 800 -> minimums are respected (480 / 320); grow back -> ratio survives!
        let shrunk = panes.layout(800.0, 800.0);
        assert_close(shrunk.panes[0].rect.width, 480.0);
        assert_close(shrunk.panes[1].rect.width, MIN_PANE_WIDTH);
        assert_tiles_without_overlap(&shrunk, 800.0, 800.0);

        let restored = panes.layout(1200.0, 800.0);
        assert_close(restored.panes[0].rect.width, 1200.0 - MIN_PANE_WIDTH);
        assert_close(restored.panes[1].rect.width, MIN_PANE_WIDTH);

        // Double-click evens the two sides (50/50) and survives resize.
        panes.even_divider(divider).unwrap();
        let even = panes.layout(1000.0, 800.0);
        assert_close(even.panes[0].rect.width, 500.0);
        assert_close(even.panes[1].rect.width, 500.0);
        let resized = panes.layout(1400.0, 800.0);
        assert_close(resized.panes[0].rect.width, 700.0);
        assert_close(resized.panes[1].rect.width, 700.0);
    }

    #[test]
    fn nested_layout_tiles_without_overlap_and_respects_nested_minimums() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();
        let files = panes.split_right(changes, 1200.0, 800.0).unwrap();
        let _history = panes.split_down(files, 1200.0, 800.0).unwrap();

        let initial = panes.layout(1200.0, 800.0);
        assert_eq!(initial.panes.len(), 4);
        assert_eq!(initial.dividers.len(), 3);
        assert_tiles_without_overlap(&initial, 1200.0, 800.0);
        for pane in &initial.panes {
            assert!(pane.rect.width >= MIN_PANE_WIDTH - EPS);
            assert!(pane.rect.height >= MIN_PANE_HEIGHT - EPS);
        }

        // Dragging root divider right cannot squeeze the two side-by-side panes below 640 combined.
        let root_divider = initial.dividers[0].id;
        panes
            .resize_divider(root_divider, 900.0, 1200.0, 800.0)
            .unwrap();
        let clamped = panes.layout(1200.0, 800.0);
        assert_close(clamped.panes[0].rect.width, 1200.0 - 2.0 * MIN_PANE_WIDTH);
        for pane in &clamped.panes {
            assert!(pane.rect.width >= MIN_PANE_WIDTH - EPS);
            assert!(pane.rect.height >= MIN_PANE_HEIGHT - EPS);
        }
        assert_tiles_without_overlap(&clamped, 1200.0, 800.0);
    }

    #[test]
    fn independent_tabs_keep_different_layouts() {
        let mut tab_a = PaneTree::new(PaneKind::Changes).unwrap();
        let mut tab_b = PaneTree::new(PaneKind::Changes).unwrap();

        let a_changes = tab_a.pane_of_kind(PaneKind::Changes).unwrap();
        tab_a.split_down(a_changes, 1200.0, 800.0).unwrap();

        let b_changes = tab_b.pane_of_kind(PaneKind::Changes).unwrap();
        tab_b.set_kind(b_changes, PaneKind::History).unwrap();
        let b_divider = tab_b.layout(1200.0, 800.0).dividers[0].id;
        tab_b.even_divider(b_divider).unwrap();

        assert_eq!(tab_a.pane_count(), 3);
        assert_eq!(tab_b.pane_count(), 2);
        assert_eq!(tab_a.pane_of_kind(PaneKind::History), None);
        assert!(tab_b.pane_of_kind(PaneKind::History).is_some());
        assert_close(
            tab_a.layout(1200.0, 800.0).panes[0].rect.width,
            THREAD_INITIAL_WIDTH,
        );
        assert_close(tab_b.layout(1200.0, 800.0).panes[0].rect.width, 600.0);
    }
    fn three_columns() -> (PaneTree, PaneId, PaneId) {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();
        let files = panes.split_right(changes, 1200.0, 800.0).unwrap();
        (panes, changes, files)
    }

    #[test]
    fn an_area_under_the_minimums_does_not_fit() {
        let (panes, _, _) = three_columns();
        assert_eq!(panes.min_size(), (3.0 * MIN_PANE_WIDTH, MIN_PANE_HEIGHT));
        assert!(panes.fits(960.0, 160.0));
        assert!(!panes.fits(800.0, 800.0));
        assert!(!panes.fits(1200.0, 159.0));
        assert!(!panes.fits(f32::NAN, 800.0));
        // Two panes fit the narrowest window Work view allows.
        let two = PaneTree::new(PaneKind::Changes).unwrap();
        assert!(two.fits(WORK_VIEW_MIN_WINDOW_WIDTH, MIN_PANE_HEIGHT));
    }

    #[test]
    fn a_drag_without_room_is_refused_and_keeps_the_ratio() {
        let (mut panes, _, _) = three_columns();
        let before = panes.layout(1200.0, 800.0);
        let root = before.dividers[0].id;
        assert_eq!(
            panes.resize_divider(root, 500.0, 800.0, 800.0),
            Err(PaneRefusal::TooSmallToResize)
        );
        assert_eq!(panes.layout(1200.0, 800.0), before);
    }

    #[test]
    fn tiny_and_empty_areas_give_finite_rectangles_inside_them() {
        let (panes, _, _) = three_columns();
        for (width, height) in [(0.0, 800.0), (5.0, 5.0), (f32::NAN, f32::INFINITY)] {
            let layout = panes.layout(width, height);
            let (width, height) = (sanitize_dimension(width), sanitize_dimension(height));
            let inside = |rect: Rect| {
                rect.x.is_finite()
                    && rect.y.is_finite()
                    && rect.x >= 0.0
                    && rect.y >= 0.0
                    && rect.width >= 0.0
                    && rect.height >= 0.0
                    && rect.right() <= width + EPS
                    && rect.bottom() <= height + EPS
            };
            assert!(layout.panes.iter().all(|pane| inside(pane.rect)));
            assert!(
                layout
                    .dividers
                    .iter()
                    .all(|divider| inside(divider.hairline) && inside(divider.drag))
            );
        }
    }

    #[test]
    fn a_closed_pane_and_its_divider_are_gone_and_ids_are_not_reused() {
        let (mut panes, changes, files) = three_columns();
        let divider = panes
            .layout(1200.0, 800.0)
            .dividers
            .iter()
            .map(|divider| divider.id)
            .max_by_key(|id| id.get())
            .unwrap();
        assert_eq!(
            panes.close(files),
            Ok(CloseOutcome::Closed { focused: changes })
        );
        assert_eq!(panes.focus(files), Err(PaneRefusal::PaneNotFound(files)));
        assert_eq!(panes.close(files), Err(PaneRefusal::PaneNotFound(files)));
        assert_eq!(
            panes.set_kind(files, PaneKind::History),
            Err(PaneRefusal::PaneNotFound(files))
        );
        assert_eq!(
            panes.split_down(files, 1200.0, 800.0),
            Err(PaneRefusal::PaneNotFound(files))
        );
        assert_eq!(
            panes.even_divider(divider),
            Err(PaneRefusal::DividerNotFound(divider))
        );
        assert_eq!(
            panes.resize_divider(divider, 600.0, 1200.0, 800.0),
            Err(PaneRefusal::DividerNotFound(divider))
        );
        let again = panes.split_right(changes, 1200.0, 800.0).unwrap();
        assert_ne!(again, files);
        assert!(
            panes
                .layout(1200.0, 800.0)
                .dividers
                .iter()
                .all(|new| new.id != divider)
        );
        // A view action still has a side pane to change after the close.
        assert_eq!(panes.last_focused_side(), again);
    }

    #[test]
    fn focus_finishes_one_column_before_the_next() {
        let mut panes = PaneTree::new(PaneKind::Changes).unwrap();
        let thread = panes.thread_pane();
        let changes = panes.pane_of_kind(PaneKind::Changes).unwrap();
        let under_changes = panes.split_down(changes, 1400.0, 900.0).unwrap();
        let under_thread = panes.split_down(thread, 1400.0, 900.0).unwrap();
        let order = [thread, under_thread, changes, under_changes];
        assert_eq!(
            panes
                .panes_in_order()
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            order
        );
        panes.focus(thread).unwrap();
        for expected in [under_thread, changes, under_changes, thread] {
            assert_eq!(panes.focus_next(), expected);
        }
        for expected in [under_changes, changes, under_thread, thread] {
            assert_eq!(panes.focus_previous(), expected);
        }
    }
}
