//! A search field over a ranked list, shown as a centered overlay.
//!
//! The palette does not know what its items do. A caller supplies items and
//! receives the chosen item's command, so a new source needs no change here.
//! Ranking takes plain data and returns rows; it runs on the background
//! executor so a large list cannot delay typing.

use super::{
    BackTab, Tab,
    commands::Command,
    text_field::{TextField, TextFieldEvent},
    theme::*,
};
use gpui::{prelude::*, *};
use plait_core::fuzzy::fuzzy_match;
use std::{collections::HashSet, ops::Range, rc::Rc, sync::Arc};

/// Rows shown before the list scrolls.
const VISIBLE_ROWS: usize = 12;

actions!(
    palette,
    [Up, Down, PageUp, PageDown, Confirm, Aside, Dismiss, Mark]
);

pub(super) fn bind_keys(cx: &mut App) {
    let context = Some("PlaitPalette");
    cx.bind_keys([
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("pageup", PageUp, context),
        KeyBinding::new("pagedown", PageDown, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("cmd-enter", Aside, context),
        KeyBinding::new("escape", Dismiss, context),
        KeyBinding::new("cmd-d", Mark, context),
    ]);
}

/// One choice. Build with [`Item::new`] and the chained setters.
#[derive(Clone)]
pub(super) struct Item {
    /// Stable identity. The same key in several sections is one choice listed
    /// more than once; a search shows it once.
    pub key: SharedString,
    pub title: SharedString,
    /// Secondary text. Searched as well as the title.
    pub detail: SharedString,
    /// Heading the item sits under before a search is typed.
    pub section: SharedString,
    pub glyph: Option<(&'static str, u32)>,
    /// Trailing text such as a shortcut. Not searched.
    pub hint: SharedString,
    /// A state in words, with its accent.
    pub badge: Option<(SharedString, u32)>,
    /// A disabled item is listed and announced, but cannot be chosen.
    pub enabled: bool,
    /// `Some` adds a toggle to the row; the value is whether it is on.
    pub marked: Option<bool>,
    /// Label of a second thing the row can do, shown as a button on it.
    pub aside: Option<SharedString>,
    pub command: Command,
}

impl Item {
    pub fn new(
        key: impl Into<SharedString>,
        title: impl Into<SharedString>,
        command: Command,
    ) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            detail: SharedString::default(),
            section: SharedString::default(),
            glyph: None,
            hint: SharedString::default(),
            badge: None,
            enabled: true,
            marked: None,
            aside: None,
            command,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn section(mut self, section: impl Into<SharedString>) -> Self {
        self.section = section.into();
        self
    }

    pub fn glyph(mut self, glyph: &'static str, tint: u32) -> Self {
        self.glyph = Some((glyph, tint));
        self
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn badge(mut self, text: impl Into<SharedString>, tint: u32) -> Self {
        self.badge = Some((text.into(), tint));
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn marked(mut self, marked: bool) -> Self {
        self.marked = Some(marked);
        self
    }

    pub fn aside(mut self, label: impl Into<SharedString>) -> Self {
        self.aside = Some(label.into());
        self
    }

    /// What assistive technology reads for the row.
    fn spoken(&self) -> String {
        let mut parts = vec![self.title.to_string()];
        if !self.detail.is_empty() {
            parts.push(self.detail.to_string());
        }
        if let Some((badge, _)) = &self.badge {
            parts.push(badge.to_string());
        }
        if !self.hint.is_empty() {
            parts.push(self.hint.to_string());
        }
        parts.join(", ")
    }
}

/// The searchable part of an item: plain data that can cross to another thread.
#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub key: SharedString,
    pub title: SharedString,
    pub detail: SharedString,
    pub section: SharedString,
    pub enabled: bool,
}

impl From<&Item> for Entry {
    fn from(item: &Item) -> Self {
        Self {
            key: item.key.clone(),
            title: item.title.clone(),
            detail: item.detail.clone(),
            section: item.section.clone(),
            enabled: item.enabled,
        }
    }
}

/// One line of the list. `index` points into the items; the offsets are the
/// matched characters to emphasize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Heading(SharedString),
    Item {
        index: usize,
        title: Vec<usize>,
        detail: Vec<usize>,
    },
}

impl Row {
    fn item(&self) -> Option<usize> {
        match self {
            Self::Item { index, .. } => Some(*index),
            Self::Heading(_) => None,
        }
    }
}

/// The rows for a query. With nothing typed, every item in the given order
/// under its section heading. Otherwise the matches, best first.
pub(super) fn arrange(entries: &[Entry], query: &str) -> Vec<Row> {
    let terms = query.split_whitespace().collect::<Vec<_>>();
    if terms.is_empty() {
        grouped(entries)
    } else {
        ranked(entries, &terms)
    }
}

fn grouped(entries: &[Entry]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(entries.len() + 8);
    let mut section = None;
    for (index, entry) in entries.iter().enumerate() {
        if section != Some(&entry.section) {
            section = Some(&entry.section);
            if !entry.section.is_empty() {
                rows.push(Row::Heading(entry.section.clone()));
            }
        }
        rows.push(Row::Item {
            index,
            title: Vec::new(),
            detail: Vec::new(),
        });
    }
    rows
}

struct Found {
    index: usize,
    score: i32,
    title: Vec<usize>,
    detail: Vec<usize>,
}

/// Every term must match the title or the detail; each counts where it fits best.
fn find(index: usize, entry: &Entry, terms: &[&str]) -> Option<Found> {
    let mut found = Found {
        index,
        score: 0,
        title: Vec::new(),
        detail: Vec::new(),
    };
    for term in terms {
        let title = fuzzy_match(term, &entry.title);
        let detail = fuzzy_match(term, &entry.detail).filter(|_| !entry.detail.is_empty());
        let (matched, positions) = match (title, detail) {
            (Some(title), Some(detail)) if detail.score > title.score => {
                (detail, &mut found.detail)
            }
            (Some(title), _) => (title, &mut found.title),
            (None, Some(detail)) => (detail, &mut found.detail),
            (None, None) => return None,
        };
        found.score += matched.score;
        positions.extend(matched.positions);
    }
    for positions in [&mut found.title, &mut found.detail] {
        positions.sort_unstable();
        positions.dedup();
    }
    Some(found)
}

/// Usable items first, then by score. Ties go to the shorter title, then to
/// the caller's order, so the list does not shuffle between keystrokes.
fn ranked(entries: &[Entry], terms: &[&str]) -> Vec<Row> {
    let mut seen = HashSet::new();
    let mut found = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| find(index, entry, terms))
        .filter(|found| seen.insert(&entries[found.index].key))
        .collect::<Vec<_>>();
    found.sort_by(|a, b| {
        let (first, second) = (&entries[a.index], &entries[b.index]);
        second
            .enabled
            .cmp(&first.enabled)
            .then(b.score.cmp(&a.score))
            .then(first.title.len().cmp(&second.title.len()))
            .then(a.index.cmp(&b.index))
    });
    found
        .into_iter()
        .map(|found| Row::Item {
            index: found.index,
            title: found.title,
            detail: found.detail,
        })
        .collect()
}

/// The item row `count` items from `from`. A single step wraps around the
/// list; a page stops at the ends. Headings are never selected.
pub(super) fn step(
    rows: &[Row],
    from: Option<usize>,
    forward: bool,
    count: usize,
) -> Option<usize> {
    let items = (0..rows.len())
        .filter(|row| rows[*row].item().is_some())
        .collect::<Vec<_>>();
    let last = items.len().checked_sub(1)?;
    let Some(current) = from.and_then(|from| items.iter().position(|row| *row == from)) else {
        return Some(items[if forward { 0 } else { last }]);
    };
    let next = match (forward, count) {
        (true, 1) => (current + 1) % items.len(),
        (false, 1) => (current + last) % items.len(),
        (true, _) => (current + count).min(last),
        (false, _) => current.saturating_sub(count),
    };
    Some(items[next])
}

/// Where the selection starts: the preferred item, else the first usable one.
pub(super) fn start(rows: &[Row], entries: &[Entry], preferred: Option<&str>) -> Option<usize> {
    let row_of = |wanted: &dyn Fn(&Entry) -> bool| {
        rows.iter()
            .position(|row| row.item().is_some_and(|index| wanted(&entries[index])))
    };
    preferred
        .and_then(|key| row_of(&|entry| entry.key.as_ref() == key))
        .or_else(|| row_of(&|entry| entry.enabled))
        .or_else(|| row_of(&|_| true))
}

/// The row showing `key` that is closest to `near`, to follow an item when the list changes.
fn nearest(rows: &[Row], entries: &[Entry], key: &str, near: usize) -> Option<usize> {
    (0..rows.len())
        .filter(|row| {
            rows[*row]
                .item()
                .is_some_and(|index| entries[index].key.as_ref() == key)
        })
        .min_by_key(|row| row.abs_diff(near))
}

/// Matched character offsets merged into byte ranges of `text`.
pub(super) fn runs(text: &str, positions: &[usize]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for start in positions {
        let Some(ch) = text.get(*start..).and_then(|rest| rest.chars().next()) else {
            continue;
        };
        let end = start + ch.len_utf8();
        match runs.last_mut() {
            Some(run) if run.end == *start => run.end = end,
            _ => runs.push(*start..end),
        }
    }
    runs
}

/// Matched characters are heavier, not tinted: accents stay reserved for state.
fn emphasized(text: &SharedString, positions: &[usize]) -> StyledText {
    let style = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    StyledText::new(text.clone())
        .with_highlights(runs(text, positions).into_iter().map(|run| (run, style)))
}

pub(super) struct Options {
    /// Accessible name of the dialog and its search field.
    pub name: SharedString,
    pub placeholder: SharedString,
    /// Shown in place of the list when there are no items at all.
    pub empty: SharedString,
    /// Key of the item to start on.
    pub initial: Option<SharedString>,
    /// What Cmd-D toggles on a marked item, for example "Favorite".
    pub mark: Option<SharedString>,
    /// What Cmd-Enter does on an item with an aside, for example "Set default".
    pub aside: Option<SharedString>,
    /// A standing remark in the footer, such as why nothing can be chosen.
    pub note: Option<SharedString>,
}

/// Builds an item from the typed text, such as "Create branch <text>", given
/// the caller's items; `None` offers nothing. The item is listed first, and
/// `true` also starts the selection on it rather than on the best match.
pub(super) type Typed = Rc<dyn Fn(&str, &[Item]) -> Option<(Item, bool)>>;

pub(super) enum PaletteEvent {
    /// An enabled item was chosen.
    Run(Command),
    /// The toggle of a marked item was used. The caller answers with `set_items`.
    Mark(Command),
    /// The item's second action was used. The palette stays open.
    Aside(Command),
    /// Close the overlay. `restore` is false when focus already moved elsewhere.
    Dismiss { restore: bool },
}

pub(super) struct Palette {
    options: Options,
    field: Entity<TextField>,
    /// The caller's items, then the typed item when there is one.
    items: Vec<Item>,
    entries: Arc<[Entry]>,
    /// How many of `items` are the caller's.
    base: usize,
    typed: Option<Typed>,
    /// The typed item takes the selection when it appears.
    typed_leads: bool,
    rows: Vec<Row>,
    /// A row index; always an item row.
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    /// Counts queries, so only the newest ranking is shown.
    generation: u64,
    /// The query generation `rows` belongs to.
    arranged: u64,
    /// The control that opened this picker, so it opens beside that control
    /// instead of at the top of the window.
    pub(super) anchor: Option<Bounds<Pixels>>,
    _subscriptions: [Subscription; 2],
}

impl Palette {
    pub fn new(
        options: Options,
        items: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            TextField::new(options.name.clone(), options.placeholder.clone(), cx).search()
        });
        let focus = field.focus_handle(cx);
        let subscriptions = [
            cx.subscribe(&field, |this, _, _: &TextFieldEvent, cx| this.requery(cx)),
            // Focus left for something behind the overlay, such as a shortcut
            // that selected a thread. Step aside without pulling focus back.
            cx.on_blur(&focus, window, |_, _, cx| {
                cx.emit(PaletteEvent::Dismiss { restore: false })
            }),
        ];
        let entries = items.iter().map(Entry::from).collect::<Arc<[_]>>();
        let rows = arrange(&entries, "");
        let this = Self {
            selected: start(&rows, &entries, options.initial.as_deref()),
            options,
            field,
            base: items.len(),
            items,
            entries,
            typed: None,
            typed_leads: false,
            rows,
            scroll: UniformListScrollHandle::new(),
            generation: 0,
            arranged: 0,
            anchor: None,
            _subscriptions: subscriptions,
        };
        this.reveal();
        this
    }

    /// Whether the caller supplied no items; a typed item does not count.
    pub fn is_empty(&self) -> bool {
        self.base == 0
    }

    /// Offer an item built from the typed text, ahead of the matches.
    pub fn set_typed(&mut self, typed: Typed, cx: &mut Context<Self>) {
        self.typed = Some(typed);
        self.requery(cx);
    }

    /// Put the typed item, if any, after the caller's items for `query`.
    fn place_typed(&mut self, query: &str) {
        // Most palettes offer nothing typed; their entries stay as built.
        let Some(typed) = self.typed.clone() else {
            return;
        };
        self.items.truncate(self.base);
        let typed = typed(query, &self.items);
        self.typed_leads = typed.as_ref().is_some_and(|(_, leads)| *leads);
        self.items.extend(typed.map(|(item, _)| item));
        self.entries = self.items.iter().map(Entry::from).collect();
    }

    /// The typed item's row first, above the ranked caller items.
    fn with_typed(&self, mut rows: Vec<Row>) -> Vec<Row> {
        if self.items.len() > self.base {
            rows.insert(
                0,
                Row::Item {
                    index: self.base,
                    title: Vec::new(),
                    detail: Vec::new(),
                },
            );
        }
        rows
    }

    /// Replace the items, keeping the query and following the selected item.
    pub fn set_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
        let followed = self.selected.and_then(|row| {
            let index = self.rows.get(row)?.item()?;
            Some((self.entries[index].key.clone(), row))
        });
        self.base = items.len();
        self.entries = items.iter().map(Entry::from).collect();
        self.items = items;
        let query = self.field.read(cx).text().to_owned();
        self.place_typed(&query);
        self.generation += 1;
        self.arranged = self.generation;
        self.rows = self.with_typed(arrange(&self.entries[..self.base], &query));
        self.selected = followed
            .and_then(|(key, row)| nearest(&self.rows, &self.entries, &key, row))
            .or_else(|| start(&self.rows, &self.entries, None));
        if let Some(row) = self.selected {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// Empty the search field, as when the picker shows another folder.
    pub fn clear_query(&mut self, cx: &mut Context<Self>) {
        if !self.field.read(cx).text().is_empty() {
            self.field.update(cx, |field, cx| field.set_text("", cx));
        }
    }

    /// Replace the search text, as typing would. Scenes use it; drop the
    /// attribute when a caller needs to open the palette with text in it.
    #[cfg(test)]
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.set_text(query, cx));
    }

    /// Titles of the listed items, top to bottom, for tests of real key input.
    #[cfg(test)]
    pub fn shown(&self) -> Vec<String> {
        let items = self.rows.iter().filter_map(Row::item);
        items
            .map(|index| self.items[index].title.to_string())
            .collect()
    }

    pub fn set_note(&mut self, note: Option<SharedString>, cx: &mut Context<Self>) {
        self.options.note = note;
        cx.notify();
    }

    /// Rank for the current text. Matching runs off the UI thread; an empty
    /// query only groups, which is cheap enough to do here.
    fn requery(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let query = self.field.read(cx).text().to_owned();
        self.place_typed(&query);
        if query.trim().is_empty() {
            let rows = arrange(&self.entries[..self.base], "");
            return self.show(rows, generation, false, cx);
        }
        let (entries, base) = (self.entries.clone(), self.base);
        let ranking = cx
            .background_executor()
            .spawn(async move { arrange(&entries[..base], &query) });
        cx.spawn(async move |this, cx| {
            let rows = ranking.await;
            let _ = this.update(cx, |this, cx| {
                // A newer keystroke, or Enter, already replaced this result.
                if this.generation == generation && this.arranged != generation {
                    this.show(rows, generation, true, cx);
                }
            });
        })
        .detach();
    }

    /// `searched` rows start on the best match; the preferred item is for the full list.
    fn show(&mut self, rows: Vec<Row>, generation: u64, searched: bool, cx: &mut Context<Self>) {
        let initial = self.options.initial.as_deref().filter(|_| !searched);
        let typed = self.items.len() > self.base;
        let rows = self.with_typed(rows);
        self.selected = if typed && !self.typed_leads && rows.len() > 1 {
            // The best match, below the typed item.
            start(&rows[1..], &self.entries, None).map(|row| row + 1)
        } else {
            start(&rows, &self.entries, initial)
        };
        self.rows = rows;
        self.arranged = generation;
        self.reveal();
        cx.notify();
    }

    /// Scroll to the selection. The first item also shows the heading above it.
    fn reveal(&self) {
        let first = step(&self.rows, None, true, 1);
        match self.selected {
            Some(row) if self.selected != first => {
                self.scroll.scroll_to_item(row, ScrollStrategy::Center)
            }
            _ => self.scroll.scroll_to_item(0, ScrollStrategy::Top),
        }
    }

    pub(super) fn move_by(&mut self, forward: bool, count: usize, cx: &mut Context<Self>) {
        self.selected = step(&self.rows, self.selected, forward, count);
        if let Some(row) = self.selected {
            // Moving up to the first item should bring its heading back too.
            let first = step(&self.rows, None, true, 1) == Some(row);
            self.scroll
                .scroll_to_item(if first { 0 } else { row }, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn item_at(&self, row: usize) -> Option<&Item> {
        self.items.get(self.rows.get(row)?.item()?)
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        // Enter commits a composition first; the input method owns that key.
        if self.field.read(cx).is_composing() {
            return;
        }
        // Enter can arrive before the background ranking does. Rank now, so
        // the choice always matches the text in the field.
        if self.arranged != self.generation {
            let rows = arrange(&self.entries[..self.base], self.field.read(cx).text());
            self.show(rows, self.generation, true, cx);
        }
        if let Some(row) = self.selected {
            self.choose(row, cx);
        }
    }

    fn choose(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(item) = self.item_at(row).filter(|item| item.enabled) else {
            return;
        };
        cx.emit(PaletteEvent::Run(item.command.clone()));
    }

    fn mark(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(item) = self.item_at(row).filter(|item| item.marked.is_some()) else {
            return;
        };
        let command = item.command.clone();
        self.selected = Some(row);
        cx.emit(PaletteEvent::Mark(command));
        cx.notify();
    }

    fn aside(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(item) = self.item_at(row).filter(|item| item.aside.is_some()) else {
            return;
        };
        let command = item.command.clone();
        self.selected = Some(row);
        cx.emit(PaletteEvent::Aside(command));
        cx.notify();
    }

    /// A quiet text button for the row's second action.
    fn aside_button(&self, row: usize, label: SharedString, cx: &mut Context<Self>) -> Control {
        let entity = cx.weak_entity();
        div()
            .id(("palette-aside", row))
            .role(Role::Button)
            .aria_label(label.clone())
            .audited()
            .flex_shrink_0()
            .px(px(6.))
            .h(px(20.))
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(rgb(colors().edge))
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(rgb(colors().muted))
            .hover(|style| {
                style
                    .bg(rgba(colors().hover))
                    .text_color(rgb(colors().text))
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                // The row underneath would otherwise run the item.
                cx.stop_propagation();
                this.aside(row, cx);
            }))
            .on_a11y_action(AccessibleAction::Click, move |_, _, cx| {
                let _ = entity.update(cx, |this, cx| this.aside(row, cx));
            })
    }

    fn heading(&self, row: usize, text: &SharedString) -> AnyElement {
        div()
            .id(("palette-heading", row))
            .role(Role::Heading)
            .aria_label(text.clone())
            .audited()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_2()
            .pb(px(5.))
            .flex()
            .items_end()
            .child(section_label("palette-heading-label", text.clone()))
            .into_any_element()
    }

    fn toggle(&self, row: usize, item: &Item, on: bool, cx: &mut Context<Self>) -> Control {
        let name = self.options.mark.clone().unwrap_or_else(|| "Mark".into());
        let entity = cx.weak_entity();
        div()
            .id(("palette-mark", row))
            .role(Role::Button)
            .aria_label(format!("{name}: {}", item.title))
            .aria_toggled(if on { Toggled::True } else { Toggled::False })
            .audited()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(20.))
            .rounded_md()
            .cursor_pointer()
            .hover(|style| style.bg(rgba(colors().hover)))
            // Filled or outlined: the shape carries the state, not the tint.
            .child(
                icon(if on { "star-filled" } else { "star" })
                    .size(px(12.))
                    .text_color(rgb(if on { colors().text } else { colors().subtle })),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                // The row underneath would otherwise run the item.
                cx.stop_propagation();
                this.mark(row, cx);
            }))
            .on_a11y_action(AccessibleAction::Click, move |_, _, cx| {
                let _ = entity.update(cx, |this, cx| this.mark(row, cx));
            })
    }

    fn item_row(
        &self,
        row: usize,
        item: &Item,
        title: &[usize],
        detail: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected == Some(row);
        let hover = item.enabled && !selected;
        row_base(row, item, selected)
            .when(hover, |el| el.hover(|style| style.bg(rgba(colors().hover))))
            .when_some(item.glyph, |el, (glyph, tint)| {
                el.child(icon(glyph).text_color(rgb(tint)))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(relative(0.7))
                    .truncate()
                    .text_color(rgb(if item.enabled {
                        colors().text
                    } else {
                        colors().muted
                    }))
                    .child(emphasized(&item.title, title)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(rgb(colors().subtle))
                    .child(emphasized(&item.detail, detail)),
            )
            .when_some(item.badge.clone(), |el, (badge, tint)| {
                el.child(tag("palette-row-badge", badge, tint).flex_shrink_0())
            })
            .when_some(item.aside.clone(), |el, label| {
                el.child(self.aside_button(row, label, cx))
            })
            .when_some(item.marked, |el, on| {
                el.child(self.toggle(row, item, on, cx))
            })
            .when(!item.hint.is_empty(), |el| {
                el.child(
                    tag("palette-row-hint", item.hint.clone(), colors().subtle).flex_shrink_0(),
                )
            })
            .when(item.enabled, |el| {
                el.cursor_pointer()
                    .on_activate(cx, move |this, _, cx| this.choose(row, cx))
            })
            .into_any_element()
    }

    fn render_row(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        match self.rows.get(row) {
            Some(Row::Heading(text)) => self.heading(row, text),
            Some(Row::Item {
                index,
                title,
                detail,
            }) => match self.items.get(*index) {
                Some(item) => self.item_row(row, item, title, detail, cx),
                None => div().into_any_element(),
            },
            None => div().into_any_element(),
        }
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.items.is_empty() {
            return message(self.options.empty.clone());
        }
        if self.rows.is_empty() {
            return message("No matches. Try fewer letters.".into());
        }
        let count = self.rows.len();
        div()
            .id("palette-list")
            .role(Role::ListBox)
            .aria_label(format!("{} results", self.options.name))
            .audited()
            .py_1()
            .child(
                uniform_list(
                    "palette-rows",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| {
                        // A palette opens above everything, a settings modal included.
                        let _above = super::theme::AboveModal::enter();
                        range
                            // The list sizes a bare row to its content; this
                            // gives every row the panel's width and a margin.
                            .map(|row| div().w_full().px_1().child(this.render_row(row, cx)))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .w_full()
                .h(px(ROW_HEIGHT * count.min(VISIBLE_ROWS) as f32)),
            )
            .into_any_element()
    }

    fn footer(&self, searching: bool) -> impl IntoElement {
        // The typed item is an offer, not a match.
        let matches = self
            .rows
            .iter()
            .filter(|row| row.item().is_some_and(|index| index < self.base))
            .count();
        let offered = self.items.len() > self.base;
        let status = self.options.note.clone().or_else(|| {
            // With only the typed item, "0 matches" would contradict it.
            (searching && !(offered && matches == 0)).then(|| match matches {
                1 => "1 match".into(),
                count => format!("{count} matches").into(),
            })
        });
        let mut keys = String::from("↑↓ Move · ↵ Choose");
        if let Some(aside) = &self.options.aside {
            keys.push_str(&format!(" · ⌘↵ {aside}"));
        }
        if let Some(mark) = &self.options.mark {
            keys.push_str(&format!(" · ⌘D {mark}"));
        }
        keys.push_str(" · esc Close");
        row()
            .h(px(ROW_HEIGHT))
            .px_3()
            .gap_3()
            .flex_shrink_0()
            .justify_between()
            .border_t_1()
            .border_color(rgb(colors().border))
            .child(div().min_w_0().truncate().when_some(status, |el, status| {
                el.child(tag("palette-footer-status", status, colors().muted))
            }))
            .child(
                tag("palette-footer-keys", keys, colors().subtle)
                    .flex_shrink_0()
                    .whitespace_nowrap(),
            )
    }
}

/// The list row without its content: identity, selection, and state.
fn row_base(row: usize, item: &Item, selected: bool) -> Control {
    self::row()
        .id(("palette-row", row))
        .role(Role::ListBoxOption)
        .aria_label(item.spoken())
        .aria_selected(selected)
        .audited()
        // Focus stays in the search field; the selected row reports as focused.
        .when(selected, |el| el.aria_active_descendant())
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_2()
        .rounded_md()
        .text_size(px(13.))
        .when(selected, |el| el.bg(rgb(colors().selected)))
        .disabled(!item.enabled)
}

fn message(text: SharedString) -> AnyElement {
    div()
        .px_4()
        .py_6()
        .text_size(px(12.))
        .line_height(px(19.))
        .text_color(rgb(colors().muted))
        .child(a11y_text("palette-empty-message", text))
        .into_any_element()
}

impl Focusable for Palette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.focus_handle(cx)
    }
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _above = super::theme::AboveModal::enter();
        let searching = !self.field.read(cx).text().trim().is_empty();
        let panel = column()
            .id("palette")
            .role(Role::Dialog)
            .aria_label(self.options.name.clone())
            .audited()
            .key_context("PlaitPalette")
            .on_action(cx.listener(|this, _: &Up, _, cx| this.move_by(false, 1, cx)))
            .on_action(cx.listener(|this, _: &Down, _, cx| this.move_by(true, 1, cx)))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.move_by(false, VISIBLE_ROWS, cx)))
            .on_action(
                cx.listener(|this, _: &PageDown, _, cx| this.move_by(true, VISIBLE_ROWS, cx)),
            )
            .on_action(cx.listener(|this, _: &Confirm, _, cx| this.confirm(cx)))
            .on_action(
                cx.listener(|_, _: &Dismiss, _, cx| {
                    cx.emit(PaletteEvent::Dismiss { restore: true })
                }),
            )
            .on_action(cx.listener(|this, _: &Aside, _, cx| {
                if let Some(row) = this.selected {
                    this.aside(row, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Mark, _, cx| {
                if let Some(row) = this.selected {
                    this.mark(row, cx);
                }
            }))
            // The field is the overlay's one tab stop; Tab must not reach the
            // window behind it.
            .on_action(cx.listener(|_, _: &Tab, _, _| {}))
            .on_action(cx.listener(|_, _: &BackTab, _, _| {}))
            .on_mouse_down_out(
                cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismiss { restore: true })),
            )
            .w_full()
            .max_w(px(620.))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors().edge))
            .bg(rgb(colors().rail))
            .child(div().p_2().child(self.field.clone()))
            .child(self.list(cx))
            .child(self.footer(searching));
        // A full-window layer: it blocks the workspace behind and dims it,
        // because the panel has no shadow to lift it.
        let layer = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(rgba(colors().scrim));
        let Some(anchor) = self.anchor else {
            return layer
                .flex()
                .flex_col()
                .items_center()
                .px_6()
                .pt(px(72.))
                .pb_6()
                .child(panel);
        };
        let place = anchored_place(anchor, window.viewport_size());
        let panel = panel.max_w(place.width).max_h(place.height);
        let positioned = div()
            .absolute()
            .left(place.left)
            .w(place.width)
            .flex()
            .flex_col()
            .when(place.above, |el| el.bottom(place.edge).justify_end())
            .when(!place.above, |el| el.top(place.edge))
            .child(panel);
        layer.child(positioned)
    }
}

