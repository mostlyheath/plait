//! Find in the open conversation, with the Files tab's bar and keys.
//!
//! The search runs on the background executor over the projection the view
//! already holds (`formatted` and the rows it was built for), one search at a
//! time. A result for an older query or an older transcript is dropped and the
//! search runs again. Drawing a row looks up only that row's matches.

use super::{
    Plait,
    file_view::{FindIn, find_highlight},
    selection::{Owner, Site},
    text_field::{TextField, TextFieldEvent},
};
use gpui::{prelude::*, *};
use plait_client::conversation_find::{self, Matches};
use std::{ops::Range, sync::Arc};

/// How far inside the list's edges a match must sit to count as in view.
const REVEAL_MARGIN: f32 = 24.;

#[derive(Default)]
pub(super) struct ConversationFind {
    pub open: bool,
    pub field: Option<Entity<TextField>>,
    pub matches: Arc<Matches>,
    pub current: usize,
    pub(super) restore_match: Option<plait_client::tabs::bookmark::TextPosition>,
    /// A loaded message shows only part of its text.
    pub partial: bool,
    /// The query `matches` answer. A different one moves to a match; the
    /// same one over new rows keeps the current match and the scroll.
    query: String,
    /// Bumped by a new query or new rows, so a running search knows it is stale.
    generation: u64,
    /// One search at a time; a stale result starts the next.
    searching: bool,
    reveal: Option<Reveal>,
    _events: Option<Subscription>,
}

/// Bringing the current match into view takes a drawn frame: only drawn text
/// knows where a match is within its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reveal {
    /// The list was scrolled to the match's row; wait for it to be drawn.
    AfterDraw,
    /// The row has been drawn; place the match within the view.
    Measure,
}

/// What a finished search found, and what it was for.
pub(super) struct Found {
    pub generation: u64,
    pub query: String,
    pub transcript: Arc<plait_client::markdown::Transcript>,
    pub matches: Matches,
    pub partial: bool,
}

impl ConversationFind {
    pub(super) fn bookmark_match(&self) -> Option<plait_client::tabs::bookmark::TextPosition> {
        self.matches
            .hits
            .get(self.current)
            .map(|hit| plait_client::tabs::bookmark::TextPosition {
                source: hit.message.clone(),
                block: hit.block as u32,
                byte: hit.range.start as u32,
            })
    }
}

