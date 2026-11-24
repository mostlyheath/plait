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
    cached_rows: HashMap<String, (ThreadRowKey, Entity<ThreadRow>)>,
    row_cache: RefCell<RowCache>,
    pub scroll: UniformListScrollHandle,
    pub collapsed: HashSet<PathBuf>,
    pub available: HashMap<PathBuf, bool>,
    pub archived: bool,
    pub attention: bool,
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
            row_cache: RefCell::new(RowCache::default()),
            scroll: UniformListScrollHandle::new(),
            collapsed: HashSet::new(),
            available: HashMap::new(),
            archived: false,
            attention: false,
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
    attention: bool,
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
        for (id, (key, view)) in &mut self.cached_rows {
            let unread = lists.unread.contains(id);
            let worktree = lists.worktree(&key.summary.checkout).is_some();
            if key.unread != unread || key.worktree != worktree {
                key.unread = unread;
                key.worktree = worktree;
                view.update(cx, |_, cx| cx.notify());
            }
        }
    }
}

#[derive(PartialEq, Eq)]
struct ThreadRowKey {
    summary: ThreadSummary,
    title: String,
    selected: bool,
    unread: bool,
    worktree: bool,
    protocol: u32,
    theme: u32,
    age: String,
    index: usize,
}
struct ThreadRow {
    owner: WeakEntity<Plait>,
    index: usize,
}
impl Render for ThreadRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |p, cx| p.host_thread_row(self.index, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
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

/// Left edge of the rail that joins a project's threads, under its chevron.
const RAIL_X: f32 = 15.;

/// One-line title that gives way to the trailing state.
fn title(text: impl Into<SharedString>) -> Div {
    div().flex_1().min_w_0().truncate().child(text.into())
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
            attention: self.navigation.attention,
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
            if (!key.archived && thread.archived == Some(true))
                || (key.attention && !nav::needs_attention(thread, &self.lists().unread))
            {
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
            if matching.is_none() && !draft && key.attention {
                continue;
            }
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
            if let Some(indices) = matching {
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = &self.state.projects[pi];
        let checkout = project.checkout.clone();
        let hide = checkout.clone();
        let start = checkout.clone();
        let group = SharedString::from(format!("project-row-{pi}"));
        let expanded = !self.navigation.collapsed.contains(&checkout);
        let unavailable = self.navigation.available.get(&checkout) == Some(&false);
        row()
            .group(group.clone())
            .w_full()
            .h(px(ROW_HEIGHT))
            .gap_0()
            // A hairline between groups; uniform rows leave no room for a gap.
            .when(after, |el| {
                el.border_t_1().border_color(rgb(colors().border))
            })
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
                .role(Role::TreeItem)
                .aria_level(1)
                .aria_expanded(expanded)
                .child(icon(if expanded { "down" } else { "right" }))
                .child(icon("folder").text_color(rgb(if unavailable {
                    colors().amber
                } else {
                    colors().mint
                })))
                .child(
                    title(project.name.clone())
                        .text_color(rgb(colors().strong))
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
                .opacity(0.)
                .group_hover(group, |s| s.opacity(1.))
                .focus_visible(|s| s.border_color(rgb(colors().blue)).opacity(1.))
                .tooltip(|_, cx| cx.new(|_| TextTooltip("New thread here".into())).into())
                .on_activate(cx, move |this, window, cx| {
                    this.open_draft_in(start.clone(), window, cx)
                }),
            )
            .child(
                icon_button(
                    ("project-actions", pi),
                    "more",
                    format!("Actions for {} project", project.name),
                )
                .on_activate(cx, move |this, _, cx| {
                    this.navigation.menu = Some(format!("project:{}", hide.display()));
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn cached_thread_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let thread = &self.host.connection.threads()[index];
        if self
            .navigation
            .rename
            .as_ref()
            .is_some_and(|(id, _)| id == &thread.id)
        {
            return self.host_thread_row(index, cx);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let key = ThreadRowKey {
            summary: thread.clone(),
            title: self.summary_title(thread),
            selected: self.host.thread.selected.as_ref() == Some(&thread.id),
            unread: self.lists().unread.contains(&thread.id),
            worktree: self.lists().worktree(&thread.checkout).is_some(),
            protocol: self.host.protocol(),
            theme: colors().bg,
            age: nav::age(thread.updated_at, now),
            index,
        };
        let id = thread.id.clone();
        if self.navigation.cached_rows.len() >= 128
            && !self.navigation.cached_rows.contains_key(&id)
        {
            self.navigation.cached_rows.clear();
        }
        let view = if let Some((old, view)) = self.navigation.cached_rows.get_mut(&id) {
            if old != &key {
                *old = key;
                view.update(cx, |row, cx| {
                    row.index = index;
                    cx.notify();
                });
            }
            view.clone()
        } else {
            let owner = cx.weak_entity();
            let view = cx.new(|_| ThreadRow { owner, index });
            self.navigation.cached_rows.insert(id, (key, view.clone()));
            view
        };
        // Not `cached`: a view reused from the last frame contributes no
        // accessibility nodes, so every thread row would vanish from them.
        div()
            .w_full()
            .h(px(ROW_HEIGHT))
            .child(view)
            .into_any_element()
    }

    fn host_thread_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let thread = &self.host.connection.threads()[index];
        let id = thread.id.clone();
        let selected = self.host.thread.selected.as_ref() == Some(&id);
        if let Some((editing, field)) = &self.navigation.rename
            && editing == &id
        {
            return row()
                .w_full()
                .h(px(ROW_HEIGHT))
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
        let menu_id = id.clone();
        let context_id = id.clone();
        let unread = self.lists().unread.contains(&id);
        let state = nav::thread_row_state(thread, &self.lists().unread);
        let (node, title_color, detail_color) = thread_row_colors(state, selected);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let age = nav::age(thread.updated_at, now);
        let label = self.summary_title(thread);
        let worktree = self.lists().worktree(&thread.checkout).is_some();
        row()
            .w_full()
            .h(px(ROW_HEIGHT))
            .gap_0()
            .relative()
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
                // Each row is its own view, keyed by thread, so these ids are
                // already unique; an index would change as the list moves.
                nav_row(
                    "host-thread",
                    format!(
                        "Thread {} · {} · {} / {}. {}{}{}",
                        id,
                        state.spoken(thread),
                        thread.model.provider,
                        thread.model.model_id,
                        label,
                        if worktree {
                            ". In its own worktree"
                        } else {
                            ""
                        },
                        if unread { ". Unread activity" } else { "" }
                    ),
                    selected,
                )
                .w_auto()
                .flex_1()
                .min_w_0()
                .when_some(
                    thread
                        .checkout_queue
                        .as_ref()
                        .map(|q| q.detail().to_owned()),
                    |el, detail| {
                        el.tooltip(move |_, cx| cx.new(|_| TextTooltip(detail.clone())).into())
                    },
                )
                .role(Role::TreeItem)
                .aria_level(2)
                .aria_selected(selected)
                .pl(px(RAIL_X - 4.))
                .child(thread_mark(node))
                .child(
                    title(label)
                        .text_color(rgb(title_color))
                        .when(state == nav::ThreadRowState::FinishedUnread, |el| {
                            el.font_weight(FontWeight::MEDIUM)
                        }),
                )
                // Pi needs no mark; another harness's logo says where the thread runs.
                .when_some(
                    thread
                        .harness
                        .filter(|harness| *harness != plait_client::host::harness::HarnessId::Pi)
                        .and_then(harness_logo),
                    |el, logo| el.child(logo.size(px(12.)).text_color(rgb(colors().subtle))),
                )
                // A glyph, not a word: the title needs the width more.
                .when(worktree, |el| {
                    el.child(
                        icon("branch")
                            .size(px(12.))
                            .text_color(rgb(colors().subtle)),
                    )
                })
                .when(thread.archived == Some(true), |el| {
                    el.child(tag("thread-archived", "Archived", colors().subtle))
                })
                .child(
                    tag("thread-status", state.label(&age).to_owned(), detail_color)
                        .flex_shrink_0(),
                )
                .on_activate(cx, move |this, window, cx| {
                    this.select_host_thread(id.clone(), window, cx)
                }),
            )
            .when(self.host.protocol() >= 3, |el| {
                el.child(
                    icon_button(
                        "thread-actions",
                        "more",
                        format!("Actions for {}", self.thread_name(&menu_id)),
                    )
                    .on_activate(cx, move |this, _, cx| {
                        this.navigation.menu = Some(menu_id.clone());
                        cx.notify();
                    }),
                )
            })
            .into_any_element()
    }

    fn tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.navigation_rows(cx);
        let empty = rows.is_empty();
        column()
            .id("project-tree")
            .role(Role::Tree)
            .aria_label("Projects and threads")
            .audited()
            .flex_1()
            .min_h_0()
            .px_2()
            .py_2()
            .when(empty, |el| {
                el.child(tag("navigation-empty", "No matching threads.", colors().muted).p_2())
            })
            .when(!empty, |el| {
                el.child(
                    uniform_list(
                        "navigation-list",
                        rows.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| match rows[index] {
                                    Row::Project {
                                        index,
                                        after,
                                        folded,
                                    } => this.project_row(index, after, folded, cx),
                                    Row::Thread(ti) => this.cached_thread_row(ti, cx),
                                    Row::Draft => this.draft_row(cx),
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.navigation.scroll)
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
        self.navigation.attention = false;
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
        if let Some(index) = self
            .navigation_rows(cx)
            .iter()
            .position(|row| matches!(row, Row::Thread(index) if self.host.connection.threads()[*index].id == id))
        {
            self.navigation
                .scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        let title = self.rename_text(&id);
        let field = cx.new(|cx| {
            let mut field = TextField::new("Thread name", "Name this thread", cx);
            field.set_text(&title, cx);
            field
        });
        window.focus(&field.focus_handle(cx), cx);
        self.navigation.rename = Some((id, field));
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
    fn draft_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.draft_visible();
        row()
            .w_full()
            .h(px(ROW_HEIGHT))
            .gap_0()
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(RAIL_X))
                    .top_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(rgb(colors().edge)),
            )
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
                .pl(px(RAIL_X + 5.))
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
            self.navigation.attention,
        )
        .h(px(30.))
        .text_size(px(13.5))
        .aria_toggled(if self.navigation.attention {
            Toggled::True
        } else {
            Toggled::False
        })
        .child(row().w(px(16.)).justify_center().child(dot(tint)))
        .child(title("Needs you"))
        .child(tag("needs-me-count", count.to_string(), tint).font_weight(FontWeight::MEDIUM))
        .on_activate(cx, |this, _, cx| {
            this.navigation.attention = !this.navigation.attention;
            cx.notify();
        })
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
                .on_activate(cx, |this, window, cx| this.open_draft(window, cx)),
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
