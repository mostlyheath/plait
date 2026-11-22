//! The Files tab: one checkout file, read-only, with line numbers, syntax
//! colors, find, and quick open. Reads run on the background executor; a
//! reopened or changed file is checked by size and time before it is reread,
//! and only when something could have changed it (a Git status read, the
//! window coming forward, or the tab being shown). Nothing polls.

use super::{
    Plait, Workspace,
    commands::Command,
    markdown::token_color,
    palette::{Item, Options, Palette},
    text_field::{TextField, TextFieldEvent},
    theme::*,
    ways_out,
};
use gpui::{prelude::*, *};
use plait_core::{
    file_view::{self, Contents, FIND_LIMIT, LINE_CLIP, Opened},
    highlight::{Span, Token},
    state::Load,
};
use std::{ops::Range, path::PathBuf, sync::Arc};

pub(super) const LINE_HEIGHT: f32 = 20.;
const TAB_WIDTH: usize = 4;

#[derive(Default)]
pub(super) struct FileViewer {
    /// Checkout-relative path, and the root it is relative to.
    pub path: Option<PathBuf>,
    pub root: PathBuf,
    pub opened: Option<Arc<Opened>>,
    pub error: Option<String>,
    pub loading: bool,
    /// After a reload: what changed on disk.
    pub note: Option<String>,
    /// Guards reads and stamp checks against newer ones.
    generation: u64,
    pub scroll: UniformListScrollHandle,
    pub find: Find,
    /// A line to show once the file is read.
    pub reveal: Option<usize>,
}

impl FileViewer {
    pub(super) fn clear_checkout(&mut self) {
        *self = Self {
            generation: self.generation + 1,
            ..Default::default()
        };
    }

    pub(super) fn retire_read(&mut self) {
        self.generation += 1;
        self.loading = false;
    }
}

/// A file is local: a host reconnect must not strand its read. Selecting a
/// thread or changing checkout also retires the viewer's generation.
fn same_file_owner(
    owner: &plait_core::ownership::ReadStamp,
    now: &plait_core::ownership::ReadStamp,
) -> bool {
    owner.checkout == now.checkout && owner.thread == now.thread
}

/// What a find bar searches. Both share one bar, its keys, and its counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FindIn {
    File,
    Conversation,
}

#[derive(Default)]
pub(super) struct Find {
    pub open: bool,
    pub field: Option<Entity<TextField>>,
    pub matches: Arc<Vec<Range<usize>>>,
    pub current: usize,
    restore_match: Option<plait_core::tabs::bookmark::TextPosition>,
    generation: u64,
    _events: Option<Subscription>,
}

impl Find {
    pub(super) fn bookmark_match(&self) -> Option<plait_core::tabs::bookmark::TextPosition> {
        self.matches
            .get(self.current)
            .map(|hit| plait_core::tabs::bookmark::TextPosition {
                source: "file".into(),
                block: 0,
                byte: hit.start as u32,
            })
    }
}

impl Plait {
    pub(super) fn restore_find_bookmark(
        &mut self,
        target: FindIn,
        bookmark: &plait_core::tabs::bookmark::FindBookmark,
        cx: &mut Context<Self>,
    ) {
        let name = match target {
            FindIn::File => "Find in file",
            FindIn::Conversation => "Find in conversation",
        };
        let field = cx.new(|cx| {
            let mut field = TextField::new(name, name, cx).search();
            field.set_text(&bookmark.query, cx);
            field
        });
        let events = cx.subscribe(
            &field,
            move |this, _, _: &TextFieldEvent, cx| match target {
                FindIn::File => this.update_find(cx),
                FindIn::Conversation => this.search_conversation(cx),
            },
        );
        match target {
            FindIn::File => {
                let find = &mut self.workspace.file_viewer.find;
                find.open = bookmark.open;
                find.restore_match = bookmark.current.clone();
                find.field = Some(field);
                find._events = Some(events);
            }
            FindIn::Conversation => {
                self.restore_conversation_find(field, events, bookmark.open);
                self.workspace.conversation_find.restore_match = bookmark.current.clone();
            }
        }
    }
    /// The root quick-open paths are relative to: the repository's top level.
    pub(super) fn file_root(&self) -> PathBuf {
        self.state.repository.snapshot().ready().map_or_else(
            || self.state.repository.path().to_path_buf(),
            |s| s.root.clone(),
        )
    }

