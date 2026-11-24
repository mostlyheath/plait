//! Selecting and copying conversation text, and following its links.
//!
//! Every piece of selectable text paints through [`Selectable`], which records
//! where it landed. Pointer events map those records to positions in the
//! conversation; `plait_client::selection` decides order, ranges, and the copied
//! text. The selection is drawn as a background on the text runs, so it follows
//! wrapping exactly, and it is kept by row identity so it survives streaming.

use super::{Plait, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    markdown::{self, Block},
    selection::{self, Position, Span},
    transcript::{self, Row},
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
};

actions!(
    plait,
    [CopySelection, QuoteSelection, HistoryStart, HistoryEnd]
);

pub(super) fn bind_keys(cx: &mut App) {
    // The composer binds Cmd-C in its own, deeper context and wins while it has focus.
    cx.bind_keys([
        KeyBinding::new("cmd-c", CopySelection, Some("Plait")),
        KeyBinding::new("cmd-up", HistoryStart, Some("Plait")),
        KeyBinding::new("home", HistoryStart, Some("Plait")),
        KeyBinding::new("cmd-down", HistoryEnd, Some("Plait")),
        KeyBinding::new("end", HistoryEnd, Some("Plait")),
    ]);
}

/// The most one pointer movement scrolls the list while dragging past its edge.
const SCROLL_STEP: f32 = 48.;

/// The conversation row a piece of text belongs to, by identity rather than position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    Message(SharedString),
    Stream,
    Reasoning(SharedString),
    LiveReasoning,
    Tool(SharedString),
}

impl Owner {
    fn key(&self) -> String {
        match self {
            Owner::Message(id) => format!("message:{id}"),
            Owner::Reasoning(id) => format!("reasoning:{id}"),
            Owner::Tool(id) => format!("tool:{id}"),
            Owner::Stream => "stream".into(),
            Owner::LiveReasoning => "live-reasoning".into(),
        }
    }
}

/// Where a block is drawn: its row, that row's position this frame, and its index.
#[derive(Clone)]
pub(super) struct Site {
    pub owner: Owner,
    pub row: usize,
    pub block: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Anchor {
    owner: Owner,
    block: usize,
    offset: usize,
}

struct Selection {
    thread: String,
    anchor: Anchor,
    head: Anchor,
}

/// One painted piece of text, valid until the next frame.
struct Region {
    site: Site,
    /// The part of the block's text this piece shows.
    range: Range<usize>,
    by_line: bool,
    links: Rc<[(Range<usize>, SharedString)]>,
    layout: TextLayout,
    hitbox: Hitbox,
}

/// What the pointer is over, or nearest to while dragging.
pub(super) struct Hit {
    anchor: Anchor,
    range: Range<usize>,
    by_line: bool,
    link: Option<SharedString>,
}

/// Reading state for the open conversation.
#[derive(Default)]
pub(super) struct Reading {
    selection: Option<Selection>,
    dragging: bool,
    /// The link under the press; it opens if the release is still on it.
    pressed_link: Option<SharedString>,
    /// Shown before a click so the destination is never a surprise.
    pub hovered_link: Option<SharedString>,
    regions: Rc<RefCell<Vec<Region>>>,
    /// The selection in row order for the frame being built.
    span: Cell<Option<Span>>,
    /// Open tool groups of `groups_thread`, each by its first tool.
    pub open_groups: HashSet<String>,
    pub groups_thread: String,
    /// The projection the tool rows were last reconciled with.
    pub planned: Cell<usize>,
    /// How each grouped tool row was last drawn: hidden, and whether it led its group.
    pub folds: RefCell<HashMap<String, (bool, bool)>>,
    /// Conversation size when the end of it was last in view.
    pub seen: Cell<(usize, usize)>,
}

impl Reading {
    pub(super) fn bookmark_selection(
        &self,
    ) -> Option<(
        plait_client::tabs::bookmark::TextPosition,
        plait_client::tabs::bookmark::TextPosition,
    )> {
        let selection = self.selection.as_ref()?;
        let position = |a: &Anchor| plait_client::tabs::bookmark::TextPosition {
            source: a.owner.key(),
            block: a.block as u32,
            byte: a.offset as u32,
        };
        Some((position(&selection.anchor), position(&selection.head)))
    }