/// Where an anchored picker goes: beside its control, on the side with more
/// room, kept inside the window.
#[derive(Debug, PartialEq)]
struct Place {
    left: Pixels,
    width: Pixels,
    /// Opening upward, `edge` is the distance from the window's bottom;
    /// otherwise from its top.
    above: bool,
    edge: Pixels,
    height: Pixels,
}

fn anchored_place(anchor: Bounds<Pixels>, window: Size<Pixels>) -> Place {
    const MARGIN: f32 = 16.;
    const GAP: f32 = 6.;
    let width = px(480.).min(window.width - px(2. * MARGIN)).max(px(240.));
    let left = anchor
        .left()
        .min(window.width - width - px(MARGIN))
        .max(px(MARGIN));
    let room_above = anchor.top() - px(MARGIN + GAP);
    let room_below = window.height - anchor.bottom() - px(MARGIN + GAP);
    let above = room_above > room_below;
    Place {
        left,
        width,
        above,
        edge: if above {
            window.height - anchor.top() + px(GAP)
        } else {
            anchor.bottom() + px(GAP)
        },
        height: if above { room_above } else { room_below }.max(px(160.)),
    }
}

#[cfg(test)]
mod tests {
    // Named imports: a glob would bring in GPUI's own `test` attribute.
    use super::{Entry, Row, anchored_place, arrange, nearest, runs, start, step};
    use gpui::{Bounds, point, px, size};

