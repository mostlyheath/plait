//! Capture semantic locations before shedding a thread's rendered payload.
use super::{Plait, Workspace, file_view::FindIn, text_field::TextField};
use gpui::*;
use plait_client::{
    tabs::bookmark::*,
    transcript::{self, row_key},
};

fn milli(value: Pixels) -> i32 {
    (f32::from(value) * 1000.).round() as i32
}
fn pixels(value: i32) -> Pixels {
    px(value as f32 / 1000.)
}
fn index(handle: &UniformListScrollHandle) -> u32 {
    handle.0.borrow().base_handle.logical_scroll_top().0 as u32
}
fn offset(handle: &UniformListScrollHandle) -> i32 {
    milli(handle.0.borrow().base_handle.offset().y)
}
fn horizontal(handle: &UniformListScrollHandle) -> i32 {
    milli(handle.0.borrow().base_handle.offset().x)
}
fn set_offset(handle: &UniformListScrollHandle, value: i32) {
    handle.0.borrow_mut().deferred_scroll_to_item = None;
    handle
        .0
        .borrow()
        .base_handle
        .set_offset(point(px(0.), pixels(value)));
}
fn query(field: &Option<Entity<TextField>>, cx: &App) -> String {
    field
        .as_ref()
        .map(|f| f.read(cx).text().to_owned())
        .unwrap_or_default()
}