    pub(super) fn restore_selection(
        &mut self,
        thread: &str,
        details: &plait_client::tabs::bookmark::ViewDetails,
    ) {
        self.groups_thread = thread.into();
        self.open_groups = details.folded.iter().cloned().collect();
        let anchor = |p: &plait_client::tabs::bookmark::TextPosition| -> Option<Anchor> {
            let owner = if let Some(id) = p.source.strip_prefix("message:") {
                Owner::Message(id.to_owned().into())
            } else if let Some(id) = p.source.strip_prefix("reasoning:") {
                Owner::Reasoning(id.to_owned().into())
            } else if let Some(id) = p.source.strip_prefix("tool:") {
                Owner::Tool(id.to_owned().into())
            } else if p.source == "stream" {
                Owner::Stream
            } else if p.source == "live-reasoning" {
                Owner::LiveReasoning
            } else {
                return None;
            };
            Some(Anchor {
                owner,
                block: p.block as usize,
                offset: p.byte as usize,
            })
        };
        self.selection = details.selection.as_ref().and_then(|(a, b)| {
            Some(Selection {
                thread: thread.into(),
                anchor: anchor(a)?,
                head: anchor(b)?,
            })
        });
    }

    /// The selected part of a block's `range`, relative to that range.
    pub(super) fn selected(
        &self,
        row: usize,
        block_index: usize,
        block: &Block,
        range: &Range<usize>,
    ) -> Option<Range<usize>> {
        let all = self
            .span
            .get()?
            .within(row, block_index, block.text.len())?;
        let start = selection::clamp(&block.text, all.start.max(range.start));
        let end = selection::clamp(&block.text, all.end.min(range.end));
        (start < end).then(|| start - range.start..end - range.start)
    }
}

/// Text that can be selected. It draws exactly as its `StyledText` would.
pub(super) struct Selectable {
    text: StyledText,
    site: Site,
    range: Range<usize>,
    by_line: bool,
    links: Rc<[(Range<usize>, SharedString)]>,
    regions: Rc<RefCell<Vec<Region>>>,
}

impl Element for Selectable {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(None, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.text
            .prepaint(None, inspector, bounds, state, window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .paint(None, inspector, bounds, state, &mut (), window, cx);
        let region = Region {
            site: self.site.clone(),
            range: self.range.clone(),
            by_line: self.by_line,
            links: self.links.clone(),
            layout: self.text.layout().clone(),
            hitbox: hitbox.clone(),
        };
        let over_link = hitbox.is_hovered(window) && region.link_at(window.mouse_position());
        window.set_cursor_style(
            if over_link {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            },
            hitbox,
        );
        self.regions.borrow_mut().push(region);
    }
}

/// The byte offset in a laid-out text whose caret position is nearest `position`.
fn closest_index(layout: &TextLayout, position: Point<Pixels>) -> usize {
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    if position.y < bounds.top() {
        return 0;
    }
    let mut origin = bounds.origin;
    let mut start = 0;
    for line in layout.line_layouts() {
        let bottom = origin.y + line.size(line_height).height;
        if position.y >= bottom {
            origin.y = bottom;
            // Lines of one text are separated by a newline.
            start += line.len() + 1;
            continue;
        }
        let (Ok(index) | Err(index)) =
            line.closest_index_for_position(position - origin, line_height);
        return start + index;
    }
    layout.len()
}

impl Region {
    /// The part of the text that is on screen.
    fn visible(&self) -> Bounds<Pixels> {
        self.hitbox
            .bounds
            .intersect(&self.hitbox.content_mask.bounds)
    }

    fn link(&self, position: Point<Pixels>) -> Option<SharedString> {
        let index = self.layout.index_for_position(position).ok()?;
        self.links
            .iter()
            .find(|(range, _)| range.contains(&index))
            .map(|(_, url)| url.clone())
    }

    fn link_at(&self, position: Point<Pixels>) -> bool {
        !self.links.is_empty() && self.link(position).is_some()
    }

    fn hit(&self, position: Point<Pixels>) -> Hit {
        let offset = closest_index(&self.layout, position).min(self.range.len());
        Hit {
            anchor: Anchor {
                owner: self.site.owner.clone(),
                block: self.site.block,
                offset: self.range.start + offset,
            },
            range: self.range.clone(),
            by_line: self.by_line,
            link: self.link(position),
        }
    }

    /// How far `position` is from the visible text: rows first, then columns.
    fn distance(&self, position: Point<Pixels>) -> Option<(Pixels, Pixels)> {
        let visible = self.visible();
        if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
            return None;
        }
        let outside = |at: Pixels, low: Pixels, high: Pixels| {
            if at < low {
                low - at
            } else if at > high {
                at - high
            } else {
                px(0.)
            }
        };
        Some((
            outside(position.y, visible.top(), visible.bottom()),
            outside(position.x, visible.left(), visible.right()),
        ))
    }
}

/// The text directly under the pointer.
fn under(regions: &[Region], position: Point<Pixels>, window: &Window) -> Option<Hit> {
    regions
        .iter()
        .rev()
        .find(|region| region.hitbox.is_hovered(window))
        .map(|region| region.hit(position))
}

/// The text nearest the pointer, so a drag keeps extending through gaps and margins.
fn nearest(regions: &[Region], position: Point<Pixels>) -> Option<Hit> {
    regions
        .iter()
        .filter_map(|region| Some((region.distance(position)?, region)))
        .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, region)| region.hit(position))
}