    /// Show `path` in the Files tab, at `line` (zero-based) when given.
    pub(super) fn open_file(&mut self, path: PathBuf, line: Option<usize>, cx: &mut Context<Self>) {
        if !self.ensure_view_tab() {
            cx.notify();
            return;
        }
        self.tabs.lifecycle.view_revision += 1;
        self.pause_conversation(None, cx);
        self.workspace.restore_pending[3] = false;
        let root = self.file_root();
        let viewer = &mut self.workspace.file_viewer;
        let same = viewer.path.as_ref() == Some(&path) && viewer.root == root;
        viewer.reveal = line;
        self.workspace.tab = Workspace::Files;
        self.queue_tab_save(cx);
        self.settings_open = false;
        if same && self.workspace.file_viewer.opened.is_some() {
            self.reveal_line(cx);
            return self.check_file(cx);
        }
        let viewer = &mut self.workspace.file_viewer;
        viewer.path = Some(path);
        viewer.root = root;
        viewer.opened = None;
        viewer.error = None;
        viewer.note = None;
        viewer.find.matches = Arc::default();
        viewer.scroll = UniformListScrollHandle::new();
        self.read_file(false, cx);
    }

    fn read_file(&mut self, reload: bool, cx: &mut Context<Self>) {
        let backend = self.repository_backend();
        let owner = self.host.read_stamp();
        let cancelled = self.workspace.cancelled.clone();
        let expected = self.state.checkout_key().cloned();
        let viewer = &mut self.workspace.file_viewer;
        let Some(path) = viewer.path.clone() else {
            return;
        };
        viewer.generation += 1;
        viewer.loading = !reload;
        let generation = viewer.generation;
        let root = viewer.root.clone();
        let task = cx
            .background_executor()
            .spawn(async move { backend.file(&root, expected.as_ref(), &path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if cancelled.load(std::sync::atomic::Ordering::Acquire)
                    || !same_file_owner(&owner, &this.host.read_stamp())
                {
                    return;
                }
                let viewer = &mut this.workspace.file_viewer;
                if viewer.generation != generation {
                    return;
                }
                viewer.loading = false;
                match result {
                    Ok(opened) => {
                        let changed = viewer
                            .opened
                            .as_ref()
                            .is_none_or(|old| old.contents != opened.contents);
                        if reload && changed {
                            viewer.note = Some("Updated from disk.".into());
                        } else if reload {
                            viewer.note = None;
                        }
                        viewer.opened = Some(Arc::new(opened));
                        viewer.error = None;
                        if changed {
                            this.update_find(cx);
                        }
                        this.reveal_line(cx);
                        this.restore_ready_bookmarks(cx);
                    }
                    // A reload that fails keeps the last good text on screen.
                    Err(error) if reload && viewer.opened.is_some() => {
                        viewer.note = Some(format!("{error} Showing the version read before."));
                    }
                    Err(error) => viewer.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reread the open file if its size or time changed. Cheap: one stat.
    pub(super) fn check_file(&mut self, cx: &mut Context<Self>) {
        if self.workspace.tab != Workspace::Files {
            return;
        }
        let owner = self.host.read_stamp();
        let cancelled = self.workspace.cancelled.clone();
        let expected = self.state.checkout_key().cloned();
        let viewer = &self.workspace.file_viewer;
        if self.workspace.tab == Workspace::Files
            && viewer.path.is_some()
            && viewer.opened.is_none()
            && viewer.error.is_none()
            && !viewer.loading
        {
            self.read_file(false, cx);
            return;
        }
        let (Some(path), Some(opened)) = (viewer.path.clone(), viewer.opened.clone()) else {
            return;
        };
        if viewer.loading {
            return;
        }
        let root = viewer.root.clone();
        let generation = viewer.generation;
        let backend = self.repository_backend();
        let task = cx
            .background_executor()
            .spawn(async move { backend.file_stamp(&root, expected.as_ref(), &path) });
        cx.spawn(async move |this, cx| {
            let stamp = task.await;
            let _ = this.update(cx, |this, cx| {
                if !cancelled.load(std::sync::atomic::Ordering::Acquire)
                    && same_file_owner(&owner, &this.host.read_stamp())
                    && this.workspace.file_viewer.generation == generation
                    && stamp.as_ref().ok() != Some(&opened.stamp)
                {
                    this.read_file(true, cx);
                }
            });
        })
        .detach();
    }

    fn reveal_line(&mut self, cx: &mut Context<Self>) {
        if let Some(line) = self.workspace.file_viewer.reveal.take() {
            self.workspace
                .file_viewer
                .scroll
                .scroll_to_item(line, ScrollStrategy::Center);
            cx.notify();
        }
    }

    /// Cmd-P: pick a file from the checkout's list, read in the background.
    pub(super) fn quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.repository.request_files();
        self.pump(cx);
        let (items, empty, note) = self.file_items();
        let options = Options {
            name: "Open file".into(),
            placeholder: "Open a file in this checkout…".into(),
            empty: empty.into(),
            initial: self
                .workspace
                .file_viewer
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned().into()),
            mark: None,
            aside: None,
            note,
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.files = true;
        }
    }

    /// Rows for quick open: file name first, then its directory.
    fn file_items(&self) -> (Vec<Item>, &'static str, Option<SharedString>) {
        match &self.state.repository.files() {
            Load::Ready(list) => {
                let items = list
                    .paths
                    .iter()
                    .map(|path| {
                        let (name, directory) = match path.rfind('/') {
                            Some(slash) => (&path[slash + 1..], &path[..slash]),
                            None => (path.as_str(), ""),
                        };
                        Item::new(
                            path.clone(),
                            name.to_owned(),
                            Command::OpenFile(path.clone()),
                        )
                        .detail(directory.to_owned())
                    })
                    .collect();
                let note = list
                    .truncated
                    .then(|| "Only the first files are listed.".into());
                (items, "No files in this checkout.", note)
            }
            Load::Error(error) => (
                Vec::new(),
                "Files could not be listed.",
                Some(error.clone().into()),
            ),
            _ => (Vec::new(), "Listing files…", None),
        }
    }

    /// After a Git job: fill an open quick-open list once its files arrive.
    pub(super) fn deliver_quick_open(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.overlay.as_ref().filter(|overlay| overlay.files) else {
            return;
        };
        if !matches!(
            self.state.repository.files(),
            Load::Ready(_) | Load::Error(_)
        ) || !overlay.palette.read(cx).is_empty()
        {
            return;
        }
        let palette = overlay.palette.clone();
        let (items, _, _) = self.file_items();
        palette.update(cx, |palette, cx| palette.set_items(items, cx));
    }

    /// Cmd-F: show the find field over the open file.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.tab != Workspace::Files || self.workspace.file_viewer.opened.is_none() {
            return;
        }
        let field = match self.workspace.file_viewer.find.field.clone() {
            Some(field) => field,
            None => {
                let field =
                    cx.new(|cx| TextField::new("Find in file", "Find in file", cx).search());
                self.workspace.file_viewer.find._events =
                    Some(cx.subscribe(&field, |this, _, _: &TextFieldEvent, cx| {
                        this.update_find(cx)
                    }));
                self.workspace.file_viewer.find.field = Some(field.clone());
                field
            }
        };
        if !self.workspace.file_viewer.find.open {
            self.workspace.file_viewer.find.open = true;
            // A query kept from last time is searched again over the file as it is now.
            self.update_find(cx);
        }
        window.focus(&field.focus_handle(cx), cx);
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let find = &mut self.workspace.file_viewer.find;
        find.open = false;
        find.matches = Arc::default();
        // Whatever search is running now answers nothing.
        find.generation += 1;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Search off the UI thread; a large file with no match scans it all.
    fn update_find(&mut self, cx: &mut Context<Self>) {
        let owner = self.host.read_stamp();
        let cancelled = self.workspace.cancelled.clone();
        let find = &mut self.workspace.file_viewer.find;
        find.generation += 1;
        let generation = find.generation;
        let query = match (&find.field, find.open) {
            (Some(field), true) => field.read(cx).text().to_owned(),
            _ => String::new(),
        };
        let text = match self.workspace.file_viewer.opened.as_deref() {
            Some(Opened {
                contents: Contents::Text(text),
                ..
            }) => text.text.clone(),
            _ => String::new(),
        };
        let task = cx
            .background_executor()
            .spawn(async move { file_view::find(&text, &query) });
        cx.spawn(async move |this, cx| {
            let matches = task.await;
            let _ = this.update(cx, |this, cx| {
                if cancelled.load(std::sync::atomic::Ordering::Acquire)
                    || !same_file_owner(&owner, &this.host.read_stamp())
                {
                    return;
                }
                let find = &mut this.workspace.file_viewer.find;
                if find.generation != generation {
                    return;
                }
                find.matches = Arc::new(matches);
                if let Some(saved) = find.restore_match.take() {
                    find.current = find
                        .matches
                        .iter()
                        .position(|range| range.start == saved.byte as usize)
                        .unwrap_or(0);
                    cx.notify();
                    return;
                }
                find.current = 0;
                this.show_match(cx);
            });
        })
        .detach();
    }

    /// Enter, Cmd-G, and their Shift forms: the next or previous match, wrapping.
    pub(super) fn step_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        let find = &mut self.workspace.file_viewer.find;
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        find.current = if forward {
            (find.current + 1) % count
        } else {
            (find.current + count - 1) % count
        };
        self.show_match(cx);
    }

    fn show_match(&mut self, cx: &mut Context<Self>) {
        let viewer = &self.workspace.file_viewer;
        if let (Some(found), Some(Contents::Text(text))) = (
            viewer.find.matches.get(viewer.find.current),
            viewer.opened.as_deref().map(|opened| &opened.contents),
        ) {
            let line = file_view::line_of(&text.lines, found.start);
            viewer.scroll.scroll_to_item(line, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// The Files tab.
    pub(super) fn file_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let viewer = &self.workspace.file_viewer;
        let Some(path) = viewer.path.as_ref() else {
            return empty(
                "No file open",
                format!(
                    "Press {} to open any file in this checkout.",
                    match self.shortcut_label("file.open") {
                        label if label.is_empty() => "Open File…".to_owned(),
                        label => label,
                    }
                ),
            )
            .child(
                button("file-quick-open", "Open File…")
                    .on_activate(cx, |this, window, cx| this.quick_open(window, cx)),
            )
            .into_any_element();
        };
        let display = path.to_string_lossy().into_owned();
        let (name, directory) = match display.rfind('/') {
            Some(slash) => (display[slash + 1..].to_owned(), display[..slash].to_owned()),
            None => (display.clone(), String::new()),
        };
        let summary = match viewer.opened.as_deref().map(|opened| &opened.contents) {
            Some(Contents::Text(text)) => Some(format!(
                "{} {} · {}",
                text.lines.len(),
                if text.lines.len() == 1 {
                    "line"
                } else {
                    "lines"
                },
                size(text.text.len() as u64)
            )),
            _ => None,
        };
        let header = row()
            .h(px(44.))
            .px_4()
            .gap_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(colors().border))
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(rgb(colors().subtle))
                    .child(directory),
            )
            .children(
                viewer
                    .note
                    .clone()
                    .map(|note| tag("file-viewer-note", note, colors().amber)),
            )
            .children(summary.map(|summary| tag("file-viewer-summary", summary, colors().subtle)))
            .child(
                button("file-find", "Find")
                    .disabled(viewer.opened.is_none())
                    .on_activate(cx, |this, window, cx| this.open_find(window, cx)),
            )
            .child(
                button("file-quick-open", "Open File…")
                    .on_activate(cx, |this, window, cx| this.quick_open(window, cx)),
            )
            .child(ways_out::editor_button(
                "file-editor",
                &display,
                self.shortcut_aria("file.open_in_editor"),
                self.host_is_local(),
                cx,
            ))
            .child(ways_out::finder_button(
                "file-finder",
                &display,
                self.host_is_local(),
                cx,
            ));
        let body = match (&viewer.error, viewer.opened.as_deref(), viewer.loading) {
            (Some(error), _, _) => {
                empty("This file cannot be shown", error.clone()).into_any_element()
            }
            (None, None, _) => empty("Reading…", display.clone()).into_any_element(),
            (None, Some(opened), _) => match &opened.contents {
                Contents::Text(_) => self.file_lines(cx),
                Contents::Binary { len } => empty(
                    "Binary file",
                    format!("{} of binary data is not shown.", size(*len)),
                )
                .into_any_element(),
                Contents::TooLarge { len } => empty(
                    "Too large to show",
                    format!(
                        "{} is over the {} the viewer reads.",
                        size(*len),
                        size(file_view::READ_LIMIT)
                    ),
                )
                .into_any_element(),
                Contents::NotUtf8 { len } => empty(
                    "Not UTF-8 text",
                    format!("{} in another encoding is not shown.", size(*len)),
                )
                .into_any_element(),
            },
        };
        column()
            .id("file-view")
            .flex_1()
            .min_h_0()
            .child(header)
            .children(viewer.find.open.then(|| self.find_bar(FindIn::File, cx)))
            .child(body)
            .into_any_element()
    }

    /// Where Find, Find Next, and Find Previous act: the file or the
    /// conversation on screen, or nothing.
    pub(super) fn find_target(&self) -> Option<FindIn> {
        if self.settings_open {
            return None;
        }
        match self.workspace.tab {
            Workspace::Files => Some(FindIn::File),
            Workspace::Conversation if self.host.thread.selected.is_some() => {
                Some(FindIn::Conversation)
            }
            _ => None,
        }
    }

    pub(super) fn open_find_in(
        &mut self,
        target: FindIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            FindIn::File => self.open_find(window, cx),
            FindIn::Conversation => self.open_conversation_find(window, cx),
        }
    }