impl Plait {
    pub(super) fn capture_tab_bookmark(&self, window: Option<&Window>, cx: &App) -> ThreadBookmark {
        let w = &self.workspace;
        let top = w.live_scroll.logical_scroll_top();
        let scroll = if w.live_scroll.is_scrolled_to_end().unwrap_or(true) {
            ConversationScroll::Latest
        } else {
            self.conversation_snapshot()
                .and_then(|s| match w.live_layout.rows.get(top.item_ix)? {
                    plait_client::folding::DisplayRow::Slot(slot) => {
                        transcript::resolve(s, *slot).map(row_key)
                    }
                    plait_client::folding::DisplayRow::FoldedTurn { id, .. } => {
                        Some(format!("message:{id}"))
                    }
                })
                .map(|row| ConversationScroll::Anchored {
                    row,
                    offset_millipoints: milli(top.offset_in_item),
                })
                .unwrap_or_default()
        };
        let selected = self.state.repository.selected_diff();
        let recorded = w.turn_review.target.is_some() && !w.turn_review.current;
        let diff = if recorded {
            w.turn_review.diff.as_ref()
        } else {
            self.state.repository.diff().ready()
        };
        let mut bookmark = ThreadBookmark {
            view: w.tab,
            conversation: ConversationBookmark {
                scroll,
                before: w.history.cache.restore_cursor(
                    self.conversation_snapshot()
                        .and_then(|s| match w.live_layout.rows.get(top.item_ix)? {
                            plait_client::folding::DisplayRow::Slot(slot) => {
                                transcript::resolve(s, *slot)
                                    .and_then(plait_client::history::Key::row)
                            }
                            plait_client::folding::DisplayRow::FoldedTurn { id, .. } => {
                                Some(plait_client::history::Key::Message(id.clone()))
                            }
                        })
                        .as_ref(),
                ),
                pages: 1,
                find: FindBookmark {
                    open: w.conversation_find.open,
                    query: query(&w.conversation_find.field, cx),
                    current: w.conversation_find.bookmark_match(),
                },
            },
            changes: ChangesBookmark {
                path: if recorded {
                    w.turn_review
                        .target
                        .as_ref()
                        .and_then(|t| t.path.as_ref())
                        .map(|p| p.to_string_lossy().into_owned())
                } else {
                    selected.map(|key| key.path.to_string_lossy().into_owned())
                },
                staged: selected
                    .is_some_and(|key| key.mode == plait_client::model::DiffMode::Staged),
                scope: w
                    .turn_review
                    .target
                    .as_ref()
                    .map(|target| ChangeScope::Recorded {
                        record: target.record.clone(),
                        original: target.original.clone(),
                        compare_current: w.turn_review.current,
                    })
                    .unwrap_or_default(),
                anchor: diff.map(|diff| PatchAnchor {
                    fingerprint: diff.fingerprint(),
                    hunk: 0,
                    line: index(&w.diff_scroll),
                    offset_millipoints: offset(&w.diff_scroll),
                    horizontal_millipoints: horizontal(&w.diff_scroll),
                }),
                selection: w
                    .line_pick
                    .as_ref()
                    .filter(|pick| {
                        selected.is_some_and(|key| key.path == pick.path && key.mode == pick.mode)
                    })
                    .map(|pick| (pick.anchor as u32, pick.end as u32)),
                list_offset_millipoints: offset(&w.file_scroll),
            },
            history: HistoryBookmark {
                commit: self.state.repository.view.history.selected.clone(),
                anchor: self
                    .state
                    .repository
                    .view
                    .history
                    .patch
                    .ready()
                    .map(|diff| PatchAnchor {
                        fingerprint: diff.fingerprint(),
                        hunk: 0,
                        line: index(&w.commit_scroll),
                        offset_millipoints: offset(&w.commit_scroll),
                        horizontal_millipoints: horizontal(&w.commit_scroll),
                    }),
                list_offset_millipoints: offset(&w.history_scroll),
            },
            files: FileBookmark {
                path: w
                    .file_viewer
                    .path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                line: index(&w.file_viewer.scroll),
                column: 0,
                offset_millipoints: offset(&w.file_viewer.scroll),
                horizontal_millipoints: horizontal(&w.file_viewer.scroll),
                find: FindBookmark {
                    open: w.file_viewer.find.open,
                    query: query(&w.file_viewer.find.field, cx),
                    current: w.file_viewer.find.bookmark_match(),
                },
            },
            draft: self
                .visible_composer()
                .map(|c| c.read(cx).draft_bookmark())
                .unwrap_or_default(),
            panels: PanelBookmark {
                changed_files_collapsed: w.thread_files.collapsed,
                context_open: w.thread_controls.usage_open,
                enlarged_image: w
                    .images
                    .enlarged
                    .as_ref()
                    .map(|(image, _)| image.id.clone()),
                undo_conflict_record: w
                    .turn_records
                    .blocked
                    .as_ref()
                    .map(|b| b.reply.record_id.clone()),
                selected_turn: w.turn_review.target.as_ref().map(|t| t.record.clone()),
                ..Default::default()
            },
            details: ViewDetails {
                expanded: w
                    .expanded_live_tools
                    .iter()
                    .map(|s| format!("tool:{s}"))
                    .chain(w.history.expanded.iter().map(|s| format!("history:{s}")))
                    .collect(),
                folded: w.reading.open_groups.iter().cloned().collect(),
                selection: w.reading.bookmark_selection(),
                commit_message: w.commit_message.read(cx).text().to_owned(),
            },
            ..Default::default()
        };
        bookmark.details.expanded.sort();
        bookmark.details.folded.sort();
        bookmark.focus = match w.tab {
            Workspace::Changes => FocusTarget::Changes,
            Workspace::History => FocusTarget::History,
            Workspace::Files => {
                if window.is_some_and(|w| {
                    self.workspace
                        .file_viewer
                        .find
                        .field
                        .as_ref()
                        .is_some_and(|f| f.focus_handle(cx).is_focused(w))
                }) {
                    FocusTarget::FileFind
                } else {
                    FocusTarget::Files
                }
            }
            Workspace::Conversation => {
                if window.is_some_and(|w| {
                    self.workspace
                        .conversation_find
                        .field
                        .as_ref()
                        .is_some_and(|f| f.focus_handle(cx).is_focused(w))
                }) {
                    FocusTarget::ConversationFind
                } else if window.is_some_and(|w| {
                    self.visible_composer()
                        .is_some_and(|c| c.focus_handle(cx).is_focused(w))
                }) {
                    FocusTarget::Composer
                } else {
                    FocusTarget::Conversation
                }
            }
        };
        // An unvisited view has no loaded geometry yet; keep its semantic location.
        if let Some(saved) = &w.restore {
            if w.restore_pending[0] {
                bookmark.conversation.scroll = saved.conversation.scroll.clone();
                bookmark.conversation.before = saved.conversation.before.clone();
                bookmark.conversation.pages = saved.conversation.pages;
                if bookmark.details.selection.is_none() {
                    bookmark.details.selection = saved.details.selection.clone();
                }
            }
            if w.restore_pending[1] {
                bookmark.changes = saved.changes.clone();
            }
            if w.restore_pending[2] {
                bookmark.history = saved.history.clone();
            }
            if w.restore_pending[3] {
                let find = bookmark.files.find;
                bookmark.files = saved.files.clone();
                bookmark.files.find = find;
            }
            if bookmark.panels.enlarged_image.is_none() {
                bookmark.panels.enlarged_image = saved.panels.enlarged_image.clone();
            }
        }
        bookmark
    }