impl Plait {
    /// Selectable text for `range` of a block, with `runs` already relative to it.
    pub(super) fn selectable(
        &self,
        site: &Site,
        block: &Block,
        range: Range<usize>,
        text: StyledText,
        by_line: bool,
        spans: &[(Range<usize>, plait_client::markdown::Style)],
    ) -> Selectable {
        let links = spans
            .iter()
            .filter_map(|(span, style)| {
                let url = block.links.get(usize::from(style.link?))?;
                Some((span.clone(), SharedString::from(url.clone())))
            })
            .collect();
        Selectable {
            text,
            site: site.clone(),
            range,
            by_line,
            links,
            regions: self.workspace.reading.regions.clone(),
        }
    }

    /// Pointer handling for the conversation, sized to cover its list.
    pub(super) fn selection_surface(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let regions = self.workspace.reading.regions.clone();
        let fresh = regions.clone();
        let entity = cx.weak_entity();
        canvas(
            // Text paints after this and records itself for the new frame.
            move |bounds, window, _| {
                fresh.borrow_mut().clear();
                window.insert_hitbox(bounds, HitboxBehavior::Normal)
            },
            move |bounds, surface, window, _| {
                let (press_regions, press_entity) = (regions.clone(), entity.clone());
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || !surface.is_hovered(window)
                    {
                        return;
                    }
                    let (count, extend) = (event.click_count, event.modifiers.shift);
                    let hit = under(&press_regions.borrow(), event.position, window);
                    let _ = press_entity.update(cx, |this, cx| {
                        this.press_text(hit, count, extend, window, cx)
                    });
                });
                let (move_regions, move_entity) = (regions.clone(), entity.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    if event.pressed_button == Some(MouseButton::Left) {
                        let hit = nearest(&move_regions.borrow(), event.position);
                        // How far the pointer is past the top or bottom of the list.
                        let y = event.position.y;
                        let past =
                            (y - bounds.bottom()).max(px(0.)) + (y - bounds.top()).min(px(0.));
                        let _ = move_entity.update(cx, |this, cx| this.drag_text(hit, past, cx));
                    } else {
                        let link = under(&move_regions.borrow(), event.position, window)
                            .and_then(|hit| hit.link);
                        let _ = move_entity.update(cx, |this, cx| this.hover_link(link, cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let link =
                        under(&regions.borrow(), event.position, window).and_then(|hit| hit.link);
                    let _ = entity.update(cx, |this, cx| this.release_text(link, cx));
                });
            },
        )
        .absolute()
        .size_full()
    }

    /// The blocks a row shows, as they are indexed for selection.
    fn row_blocks(&self, row: Row<'_>) -> Vec<&Block> {
        match row {
            Row::Message(message) => self
                .workspace
                .formatted
                .messages
                .get(&message.id)
                .map(|message| message.blocks.iter().collect()),
            Row::Streaming(_) => self
                .workspace
                .formatted
                .live
                .as_ref()
                .map(|message| message.blocks.iter().collect()),
            Row::Reasoning(message) => self
                .workspace
                .formatted
                .reasoning
                .get(&message.id)
                .filter(|_| self.workspace.history.expanded.contains(&message.id))
                .map(|message| message.blocks.iter().collect()),
            Row::LiveReasoning(_) => self
                .workspace
                .formatted
                .live_reasoning
                .as_ref()
                .map(|message| message.blocks.iter().collect()),
            // Details that are folded away are not part of what is on screen.
            Row::Tool(tool) => self
                .workspace
                .formatted
                .tools
                .get(&tool.id)
                .filter(|_| self.tool_details_shown(&tool.id))
                .map(|view| view.sections.iter().map(|section| &section.block).collect()),
        }
        .unwrap_or_default()
    }

    fn anchored_block(&self, anchor: &Anchor) -> Option<&Block> {
        let snapshot = self.conversation_snapshot()?;
        let layout = &self.workspace.live_layout;
        let index = *layout.raw_index_by_key.get(&anchor.owner.key())?;
        let row = transcript::resolve(snapshot, layout.slots[index])?;
        self.row_blocks(row).get(anchor.block).copied()
    }

