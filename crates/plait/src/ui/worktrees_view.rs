//! One visible project graph. Host/Git/storage reads retire at the visibility boundary.
mod presentation;
mod render;
use super::{Plait, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{
    host::{
        self,
        worktree::{self, Checkout},
    },
    tabs::{ReadStamp, TabTarget},
    worktree_graph::{self, Graph},
    worktree_removal::{self, Choices},
};
use plait_protocol::worktree_size::Size;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    root: PathBuf,
    identity: Option<plait_protocol::git::worktree::Identity>,
    head: Option<String>,
    last_used: Option<u64>,
    changed: Option<u64>,
    removed: bool,
    build_directories: Vec<String>,
}
#[derive(Default)]
struct Cache(Vec<(Key, Size)>);
impl Cache {
    fn get(&self, key: &Key) -> Option<&Size> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, s)| s)
    }
    fn put(&mut self, key: Key, size: Size) {
        self.0.retain(|(k, _)| k.root != key.root);
        self.0.push((key, size));
        if self.0.len() > 256 {
            self.0.remove(0);
        }
    }
    fn clear(&mut self) {
        self.0.clear();
    }
}
actions!(plait, [Worktrees]);
#[derive(Clone)]
pub(super) enum Item {
    Head(usize, usize),
    Commit(usize),
    Changed(usize, usize),
    Detached(usize),
}
#[derive(Default)]
pub(super) struct View {
    pub rows: Vec<Checkout>,
    pub graph: Graph,
    pub items: Vec<Item>,
    pub sizes: Vec<Option<Result<Size, String>>>,
    pub titles: Vec<String>,
    pub head_labels: Vec<String>,
    pub commit_labels: Vec<String>,
    head_styles: Vec<presentation::LabelStyle>,
    commit_styles: Vec<presentation::LabelStyle>,
    strokes: Vec<Vec<presentation::Stroke>>,
    pub hovered: Option<usize>,
    pub ages: Vec<String>,
    pub total: String,
    /// Logical bytes of the project's build seed, from the last full read.
    pub seed_bytes: Option<u64>,
    pub largest: f32,
    pub words: Vec<&'static str>,
    pub openable: Vec<bool>,
    pub selected: usize,
    pub selection: Option<PathBuf>,
    pub error: Option<String>,
    pub loading: bool,
    pub generation: u64,
    pub cancelled: Arc<AtomicBool>,
    pub scroll: UniformListScrollHandle,
    cache: Cache,
    pub sort_size: bool,
    pub removal: plait_client::worktree_removal::Removal,
    /// The project whose view asked for `removal`: one window has one removal
    /// state, and another project's Worktrees tab must not show or resend it.
    pub removal_project: Option<PathBuf>,
    /// One inventory at a time: its helper and Git reads run to the end even
    /// when their result is retired, so overlapping refreshes would stack.
    pub reading: bool,
    /// The read asked for while one was reading.
    pub again: Option<Again>,
}
/// A read waiting for the one in flight. Two single checkouts become one full read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Again {
    Full { force: bool },
    Only(PathBuf),
}
impl Again {
    fn and(self, next: Again) -> Again {
        match (self, next) {
            (Again::Only(a), Again::Only(b)) if a == b => Again::Only(a),
            (Again::Full { force: a }, Again::Full { force: b }) => Again::Full { force: a || b },
            (Again::Full { force }, Again::Only(_)) | (Again::Only(_), Again::Full { force }) => {
                Again::Full { force }
            }
            (Again::Only(_), Again::Only(_)) => Again::Full { force: false },
        }
    }
}
impl View {
    pub fn park(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.generation += 1;
        self.rows.clear();
        self.graph = Graph::default();
        self.items.clear();
        self.sizes.clear();
        self.titles.clear();
        self.head_labels.clear();
        self.commit_labels.clear();
        self.head_styles.clear();
        self.commit_styles.clear();
        self.strokes.clear();
        self.hovered = None;
        self.ages.clear();
        self.total.clear();
        self.seed_bytes = None;
        self.largest = 1.;
        self.words.clear();
        self.openable.clear();
        self.loading = false;
    }
    pub fn account(&mut self) {
        let total = self
            .sizes
            .iter()
            .filter_map(|s| s.as_ref().and_then(|s| s.as_ref().ok()))
            .map(Size::total)
            .fold(self.seed_bytes.unwrap_or(0), u64::saturating_add);
        let incomplete = self.sizes.iter().any(|s| {
            s.as_ref()
                .is_none_or(|s| s.as_ref().map_or(true, |s| s.incomplete))
        });
        self.largest = self
            .sizes
            .iter()
            .filter_map(|s| s.as_ref().and_then(|s| s.as_ref().ok()))
            .map(Size::total)
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let measured = self.sizes.iter().any(|s| matches!(s, Some(Ok(_))));
        self.total = if self.sizes.iter().all(Option::is_none) {
            "Measuring".into()
        } else if !measured && self.seed_bytes.is_none() && self.sizes.iter().all(Option::is_some) {
            // Nothing was measured, so a total of zero would be untrue.
            "Size unavailable".into()
        } else {
            format!(
                "{}{} total",
                if incomplete { "over " } else { "" },
                bytes(total)
            )
        };
    }
    pub fn prepare(&mut self) {
        self.account();
        let now = super::commits::now().max(0) as u64 * 1000;
        self.ages = self
            .rows
            .iter()
            .map(|r| {
                r.last_used.map_or("Unknown".into(), |t| {
                    plait_client::navigation::age(Some(t), now)
                })
            })
            .collect();
        self.commit_labels = self
            .graph
            .rows
            .iter()
            .map(|r| {
                format!(
                    "{}  {}  · {}",
                    r.commit.commit.short,
                    r.commit.commit.subject,
                    plait_client::navigation::age(
                        Some(r.commit.commit.time.max(0) as u64 * 1000),
                        now
                    )
                )
            })
            .collect();
        self.commit_styles = self
            .graph
            .rows
            .iter()
            .map(|r| {
                let hash = r.commit.commit.short.len();
                presentation::LabelStyle {
                    title: hash + 2..hash + 2 + r.commit.commit.subject.len(),
                    state: 0..0,
                    code: 0..hash,
                }
            })
            .collect();
        self.items.clear();
        for (i, row) in self.graph.rows.iter().enumerate() {
            for &lane in &row.heads {
                if self.rows[lane].preservation.changed.is_some_and(|n| n > 0) {
                    self.items.push(Item::Changed(i, lane));
                }
                self.items.push(Item::Head(i, lane));
            }
            self.items.push(Item::Commit(i));
        }
        for (lane, value) in self.graph.lanes.iter().enumerate() {
            if value.head.is_none() {
                self.items.push(Item::Detached(lane));
            }
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.strokes = presentation::prepare(&self.items, &self.graph);
        self.hovered = None;
    }
}
fn size_label(size: Option<&Result<Size, String>>) -> String {
    match size {
        None => "Measuring".into(),
        Some(Err(_)) => "Unavailable".into(),
        Some(Ok(s)) => format!(
            "{}{}",
            if s.incomplete { "over " } else { "" },
            bytes(s.total())
        ),
    }
}
pub(super) fn bytes(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GiB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.1} KiB", n as f64 / 1024.)
    }
}
fn key(row: &Checkout) -> Key {
    Key {
        build_directories: row.build_directories.clone(),
        root: row.path.clone(),
        identity: row.identity.clone(),
        head: row.head.clone(),
        last_used: row.last_used,
        changed: row.preservation.changed,
        removed: row.removed,
    }
}
fn hover(text: String, cx: &mut App) -> AnyView {
    cx.new(|_| TextTooltip(text)).into()
}
fn safe_text(text: &str) -> String {
    plait_client::display_text::escape(text)
}
impl Plait {
    pub(super) fn restore_worktrees_bookmark(&mut self) {
        let bookmark = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .map(|t| t.session().bookmark());
        let (selected, size_sort) = match bookmark {
            Some(plait_client::tabs::bookmark::BookmarkRecord::Worktrees {
                selected,
                size_sort,
            }) => (selected.clone(), *size_sort),
            _ => (None, false),
        };
        self.worktrees.selection = selected;
        self.worktrees.sort_size = size_sort;
    }
    fn select_worktree_lane(&mut self, lane: usize, cx: &mut Context<Self>) {
        self.worktrees.selected = lane;
        self.save_worktrees_bookmark(cx);
        cx.notify();
    }
    fn save_worktrees_bookmark(&mut self, cx: &mut Context<Self>) {
        self.worktrees.selection = self
            .worktrees
            .rows
            .get(self.worktrees.selected)
            .map(|r| r.path.clone());
        if let Some(id) = self.tabs.model.active() {
            let result = self.tabs.model.save_bookmark(
                id,
                plait_client::tabs::bookmark::BookmarkRecord::Worktrees {
                    selected: self.worktrees.selection.clone(),
                    size_sort: self.worktrees.sort_size,
                },
            );
            self.tab_error(result);
            self.queue_tab_save(cx);
        }
    }