    #[core::prelude::v1::test]
    fn a_picker_opens_beside_its_control_and_stays_in_the_window() {
        let window = size(px(1200.), px(800.));
        // A chip under the composer: the picker opens upward from its top edge.
        let chip = Bounds::new(point(px(400.), px(740.)), size(px(120.), px(24.)));
        let place = anchored_place(chip, window);
        assert!(place.above);
        assert_eq!(place.left, px(400.));
        assert_eq!(place.edge, px(800. - 740. + 6.));
        assert_eq!(place.width, px(480.));
        // A control near the top: it opens downward from the control's bottom edge.
        let row = Bounds::new(point(px(300.), px(40.)), size(px(24.), px(24.)));
        let place = anchored_place(row, window);
        assert!(!place.above);
        assert_eq!(place.edge, px(70.));
        // Near the right edge it moves left to stay inside the window.
        let right = Bounds::new(point(px(1150.), px(740.)), size(px(40.), px(24.)));
        assert_eq!(anchored_place(right, window).left, px(1200. - 480. - 16.));
        // A narrow window shrinks the picker but keeps the margin.
        let narrow = anchored_place(chip, size(px(400.), px(800.)));
        assert_eq!(narrow.width, px(368.));
        assert_eq!(narrow.left, px(16.));
    }
    use std::ops::Range;

