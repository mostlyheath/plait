use super::{
    OpenRepository, Plait, host::WatchLease, text_field::TextField, theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_client::{
    host::{Request, ThreadReply, ThreadSummary},
    navigation as nav,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct Navigation {
    cached_rows: HashMap<String, (ThreadRowFlags, Entity<ThreadCard>)>,
    cached_rows_revision: Option<u64>,
    row_cache: RefCell<RowCache>,
    layout: RefCell<TreeLayout>,
    pub scroll: ListState,
    pub collapsed: HashSet<PathBuf>,
    pub available: HashMap<PathBuf, bool>,
    pub archived: bool,
    pub rename: Option<(String, Entity<TextField>)>,
    pub menu: Option<String>,
    pub managing: Option<ThreadManagement>,
    management_generation: u64,
    pub error: Option<String>,
    pub loading: bool,
    pub generation: u64,
    pub watch: Option<WatchLease>,
}

impl Navigation {
    pub fn new() -> Self {
        Self {
            cached_rows: HashMap::new(),
            cached_rows_revision: None,
            row_cache: RefCell::new(RowCache::default()),
            layout: RefCell::new(TreeLayout::default()),
            scroll: ListState::new(0, ListAlignment::Top, px(120.)),
            collapsed: HashSet::new(),
            available: HashMap::new(),
            archived: false,
            rename: None,
            menu: None,
            managing: None,
            management_generation: 0,
            error: None,
            loading: false,
            generation: 0,
            watch: None,
        }
    }
}

#[derive(Default)]
struct RowCache {
    key: Option<RowCacheKey>,
    needs_me: usize,
    activity: nav::MachineActivity,
    indices: HashMap<String, usize>,
    indices_revision: Option<u64>,
    projects: Option<HashMap<PathBuf, usize>>,
    rows: Rc<Vec<Row>>,
    filtered: Vec<usize>,
    #[cfg(test)]
    recomputations: usize,
}

#[derive(PartialEq, Eq)]
struct RowCacheKey {
    threads_revision: u64,
    archived: bool,
    draft: bool,
    checkout: PathBuf,
}

impl Navigation {
    #[cfg(test)]
    pub(super) fn cached_row_flags(&self, id: &str) -> Option<(bool, bool)> {
        self.cached_rows
            .get(id)
            .map(|(key, _)| (key.unread, key.worktree))
    }

    #[cfg(test)]
    pub(super) fn row_recomputations(&self) -> usize {
        self.row_cache.borrow().recomputations
    }

    pub(super) fn invalidate_rows(&self) {
        self.row_cache.borrow_mut().key = None;
    }

    pub(super) fn invalidate_projects(&self) {
        let mut cache = self.row_cache.borrow_mut();
        cache.key = None;
        cache.projects = None;
    }

    pub(super) fn update_cached_list_flags(
        &mut self,
        lists: &nav::Preferences,
        cx: &mut Context<Plait>,
    ) {
        self.invalidate_rows();
        for (id, (key, view)) in &mut self.cached_rows {
            let unread = lists.unread.contains(id);
            let worktree = lists.worktree(&key.checkout).is_some();
            if key.unread != unread || key.worktree != worktree {
                key.unread = unread;
                key.worktree = worktree;
                view.update(cx, |_, cx| cx.notify());
            }
        }
    }
}

#[derive(Clone)]
struct ThreadRowFlags {
    checkout: PathBuf,
    unread: bool,
    worktree: bool,
}
struct ThreadCard {
    owner: WeakEntity<Plait>,
    id: String,
    focus: FocusHandle,
}
impl Render for ThreadCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |p, cx| p.thread_card(&self.id, &self.focus, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum RowId {
    Project(PathBuf),
    Thread(String),
    Draft,
}

#[derive(Default)]
struct TreeLayout {
    rows: Rc<Vec<Row>>,
    items: Rc<Vec<(RowId, FocusHandle)>>,
    editing: Option<String>,
}

#[derive(Clone)]
pub(super) struct ThreadManagement {
    epoch: u64,
    generation: u64,
    id: String,
    rename: Option<(Entity<TextField>, String)>,
}

#[derive(Clone, Copy)]
pub(super) enum Row {
    /// A project, whether it follows another group, and its thread count when collapsed.
    Project {
        index: usize,
        after: bool,
        folded: Option<usize>,
    },
    Thread(usize),
    /// The new-thread draft, at the top of its project's threads.
    Draft,
}

/// One-line title that gives way to the trailing state.
fn title(text: impl Into<SharedString>) -> Div {
    div().flex_1().min_w_0().truncate().child(text.into())
}

fn card_detail_line() -> Div {
    row()
        .pl(px(15.))
        .gap(px(5.))
        .line_height(px(14.))
        .text_size(px(11.))
        .text_color(rgb(colors().subtle))
}

pub(super) fn card_mark(
    state: nav::ThreadRowState,
    color: u32,
    dot_size: Pixels,
    connected: bool,
) -> AnyElement {
    use nav::ThreadRowState::*;
    match state {
        Working if connected => running_mark(px(13.)).into_any_element(),
        Queued(_) | Other => div()
            .size(dot_size)
            .rounded_full()
            .border_1()
            .border_color(rgb(color))
            .into_any_element(),
        Working | Approval | Failed | Interrupted | FinishedUnread => {
            dot(color).size(dot_size).into_any_element()
        }
    }
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Plait {
    pub(super) fn active_project(&self) -> &plait_client::model::Project {
        let mut cache = self.navigation.row_cache.borrow_mut();
        let projects = cache.projects.get_or_insert_with(|| {
            self.state
                .projects
                .iter()
                .enumerate()
                .map(|(index, project)| (project.checkout.clone(), index))
                .collect()
        });
        projects
            .get(self.project_of(self.visible_project_checkout()))
            .and_then(|index| self.state.projects.get(*index))
            .or(self.state.projects.first())
            .unwrap_or(&self.no_project)
    }

    /// The project a checkout lists under; a Plait worktree belongs to its repository.
    pub(super) fn project_of<'a>(&'a self, checkout: &'a std::path::Path) -> &'a std::path::Path {
        self.lists().project_of(checkout)
    }

    pub(super) fn thread_index(&self, id: &str) -> Option<usize> {
        let revision = self.host.connection.threads_revision();
        let mut cache = self.navigation.row_cache.borrow_mut();
        if cache.indices_revision != Some(revision) {
            cache.indices = self
                .host
                .connection
                .threads()
                .iter()
                .enumerate()
                .map(|(index, thread)| (thread.id.clone(), index))
                .collect();
            cache.indices_revision = Some(revision);
        }
        cache.indices.get(id).copied()
    }

    pub(super) fn filtered_threads(&self, _: &App) -> Vec<usize> {
        self.ensure_navigation_rows();
        self.navigation.row_cache.borrow().filtered.clone()
    }

    pub(super) fn navigation_rows(&self, _: &App) -> Rc<Vec<Row>> {
        self.ensure_navigation_rows();
        self.navigation.row_cache.borrow().rows.clone()
    }

    pub(super) fn activity(&self) -> nav::MachineActivity {
        self.ensure_navigation_rows();
        self.navigation.row_cache.borrow().activity
    }

    fn ensure_navigation_rows(&self) {
        let key = RowCacheKey {
            threads_revision: self.host.connection.threads_revision(),
            archived: self.navigation.archived,
            draft: self.draft.open,
            checkout: self.host.creation.checkout.clone(),
        };
        let mut cache = self.navigation.row_cache.borrow_mut();
        if cache.key.as_ref() == Some(&key) {
            return;
        }
        cache.activity =
            nav::machine_activity(self.host.connection.threads(), &self.lists().unread);
        cache.needs_me = nav::needs_me_count(self.host.connection.threads(), &self.lists().unread);
        let mut grouped = HashMap::<&std::path::Path, Vec<usize>>::new();
        for (index, thread) in self.host.connection.threads().iter().enumerate() {
            if !key.archived && thread.archived == Some(true) {
                continue;
            }
            grouped
                .entry(self.project_of(&thread.checkout))
                .or_default()
                .push(index);
        }
        let mut filtered = Vec::new();
        let drafted = key.draft.then(|| self.project_of(&key.checkout));
        let mut rows = Vec::new();
        for (pi, project) in self.state.projects.iter().enumerate() {
            let matching = grouped.get(project.checkout.as_path());
            let draft = drafted == Some(project.checkout.as_path());
            let open = !self.navigation.collapsed.contains(&project.checkout);
            rows.push(Row::Project {
                index: pi,
                after: !rows.is_empty(),
                folded: (!open)
                    .then(|| matching.map_or(0, Vec::len))
                    .filter(|count| *count > 0),
            });
            if draft {
                rows.push(Row::Draft);
            }
            if let Some(indices) = grouped.get_mut(project.checkout.as_path()) {
                nav::sort_cards(
                    indices,
                    self.host.connection.threads(),
                    &self.lists().unread,
                );
                filtered.extend_from_slice(indices);
                if open {
                    rows.extend(indices.iter().copied().map(Row::Thread));
                }
            }
        }
        cache.filtered = filtered;
        cache.rows = Rc::new(rows);
        cache.key = Some(key);
        #[cfg(test)]
        {
            cache.recomputations += 1;
        }
    }

    fn project_row(
        &self,
        pi: usize,
        after: bool,
        folded: Option<usize>,
        focus: &FocusHandle,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = &self.state.projects[pi];
        let checkout = project.checkout.clone();
        let hide = checkout.clone();
        let start = checkout.clone();
        let group = SharedString::from(format!("project-row-{pi}"));
        let expanded = !self.navigation.collapsed.contains(&checkout);
        let unavailable = self.navigation.available.get(&checkout) == Some(&false);
        column()
            .w_full()
            .when(after, |el| el.pt(px(12.)))
            .child(
                row()
                    .group(group.clone())
                    .h(px(26.))
                    .gap_0()
                    .child(
                        nav_row(
                            ("project", pi),
                            format!(
                                "{} project, {}{}",
                                project.name,
                                checkout.display(),
                                if unavailable {
                                    ", folder unavailable"
                                } else {
                                    ""
                                }
                            ),
                            false,
                        )
                        .w_auto()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .px(px(9.))
                        .track_focus(focus)
                        .role(Role::TreeItem)
                        .aria_level(1)
                        .aria_expanded(expanded)
                        .child(icon(if expanded { "down" } else { "right" }).size(px(9.)))
                        .child(
                            title(project.name.clone())
                                .text_size(px(11.))
                                .text_color(rgb(colors().subtle))
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .when(unavailable, |el| {
                            el.child(tag("project-missing", "Missing", colors().amber))
                        })
                        .when_some(folded, |el, count| {
                            el.child(
                                tag("project-folded-count", count.to_string(), colors().subtle)
                                    .flex_shrink_0(),
                            )
                        })
                        .on_activate(cx, move |this, _, cx| {
                            if !this.navigation.collapsed.remove(&checkout) {
                                this.navigation.collapsed.insert(checkout.clone());
                            }
                            this.navigation.invalidate_rows();
                            cx.notify();
                        }),
                    )
                    // Out of the way until the row is hovered or the button has focus.
                    .child(
                        icon_button(
                            ("project-new-thread", pi),
                            "plus",
                            format!("New thread in {}", project.name),
                        )
                        .size(px(22.))
                        .opacity(if focused { 1. } else { 0. })
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .focus_visible(|s| s.border_color(rgb(colors().blue)).opacity(1.))
                        .tooltip(|_, cx| cx.new(|_| TextTooltip("New thread here".into())).into())
                        .on_activate(cx, move |this, window, cx| {
                            this.close_overview(window, cx);
                            this.open_draft_in(start.clone(), window, cx)
                        }),
                    )
                    .child(
                        icon_button(
                            ("project-actions", pi),
                            "more",
                            format!("Actions for {} project", project.name),
                        )
                        .size(px(22.))
                        .opacity(if focused { 1. } else { 0. })
                        .group_hover(group, |s| s.opacity(1.))
                        .focus_visible(|s| s.border_color(rgb(colors().blue)).opacity(1.))
                        .on_activate(cx, move |this, _, cx| {
                            this.navigation.menu = Some(format!("project:{}", hide.display()));
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }

    fn thread_card_entity(
        &mut self,
        index: usize,
        focus: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let revision = self.host.connection.threads_revision();
        let threads = self.host.connection.threads();
        if self.navigation.cached_rows_revision != Some(revision) {
            let ids: HashSet<_> = threads.iter().map(|thread| thread.id.as_str()).collect();
            self.navigation
                .cached_rows
                .retain(|id, _| ids.contains(id.as_str()));
            self.navigation.cached_rows_revision = Some(revision);
        }
        let thread = &threads[index];
        let flags = ThreadRowFlags {
            checkout: thread.checkout.clone(),
            unread: self.lists().unread.contains(&thread.id),
            worktree: self.lists().worktree(&thread.checkout).is_some(),
        };
        let id = thread.id.clone();
        let (old, view) = self
            .navigation
            .cached_rows
            .entry(id.clone())
            .or_insert_with(|| {
                let owner = cx.weak_entity();
                let focus = focus.clone();
                let view = cx.new(|_| ThreadCard { owner, id, focus });
                (flags.clone(), view)
            });
        *old = flags;
        if view.read(cx).focus != *focus {
            view.update(cx, |card, _| card.focus = focus.clone());
        }
        // Keep the entity's accessibility IDs through reorder and scrolling.
        // The view itself must render every frame to contribute its nodes.
        view.clone().into_any_element()
    }

    fn thread_card(&self, id: &str, focus: &FocusHandle, cx: &mut Context<Self>) -> AnyElement {
        let Some(index) = self.thread_index(id) else {
            return div().into_any_element();
        };
        let thread = &self.host.connection.threads()[index];
        let selected = self.host.thread.selected.as_deref() == Some(id);
        if let Some((editing, field)) = &self.navigation.rename
            && editing == id
        {
            return row()
                .id("rename-card")
                .track_focus(focus)
                .w_full()
                .h(px(ROW_HEIGHT + 2.))
                .gap_1()
                .pl_2()
                .child(div().flex_1().min_w_0().child(field.clone()))
                .child(
                    icon_button("rename-save", "right", "Save thread name")
                        .on_activate(cx, |this, window, cx| this.finish_rename(window, cx)),
                )
                .child(
                    icon_button("rename-cancel", "close", "Cancel rename").on_activate(
                        cx,
                        |this, window, cx| {
                            this.navigation.rename = None;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        },
                    ),
                )
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if this
                        .navigation
                        .rename
                        .as_ref()
                        .is_some_and(|(_, field)| field.read(cx).is_composing())
                    {
                        return;
                    }
                    if event.keystroke.key == "enter" {
                        this.finish_rename(window, cx);
                        cx.stop_propagation();
                    } else if event.keystroke.key == "escape" {
                        this.navigation.rename = None;
                        window.focus(&this.focus, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .into_any_element();
        }
        let menu_id = id.to_owned();
        let context_id = menu_id.clone();
        let id = menu_id.clone();
        let card = nav::Card::new(
            thread,
            self.lists(),
            self.summary_title(thread),
            self.card_model_name(thread),
            now_ms(),
        );
        let palette = colors();
        use nav::ThreadRowState::*;
        let state_color = thread_row_colors(card.state, selected).0;
        let emphasis = !matches!(card.state, Queued(_) | Other);
        let actions = self.host.protocol() >= 3;
        let tooltip = thread.checkout_queue.as_ref().map_or_else(
            || card.title.clone(),
            |queue| format!("{} · {}", card.title, queue.detail()),
        );
        let name = format!(
            "Thread {id} · {}. {}{}{}",
            card.spoken_state,
            card.title,
            if card.branch.is_some() {
                ". In its own worktree"
            } else {
                ""
            },
            if card.unread { ". Unread activity" } else { "" },
        );
        column()
            .w_full()
            .pb(px(2.))
            .relative()
            .group("thread-card")
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    if this.host.protocol() >= 3 {
                        this.navigation.menu = Some(context_id.clone());
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                nav_row("host-thread", name, selected)
                    .role(Role::TreeItem)
                    .aria_level(2)
                    .aria_selected(selected)
                    .track_focus(focus)
                    .h_auto()
                    .flex_col()
                    .items_stretch()
                    // Include the one-point outline in the specified padding.
                    .px(px(8.))
                    .pt(px(6.))
                    .pb(px(7.))
                    .gap(px(3.))
                    .when(selected, |el| {
                        el.border_color(rgb(palette.edge))
                            .hover(|s| s.bg(rgba(colors().hover)))
                            .active(|s| s.opacity(0.6))
                    })
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(tooltip.clone())).into())
                    .child(
                        row()
                            .line_height(px(16.))
                            .gap(px(2.))
                            .when(actions, |el| el.pr(px(19.)))
                            .child(row().w(px(13.)).justify_center().flex_shrink_0().child(
                                card_mark(
                                    card.state,
                                    state_color,
                                    px(7.),
                                    self.host.connection.connected,
                                ),
                            ))
                            .child(
                                title(card.title)
                                    .text_size(px(12.))
                                    .when(emphasis, |el| {
                                        el.text_color(rgb(palette.strong))
                                            .font_weight(FontWeight::MEDIUM)
                                    })
                                    .when(!emphasis, |el| el.text_color(rgb(palette.muted))),
                            )
                            .when(card.archived, |el| {
                                el.child(
                                    tag("thread-archived", "Archived", palette.subtle)
                                        .flex_shrink_0(),
                                )
                            })
                            .when_some(card.word, |el, word| {
                                el.child(
                                    tag("thread-status", word, state_color)
                                        .font_weight(FontWeight::MEDIUM)
                                        .flex_shrink_0(),
                                )
                            }),
                    )
                    .when_some(card.branch, |el, branch| {
                        el.child(
                            card_detail_line()
                                .font_family(mono())
                                .child(icon("branch").size(px(11.)).text_color(rgb(palette.subtle)))
                                .child(title(branch.to_owned())),
                        )
                    })
                    .when_some(card.details, |el, details| {
                        el.child(
                            card_detail_line()
                                .when_some(details.harness, |el, harness| {
                                    if let Some(logo) = harness_logo(harness) {
                                        el.child(logo.size(px(11.)).text_color(rgb(palette.subtle)))
                                    } else {
                                        el.child(div().child(harness.label()))
                                    }
                                })
                                .when(details.model.is_none(), |el| el.child(div().flex_1()))
                                .when_some(details.model, |el, model| {
                                    el.child(title(model.into_owned()))
                                })
                                .when_some(details.age, |el, age| {
                                    el.child(div().flex_shrink_0().font_family(mono()).child(age))
                                }),
                        )
                    })
                    .on_activate(cx, move |this, window, cx| {
                        this.close_overview(window, cx);
                        this.select_host_thread(id.clone(), window, cx)
                    }),
            )
            .when(actions, |el| {
                el.child(
                    icon_button(
                        "thread-actions",
                        "more",
                        format!("Actions for {}", self.thread_name(&menu_id)),
                    )
                    .absolute()
                    .top(px(5.))
                    .right(px(4.))
                    .size(px(20.))
                    .opacity(0.)
                    .group_hover("thread-card", |s| s.opacity(1.))
                    .focus_visible(|s| focused_control(s).opacity(1.))
                    .on_activate(cx, move |this, _, cx| {
                        this.navigation.menu = Some(menu_id.clone());
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
            })
            .into_any_element()
    }

    pub(super) fn card_model_name(&self, thread: &ThreadSummary) -> Option<&str> {
        // A card can belong to a different harness from the selected thread.
        thread
            .harness
            .and_then(|harness| self.harness_model(harness, &thread.model))
            .or_else(|| {
                self.host
                    .connection
                    .status
                    .as_ref()?
                    .models
                    .iter()
                    .find(|model| {
                        model.provider == thread.model.provider
                            && model.model_id == thread.model.model_id
                    })
            })
            .map(|model| model.name.as_str())
    }

    fn tree_items(&self, rows: &Rc<Vec<Row>>, cx: &App) -> Rc<Vec<(RowId, FocusHandle)>> {
        let scroll = &self.navigation.scroll;
        let mut layout = self.navigation.layout.borrow_mut();
        if !Rc::ptr_eq(&layout.rows, rows) {
            let top = scroll.logical_scroll_top();
            let anchor = layout.items.get(top.item_ix).map(|(id, _)| id);
            let handles: HashMap<_, _> =
                layout.items.iter().map(|(id, focus)| (id, focus)).collect();
            let items: Vec<_> = rows
                .iter()
                .map(|row| {
                    let id = match *row {
                        Row::Project { index, .. } => {
                            RowId::Project(self.state.projects[index].checkout.clone())
                        }
                        Row::Thread(index) => {
                            RowId::Thread(self.host.connection.threads()[index].id.clone())
                        }
                        Row::Draft => RowId::Draft,
                    };
                    let focus = handles.get(&id).map_or_else(
                        || cx.focus_handle().tab_stop(true),
                        |focus| (*focus).clone(),
                    );
                    (id, focus)
                })
                .collect();
            if items
                .iter()
                .map(|(id, _)| id)
                .eq(layout.items.iter().map(|(id, _)| id))
            {
                scroll.remeasure();
            } else {
                scroll.splice_focusable(
                    0..scroll.item_count(),
                    items.iter().map(|(_, handle)| Some(handle.clone())),
                );
                if top != ListOffset::default() && !items.is_empty() {
                    let index =
                        anchor.and_then(|anchor| items.iter().position(|(id, _)| id == anchor));
                    scroll.scroll_to(index.map_or(
                        ListOffset {
                            item_ix: top.item_ix.min(items.len() - 1),
                            offset_in_item: px(0.),
                        },
                        |item_ix| ListOffset { item_ix, ..top },
                    ));
                }
            }
            layout.rows = rows.clone();
            layout.items = Rc::new(items);
        }
        let editing = self.navigation.rename.as_ref().map(|(id, _)| id);
        if editing != layout.editing.as_ref() {
            for (index, (id, _)) in layout.items.iter().enumerate() {
                if let RowId::Thread(id) = id
                    && (Some(id) == editing || Some(id) == layout.editing.as_ref())
                {
                    scroll.remeasure_items(index..index + 1);
                }
            }
            layout.editing = editing.cloned();
        }
        layout.items.clone()
    }

    fn reveal_tree_item(&self, index: usize) {
        let scroll = &self.navigation.scroll;
        if scroll.bounds_for_item(index).is_some() {
            scroll.scroll_to_reveal_item(index);
        } else {
            // A distant card may have no measured position yet.
            scroll.scroll_to(ListOffset {
                item_ix: index,
                offset_in_item: px(0.),
            });
        }
    }

    fn tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.navigation_rows(cx);
        let items = self.tree_items(&rows, cx);
        let empty = rows.is_empty();
        let keyboard = items.clone();
        column()
            .id("project-tree")
            .role(Role::Tree)
            .aria_label("Projects and threads")
            .audited()
            .flex_1()
            .min_h_0()
            .px_2()
            .py_2()
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let step = match event.keystroke.key.as_str() {
                    "up" => -1,
                    "down" => 1,
                    _ => return,
                };
                if event.keystroke.modifiers != Modifiers::default() {
                    return;
                }
                if let Some(index) = keyboard
                    .iter()
                    .position(|(_, focus)| focus.is_focused(window))
                {
                    cx.stop_propagation();
                    if let Some(next) = index.checked_add_signed(step)
                        && let Some((_, focus)) = keyboard.get(next)
                    {
                        this.reveal_tree_item(next);
                        window.focus(focus, cx);
                        cx.notify();
                    }
                }
            }))
            .when(empty, |el| {
                el.child(tag("navigation-empty", "No matching threads.", colors().muted).p_2())
            })
            .when(!empty, |el| {
                el.child(
                    list(
                        self.navigation.scroll.clone(),
                        cx.processor(move |this, position, window, cx| {
                            let (_, focus) = &items[position];
                            match rows[position] {
                                Row::Project {
                                    index,
                                    after,
                                    folded,
                                } => this.project_row(
                                    index,
                                    after,
                                    folded,
                                    focus,
                                    focus.is_focused(window),
                                    cx,
                                ),
                                Row::Thread(index) => this.thread_card_entity(index, focus, cx),
                                Row::Draft => this.draft_row(focus, cx),
                            }
                        }),
                    )
                    .size_full(),
                )
            })
    }

    /// Reads the thread list and folder state again, for threads older than
    /// the host streams and folders that came back.
    pub(super) fn refresh_thread_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigation.available.clear();
        self.load_threads(window, cx);
    }

    /// Lists every hidden project of the selected machine again.
    pub(super) fn show_hidden_projects(&mut self, cx: &mut Context<Self>) {
        // Only the hidden flags change: opening a project asks the host to
        // check its folder, and one missing folder would stop the rest.
        self.update_lists(|lists| lists.hidden.clear(), cx);
        self.rebuild_projects(cx);
    }

    /// Notices about the list itself. Its tools are menu and palette actions.
    fn navigation_controls(&self) -> impl IntoElement {
        column()
            .px_2()
            .pt_2()
            .gap_2()
            .when(self.navigation.managing.is_some(), |el| {
                el.child(tag("navigation-saving", "Saving thread…", colors().subtle))
            })
            .when(self.navigation.loading, |el| {
                el.child(tag(
                    "navigation-loading",
                    "Loading threads…",
                    colors().subtle,
                ))
            })
            .when(self.host.connection.threads().len() > 200, |el| {
                el.child(tag(
                    "navigation-limit",
                    "Newest 200 live · Refresh older threads",
                    colors().subtle,
                ))
            })
            .when_some(self.navigation.error.clone(), |el, error| {
                el.child(
                    tag("navigation-error-text", error, colors().amber)
                        .id("navigation-error")
                        .role(Role::Alert)
                        .audited()
                        .line_height(px(16.)),
                )
            })
    }

    fn navigation_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = &self.navigation.menu else {
            return div().into_any_element();
        };
        let mut panel = column()
            .mx_2()
            .mb_2()
            .p_2()
            .gap_2()
            .border_1()
            .border_color(rgb(colors().edge))
            .rounded(px(SURFACE_CORNER))
            .shadow(floating_shadow())
            .bg(rgb(colors().raised));
        // The machine menu opens above its control in the foot.
        if id == "machines" {
            return div().into_any_element();
        }
        if let Some(path) = id.strip_prefix("project:") {
            let checkout = PathBuf::from(path);
            let select = checkout.clone();
            let configure = checkout.clone();
            let trees = checkout.clone();
            let issues = checkout.clone();
            let hide = checkout.clone();
            panel = panel.child(tag("project-menu-heading", "Project", colors().muted))
                .child(button("project-open", "Open project").on_activate(cx, move |this, window, cx| { this.navigation.menu = None; this.change_checkout(select.clone(), window, cx); }))
                .child(button("project-worktrees", "Worktrees").on_activate(cx, move |this,window,cx| this.open_worktrees(trees.clone(),window,cx)))
                .child(button("project-issues", "Linear issues").on_activate(cx, move |this,window,cx| this.open_issues(issues.clone(),window,cx)))
                .child(button("project-settings", "Project settings…").on_activate(cx, move |this, window, cx| {
                    this.navigation.menu = None;
                    this.show_settings_for_project(configure.clone(), window, cx);
                }))
                .child(button("project-hide", "Remove from sidebar").on_activate(cx, move |this, _, cx| {
                    if this.project_of(&this.host.creation.checkout) == hide { this.navigation.error = Some("Open another project before removing this one. Host threads are kept.".into()); }
                    else { this.update_lists(|lists| lists.hide(&hide), cx); this.rebuild_projects(cx); }
                    this.navigation.menu = None; cx.notify();
                }))
                .child(tag("project-menu-note", "Removing a project keeps its host threads.", colors().subtle));
        } else if let Some(thread) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == *id)
        {
            let rename = id.clone();
            let archive = id.clone();
            let archived = thread.archived == Some(true);
            let worktrees = thread.checkout.clone();
            let tree = self.lists().worktree(&thread.checkout);
            let branch = tree.map(|tree| tree.branch.clone());
            let remove = thread.checkout.clone();
            panel = panel
                .child(tag("thread-menu-title", self.thread_title(id), colors().strong).truncate())
                .child(
                    button("thread-rename", "Rename thread…")
                        .on_activate(cx, move |this, window, cx| {
                            this.begin_rename(rename.clone(), window, cx)
                        }),
                )
                .child(
                    button(
                        "thread-archive",
                        if archived {
                            "Unarchive thread"
                        } else {
                            "Archive thread"
                        },
                    )
                    .on_activate(cx, move |this, window, cx| {
                        this.archive_thread(archive.clone(), !archived, window, cx)
                    }),
                )
                .when_some(branch, |el, branch| {
                    el.child(
                        tag(
                            "thread-menu-worktree",
                            format!("Worktree on {branch}"),
                            colors().subtle,
                        )
                        .truncate(),
                    )
                    .child(
                        button("thread-worktrees", "Worktrees").on_activate(
                            cx,
                            move |this, window, cx| {
                                this.open_worktrees(worktrees.clone(), window, cx)
                            },
                        ),
                    )
                    .child(
                        button("thread-remove-worktree", "Remove worktree…")
                            .disabled(self.worktree_busy)
                            .on_activate(cx, move |this, window, cx| {
                                this.remove_worktree(remove.clone(), window, cx)
                            }),
                    )
                });
        }
        panel
            .child(
                button("navigation-menu-close", "Close").on_activate(cx, |this, _, cx| {
                    this.navigation.menu = None;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    pub(super) fn begin_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.protocol() < 3 || !self.host.connection.connected {
            return;
        }
        if !self.visible_sidebar() {
            self.toggle_sidebar(window, cx);
        }
        if let Some(thread) = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
        {
            let project = self.project_of(&thread.checkout).to_path_buf();
            self.navigation.collapsed.remove(&project);
            if thread.archived == Some(true) {
                self.navigation.archived = true;
            }
        }
        self.navigation.invalidate_rows();
        let title = self.rename_text(&id);
        let field = cx.new(|cx| {
            let mut field = TextField::new("Thread name", "Name this thread", cx);
            field.set_text(&title, cx);
            field
        });
        window.focus(&field.focus_handle(cx), cx);
        self.navigation.rename = Some((id.clone(), field));
        let rows = self.navigation_rows(cx);
        let items = self.tree_items(&rows, cx);
        if let Some(index) = items
            .iter()
            .position(|(item, _)| matches!(item, RowId::Thread(thread) if thread == &id))
        {
            self.reveal_tree_item(index);
        }
        self.navigation.menu = None;
        self.navigation.error = None;
        cx.notify();
    }

    fn finish_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, field)) = &self.navigation.rename else {
            return;
        };
        let name = field.read(cx).text().trim().to_owned();
        if name.is_empty() || name.chars().count() > 200 {
            self.navigation.error = Some("Use a name between 1 and 200 characters.".into());
            cx.notify();
            return;
        }
        let id = id.clone();
        self.manage_thread(id.clone(), Request::rename_thread(id, name), window, cx);
    }

    pub(super) fn archive_thread(
        &mut self,
        id: String,
        archived: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.manage_thread(
            id.clone(),
            Request::archive_thread(id, archived),
            window,
            cx,
        );
    }

    fn manage_thread(
        &mut self,
        id: String,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(operation) = self.begin_thread_management(id, &request, cx) else {
            return;
        };
        self.host_request::<ThreadReply>(request, window, cx, move |this, reply, window, cx| {
            this.finish_thread_management(operation, reply, window, cx);
        });
    }

    pub(super) fn begin_thread_management(
        &mut self,
        id: String,
        request: &Request,
        cx: &App,
    ) -> Option<ThreadManagement> {
        if self.navigation.managing.is_some()
            || !self.host.connection.connected
            || self.host.protocol() < 3
        {
            return None;
        }
        self.navigation.management_generation += 1;
        let operation = ThreadManagement {
            epoch: self.host.connection.epoch,
            generation: self.navigation.management_generation,
            rename: self.navigation.rename.as_ref().and_then(|(thread, field)| {
                (matches!(request, Request::RenameThread { .. }) && thread == &id)
                    .then(|| (field.clone(), field.read(cx).text().to_owned()))
            }),
            id,
        };
        if self.navigation.menu.as_deref() == Some(&operation.id) {
            self.navigation.menu = None;
        }
        self.navigation.managing = Some(operation.clone());
        self.navigation.error = None;
        Some(operation)
    }

    pub(super) fn finish_thread_management(
        &mut self,
        operation: ThreadManagement,
        reply: Result<ThreadReply, plait_client::host::Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.host.connection.epoch != operation.epoch
            || self
                .navigation
                .managing
                .as_ref()
                .map(|pending| pending.generation)
                != Some(operation.generation)
        {
            return;
        }
        self.navigation.managing = None;
        let result = match reply {
            Ok(reply) if reply.thread.id == operation.id => {
                self.apply_managed_thread(ThreadSummary::from(&reply.thread), cx)
            }
            Ok(_) => Err("Host returned a different thread. Refresh before trying again.".into()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = result {
            self.navigation.error = Some(error);
            cx.notify();
            return;
        }
        // A reply belongs to the field that sent it, not a later edit or focus.
        if let Some((sent_field, sent_text)) = operation.rename
            && self.navigation.rename.as_ref().is_some_and(|(id, field)| {
                id == &operation.id && field == &sent_field && field.read(cx).text() == sent_text
            })
        {
            if sent_field.focus_handle(cx).is_focused(window) {
                window.focus(&self.focus, cx);
            }
            self.navigation.rename = None;
        }
        cx.notify();
    }

    /// The draft under its project: selected while on screen, and discardable here.
    fn draft_row(&self, focus: &FocusHandle, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.draft_visible();
        row()
            .w_full()
            .h(px(ROW_HEIGHT))
            .gap_0()
            .relative()
            .child(
                nav_row(
                    "draft-thread",
                    format!(
                        "{}, not started. Sending its first message creates it here.",
                        self.current_draft_title()
                    ),
                    selected,
                )
                .w_auto()
                .flex_1()
                .min_w_0()
                .role(Role::TreeItem)
                .aria_level(2)
                .aria_selected(selected)
                .track_focus(focus)
                .pl(px(9.))
                .child(icon("plus").size(px(12.)).text_color(rgb(colors().mint)))
                .child(
                    title(self.current_draft_title().to_owned()).text_color(rgb(colors().strong)),
                )
                .child(tag("draft-badge", "Draft", colors().subtle).flex_shrink_0())
                .on_activate(cx, |this, window, cx| {
                    this.show_tab(super::Workspace::Conversation, window, cx);
                    this.focus_composer(window, cx);
                }),
            )
            .child(
                icon_button("draft-discard-row", "close", "Discard new thread")
                    .on_activate(cx, |this, window, cx| this.discard_draft(window, cx)),
            )
            .into_any_element()
    }

    fn shortcut_row(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: &'static str,
        tint: u32,
        action: &'static str,
    ) -> Control {
        // Only the key hint inherits this color. The label and glyph stay visible.
        nav_row(id, label, false)
            .h(px(30.))
            .text_size(px(13.5))
            .text_color(transparent_black())
            .group(id)
            .focus(|s| s.text_color(rgb(colors().subtle)))
            .aria_keyshortcuts(self.shortcut_aria(action))
            .child(icon(glyph).size(px(16.)).text_color(rgb(tint)))
            .child(title(label).text_color(rgb(colors().muted)))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .group_hover(id, |s| s.text_color(rgb(colors().subtle)))
                    .child(self.shortcut_label(action)),
            )
    }

    fn needs_me_row(&self, cx: &mut Context<Self>) -> Control {
        self.ensure_navigation_rows();
        let count = self.navigation.row_cache.borrow().needs_me;
        let tint = if count == 0 {
            colors().subtle
        } else {
            colors().amber
        };
        nav_row(
            "needs-me",
            format!("Needs you, {count}"),
            self.overview.is_some(),
        )
        .h(px(30.))
        .text_size(px(13.5))
        .aria_keyshortcuts(self.shortcut_aria("view.overview"))
        .aria_toggled(if self.overview.is_some() {
            Toggled::True
        } else {
            Toggled::False
        })
        .child(row().w(px(16.)).justify_center().child(dot(tint)))
        .child(title("Needs you"))
        .child(tag("needs-me-count", count.to_string(), tint).font_weight(FontWeight::MEDIUM))
        .on_activate(cx, Self::toggle_overview)
    }

    fn actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        column()
            .flex_shrink_0()
            .px_2()
            .pb_1()
            .gap(px(1.))
            .child(
                self.shortcut_row(
                    "new-local-thread",
                    "New thread",
                    "plus",
                    colors().mint,
                    "thread.new",
                )
                .on_activate(cx, |this, window, cx| {
                    this.close_overview(window, cx);
                    this.open_draft(window, cx)
                }),
            )
            .child(self.needs_me_row(cx))
            .child(
                self.shortcut_row(
                    "find-anything",
                    "Find anything",
                    "search",
                    colors().muted,
                    "palette.toggle",
                )
                .on_activate(cx, |this, window, cx| {
                    this.toggle_command_palette(window, cx)
                }),
            )
    }

    fn open_project_row(&self, cx: &mut Context<Self>) -> Control {
        nav_row("open-repository", "Open project…", false)
            .h(px(30.))
            .aria_keyshortcuts(self.shortcut_aria("repository.open"))
            .child(icon("folder"))
            .child(title("Open project…").text_color(rgb(colors().subtle)))
            .child(tag(
                "open-repository-shortcut",
                self.shortcut_label("repository.open"),
                colors().subtle,
            ))
            .on_activate(cx, |this, window, cx| {
                this.open_repository(&OpenRepository, window, cx)
            })
    }

    pub(super) fn next_needing_me(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((index, _)) = self.activity().next {
            let id = self.host.connection.threads()[index].id.clone();
            self.select_host_thread(id, window, cx);
        }
    }

    fn machine_control(&self, cx: &mut Context<Self>) -> Control {
        let machine = self
            .machines
            .connecting
            .as_ref()
            .and_then(|id| self.preferences.value.machines.get(id))
            .map(|m| m.name.clone())
            .unwrap_or_else(|| self.machine_label());
        let (state, tint) = if self.host.connection.connecting || self.machines.connecting.is_some()
        {
            ("Connecting…", colors().muted)
        } else if self.host.connection.connected {
            ("Connected", colors().mint)
        } else {
            ("Offline", colors().amber)
        };
        let hover = format!("{machine}, {state}. Switch machine…");
        button("machine-switcher", "")
            .h(px(24.))
            .px_1()
            .gap_1()
            .min_w_0()
            .flex_shrink_1()
            .aria_label(hover.clone())
            .aria_expanded(self.navigation.menu.as_deref() == Some("machines"))
            .tooltip(move |_, cx| cx.new(|_| TextTooltip(hover.clone())).into())
            .child(dot(tint))
            .child(div().min_w_0().truncate().child(machine))
            .when(state != "Connected", |el| {
                el.child(div().flex_shrink_0().child(state))
            })
            .on_activate(cx, |this, _, cx| {
                this.navigation.menu = (this.navigation.menu.as_deref() != Some("machines"))
                    .then(|| "machines".into());
                cx.notify();
            })
    }

    /// The machines this device knows, and where to manage them.
    fn machine_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = column()
            .id("sidebar-machines-menu")
            .absolute()
            .bottom(px(38.))
            .left(px(8.))
            .right(px(8.))
            .max_h(relative(0.8))
            .overflow_y_scroll()
            .p_2()
            .gap_1()
            .border_1()
            .border_color(rgb(colors().edge))
            .rounded(px(SURFACE_CORNER))
            .shadow(floating_shadow())
            .bg(rgb(colors().raised))
            .occlude()
            .child(tag("machine-menu-heading", "Machines", colors().muted));
        for (index, machine) in self.preferences.value.machines.list().iter().enumerate() {
            let id = &machine.id;
            let label = if self.machines.connecting.as_ref() == Some(id) {
                format!("{} · Connecting…", machine.name)
            } else if id == &self.machines.active {
                format!("{} ✓", machine.name)
            } else {
                machine.name.clone()
            };
            let enabled = self.machines.connecting.as_ref() != Some(id)
                && (id != &self.machines.active || self.machines.connecting.is_some());
            let id = id.clone();
            panel = panel.child(
                button(("machine-use", index), label)
                    .disabled(!enabled)
                    .when(enabled, |el| {
                        el.on_activate(cx, move |this, window, cx| {
                            this.navigation.menu = None;
                            this.use_machine(id.clone(), window, cx);
                        })
                    }),
            );
        }
        panel.child(button("machine-manage", "Manage machines…").on_activate(
            cx,
            |this, window, cx| {
                this.navigation.menu = None;
                this.settings_tab = super::settings::SettingsTab::Machines;
                this.show_settings(true, window, cx);
            },
        ))
    }

    fn sidebar_foot(&self, cx: &mut Context<Self>) -> Div {
        row()
            .h(px(30.))
            .mx_2()
            .mb_2()
            .gap_1()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(colors().border))
            .child(self.machine_control(cx))
            .child(div().flex_1())
            .when(self.update_available(), |el| {
                el.child(
                    toned_button("sidebar-update", "Update", Tone::Filled)
                        .h(px(24.))
                        .aria_label("Update available. Open Settings to install.")
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                TextTooltip("Update available. Open Settings to install.".into())
                            })
                            .into()
                        })
                        .on_activate(cx, |this, window, cx| {
                            this.navigation.menu = None;
                            this.settings_tab = super::settings::SettingsTab::About;
                            this.show_settings(true, window, cx);
                        }),
                )
            })
            .child(
                icon_button("settings", "sliders", "Settings…")
                    .size(px(24.))
                    .aria_keyshortcuts(self.shortcut_aria("app.settings"))
                    .on_activate(cx, |this, window, cx| this.show_settings(true, window, cx)),
            )
    }

    pub(super) fn navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        column()
            .relative()
            .w(px(self.panel_width(super::panels::Panel::Sidebar)))
            .flex_shrink_0()
            .bg(rgb(colors().rail))
            .child(self.actions(cx))
            .child(self.navigation_controls())
            .child(self.tree(cx))
            .child(self.navigation_menu(cx))
            .child(div().px_2().child(self.open_project_row(cx)))
            .child(self.sidebar_foot(cx))
            .when(self.navigation.menu.as_deref() == Some("machines"), |el| {
                el.child(self.machine_menu(cx))
            })
            .child(self.resize_handle(super::panels::Panel::Sidebar, cx))
    }
}