    pub(super) fn step_find_in(&mut self, target: FindIn, forward: bool, cx: &mut Context<Self>) {
        match target {
            FindIn::File => self.step_find(forward, cx),
            FindIn::Conversation => self.step_conversation_find(forward, cx),
        }
    }

    fn close_find_in(&mut self, target: FindIn, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            FindIn::File => self.close_find(window, cx),
            FindIn::Conversation => self.close_conversation_find(window, cx),
        }
    }

    /// One bar for both: the field, "3 of 17", previous, next, and close. The
    /// conversation's also says, briefly, when not all of it is loaded.
    pub(super) fn find_bar(&self, target: FindIn, cx: &mut Context<Self>) -> AnyElement {
        let (prefix, field, current, count, unsearched) = match target {
            FindIn::File => {
                let find = &self.workspace.file_viewer.find;
                (
                    "file-find",
                    &find.field,
                    find.current,
                    find.matches.len(),
                    None,
                )
            }
            FindIn::Conversation => {
                let find = &self.workspace.conversation_find;
                (
                    "conversation-find",
                    &find.field,
                    find.current,
                    find.matches.len(),
                    self.unsearched(),
                )
            }
        };
        let id = |part: &str| SharedString::from(format!("{prefix}-{part}"));
        let query_empty = field
            .as_ref()
            .is_none_or(|field| field.read(cx).text().is_empty());
        let status = find_status(query_empty, current, count);
        row()
            .id(id("bar"))
            .role(Role::Search)
            .audited()
            .px_4()
            .py_1()
            .gap_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(rgb(colors().border))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => this.step_find_in(target, !event.keystroke.modifiers.shift, cx),
                    "escape" => this.close_find_in(target, window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .children(field.clone().map(|field| div().w(px(280.)).child(field)))
            .child(
                // Plain text is not in the accessibility tree; the count is
                // named so VoiceOver and the native acceptance check can read it.
                div()
                    .id(id("status"))
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .audited()
                    .min_w(px(72.))
                    .text_size(px(12.))
                    .text_color(rgb(colors().subtle))
                    .child(status),
            )
            .child(
                icon_button(id("previous"), "up", "Previous match")
                    .on_activate(cx, move |this, _, cx| this.step_find_in(target, false, cx)),
            )
            .child(
                icon_button(id("next"), "down", "Next match")
                    .on_activate(cx, move |this, _, cx| this.step_find_in(target, true, cx)),
            )
            .child(div().flex_1())
            .when_some(unsearched, |el, why| {
                el.child(
                    div()
                        .id(id("unsearched"))
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(rgb(colors().subtle))
                        .aria_description(why)
                        .tooltip(move |_, cx| {
                            cx.new(|_| super::thread_controls::TextTooltip(why.into()))
                                .into()
                        })
                        .child("Loaded messages only"),
                )
            })
            .child(
                icon_button(id("close"), "close", "Close find")
                    .on_activate(cx, move |this, window, cx| {
                        this.close_find_in(target, window, cx)
                    }),
            )
            .into_any_element()
    }

    /// Every line, virtualized: only those on screen are laid out.
    fn file_lines(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(opened) = self.workspace.file_viewer.opened.clone() else {
            return div().into_any_element();
        };
        let Contents::Text(text) = &opened.contents else {
            return div().into_any_element();
        };
        let count = text.lines.len();
        let digits = count.to_string().len().max(3);
        let gutter = digits as f32 * 7.5 + 24.;
        let widest = text.widest_index;
        let matches = self.workspace.file_viewer.find.matches.clone();
        let current = self.workspace.file_viewer.find.current;
        uniform_list(
            "file-lines",
            count,
            cx.processor(move |_, range: Range<usize>, _, _| {
                let Contents::Text(text) = &opened.contents else {
                    return Vec::new();
                };
                range
                    .map(|index| {
                        let (shown, runs) = line_runs(text, index, &matches, matches.get(current));
                        let highlights = runs
                            .into_iter()
                            .map(|(range, style)| (range, style.highlight()))
                            .collect::<Vec<_>>();
                        row()
                            .id(("file-line", index))
                            .h(px(LINE_HEIGHT))
                            .font_family(mono())
                            .text_size(px(12.))
                            .line_height(px(LINE_HEIGHT))
                            .child(
                                div()
                                    .w(px(gutter))
                                    .flex_shrink_0()
                                    .pr_4()
                                    .text_right()
                                    .text_color(rgb(colors().subtle))
                                    .child((index + 1).to_string()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .pr_4()
                                    .whitespace_nowrap()
                                    .text_color(rgb(colors().text))
                                    .child(StyledText::new(shown).with_highlights(highlights)),
                            )
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .with_width_from_item(Some(widest))
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(&self.workspace.file_viewer.scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
}

/// How one stretch of a line is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RunStyle {
    pub token: Option<Token>,
    /// 0: not found, 1: a match, 2: the current match.
    pub found: u8,
}

impl RunStyle {
    fn highlight(self) -> HighlightStyle {
        let mut style = HighlightStyle {
            color: self.token.map(|token| rgb(token_color(token)).into()),
            ..HighlightStyle::default()
        };
        if self.found > 0 {
            find_highlight(&mut style, self.found == 2);
        }
        style
    }
}

/// How a find match is drawn, in a file and in the conversation alike. Every
/// match uses the selection's neutral fill; the current one has the same fill
/// at twice the strength and an underline, so it never depends on color.
pub(super) fn find_highlight(style: &mut HighlightStyle, current: bool) {
    let fill = colors().selection;
    let alpha = if current {
        ((fill & 0xff) * 2).min(0xff)
    } else {
        fill & 0xff
    };
    style.background_color = Some(rgba(fill & !0xff | alpha).into());
    if current {
        style.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(rgb(colors().text).into()),
            wavy: false,
        });
    }
}

/// Line `index` as shown (tabs expanded, clipped at `LINE_CLIP` characters)
/// and its styled runs in display offsets. Spans and matches are sorted.
pub(super) fn line_runs(
    text: &file_view::Text,
    index: usize,
    matches: &[Range<usize>],
    current: Option<&Range<usize>>,
) -> (SharedString, Vec<(Range<usize>, RunStyle)>) {
    let line = text.lines[index].clone();
    let source = &text.text[line.clone()];
    // Display offset of each source byte that starts a character, and the end.
    let mut at = vec![usize::MAX; source.len() + 1];
    let mut shown = String::with_capacity(source.len());
    let mut column = 0;
    let mut clipped = source.len();
    for (count, (offset, ch)) in source.char_indices().enumerate() {
        if count == LINE_CLIP {
            clipped = offset;
            break;
        }
        at[offset] = shown.len();
        if ch == '\t' {
            let spaces = TAB_WIDTH - column % TAB_WIDTH;
            shown.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            shown.push(ch);
            column += 1;
        }
    }
    let end = shown.len();
    for slot in &mut at[clipped..] {
        *slot = end;
    }
    let map = |offset: usize| at[offset.clamp(line.start, line.start + clipped) - line.start];
    // Every place the style can change, within the shown part of the line.
    let visible = line.start..line.start + clipped;
    let first_span = text
        .spans
        .partition_point(|(range, _)| range.end <= visible.start);
    let first_match = matches.partition_point(|range| range.end <= visible.start);
    let mut cuts = vec![visible.start, visible.end];
    let spans: &[Span] = &text.spans[first_span..];
    for (range, _) in spans
        .iter()
        .take_while(|(range, _)| range.start < visible.end)
    {
        cuts.extend([range.start, range.end]);
    }
    for range in matches[first_match..]
        .iter()
        .take_while(|range| range.start < visible.end)
    {
        cuts.extend([range.start, range.end]);
    }
    cuts.retain(|cut| visible.contains(cut) || *cut == visible.end);
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs: Vec<(Range<usize>, RunStyle)> = Vec::new();
    for pair in cuts.windows(2) {
        let (start, stop) = (pair[0], pair[1]);
        let token = spans
            .iter()
            .take_while(|(range, _)| range.start <= start)
            .find(|(range, _)| range.contains(&start))
            .map(|(_, token)| *token);
        let found = match matches[first_match..]
            .iter()
            .take_while(|range| range.start <= start)
            .find(|range| range.contains(&start))
        {
            Some(range) if Some(range) == current => 2,
            Some(_) => 1,
            None => 0,
        };
        let style = RunStyle { token, found };
        let range = map(start)..map(stop);
        if style == RunStyle::default() || range.is_empty() {
            continue;
        }
        match runs.last_mut() {
            Some((last, previous)) if last.end == range.start && *previous == style => {
                last.end = range.end
            }
            _ => runs.push((range, style)),
        }
    }
    (shown.into(), runs)
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

/// The find bar's count, shown and spoken: nothing before a query, then
/// "No matches" or "3 of 17", with "+" once the match cap is reached.
fn find_status(query_empty: bool, current: usize, count: usize) -> String {
    if query_empty {
        String::new()
    } else if count == 0 {
        "No matches".into()
    } else {
        format!(
            "{} of {}{}",
            current + 1,
            count,
            if count >= FIND_LIMIT { "+" } else { "" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plait_core::file_view::prepare;

    #[core::prelude::v1::test]
    fn find_status_names_the_current_match_and_the_cap() {
        assert_eq!(find_status(true, 0, 5), "");
        assert_eq!(find_status(false, 0, 0), "No matches");
        assert_eq!(find_status(false, 0, 3), "1 of 3");
        assert_eq!(find_status(false, 2, 3), "3 of 3");
        assert_eq!(
            find_status(false, 0, FIND_LIMIT),
            format!("1 of {FIND_LIMIT}+")
        );
    }

    #[core::prelude::v1::test]
    fn tabs_expand_and_styles_follow_them() {
        let text = prepare("a\tfn x\n".into(), "a.rs");
        let (shown, runs) = line_runs(&text, 0, &[], None);
        assert_eq!(shown.as_ref(), "a   fn x");
        // `fn` moved from byte 2 to display 4 with the tab.
        assert!(
            runs.iter()
                .any(|(range, style)| *range == (4..6) && style.token == Some(Token::Keyword)),
            "{runs:?}"
        );
    }

    #[core::prelude::v1::test]
    fn matches_mark_their_runs_and_the_current_one_differs() {
        let text = prepare("one two one\n".into(), "notes.txt");
        let matches = vec![0..3, 8..11];
        let (_, runs) = line_runs(&text, 0, &matches, Some(&matches[1]));
        assert_eq!(
            runs,
            vec![
                (
                    0..3,
                    RunStyle {
                        token: None,
                        found: 1
                    }
                ),
                (
                    8..11,
                    RunStyle {
                        token: None,
                        found: 2
                    }
                ),
            ]
        );
    }

    #[core::prelude::v1::test]
    fn matches_use_the_neutral_selection_fill_and_the_current_one_is_underlined() {
        let fill = colors().selection;
        let found = |found| {
            RunStyle {
                token: Some(Token::Keyword),
                found,
            }
            .highlight()
        };
        let (other, current) = (found(1), found(2));
        assert_eq!(other.background_color, Some(rgba(fill).into()));
        assert_eq!(
            current.background_color,
            Some(rgba(fill & !0xff | ((fill & 0xff) * 2).min(0xff)).into())
        );
        assert!(other.underline.is_none() && current.underline.is_some());
        // The syntax color stays; only the fill and underline mark a match.
        assert_eq!(other.color, found(0).color);
        assert_eq!(found(0).background_color, None);
    }

    #[core::prelude::v1::test]
    fn long_lines_are_clipped_on_a_character_boundary() {
        let text = prepare("é".repeat(LINE_CLIP + 10), "a.txt");
        let (shown, _) = line_runs(&text, 0, &[0..4, 4..8], None);
        assert_eq!(shown.chars().count(), LINE_CLIP);
    }
}