    pub(super) fn worktree_context_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = self.host.thread.snapshot.as_ref()?;
        let tree = self.lists().worktree(&snapshot.checkout)?;
        let project = tree.main.clone();
        Some(
            row()
                .gap_2()
                .child(tag(
                    "thread-worktree-context",
                    format!("Worktree · {}", safe_text(&tree.branch)),
                    colors().subtle,
                ))
                .child(
                    button("thread-worktree-view", "Worktrees")
                        .tooltip(|_, cx| {
                            hover(
                                "See this project's branches, storage and removal checks.".into(),
                                cx,
                            )
                        })
                        .on_activate(cx, move |this, window, cx| {
                            this.open_worktrees(project.clone(), window, cx)
                        }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn visible_project_checkout(&self) -> &std::path::Path {
        self.tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .filter(|tab| tab.session().target().is_project_view())
            .map_or(&self.host.creation.checkout, |tab| {
                &tab.session().target().checkout().root
            })
    }
    /// Whether the visible Worktrees tab is the project that asked for removal.
    pub(super) fn removal_here(&self) -> bool {
        self.worktrees_visible()
            && self.worktrees.removal_project.as_deref() == Some(self.visible_project_checkout())
    }
    pub(super) fn worktrees_visible(&self) -> bool {
        self.tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
            .is_some_and(|tab| tab.session().target().is_worktrees())
    }
    pub(super) fn open_worktrees(
        &mut self,
        project: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation.menu = None;
        if self.settings_open {
            self.show_settings(false, window, cx);
        }
        let project = self.project_of(&project).to_owned();
        self.open_tab_target(
            TabTarget::Worktrees {
                checkout: self.tab_checkout(project),
            },
            window,
            cx,
        );
    }
    pub(super) fn refresh_worktrees_view(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.worktrees_visible() {
            return;
        }
        if self.worktrees.reading {
            self.queue_worktree_read(Again::Full { force });
            return;
        }
        self.worktrees.cancelled.store(true, Ordering::Release);
        self.worktrees.cancelled = Arc::new(AtomicBool::new(false));
        self.worktrees.generation += 1;
        let generation = self.worktrees.generation;
        self.worktrees.error = None;
        if force
            && self.worktrees.removal.pending.is_some()
            && self.removal_here()
            && !self.worktree_busy
        {
            self.finish_worktree_removal(cx);
            return;
        }
        if !self.host.connection.connected {
            self.worktrees.loading = false;
            self.worktrees.error =
                Some("Offline · Connect the host to see this project's worktrees.".into());
            return;
        }
        let Some(tab) = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.get(id))
        else {
            return;
        };
        let Some(stamp) = self.tabs.model.read_stamp(tab.id()) else {
            return;
        };
        let root = tab.session().target().checkout().root.clone();
        self.worktrees.loading = true;
        if force {
            self.worktrees.cache.clear();
        }
        let cancelled = self.worktrees.cancelled.clone();
        self.worktrees.reading = true;
        let client = self.host_client();
        // A remote host draws its own checkouts' graph.
        let backend = self.repository_backend();
        let task = cx.background_executor().spawn(async move {
            let inventory = worktree::inventory(&client, root)?;
            if cancelled.load(Ordering::Acquire) {
                return Err(host::Error::Protocol("Read cancelled.".into()));
            }
            let heads = inventory
                .checkouts
                .iter()
                .filter_map(|row| row.head.clone())
                .collect::<Vec<_>>();
            let (commits, more) = backend
                .graph(&inventory.checkouts[0].path, &heads)
                .map_err(host::Error::Protocol)?;
            let graph = worktree_graph::layout(&inventory.checkouts, commits, more);
            Ok((inventory.checkouts, graph, inventory.seed_bytes))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.worktrees.reading = false;
                if this.accepts_worktrees(stamp, generation) {
                    let result = result.map(|(rows, graph, seed)| {
                        this.worktrees.seed_bytes = seed;
                        (rows, graph)
                    });
                    this.install_worktree_read(result, stamp, generation, cx);
                }
                this.run_queued_worktree_read(cx);
            });
        })
        .detach();
    }
    fn queue_worktree_read(&mut self, next: Again) {
        self.worktrees.again = Some(match self.worktrees.again.take() {
            Some(queued) => queued.and(next),
            None => next,
        });
    }
    fn run_queued_worktree_read(&mut self, cx: &mut Context<Self>) {
        match self.worktrees.again.take() {
            Some(Again::Full { force }) => self.refresh_worktrees_view(force, cx),
            Some(Again::Only(path)) => self.reread_worktree(path, cx),
            None => {}
        }
    }
    /// Whether new summaries touch the visible rows, and the checkouts whose
    /// run just ended. A summary alone costs no Git: only a run's end can
    /// have moved a checkout's facts.
    pub(super) fn worktree_summaries(
        &self,
        live: &[plait_client::host::ThreadSummary],
    ) -> (bool, Vec<PathBuf>) {
        if !self.worktrees_visible() {
            return (false, Vec::new());
        }
        let shown: Vec<_> = live
            .iter()
            .filter(|t| self.worktrees.rows.iter().any(|row| row.path == t.checkout))
            .collect();
        let mut ended: Vec<PathBuf> = shown
            .iter()
            .filter(|t| {
                !t.status.busy()
                    && self
                        .host
                        .connection
                        .threads()
                        .iter()
                        .any(|old| old.id == t.id && old.status.busy())
            })
            .map(|t| t.checkout.clone())
            .collect();
        ended.dedup();
        (!shown.is_empty(), ended)
    }
    /// Whether a run is working in `row`. Its facts were read before the run
    /// started, so Remove waits; the host refuses it then too.
    pub(super) fn run_in_worktree(&self, row: &Checkout) -> bool {
        self.host.connection.threads().iter().any(|t| {
            t.status.busy() && (t.checkout == row.path || row.thread_id.as_ref() == Some(&t.id))
        })
    }
    /// After the summaries merged: new words and titles, and a row read for
    /// each checkout whose run ended.
    pub(super) fn worktrees_after_summaries(
        &mut self,
        relabel: bool,
        ended: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if relabel {
            self.worktree_labels();
        }
        for path in ended {
            self.reread_worktree(path, cx);
        }
    }
    /// A run in `path` ended: only its Git facts can have moved, so only its
    /// row and the graph are read again. The title and state word come from
    /// the summary already held. Anything unexpected falls back to a full read.
    pub(super) fn reread_worktree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.worktrees_visible() {
            return;
        }
        if self.worktrees.reading {
            self.queue_worktree_read(Again::Only(path));
            return;
        }
        let Some(main) = self
            .worktrees
            .rows
            .iter()
            .find(|row| row.path == path && !row.removed)
            .map(|row| row.main)
        else {
            return;
        };
        if !self.host.connection.connected || self.worktrees.loading {
            return;
        }
        let Some(stamp) = self
            .tabs
            .model
            .active()
            .and_then(|id| self.tabs.model.read_stamp(id))
        else {
            return;
        };
        let root = self.visible_project_checkout().to_owned();
        self.worktrees.cancelled.store(true, Ordering::Release);
        self.worktrees.cancelled = Arc::new(AtomicBool::new(false));
        self.worktrees.generation += 1;
        let generation = self.worktrees.generation;
        let cancelled = self.worktrees.cancelled.clone();
        let mut rows = self.worktrees.rows.clone();
        self.worktrees.reading = true;
        let client = self.host_client();
        // A remote host draws its own checkouts' graph.
        let backend = self.repository_backend();
        let task = cx.background_executor().spawn(async move {
            let row = worktree::inventory_row(&client, root, path, main)?;
            if cancelled.load(Ordering::Acquire) {
                return Err(host::Error::Protocol("Read cancelled.".into()));
            }
            let place = rows
                .iter()
                .position(|old| old.path == row.path)
                .ok_or_else(|| host::Error::Protocol("The worktree left the list.".into()))?;
            rows[place] = row;
            let heads = rows
                .iter()
                .filter_map(|row| row.head.clone())
                .collect::<Vec<_>>();
            let (commits, more) = backend
                .graph(&rows[0].path, &heads)
                .map_err(host::Error::Protocol)?;
            let graph = worktree_graph::layout(&rows, commits, more);
            Ok((rows, graph))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.worktrees.reading = false;
                if this.accepts_worktrees(stamp, generation) {
                    match result {
                        Ok(read) => this.install_worktree_read(Ok(read), stamp, generation, cx),
                        Err(_) => this.queue_worktree_read(Again::Full { force: false }),
                    }
                }
                this.run_queued_worktree_read(cx);
            });
        })
        .detach();
    }
    fn install_worktree_read(
        &mut self,
        result: Result<(Vec<Checkout>, Graph), host::Error>,
        stamp: ReadStamp,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        self.worktrees.loading = false;
        match result {
            Err(error) => self.worktrees.error = Some(error.to_string()),
            Ok((rows, graph)) => {
                self.worktrees.sizes = rows
                    .iter()
                    .map(|row| self.worktrees.cache.get(&key(row)).cloned().map(Ok))
                    .collect();
                self.worktrees.rows = rows;
                self.worktrees.graph = graph;
                self.worktrees.selected = self
                    .worktrees
                    .selection
                    .as_ref()
                    .and_then(|p| self.worktrees.rows.iter().position(|r| &r.path == p))
                    .unwrap_or(0);
                self.sort_worktree_lanes(self.worktrees.sort_size, cx);
                self.measure_worktrees(stamp, generation, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn accepts_worktrees(&self, stamp: ReadStamp, generation: u64) -> bool {
        self.worktrees_visible()
            && self.tabs.model.accepts_read(stamp)
            && self.worktrees.generation == generation
    }
    pub(super) fn worktree_labels(&mut self) {
        self.worktrees.titles.clear();
        self.worktrees.words.clear();
        self.worktrees.openable.clear();
        for row in &self.worktrees.rows {
            let thread = row
                .thread_id
                .as_ref()
                .and_then(|id| self.host.connection.threads().iter().find(|t| &t.id == id));
            self.worktrees.openable.push(thread.is_some());
            self.worktrees.titles.push(if row.main {
                "Main checkout".into()
            } else if row.creation_id.is_none() {
                "External worktree".into()
            } else {
                safe_text(thread.map_or("Thread unavailable", |t| {
                    t.title
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or("Untitled thread")
                }))
            });
            self.worktrees.words.push(if row.removed {
                "Removed"
            } else if row.missing {
                "Missing"
            } else {
                thread.map_or("", |t| {
                    plait_client::tabs::row::summary_state(t, self.lists().unread.contains(&t.id))
                        .word(true)
                })
            });
        }
        (self.worktrees.head_labels, self.worktrees.head_styles) = self
            .worktrees
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let mut facts = Vec::new();
                if let Some(n) = row.preservation.changed {
                    facts.push(if n == 0 {
                        "Clean".into()
                    } else {
                        format!("{n} changed")
                    });
                }
                if let Some(n) = row.preservation.unpushed
                    && n > 0
                {
                    facts.push(format!("{n} not pushed"));
                }
                if let Some(label) = presentation::ancestry_label(&self.worktrees.graph.lanes[i]) {
                    facts.push(label.into());
                }
                presentation::head_label(
                    &self.worktrees.titles[i],
                    self.worktrees.words[i],
                    &safe_text(row.branch.as_deref().unwrap_or("Detached")),
                    &facts,
                )
            })
            .unzip();
    }
    pub(super) fn measure_worktrees(
        &mut self,
        stamp: ReadStamp,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let pending: Vec<_> = self
            .worktrees
            .rows
            .iter()
            .enumerate()
            .filter(|(i, _)| self.worktrees.sizes[*i].is_none())
            .map(|(i, row)| (i, key(row), row.thread_id.clone(), row.removed))
            .collect();
        // Another checkout nested in this one is measured once, as itself.
        let checkouts: Vec<_> = self
            .worktrees
            .rows
            .iter()
            .filter_map(|row| {
                let id = row.identity.as_ref()?;
                Some((
                    row.path.clone(),
                    (id.dev.parse().ok()?, id.ino.parse().ok()?),
                ))
            })
            .collect();
        let cancelled = self.worktrees.cancelled.clone();
        let client = self.host_client();
        let project = self.visible_project_checkout().to_owned();
        let task = cx.background_executor().spawn(async move {
            let mut results = Vec::new();
            for (i, key, thread, _) in pending {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                let others: std::collections::HashSet<_> = checkouts
                    .iter()
                    .filter(|(path, _)| *path != key.root)
                    .map(|(_, id)| *id)
                    .collect();
                let read = host::git::Read::WorktreeSize {
                    worktree: key.root.clone(),
                    worktree_identity: key.identity.as_ref().map(|id| host::git::WireIdentity {
                        dev: id.dev.clone(),
                        ino: id.ino.clone(),
                    }),
                    thread,
                    removed: key.removed,
                    others: others
                        .into_iter()
                        .map(|(dev, ino)| plait_protocol::lease::DirIdentity { dev, ino }.into())
                        .collect(),
                    build_directories: key.build_directories.clone(),
                };
                let size = host::git::read::<Size>(&client, &project, None, read)
                    .map(|reply| reply.result)
                    .map_err(|error| error.to_string());
                results.push((i, key, size));
            }
            results
        });
        cx.spawn(async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |this, cx| {
                if !this.accepts_worktrees(stamp, generation) {
                    return;
                }
                for (_, key, size) in results {
                    let Some(i) = this
                        .worktrees
                        .rows
                        .iter()
                        .position(|row| self::key(row) == key)
                    else {
                        continue;
                    };
                    if let Ok(size) = &size {
                        this.worktrees.cache.put(key, size.clone());
                    }
                    this.worktrees.sizes[i] = Some(size);
                }
                this.sort_worktree_lanes(this.worktrees.sort_size, cx);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn sort_worktree_lanes(&mut self, size: bool, cx: &mut Context<Self>) {
        self.worktrees.sort_size = size;
        let selected = self
            .worktrees
            .rows
            .get(self.worktrees.selected)
            .map(|r| r.path.clone());
        let mut pairs: Vec<_> = std::mem::take(&mut self.worktrees.rows)
            .into_iter()
            .zip(std::mem::take(&mut self.worktrees.sizes))
            .collect();
        pairs.sort_by_key(|(row, bytes)| {
            (
                !row.main,
                std::cmp::Reverse(if size {
                    bytes
                        .as_ref()
                        .and_then(|s| s.as_ref().ok())
                        .map_or(0, Size::total)
                } else {
                    row.last_used.unwrap_or(0)
                }),
            )
        });
        (self.worktrees.rows, self.worktrees.sizes) = pairs.into_iter().unzip();
        let commits = self
            .worktrees
            .graph
            .rows
            .iter()
            .map(|r| r.commit.clone())
            .collect();
        self.worktrees.graph =
            worktree_graph::layout(&self.worktrees.rows, commits, self.worktrees.graph.more);
        self.worktrees.selected = selected
            .and_then(|p| self.worktrees.rows.iter().position(|r| r.path == p))
            .unwrap_or(0);
        self.worktrees.prepare();
        self.worktree_labels();
        self.save_worktrees_bookmark(cx);
        cx.notify();
    }
    /// `path` and `identity` are the row drawn with the button. Input reaches the
    /// last drawn frame, and a size reply can re-sort rows before the next one,
    /// so an index would name whichever worktree moved into that place.
    pub(super) fn ask_remove_view_worktree(
        &mut self,
        path: &std::path::Path,
        identity: Option<&plait_client::git::worktree::Identity>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = self.worktrees.selected;
        let Some(tree) = self.worktrees.rows.get(n) else {
            return;
        };
        if tree.path != path
            || tree.identity.as_ref() != identity
            || tree.main
            || !Choices::for_checkout(tree).any()
            || self.run_in_worktree(tree)
            || self.worktree_busy
            || !self.host.connection.connected
            || self.worktrees.loading
            || self.worktrees.error.is_some()
        {
            return;
        }
        let detail = worktree_removal::confirmation_detail(
            &tree.path,
            tree.branch.as_deref().unwrap_or("Detached"),
            Some(&tree.preservation),
        );
        if !self.worktrees.removal.ask(
            tree,
            self.tabs.host.clone(),
            plait_client::host_state::request_id(),
        ) {
            return;
        }
        self.show_worktree_removal(
            self.visible_project_checkout().to_owned(),
            detail,
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod size_cache_tests {
    use super::*;

    #[core::prelude::v1::test]
    fn changed_checkout_identity_does_not_reuse_size() {
        let mut cache = Cache::default();
        let mut key = Key {
            root: "/repo".into(),
            identity: Some(plait_protocol::git::worktree::Identity {
                dev: "1".into(),
                ino: "2".into(),
            }),
            head: None,
            last_used: None,
            changed: None,
            removed: false,
            build_directories: Vec::new(),
        };
        cache.put(key.clone(), Size::default());
        assert!(cache.get(&key).is_some());
        key.identity.as_mut().unwrap().ino = "3".into();
        assert!(cache.get(&key).is_none());
    }
}