    /// Place the selection in this frame's row order. Rows that have gone drop it.
    pub(super) fn resolve_selection(&self) {
        self.workspace.reading.span.set(None);
        let (Some(selection), Some(snapshot)) = (
            &self.workspace.reading.selection,
            self.conversation_snapshot(),
        ) else {
            return;
        };
        if selection.thread != snapshot.id {
            return;
        }
        let position = |anchor: &Anchor| {
            let row = *self
                .workspace
                .live_layout
                .raw_index_by_key
                .get(&anchor.owner.key())?;
            Some(Position {
                row,
                block: anchor.block,
                offset: anchor.offset,
            })
        };
        let span = position(&selection.anchor)
            .zip(position(&selection.head))
            .and_then(|(anchor, head)| Span::between(anchor, head));
        self.workspace.reading.span.set(span);
    }

    fn press_text(
        &mut self,
        hit: Option<Hit>,
        clicks: usize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace.reading.dragging = false;
        self.workspace.reading.pressed_link = None;
        let (Some(hit), Some(thread)) = (hit, self.host.thread.selected.clone()) else {
            // A press on anything that is not text clears the selection.
            if self.workspace.reading.selection.take().is_some() {
                cx.notify();
            }
            return;
        };
        // Cmd-C belongs to the conversation once it is being read.
        window.focus(&self.focus, cx);
        let at = |offset: usize| Anchor {
            offset,
            ..hit.anchor.clone()
        };
        let local = hit.anchor.offset - hit.range.start;
        let picked = self.anchored_block(&hit.anchor).and_then(|block| {
            let text = block.text.get(hit.range.clone())?;
            let range = match clicks {
                0 | 1 => return None,
                2 => selection::word(text, local),
                _ if hit.by_line => selection::line(text, local),
                _ => 0..text.len(),
            };
            Some(hit.range.start + range.start..hit.range.start + range.end)
        });
        let previous = self
            .workspace
            .reading
            .selection
            .take()
            .filter(|selection| extend && selection.thread == thread);
        let (anchor, head) = match (picked, previous) {
            (Some(range), _) => (at(range.start), at(range.end)),
            (None, Some(selection)) => (selection.anchor, hit.anchor.clone()),
            (None, None) => (hit.anchor.clone(), hit.anchor.clone()),
        };
        self.workspace.reading.dragging = clicks <= 1;
        self.workspace.reading.pressed_link = hit.link.filter(|_| clicks <= 1 && !extend);
        self.workspace.reading.selection = Some(Selection {
            thread,
            anchor,
            head,
        });
        cx.notify();
    }

    fn drag_text(&mut self, hit: Option<Hit>, past: Pixels, cx: &mut Context<Self>) {
        if !self.workspace.reading.dragging {
            return;
        }
        // Dragging past an edge scrolls that way, so a selection can outgrow
        // the screen. Each pointer movement scrolls once; nothing is animated.
        if past != px(0.) {
            let step = past.clamp(px(-SCROLL_STEP), px(SCROLL_STEP));
            let offset = self.workspace.live_scroll.scroll_px_offset_for_scrollbar();
            self.workspace
                .live_scroll
                .set_offset_from_scrollbar(point(px(0.), offset.y - step));
            cx.notify();
        }
        let (Some(hit), Some(selection)) = (hit, &mut self.workspace.reading.selection) else {
            return;
        };
        if selection.head != hit.anchor {
            selection.head = hit.anchor;
            // A drag is no longer a click on the link it may have started on.
            self.workspace.reading.hovered_link = None;
            cx.notify();
        }
    }

    fn release_text(&mut self, link: Option<SharedString>, cx: &mut Context<Self>) {
        self.workspace.reading.dragging = false;
        let pressed = self.workspace.reading.pressed_link.take();
        let empty = self
            .workspace
            .reading
            .selection
            .as_ref()
            .is_some_and(|selection| selection.anchor == selection.head);
        if !empty {
            return;
        }
        self.workspace.reading.selection = None;
        // A click, not a drag, that began and ended on the same link.
        if let Some(url) = pressed.filter(|url| link.as_ref() == Some(url))
            && markdown::safe_url(&url).is_some()
        {
            cx.open_url(&url);
        }
        cx.notify();
    }

    fn hover_link(&mut self, link: Option<SharedString>, cx: &mut Context<Self>) {
        if self.workspace.reading.hovered_link != link {
            self.workspace.reading.hovered_link = link;
            cx.notify();
        }
    }

    pub(super) fn clear_reading_selection(&mut self) {
        self.workspace.reading.selection = None;
        self.workspace.reading.span.set(None);
        self.workspace.reading.dragging = false;
    }