impl Plait {
    pub(super) fn restore_conversation_find(
        &mut self,
        field: Entity<TextField>,
        events: Subscription,
        open: bool,
    ) {
        self.workspace.conversation_find.field = Some(field);
        self.workspace.conversation_find._events = Some(events);
        self.workspace.conversation_find.open = open;
    }
    /// Cmd-F over the conversation: show the bar and search what is loaded.
    pub(super) fn open_conversation_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.thread.selected.is_none() {
            return;
        }
        let field = match self.workspace.conversation_find.field.clone() {
            Some(field) => field,
            None => {
                let field = cx.new(|cx| {
                    TextField::new("Find in conversation", "Find in conversation", cx).search()
                });
                self.workspace.conversation_find._events =
                    Some(cx.subscribe(&field, |this, _, _: &TextFieldEvent, cx| {
                        this.search_conversation(cx)
                    }));
                self.workspace.conversation_find.field = Some(field.clone());
                field
            }
        };
        let handle = field.focus_handle(cx);
        let find = &mut self.workspace.conversation_find;
        if !find.open {
            self.conversation_find_focus = window.focused(cx).filter(|focused| *focused != handle);
            find.open = true;
            // A query kept from last time is searched again over today's rows.
            find.query.clear();
            self.search_conversation(cx);
        }
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(super) fn close_conversation_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find = &mut self.workspace.conversation_find;
        find.open = false;
        find.matches = Arc::default();
        find.reveal = None;
        // Whatever search is running now answers nothing.
        find.generation += 1;
        let focus = self
            .conversation_find_focus
            .take()
            .unwrap_or_else(|| self.focus.clone());
        window.focus(&focus, cx);
        cx.notify();
    }

    /// The rows shown changed (a reply arrived, history loaded, another
    /// thread opened): search them again if the bar is open.
    pub(super) fn conversation_rows_changed(&mut self, cx: &mut Context<Self>) {
        if self.workspace.conversation_find.open {
            self.search_conversation(cx);
        }
    }

    /// Mark any running search stale and start one unless one is running.
    pub(super) fn search_conversation(&mut self, cx: &mut Context<Self>) {
        let find = &mut self.workspace.conversation_find;
        find.generation += 1;
        if !find.searching {
            self.start_search(cx);
        }
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        let owner = self.host.read_stamp();
        let find = &mut self.workspace.conversation_find;
        let query = match (&find.field, find.open) {
            (Some(field), true) => field.read(cx).text().to_owned(),
            _ => String::new(),
        };
        find.searching = true;
        let generation = find.generation;
        // Both are shared, already prepared, and replaced rather than changed.
        let snapshot = self
            .workspace
            .history
            .display
            .clone()
            .or_else(|| self.host.thread.snapshot.clone());
        let transcript = self.workspace.formatted.clone();
        let history = self.workspace.history.cache.clone();
        let cancelled = self.workspace.cancelled.clone();
        let task = cx.background_executor().spawn(async move {
            let matches = match &snapshot {
                Some(snapshot) => {
                    conversation_find::search_cancellable(snapshot, &transcript, &query, &|| {
                        cancelled.load(std::sync::atomic::Ordering::Acquire)
                    })
                }
                None => Matches::default(),
            };
            Found {
                generation,
                query,
                transcript,
                matches,
                partial: conversation_find::partial(&history),
            }
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                if !this.host.accepts_read(&owner) {
                    return;
                }
                this.workspace.conversation_find.searching = false;
                this.land_search(found, cx);
            });
        })
        .detach();
    }

    /// Show a finished search, unless the query or the rows changed since it
    /// started; then search again instead.
    pub(super) fn land_search(&mut self, found: Found, cx: &mut Context<Self>) {
        let find = &mut self.workspace.conversation_find;
        if found.generation != find.generation
            || !Arc::ptr_eq(&found.transcript, &self.workspace.formatted)
        {
            if find.open && !find.searching {
                self.start_search(cx);
            }
            return;
        }
        let typed = find.query != found.query;
        let previous = find.matches.hits.get(find.current).cloned();
        find.query = found.query;
        find.partial = found.partial;
        find.matches = Arc::new(found.matches);
        if let Some(saved) = find.restore_match.take() {
            find.current = find
                .matches
                .hits
                .iter()
                .position(|h| {
                    h.message == saved.source
                        && h.block == saved.block as usize
                        && h.range.start == saved.byte as usize
                })
                .unwrap_or(0);
            find.reveal = None;
            cx.notify();
            return;
        }
        let count = find.matches.len();
        if typed {
            // A new query starts at the first match from the top of the view,
            // or at the newest when the list rests on its end.
            let top = self.workspace.live_scroll.logical_scroll_top().item_ix;
            let find = &mut self.workspace.conversation_find;
            find.current = if top >= self.workspace.live_count {
                count.saturating_sub(1)
            } else {
                find.matches.first_from(top)
            };
            self.reveal_match();
        } else {
            // New rows keep the same match, and the list stays where it is.
            find.current = previous
                .and_then(|hit| find.matches.position(&hit))
                .unwrap_or(find.current.min(count.saturating_sub(1)));
        }
        cx.notify();
    }

    /// Enter, Cmd-G, and their Shift forms: the next or previous match, wrapping.
    pub(super) fn step_conversation_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        let find = &mut self.workspace.conversation_find;
        if !find.open || find.matches.is_empty() {
            return;
        }
        find.current = conversation_find::step(find.current, find.matches.len(), forward);
        self.reveal_match();
        cx.notify();
    }

    /// Scroll the current match into view, using where it was last drawn.
    /// A row not drawn yet is scrolled to first and placed after it is.
    fn reveal_match(&mut self) {
        let Some(hit) = self.current_hit() else {
            return;
        };
        if self.place_match(&hit) {
            self.workspace.conversation_find.reveal = None;
            return;
        }
        self.workspace.live_scroll.scroll_to(ListOffset {
            item_ix: hit.row,
            offset_in_item: px(0.),
        });
        self.workspace.conversation_find.reveal = Some(Reveal::AfterDraw);
    }

    fn current_hit(&self) -> Option<conversation_find::Hit> {
        let find = &self.workspace.conversation_find;
        find.matches.hits.get(find.current).cloned()
    }

    /// Scroll so the drawn match sits a third of the way down the list,
    /// unless it is in view already. False when the match was not drawn.
    fn place_match(&self, hit: &conversation_find::Hit) -> bool {
        let owner = Owner::Message(hit.message.clone().into());
        let Some(point) = self.text_point(&owner, hit.block, hit.range.start) else {
            return false;
        };
        let scroll = &self.workspace.live_scroll;
        let view = scroll.viewport_bounds();
        let margin = px(REVEAL_MARGIN);
        if point.y < view.top() + margin || point.y > view.bottom() - margin {
            // In pixels from the top: a list resting on its end reports its
            // whole height as its offset, so that is clamped to the real one.
            let top = (-scroll.scroll_px_offset_for_scrollbar().y)
                .min(scroll.max_offset_for_scrollbar().y);
            let target = top + point.y - (view.top() + view.size.height / 3.);
            scroll.set_offset_from_scrollbar(gpui::point(px(0.), -target));
        }
        true
    }

    /// Once per frame, from `render`: finish bringing a match into view.
    pub(super) fn settle_find_reveal(&mut self, window: &mut Window) {
        match self.workspace.conversation_find.reveal {
            None => {}
            Some(Reveal::AfterDraw) => {
                self.workspace.conversation_find.reveal = Some(Reveal::Measure);
                window.request_animation_frame();
            }
            Some(Reveal::Measure) => {
                self.workspace.conversation_find.reveal = None;
                if let Some(hit) = self.current_hit() {
                    self.place_match(&hit);
                }
            }
        }
    }

    /// Restyle `runs` of a block's `range` for the matches drawn in it.
    pub(super) fn mark_found(
        &self,
        site: &Site,
        range: &Range<usize>,
        runs: &mut Vec<(Range<usize>, HighlightStyle)>,
    ) {
        let find = &self.workspace.conversation_find;
        let Owner::Message(id) = &site.owner else {
            return;
        };
        if !find.open {
            return;
        }
        let (first, hits) = find.matches.in_message(id);
        for (index, hit) in hits.iter().enumerate() {
            if hit.block != site.block || hit.range.end <= range.start {
                continue;
            }
            if hit.range.start >= range.end {
                break;
            }
            let local = hit.range.start.max(range.start) - range.start
                ..hit.range.end.min(range.end) - range.start;
            let current = first + index == find.current;
            plait_client::markdown::restyle(runs, local, |style| find_highlight(style, current));
        }
    }

    /// Why some of the conversation is not searched, in one sentence, or
    /// `None` when everything the host holds is loaded.
    pub(super) fn unsearched(&self) -> Option<&'static str> {
        let history = &self.workspace.history;
        let cache = &history.cache;
        if history.enabled && cache.detached {
            Some(
                "Only loaded messages are searched. Newer messages load again with Jump to latest.",
            )
        } else if history.enabled && (!cache.loaded || cache.older.is_some()) {
            Some("Only loaded messages are searched. Earlier messages load as you scroll up.")
        } else if !history.enabled
            && self
                .host
                .thread
                .snapshot
                .as_ref()
                .is_some_and(|s| s.truncated)
        {
            Some(
                "Only loaded messages are searched. The host clipped older messages from this conversation.",
            )
        } else if self.workspace.conversation_find.partial {
            Some(
                "Only loaded text is searched. A long message shows part of its text until you load the rest.",
            )
        } else {
            None
        }
    }

    /// The bar over the conversation, when it is open.
    pub(super) fn conversation_find_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.workspace
            .conversation_find
            .open
            .then(|| self.find_bar(FindIn::Conversation, cx))
    }
}

#[cfg(test)]
impl Plait {
    /// A search result as if it had been started now, for guard tests.
    pub(super) fn found_now(&self, query: &str, matches: Matches) -> Found {
        Found {
            generation: self.workspace.conversation_find.generation,
            query: query.into(),
            transcript: self.workspace.formatted.clone(),
            matches,
            partial: false,
        }
    }
}