    pub(super) fn restore_tab_bookmark(
        &mut self,
        bookmark: ThreadBookmark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace.tab = bookmark.view;
        self.workspace.thread_files.collapsed = bookmark.panels.changed_files_collapsed;
        self.workspace.thread_controls.usage_open = bookmark.panels.context_open;
        self.workspace.commit_message.update(cx, |field, cx| {
            field.set_text(&bookmark.details.commit_message, cx)
        });
        self.workspace.reading.restore_selection(
            self.host.thread.selected.as_deref().unwrap_or_default(),
            &bookmark.details,
        );
        for id in &bookmark.details.expanded {
            if let Some(id) = id.strip_prefix("tool:") {
                self.workspace.expanded_live_tools.insert(id.into());
            }
            if let Some(id) = id.strip_prefix("history:") {
                self.workspace.history.expanded.insert(id.into());
            }
        }
        if let Some(composer) = self.visible_composer() {
            composer.update(cx, |c, _| c.restore_cursor(&bookmark.draft));
        }
        if let ChangeScope::Recorded {
            record,
            original,
            compare_current,
        } = &bookmark.changes.scope
            && let Some(thread) = self.host.thread.selected.clone()
        {
            self.clear_turn_review();
            if self.panel_recorded() {
                self.workspace.turn_records.selection.earlier(original);
            }
            self.workspace.turn_review.window = Some(window.window_handle());
            self.workspace.turn_review.target = Some(plait_client::review::recorded::Target {
                thread,
                checkout: self.host.creation.checkout.clone(),
                record: record.clone(),
                original: original.clone(),
                path: bookmark.changes.path.as_ref().map(Into::into),
            });
            self.workspace.turn_review.current = *compare_current;
        }
        let focus = bookmark.focus;
        let view = Workspace::Conversation;
        self.install_view_locations(bookmark, cx);
        if let Some(target) = self
            .tabs
            .model
            .panel()
            .filter(|p| p.scope == plait_client::panel::Scope::ThisTurn)
            .and_then(|p| p.recorded.clone())
        {
            self.workspace
                .turn_records
                .selection
                .earlier(&target.original);
            self.workspace.turn_review.target = Some(target);
            self.workspace.turn_review.window = Some(window.window_handle());
        }
        self.present_workspace(view, window, cx);
        match focus {
            FocusTarget::Composer => self.focus_composer(window, cx),
            FocusTarget::ConversationFind => {
                if let Some(field) = &self.workspace.conversation_find.field {
                    window.focus(&field.focus_handle(cx), cx);
                }
            }
            FocusTarget::FileFind => {
                if let Some(field) = &self.workspace.file_viewer.find.field {
                    window.focus(&field.focus_handle(cx), cx);
                }
            }
            _ => window.focus(&self.focus, cx),
        }
        self.pump(cx);
        self.restore_ready_bookmarks(cx);
    }

    fn install_view_locations(&mut self, bookmark: ThreadBookmark, cx: &mut Context<Self>) {
        self.restore_find_bookmark(FindIn::Conversation, &bookmark.conversation.find, cx);
        self.restore_find_bookmark(FindIn::File, &bookmark.files.find, cx);
        self.workspace.file_viewer.path = bookmark.files.path.as_ref().map(Into::into);
        self.workspace.file_viewer.root = self.file_root();
        self.workspace.file_viewer.reveal = Some(bookmark.files.line as usize);
        self.workspace.restore_pending = [true; 4];
        self.workspace.restore = Some(bookmark);
    }