    /// The text the current selection would copy, if there is one.
    pub(super) fn selected_text(&self) -> Option<String> {
        let span = self.workspace.reading.span.get()?;
        let snapshot = self.conversation_snapshot()?;
        let layout = &self.workspace.live_layout;
        let rows = layout
            .slots
            .get(span.start.row..=span.end.row)?
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                Some((span.start.row + i, transcript::resolve(snapshot, *slot)?))
            });
        let blocks = rows.flat_map(|(index, row)| {
            self.row_blocks(row)
                .into_iter()
                .enumerate()
                .map(move |(block, content)| (index, block, content))
        });
        Some(selection::text(blocks, &span)).filter(|text| !text.is_empty())
    }

    fn selected_reply(&self) -> Option<&str> {
        let selection = self.workspace.reading.selection.as_ref()?;
        if self.settings_open
            || self.workspace.tab != super::Workspace::Conversation
            || self.host.thread.selected.as_deref() != Some(selection.thread.as_str())
            || self.conversation_snapshot()?.id != selection.thread
            || selection.anchor.owner != selection.head.owner
        {
            return None;
        }
        let Owner::Message(id) = &selection.anchor.owner else {
            return None;
        };
        let row = *self.workspace.live_layout.row_of_message.get(id.as_ref())?;
        (self.workspace.reading.span.get().is_some() && self.work_open_at(row) != Some(false))
            .then_some(id.as_ref())
    }

    pub(super) fn can_quote_reply(&self, id: &str, cx: &App) -> bool {
        self.selected_reply() == Some(id)
            && self
                .host
                .thread
                .selected
                .as_ref()
                .and_then(|thread| self.workspace.composers.get(thread))
                .is_some_and(|composer| composer.read(cx).accepts_quote())
    }

    pub(super) fn quote_reply_selection(
        &mut self,
        expected: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.resolve_selection();
        let Some(reply) = self.selected_reply() else {
            return false;
        };
        if expected.is_some_and(|expected| expected != reply) {
            return false;
        }
        let Some(snapshot) = self.conversation_snapshot() else {
            return false;
        };
        if !snapshot.messages.iter().any(|message| {
            message.id == reply && message.role == plait_client::host::MessageRole::Assistant
        }) {
            return false;
        }
        let composer = self
            .host
            .thread
            .selected
            .as_ref()
            .and_then(|thread| self.workspace.composers.get(thread))
            .cloned();
        let (Some(text), Some(composer)) = (self.selected_text(), composer) else {
            return false;
        };
        if !composer.update(cx, |composer, cx| {
            composer.quote_selection(&text, window, cx)
        }) {
            return false;
        }
        self.clear_reading_selection();
        cx.notify();
        true
    }

    pub(super) fn on_selection_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_action(cx.listener(|this, _: &HistoryStart, window, cx| {
            if this.settings_open
                || this.workspace.tab != super::Workspace::Conversation
                || this.host.thread.selected.is_none()
            {
                cx.propagate();
                return;
            }
            this.workspace.live_scroll.scroll_to(ListOffset::default());
            this.load_history(super::history::Load::Older, window, cx);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &HistoryEnd, window, cx| {
            if this.settings_open
                || this.workspace.tab != super::Workspace::Conversation
                || this.host.thread.selected.is_none()
            {
                cx.propagate();
                return;
            }
            this.jump_latest(window, cx);
        }))
        .on_action(cx.listener(
            |this, _: &CopySelection, _, cx| match this.selected_text() {
                Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                None => cx.propagate(),
            },
        ))
        .on_action(cx.listener(|this, _: &QuoteSelection, window, cx| {
            if !this.quote_reply_selection(None, window, cx) {
                cx.propagate();
            }
        }))
    }

    /// The middle of the caret before `offset` of a block, where it was last
    /// drawn: for pointer tests, and for scrolling a found match into view.
    pub(super) fn text_point(
        &self,
        owner: &Owner,
        block: usize,
        offset: usize,
    ) -> Option<Point<Pixels>> {
        let regions = self.workspace.reading.regions.borrow();
        let region = regions.iter().find(|region| {
            region.site.owner == *owner
                && region.site.block == block
                && region.range.start <= offset
                && offset <= region.range.end
        })?;
        let caret = region
            .layout
            .position_for_index(offset - region.range.start)?;
        Some(point(
            caret.x + px(0.5),
            caret.y + region.layout.line_height() * 0.5,
        ))
    }
}

/// The neutral background of selected text.
pub(super) fn selection_fill() -> Hsla {
    rgba(colors().selection).into()
}