    fn entry(key: &str, title: &str, detail: &str, section: &str, enabled: bool) -> Entry {
        Entry {
            key: key.to_owned().into(),
            title: title.to_owned().into(),
            detail: detail.to_owned().into(),
            section: section.to_owned().into(),
            enabled,
        }
    }

    fn entries() -> Vec<Entry> {
        vec![
            entry(
                "t1",
                "Fix the flaky sync test",
                "Idle · p/m",
                "Threads",
                true,
            ),
            entry(
                "t2",
                "Settings page layout",
                "Running · p/m",
                "Threads",
                true,
            ),
            entry("a1", "Settings…", "Plait", "Commands", true),
            entry("a2", "Toggle Sidebar", "View", "Commands", true),
            entry("a3", "New Thread", "File", "Commands", true),
            entry(
                "m1",
                "New thread with GPT-5.6 Luna",
                "openai-codex/gpt-5.6-luna",
                "New thread",
                true,
            ),
            entry(
                "m2",
                "New thread with Claude Opus",
                "openrouter/anthropic/claude-opus",
                "New thread",
                true,
            ),
        ]
    }

    fn titles<'a>(entries: &'a [Entry], rows: &[Row]) -> Vec<&'a str> {
        rows.iter()
            .map(|row| match row {
                Row::Heading(text) => {
                    let section = entries.iter().find(|entry| entry.section == *text);
                    section.map_or("?", |entry| entry.section.as_ref())
                }
                Row::Item { index, .. } => entries[*index].title.as_ref(),
            })
            .collect()
    }

    #[test]
    fn an_empty_query_lists_everything_under_section_headings() {
        let entries = entries();
        for query in ["", "   "] {
            let rows = arrange(&entries, query);
            assert_eq!(
                titles(&entries, &rows),
                vec![
                    "Threads",
                    "Fix the flaky sync test",
                    "Settings page layout",
                    "Commands",
                    "Settings…",
                    "Toggle Sidebar",
                    "New Thread",
                    "New thread",
                    "New thread with GPT-5.6 Luna",
                    "New thread with Claude Opus",
                ]
            );
        }
        // Items without a section get no heading.
        let plain = vec![
            entry("a", "One", "", "", true),
            entry("b", "Two", "", "", true),
        ];
        assert_eq!(arrange(&plain, "").len(), 2);
        assert!(arrange(&[], "").is_empty());
    }

    #[test]
    fn a_query_ranks_matches_best_first_without_headings() {
        let entries = entries();
        let rows = arrange(&entries, "set");
        // The exact command outranks the longer thread title that starts the same way.
        assert_eq!(
            titles(&entries, &rows)[..2],
            ["Settings…", "Settings page layout"]
        );
        assert!(rows.iter().all(|row| row.item().is_some()));
        // Word starts beat letters scattered inside words.
        let rows = arrange(&entries, "nt");
        assert_eq!(titles(&entries, &rows)[0], "New Thread");
        assert!(arrange(&entries, "zzz").is_empty());
    }

    #[test]
    fn matches_report_the_characters_to_emphasize() {
        let entries = entries();
        let rows = arrange(&entries, "side");
        assert_eq!(
            rows[0],
            Row::Item {
                index: 3,
                title: vec![7, 8, 9, 10],
                detail: vec![],
            }
        );
        assert_eq!(runs("Toggle Sidebar", &[7, 8, 9, 10]), vec![7..11]);
        assert_eq!(runs("héllo", &[0, 1, 4]), vec![0..3, 4..5]);
        // Offsets that do not fit the text are skipped, not sliced.
        assert_eq!(runs("héllo", &[2, 99]), Vec::<Range<usize>>::new());
    }

    #[test]
    fn the_detail_is_searched_and_every_term_must_match() {
        let entries = entries();
        // A provider name appears only in the detail.
        let rows = arrange(&entries, "openrouter");
        assert_eq!(titles(&entries, &rows), vec!["New thread with Claude Opus"]);
        let Row::Item { title, detail, .. } = &rows[0] else {
            panic!("item row");
        };
        assert!(title.is_empty());
        assert_eq!(detail.len(), "openrouter".len());
        // Terms may match different fields, in any order.
        let rows = arrange(&entries, "opus openrouter");
        assert_eq!(titles(&entries, &rows), vec!["New thread with Claude Opus"]);
        let rows = arrange(&entries, "luna openrouter");
        assert!(rows.is_empty());
        // A space is a separator, not a character to find.
        assert_eq!(arrange(&entries, "gpt luna").len(), 1);
    }

    #[test]
    fn unusable_items_rank_after_every_usable_one() {
        let entries = vec![
            entry("off", "Opus", "p/opus", "Not signed in", false),
            entry(
                "on",
                "Claude Opus Latest",
                "q/claude-opus-latest",
                "q",
                true,
            ),
        ];
        let rows = arrange(&entries, "opus");
        assert_eq!(titles(&entries, &rows), vec!["Claude Opus Latest", "Opus"]);
    }

    #[test]
    fn a_choice_listed_in_several_sections_is_one_search_result() {
        let entries = vec![
            entry(
                "m",
                "GPT-5.6 Luna",
                "openai/gpt-5.6-luna",
                "Favorites",
                true,
            ),
            entry("m", "GPT-5.6 Luna", "openai/gpt-5.6-luna", "Recent", true),
            entry("m", "GPT-5.6 Luna", "openai/gpt-5.6-luna", "openai", true),
            entry("n", "GPT-5.6 Sol", "openai/gpt-5.6-sol", "openai", true),
        ];
        assert_eq!(arrange(&entries, "").len(), 7);
        let rows = arrange(&entries, "gpt");
        // The first listing stands for the model; the shorter title leads a tie.
        let found = rows.iter().filter_map(Row::item).collect::<Vec<_>>();
        assert_eq!(found, vec![3, 0]);
    }

    #[test]
    fn ties_keep_the_callers_order() {
        let entries = (0..50)
            .map(|index| entry(&format!("k{index}"), "Same title", "", "", true))
            .collect::<Vec<_>>();
        let rows = arrange(&entries, "same");
        let order = rows.iter().filter_map(Row::item).collect::<Vec<_>>();
        assert_eq!(order, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn arrows_skip_headings_and_wrap_while_pages_stop_at_the_ends() {
        let entries = entries();
        let rows = arrange(&entries, "");
        // Row 0 is a heading; the first item is row 1 and the last is row 9.
        assert_eq!(step(&rows, None, true, 1), Some(1));
        assert_eq!(step(&rows, None, false, 1), Some(9));
        assert_eq!(step(&rows, Some(2), true, 1), Some(4));
        assert_eq!(step(&rows, Some(4), false, 1), Some(2));
        assert_eq!(step(&rows, Some(9), true, 1), Some(1));
        assert_eq!(step(&rows, Some(1), false, 1), Some(9));
        assert_eq!(step(&rows, Some(1), true, 4), Some(6));
        assert_eq!(step(&rows, Some(6), true, 12), Some(9));
        assert_eq!(step(&rows, Some(4), false, 12), Some(1));
        // A stale selection restarts from the end the key points at.
        assert_eq!(step(&rows, Some(0), true, 1), Some(1));
        assert_eq!(step(&[], None, true, 1), None);
        assert_eq!(step(&[Row::Heading("Empty".into())], None, true, 1), None);
    }

    #[test]
    fn selection_starts_on_the_preferred_or_first_usable_item() {
        let entries = vec![
            entry("off", "Signed out", "", "A", false),
            entry("one", "One", "", "A", true),
            entry("two", "Two", "", "B", true),
        ];
        let rows = arrange(&entries, "");
        assert_eq!(start(&rows, &entries, None), Some(2));
        assert_eq!(start(&rows, &entries, Some("two")), Some(4));
        assert_eq!(start(&rows, &entries, Some("missing")), Some(2));
        // With nothing usable, the first item still takes the selection so it is announced.
        assert_eq!(start(&rows[..2], &entries, None), Some(1));
        assert_eq!(start(&[], &entries, None), None);
        // The item is followed to its nearest listing when the list changes.
        let twice = vec![
            entry("m", "Model", "", "Favorites", true),
            entry("x", "Other", "", "p", true),
            entry("m", "Model", "", "p", true),
        ];
        let rows = arrange(&twice, "");
        assert_eq!(nearest(&rows, &twice, "m", 4), Some(4));
        assert_eq!(nearest(&rows, &twice, "m", 0), Some(1));
        assert_eq!(nearest(&rows, &twice, "gone", 0), None);
    }

    #[test]
    fn a_large_catalog_ranks_within_a_frame_budget_of_work() {
        // The owner's catalog is about 1,600 models; rank ten times that.
        let entries = (0..16_000)
            .map(|index| {
                entry(
                    &format!("k{index}"),
                    &format!("Provider {}: Model {index} Preview", index % 44),
                    &format!("provider-{}/vendor/model-{index}-preview", index % 44),
                    "p",
                    index % 160 == 0,
                )
            })
            .collect::<Vec<_>>();
        let rows = arrange(&entries, "model 1599 prev");
        assert!(!rows.is_empty());
        // Usable matches come first even among thousands.
        let first_disabled = rows
            .iter()
            .position(|row| !entries[row.item().unwrap()].enabled)
            .unwrap_or(rows.len());
        assert!(
            rows[first_disabled..]
                .iter()
                .all(|row| !entries[row.item().unwrap()].enabled)
        );
    }
}
