use super::{
    OpenRepository, Plait, host::WatchLease, jump::PreferenceGroup, model_picker::ModelPurpose,
    text_field::TextField, theme::*, thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_core::{
    host::{Request, ThreadReply, ThreadSummary},
    navigation as nav,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct Navigation {
    pub filter: Entity<TextField>,
    cached_rows: HashMap<String, (ThreadRowKey, Entity<ThreadRow>)>,
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
    pub fn new(cx: &mut Context<Plait>) -> Self {
        Self {
            cached_rows: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            filter: cx.new(|cx| {
                TextField::new("Filter projects and threads", "Find a thread…", cx).search()
            }),
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
enum Row {
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
    pub(super) fn active_project(&self) -> &plait_core::model::Project {
        self.state
            .projects
            .iter()
            .find(|project| project.checkout == self.project_of(self.visible_project_checkout()))
            .or(self.state.projects.first())
            .unwrap_or(&self.no_project)
    }

    /// The project a checkout lists under; a Plait worktree belongs to its repository.
    pub(super) fn project_of<'a>(&'a self, checkout: &'a std::path::Path) -> &'a std::path::Path {
        self.lists().project_of(checkout)
    }

    pub(super) fn filtered_threads(&self, cx: &App) -> Vec<usize> {
        let query = self.navigation.filter.read(cx).text();
        let projects = self
            .state
            .projects
            .iter()
            .enumerate()
            .map(|(index, project)| (&project.checkout, (index, project)))
            .collect::<HashMap<_, _>>();
        let mut matches = self
            .host
            .connection
            .threads
            .iter()
            .enumerate()
            .filter_map(|(index, thread)| {
                let checkout = self.project_of(&thread.checkout).to_path_buf();
                let (project_index, project) = projects.get(&checkout)?;
                nav::matches(
                    thread,
                    &self.summary_title(thread),
                    project,
                    query,
                    self.navigation.archived,
                    self.navigation.attention,
                    &self.lists().unread,
                )
                .then_some((*project_index, index))
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|(project, _)| *project);
        matches.into_iter().map(|(_, thread)| thread).collect()
    }

    fn navigation_rows(&self, cx: &App) -> Vec<Row> {
        let mut grouped = HashMap::<&std::path::Path, Vec<usize>>::new();
        for index in self.filtered_threads(cx) {
            grouped
                .entry(self.project_of(&self.host.connection.threads[index].checkout))
                .or_default()
                .push(index);
        }
        let query = self.navigation.filter.read(cx).text().trim().to_lowercase();
        // Where the draft will land stays in view, whatever the filter.
        let drafted = self
            .draft
            .open
            .then(|| self.project_of(&self.host.creation.checkout));
        let mut rows = Vec::new();
        for (pi, project) in self.state.projects.iter().enumerate() {
            let matching = grouped.get(project.checkout.as_path());
            let project_matches = query.is_empty() || project.name.to_lowercase().contains(&query);
            let draft = drafted == Some(project.checkout.as_path());
            if matching.is_none() && !draft && (!project_matches || self.navigation.attention) {
                continue;
            }
            let open = !self.navigation.collapsed.contains(&project.checkout) || !query.is_empty();
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
            if open && let Some(indices) = matching {
                rows.extend(indices.iter().copied().map(Row::Thread));
            }
        }
        rows
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
                        .text_color(rgb(colors().text))
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
        let thread = &self.host.connection.threads[index];
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
            title: self.thread_title(&thread.id),
            selected: self.host.thread.selected.as_ref() == Some(&thread.id),
            unread: self
                .preferences
                .value
                .navigation
                .unread
                .contains(&thread.id),
            worktree: self
                .preferences
                .value
                .navigation
                .worktree(&thread.checkout)
                .is_some(),
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
        let thread = &self.host.connection.threads[index];
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
        let status = nav::state_label(thread);
        let short = if status == "Needs approval" {
            "Approve"
        } else {
            status
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let age = nav::age(thread.updated_at, now);
        let label = self.thread_title(&id);
        let worktree = self.lists().worktree(&thread.checkout).is_some();
        row()
            .w_full()
            .h(px(ROW_HEIGHT))
            .gap_0()
            .relative()
            // Adjacent rows draw one continuous rail, so a project's threads read as its group.
            .child(
                div()
                    .absolute()
                    .left(px(RAIL_X))
                    .top_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(rgb(colors().border)),
            )
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
                        status,
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
                .pl(px(RAIL_X + 9.))
                .child(dot(if unread {
                    colors().blue
                } else {
                    host_status_color(thread.status)
                }))
                .child(
                    title(label)
                        .text_color(rgb(colors().text))
                        .when(unread, |el| el.font_weight(FontWeight::SEMIBOLD)),
                )
                // Pi needs no mark; another harness's logo says where the thread runs.
                .when_some(
                    thread
                        .harness
                        .filter(|harness| *harness != plait_core::host::harness::HarnessId::Pi)
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
                .child(
                    tag(
                        "thread-status",
                        if thread.archived == Some(true) {
                            "Archived"
                        } else {
                            short
                        },
                        if thread.pending_approval == Some(true) {
                            colors().amber
                        } else {
                            colors().muted
                        },
                    )
                    .flex_shrink_0(),
                )
                .when(!age.is_empty() && !thread.status.busy(), |el| {
                    el.child(tag("thread-age", age, colors().subtle).flex_shrink_0())
                })
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
            .aria_label("Projects, machines, and threads")
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

    fn navigation_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        column()
            .px_2()
            .pt_2()
            .gap_2()
            .child(self.navigation.filter.clone())
            .child(
                row()
                    .gap_1()
                    .child(
                        chip("filter-attention", "Needs me", self.navigation.attention)
                            .aria_label("Show only threads needing attention")
                            .on_activate(cx, |this, _, cx| {
                                this.navigation.attention = !this.navigation.attention;
                                cx.notify();
                            }),
                    )
                    .child(
                        chip("filter-archived", "Archived", self.navigation.archived)
                            .aria_label("Include archived threads")
                            .on_activate(cx, |this, _, cx| {
                                this.navigation.archived = !this.navigation.archived;
                                cx.notify();
                            }),
                    )
                    .child(div().flex_1())
                    .when(!self.lists().hidden.is_empty(), |el| {
                        el.child(
                            tool("show-hidden-projects", "eye", "Show hidden projects")
                                .on_activate(cx, |this, _, cx| {
                                    let mut preferences = this.preferences.value.clone();
                                    preferences.navigation.hidden.clear();
                                    this.set_preferences(preferences, PreferenceGroup::View, cx);
                                    this.rebuild_projects(cx);
                                }),
                        )
                    })
                    .child({
                        let muted = self.preferences.value.navigation.mute_notifications;
                        tool(
                            "thread-alerts",
                            if muted { "bell-off" } else { "bell" },
                            if muted {
                                "Alerts off: turn on notifications and attention requests"
                            } else {
                                "Alerts on: mute notifications and attention requests"
                            },
                        )
                        .on_activate(cx, |this, _, cx| {
                            let mut preferences = this.preferences.value.clone();
                            preferences.navigation.mute_notifications =
                                !preferences.navigation.mute_notifications;
                            this.set_preferences(preferences, PreferenceGroup::View, cx);
                        })
                    })
                    .child(
                        tool(
                            "thread-list-refresh",
                            "refresh",
                            "Refresh threads and folders",
                        )
                        .on_activate(cx, |this, window, cx| {
                            this.navigation.available.clear();
                            this.load_threads(window, cx);
                        }),
                    ),
            )
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
            .when(self.host.connection.threads.len() > 200, |el| {
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
            .rounded_md()
            .bg(rgb(colors().panel));
        // The machine list opens under the row that names the machine.
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
            .threads
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
                .child(tag("thread-menu-title", self.thread_title(id), colors().text).truncate())
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
        self.sidebar = true;
        self.navigation
            .filter
            .update(cx, |field, cx| field.set_text("", cx));
        self.navigation.attention = false;
        if let Some(thread) = self
            .host
            .connection
            .threads
            .iter()
            .find(|thread| thread.id == id)
        {
            let project = self.project_of(&thread.checkout).to_path_buf();
            self.navigation.collapsed.remove(&project);
            if thread.archived == Some(true) {
                self.navigation.archived = true;
            }
        }
        if let Some(index) = self
            .navigation_rows(cx)
            .iter()
            .position(|row| matches!(row, Row::Thread(index) if self.host.connection.threads[*index].id == id))
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
        reply: Result<ThreadReply, plait_core::host::Error>,
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
                    .bg(rgb(colors().border)),
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
                .child(title(self.current_draft_title().to_owned()).text_color(rgb(colors().text)))
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

    /// New thread, the model it will start with, and a way to pick another.
    fn new_thread_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.new_thread_model().map(|model| model.name.clone());
        let name = match &model {
            Some(model) => format!("New thread with {model}"),
            None => "New thread: choose a model…".to_owned(),
        };
        row()
            .gap(px(1.))
            .child(
                nav_row("new-local-thread", name, false)
                    .aria_keyshortcuts(self.shortcut_aria("thread.new"))
                    .w_auto()
                    .flex_1()
                    .child(icon("plus").text_color(rgb(colors().mint)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(colors().text))
                            .child("New thread"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(rgb(colors().subtle))
                            .child(model.unwrap_or_else(|| "Choose a model…".into())),
                    )
                    .on_activate(cx, |this, window, cx| this.open_draft(window, cx)),
            )
            .child(
                icon_button("new-thread-model", "down", "New thread with another model…")
                    .aria_keyshortcuts(self.shortcut_aria("thread.new_with_model"))
                    .on_activate(cx, |this, window, cx| {
                        this.open_draft(window, cx);
                        this.open_model_picker(ModelPurpose::NewThread, window, cx)
                    }),
            )
    }

    /// Which machine this window works on, and the way to another.
    fn machine_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let machine = self.machine_label();
        let (state, tint) = if self.host.connection.connecting {
            ("Connecting", colors().muted)
        } else if self.host.connection.connected {
            ("Connected", colors().mint)
        } else {
            ("Offline", colors().amber)
        };
        nav_row(
            "machine-switcher",
            format!("{machine}, {state}. Switch machine…"),
            self.navigation.menu.as_deref() == Some("machines"),
        )
        .child(icon("machine"))
        .child(title(machine).text_color(rgb(colors().text)))
        .child(dot(tint))
        .child(icon("down"))
        .on_activate(cx, |this, _, cx| {
            this.navigation.menu = match this.navigation.menu.as_deref() {
                Some("machines") => None,
                _ => Some("machines".into()),
            };
            cx.notify();
        })
    }

    /// The machines this device knows, and where to manage them.
    fn machine_menu(&self, panel: Div, cx: &mut Context<Self>) -> Div {
        let mut panel = panel.child(tag("machine-menu-heading", "Machines", colors().muted));
        for (index, machine) in self.preferences.value.machines.list().iter().enumerate() {
            let id = &machine.id;
            let label = if id == &self.machines.active {
                format!("{} ✓", machine.name)
            } else {
                machine.name.clone()
            };
            let id = id.clone();
            panel = panel.child(button(("machine-use", index), label).on_activate(
                cx,
                move |this, window, cx| {
                    this.navigation.menu = None;
                    this.use_machine(id.clone(), window, cx);
                },
            ));
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

    fn actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        column()
            .flex_shrink_0()
            .px_2()
            .pt(px((CHROME_HEIGHT - ROW_HEIGHT) / 2.))
            .pb_1()
            .gap(px(1.))
            .child(self.machine_row(cx))
            .when(self.navigation.menu.as_deref() == Some("machines"), |el| {
                let panel = column()
                    .my_1()
                    .p_2()
                    .gap_1()
                    .border_1()
                    .border_color(rgb(colors().edge))
                    .rounded_md()
                    .bg(rgb(colors().panel));
                el.child(self.machine_menu(panel, cx))
            })
            .child(self.new_thread_row(cx))
            .child(
                nav_row("open-repository", "Open repository…", false)
                    .aria_keyshortcuts(self.shortcut_aria("repository.open"))
                    .child(icon("folder"))
                    .child(title("Open repository…").text_color(rgb(colors().text)))
                    .child(tag(
                        "open-repository-shortcut",
                        self.shortcut_label("repository.open"),
                        colors().subtle,
                    ))
                    .on_activate(cx, |this, window, cx| {
                        this.open_repository(&OpenRepository, window, cx)
                    }),
            )
    }

    /// Host state stays visible; remote entries are labeled where they appear.
    fn host_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let machine = self.machine_label();
        let (label, tint) = if self.host.connection.connecting {
            (format!("Connecting to {machine}…"), colors().muted)
        } else if self.host.connection.connected {
            (format!("{machine} connected"), colors().mint)
        } else {
            (format!("{machine} disconnected"), colors().amber)
        };
        row()
            .h(px(40.))
            .pl_4()
            .pr_2()
            .flex_shrink_0()
            .justify_between()
            .border_t_1()
            .border_color(rgb(colors().border))
            .child(
                pill()
                    .min_w_0()
                    .child(dot(tint))
                    .child(tag("host-footer-status", label, colors().muted).truncate()),
            )
            .child(
                row()
                    .gap_1()
                    .when(self.update_available(), |el| {
                        el.child(
                            toned_button("sidebar-update", "Update", Tone::Filled)
                                .aria_label("Update available. Open Settings to install.")
                                .on_activate(cx, |this, window, cx| {
                                    this.show_settings(true, window, cx)
                                }),
                        )
                    })
                    .child(
                        icon_button("settings", "sliders", "Settings…")
                            .aria_keyshortcuts(self.shortcut_aria("app.settings"))
                            .on_activate(cx, |this, window, cx| {
                                this.show_settings(true, window, cx)
                            }),
                    ),
            )
    }

    pub(super) fn navigation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        column()
            .relative()
            .w(px(self.panel_width(super::panels::Panel::Sidebar)))
            .flex_shrink_0()
            .bg(rgb(colors().rail))
            .border_r_1()
            .border_color(rgb(colors().border))
            .child(self.actions(cx))
            .child(self.navigation_controls(cx))
            .child(self.tree(cx))
            .child(self.navigation_menu(cx))
            .child(self.host_footer(cx))
            .child(self.resize_handle(super::panels::Panel::Sidebar, cx))
    }
}

/// A small icon control for the filter row, named on hover.
fn tool(id: &'static str, glyph: &'static str, name: &'static str) -> Control {
    icon_button(id, glyph, name)
        .size(px(24.))
        .tooltip(move |_, cx| cx.new(|_| TextTooltip(name.into())).into())
}