    pub(super) fn restore_tab_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .workspace
            .restore
            .as_ref()
            .and_then(|b| b.panels.enlarged_image.clone());
        if let Some(id) = id
            && let Some(reference) = self
                .conversation_snapshot()
                .and_then(|s| {
                    s.messages
                        .iter()
                        .flat_map(|m| &m.images)
                        .find(|i| i.id == id)
                })
                .cloned()
        {
            if let Some(bookmark) = &mut self.workspace.restore {
                bookmark.panels.enlarged_image = None;
            }
            self.enlarge_image(reference, window, cx);
        }
    }

    pub(super) fn restore_ready_bookmarks(&mut self, cx: &mut Context<Self>) {
        let Some(bookmark) = self.workspace.restore.clone() else {
            return;
        };
        if self.workspace.restore_pending[1] {
            set_offset(
                &self.workspace.file_scroll,
                bookmark.changes.list_offset_millipoints,
            );
        }
        if self.workspace.restore_pending[2] {
            set_offset(
                &self.workspace.history_scroll,
                bookmark.history.list_offset_millipoints,
            );
        }
        let recorded = matches!(
            bookmark.changes.scope,
            ChangeScope::Recorded {
                compare_current: false,
                ..
            }
        );
        let diff = if recorded {
            self.workspace.turn_review.diff.as_ref()
        } else {
            self.state.repository.diff().ready()
        };
        if self.workspace.restore_pending[1]
            && let Some(diff) = diff
        {
            if let Some(anchor) = &bookmark.changes.anchor {
                if diff.fingerprint() == anchor.fingerprint {
                    set_offset(&self.workspace.diff_scroll, anchor.offset_millipoints);
                    self.workspace
                        .diff_scroll
                        .0
                        .borrow()
                        .base_handle
                        .set_offset(point(
                            pixels(anchor.horizontal_millipoints),
                            pixels(anchor.offset_millipoints),
                        ));
                    if !recorded
                        && let Some((a, z)) = bookmark.changes.selection
                        && (a.max(z) as usize) < diff.lines.len()
                        && let Some(key) = self.state.repository.selected_diff()
                    {
                        self.workspace.line_pick = Some(super::changes::LinePick {
                            checkout: self.state.checkout_id(),
                            epoch: self.state.repository.epoch(),
                            path: key.path.clone(),
                            mode: key.mode,
                            anchor: a as usize,
                            end: z as usize,
                        });
                    }
                } else {
                    self.notice = Some("The patch changed. Showing its current contents without the old selection.".into());
                }
            }
            self.workspace.restore_pending[1] = false;
        }
        if self.workspace.restore_pending[2]
            && let Some(patch) = self.state.repository.view.history.patch.ready()
        {
            if let Some(anchor) = &bookmark.history.anchor {
                if patch.fingerprint() == anchor.fingerprint {
                    set_offset(&self.workspace.commit_scroll, anchor.offset_millipoints);
                    self.workspace
                        .commit_scroll
                        .0
                        .borrow()
                        .base_handle
                        .set_offset(point(
                            pixels(anchor.horizontal_millipoints),
                            pixels(anchor.offset_millipoints),
                        ));
                } else {
                    self.notice =
                        Some("The commit patch changed. Showing its current contents.".into());
                }
            }
            self.workspace.restore_pending[2] = false;
        }
        if self.workspace.restore_pending[3] && self.workspace.file_viewer.opened.is_some() {
            set_offset(
                &self.workspace.file_viewer.scroll,
                bookmark.files.offset_millipoints,
            );
            self.workspace
                .file_viewer
                .scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(
                    pixels(bookmark.files.horizontal_millipoints),
                    pixels(bookmark.files.offset_millipoints),
                ));
            self.workspace.restore_pending[3] = false;
        }
        if self.workspace.restore_pending[0] && self.conversation_snapshot().is_some() {
            match &bookmark.conversation.scroll {
                ConversationScroll::Latest => {
                    self.workspace.live_scroll.scroll_to_end();
                    self.workspace.restore_pending[0] = false;
                }
                ConversationScroll::Anchored {
                    row,
                    offset_millipoints,
                } => {
                    let layout = &self.workspace.live_layout;
                    if let Some(&raw) = layout.raw_index_by_key.get(row) {
                        let item_ix = layout
                            .raw_index_of_row
                            .partition_point(|&start| start <= raw)
                            - 1;
                        let inside_fold = layout.raw_index(item_ix) != raw;
                        self.workspace.live_scroll.scroll_to(ListOffset {
                            item_ix,
                            offset_in_item: if inside_fold {
                                px(0.)
                            } else {
                                pixels(*offset_millipoints)
                            },
                        });
                        self.workspace.restore_pending[0] = false;
                    } else if self.workspace.history.cache.loaded
                        && self.workspace.history.loading.is_none()
                    {
                        self.notice = Some("The saved conversation position is no longer available. Showing the nearest loaded messages.".into());
                        self.workspace.live_scroll.scroll_to(ListOffset {
                            item_ix: 0,
                            offset_in_item: px(0.),
                        });
                        self.workspace.restore_pending[0] = false;
                    }
                }
            }
        }
        cx.notify();
    }
}
