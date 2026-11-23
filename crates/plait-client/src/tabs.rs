//! Headless window tabs. Transitions have no host, filesystem or GPUI effects.
//!
//! Only one tab may own resident read state. Parking destroys that state; the
//! UI must likewise release its watch, projections, images, timers and handles.
//! Drafts and mutation acknowledgements belong to their existing owner ledgers,
//! never to a tab's visibility guard.
pub mod bookmark;
pub mod persistence;
pub mod row;
pub mod storage;

use crate::thread_view::{SelectedThreadState, ThreadRepositoryView, ThreadView};
use bookmark::BookmarkRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Bound untrusted restore input; interactive opening has no count ceiling.
pub const MAX_RESTORED_TABS: usize = 10_000;
pub const CLOSED_TAB_LIMIT: usize = 20;
pub const COLD_TAB_BYTES: usize = 16 * 1024;

/// The caller allocates a fresh ID for each window lifetime, including restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(u64);
impl TabId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HostKey {
    pub id: String,
    pub store: PathBuf,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DirectoryIdentity {
    pub device: u64,
    pub inode: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckoutKey {
    pub host: HostKey,
    /// The host-supplied root verbatim; verify its directory off the UI thread.
    pub root: PathBuf,
    pub directory: Option<DirectoryIdentity>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadKey {
    pub host: HostKey,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabTarget {
    Existing {
        id: String,
        checkout: CheckoutKey,
    },
    Draft {
        id: String,
        checkout: CheckoutKey,
    },
    Worktrees {
        checkout: CheckoutKey,
    },
    /// The project's Linear issues. Older builds drop this tab kind on restore.
    Issues {
        checkout: CheckoutKey,
    },
}
impl TabTarget {
    pub fn checkout(&self) -> &CheckoutKey {
        match self {
            Self::Existing { checkout, .. }
            | Self::Draft { checkout, .. }
            | Self::Worktrees { checkout }
            | Self::Issues { checkout } => checkout,
        }
    }
    pub fn thread_key(&self) -> Option<ThreadKey> {
        match self {
            Self::Existing { id, checkout } => Some(ThreadKey {
                host: checkout.host.clone(),
                id: id.clone(),
            }),
            Self::Draft { .. } | Self::Worktrees { .. } | Self::Issues { .. } => None,
        }
    }
    pub fn is_worktrees(&self) -> bool {
        matches!(self, Self::Worktrees { .. })
    }
    pub fn is_issues(&self) -> bool {
        matches!(self, Self::Issues { .. })
    }
    /// A project-level tab rather than a thread or draft.
    pub fn is_project_view(&self) -> bool {
        self.is_worktrees() || self.is_issues()
    }
    pub fn is_draft(&self) -> bool {
        matches!(self, Self::Draft { .. })
    }
    fn same_content(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Existing {
                    id: a,
                    checkout: ac,
                },
                Self::Existing {
                    id: b,
                    checkout: bc,
                },
            ) => a == b && ac.host == bc.host,
            (
                Self::Draft {
                    id: a,
                    checkout: ac,
                },
                Self::Draft {
                    id: b,
                    checkout: bc,
                },
            ) => a == b && ac == bc,
            (Self::Worktrees { checkout: a }, Self::Worktrees { checkout: b })
            | (Self::Issues { checkout: a }, Self::Issues { checkout: b }) => {
                a.host == b.host && a.root == b.root
            }
            _ => false,
        }
    }
    pub(crate) fn valid(&self) -> bool {
        let checkout = self.checkout();
        let id = match self {
            Self::Existing { id, .. } | Self::Draft { id, .. } => id.as_str(),
            Self::Worktrees { .. } => "worktrees",
            Self::Issues { .. } => "issues",
        };
        identifier(id)
            && identifier(&checkout.host.id)
            && [&checkout.root, &checkout.host.store].iter().all(|p| {
                p.is_absolute()
                    && p.as_os_str().len() <= 4096
                    && !p.as_os_str().as_encoded_bytes().contains(&0)
            })
    }
    fn retained_bytes(&self) -> usize {
        let checkout = self.checkout();
        let id = match self {
            Self::Existing { id, .. } | Self::Draft { id, .. } => id.capacity(),
            Self::Worktrees { .. } | Self::Issues { .. } => 0,
        };
        id + checkout.host.id.capacity() + checkout.host.store.capacity() + checkout.root.capacity()
    }
}

/// Two content kinds, with explicit dispatch and one implementation each.
pub enum TabContent {
    Thread(Box<TabSession>),
    Worktrees(Box<TabSession>),
}
impl TabContent {
    pub fn session(&self) -> &TabSession {
        match self {
            Self::Thread(t) | Self::Worktrees(t) => t,
        }
    }
    fn session_mut(&mut self) -> &mut TabSession {
        match self {
            Self::Thread(t) | Self::Worktrees(t) => t,
        }
    }
    pub fn view_commands(&self) -> &[ThreadView] {
        match self {
            Self::Worktrees(_) => &[],
            Self::Thread(_) => &[
                ThreadView::Conversation,
                ThreadView::Changes,
                ThreadView::History,
                ThreadView::Files,
            ],
        }
    }
}

pub struct TabSession {
    target: TabTarget,
    bookmark: BookmarkRecord,
    residency: Residency,
    connection: Option<u64>,
    outcomes: crate::folding::ObservedOutcomes,
    /// Earlier turns the person opened while this tab is open; never saved.
    opened_turns: HashSet<String>,
}
impl TabSession {
    pub fn target(&self) -> &TabTarget {
        &self.target
    }
    pub fn bookmark(&self) -> &BookmarkRecord {
        &self.bookmark
    }
    pub fn residency(&self) -> &Residency {
        &self.residency
    }
    pub fn connected(&self) -> bool {
        self.connection.is_some()
    }
    pub fn outcomes(&self) -> &crate::folding::ObservedOutcomes {
        &self.outcomes
    }
    pub fn opened_turns(&self) -> &HashSet<String> {
        &self.opened_turns
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColdReason {
    Opened,
    Parked,
    Disconnected,
    Restored,
    Replaced,
}

/// Cold is not an empty loaded view. Loading/Ready alone may retain payloads.
pub enum Residency {
    Cold {
        reason: ColdReason,
    },
    Loading {
        visibility: u64,
        data: Box<ResidentThread>,
    },
    Ready {
        visibility: u64,
        data: Box<ResidentThread>,
    },
}
impl Residency {
    fn visibility(&self) -> Option<u64> {
        match self {
            Self::Cold { .. } => None,
            Self::Loading { visibility, .. } | Self::Ready { visibility, .. } => Some(*visibility),
        }
    }
}
#[derive(Default)]
pub struct ResidentThread {
    pub host: SelectedThreadState,
    pub repository: ThreadRepositoryView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabLifetime {
    pub window: WindowId,
    pub tab: TabId,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadStamp {
    pub lifetime: TabLifetime,
    pub visibility: u64,
    pub connection: u64,
}

pub struct TabRecord {
    id: TabId,
    lifetime: TabLifetime,
    content: TabContent,
    work: crate::panes::WorkView,
}
impl TabRecord {
    pub fn id(&self) -> TabId {
        self.id
    }
    pub fn lifetime(&self) -> TabLifetime {
        self.lifetime
    }
    pub fn content(&self) -> &TabContent {
        &self.content
    }
    pub fn session(&self) -> &TabSession {
        self.content.session()
    }
    /// Includes allocated string/path capacities, not just logical lengths.
    pub fn cold_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<TabSession>()
            + self.session().target.retained_bytes()
            + self.session().bookmark.retained_bytes()
    }
}

struct ClosedTab {
    index: usize,
    target: TabTarget,
    bookmark: BookmarkRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabError {
    MissingTab,
    InvalidIdentity,
    IdentityMismatch,
    AlreadyOpen,
    Capacity,
    IdentityExhausted,
    InvalidBookmark,
    BookmarkTooLarge,
}
impl std::fmt::Display for TabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingTab => "The tab is no longer open.",
            Self::InvalidIdentity => "The tab identity is invalid.",
            Self::IdentityMismatch => "The thread belongs to a different checkout.",
            Self::AlreadyOpen => "The created thread already has a tab.",
            Self::Capacity => "The window has reached its tab limit.",
            Self::IdentityExhausted => "The window has exhausted its tab identities.",
            Self::InvalidBookmark => "The workspace bookmark is invalid.",
            Self::BookmarkTooLarge => "Save the extended bookmark before parking this tab.",
        })
    }
}
impl std::error::Error for TabError {}

pub struct WindowTabs {
    window: WindowId,
    serial: u64,
    tabs: Vec<TabRecord>,
    active: Option<TabId>,
    closed: Vec<ClosedTab>,
}
impl WindowTabs {
    pub fn new(window: WindowId) -> Self {
        Self {
            window,
            serial: 0,
            tabs: Vec::new(),
            active: None,
            closed: Vec::new(),
        }
    }
    pub fn tabs(&self) -> &[TabRecord] {
        &self.tabs
    }
    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    /// Used to retire private editor caches after closed-history eviction.
    pub fn retains_target(&self, target: &TabTarget) -> bool {
        self.tabs.iter().any(|tab| tab.session().target() == target)
            || self.closed.iter().any(|tab| &tab.target == target)
    }
    pub fn get(&self, id: TabId) -> Option<&TabRecord> {
        self.tabs.iter().find(|tab| tab.id == id)
    }
    /// Work view lives only for the lifetime of an open thread tab.
    pub fn work(&self) -> Option<&crate::panes::WorkView> {
        let tab = self.get(self.active?)?;
        matches!(
            tab.session().target(),
            TabTarget::Existing { .. } | TabTarget::Draft { .. }
        )
        .then_some(&tab.work)
    }
    pub fn work_mut(&mut self) -> Option<&mut crate::panes::WorkView> {
        self.work()?;
        let index = self.index(self.active?).ok()?;
        Some(&mut self.tabs[index].work)
    }
    pub fn closed_len(&self) -> usize {
        self.closed.len()
    }
    fn index(&self, id: TabId) -> Result<usize, TabError> {
        self.tabs
            .iter()
            .position(|t| t.id == id)
            .ok_or(TabError::MissingTab)
    }
    fn next_serial(&mut self) -> Result<u64, TabError> {
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or(TabError::IdentityExhausted)?;
        Ok(self.serial)
    }

    /// Promote provisional session identities once the host has answered. Existing
    /// verified identities are never rebound by a sidebar click or reconnect.
    pub fn verify_session_host(
        &mut self,
        previous: &HostKey,
        host: &HostKey,
        checkouts: &[CheckoutKey],
    ) -> Result<(), TabError> {
        let promote = |target: &TabTarget| {
            let mut target = target.clone();
            let checkout = match &mut target {
                TabTarget::Existing { checkout, .. }
                | TabTarget::Draft { checkout, .. }
                | TabTarget::Worktrees { checkout }
                | TabTarget::Issues { checkout } => checkout,
            };
            if &checkout.host == previous && checkout.directory.is_none() {
                checkout.host = host.clone();
                checkout.directory = checkouts
                    .iter()
                    .find(|c| &c.host == host && c.root == checkout.root)
                    .and_then(|c| c.directory);
            }
            target
        };
        if self
            .serial
            .checked_add(self.tabs.len() as u64 + 1)
            .is_none()
        {
            return Err(TabError::IdentityExhausted);
        }
        for tab in &self.tabs {
            validate_record(&promote(tab.session().target()), tab.session().bookmark())?;
        }
        for tab in &self.closed {
            validate_record(&promote(&tab.target), &tab.bookmark)?;
        }
        for index in 0..self.tabs.len() {
            let target = promote(self.tabs[index].session().target());
            if &target != self.tabs[index].session().target() {
                self.tabs[index].lifetime.generation = self.next_serial()?;
                let thread = self.tabs[index].content.session_mut();
                thread.target = target;
                thread.residency = Residency::Cold {
                    reason: ColdReason::Replaced,
                };
            }
        }
        for tab in &mut self.closed {
            tab.target = promote(&tab.target);
        }
        if let Some(id) = self.active {
            self.activate(id)?;
        }
        Ok(())
    }

    /// Explicitly accept the newly opened identity for every tab at this path.
    pub fn reopen_checkout(&mut self, key: &CheckoutKey) -> Result<(), TabError> {
        let count = self
            .tabs
            .iter()
            .filter(|tab| {
                let checkout = tab.session().target().checkout();
                checkout.host == key.host && checkout.root == key.root
            })
            .count() as u64;
        self.serial
            .checked_add(count)
            .ok_or(TabError::IdentityExhausted)?;
        let update = |target: &mut TabTarget| {
            let checkout = match target {
                TabTarget::Existing { checkout, .. }
                | TabTarget::Draft { checkout, .. }
                | TabTarget::Worktrees { checkout }
                | TabTarget::Issues { checkout } => checkout,
            };
            if checkout.host == key.host && checkout.root == key.root {
                *checkout = key.clone();
            }
        };
        for tab in &mut self.tabs {
            let checkout = tab.session().target().checkout();
            if checkout.host == key.host && checkout.root == key.root {
                self.serial += 1;
                tab.lifetime.generation = self.serial;
                update(&mut tab.content.session_mut().target);
            }
        }
        for tab in &mut self.closed {
            update(&mut tab.target);
        }
        Ok(())
    }

    /// Existing threads deduplicate; each new draft carries its own identity.
    /// Connection generations come from the connection owner, never selection.
    pub fn open(&mut self, target: TabTarget, connection: Option<u64>) -> Result<TabId, TabError> {
        if !target.valid() {
            return Err(TabError::InvalidIdentity);
        }
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|t| t.session().target.same_content(&target))
        {
            if tab.session().target.checkout() != target.checkout() {
                return Err(TabError::IdentityMismatch);
            }
            let id = tab.id;
            self.activate(id)?;
            return Ok(id);
        }
        let id = self.insert(
            target,
            BookmarkRecord::default(),
            self.tabs.len(),
            connection,
        )?;
        self.activate(id)?;
        Ok(id)
    }

    fn insert(
        &mut self,
        target: TabTarget,
        bookmark: BookmarkRecord,
        index: usize,
        connection: Option<u64>,
    ) -> Result<TabId, TabError> {
        validate_record(&target, &bookmark)?;
        // Reserve activation's identity before changing the ordered set.
        if self.serial >= u64::MAX - 1 {
            return Err(TabError::IdentityExhausted);
        }
        let serial = self.next_serial()?;
        let id = TabId(serial);
        self.tabs.insert(
            index.min(self.tabs.len()),
            TabRecord {
                id,
                lifetime: TabLifetime {
                    window: self.window,
                    tab: id,
                    generation: serial,
                },
                work: Default::default(),
                content: content_for(Box::new(TabSession {
                    target,
                    bookmark,
                    connection,
                    residency: Residency::Cold {
                        reason: ColdReason::Opened,
                    },
                    outcomes: Default::default(),
                    opened_turns: HashSet::new(),
                })),
            },
        );
        Ok(id)
    }

    pub fn activate(&mut self, id: TabId) -> Result<(), TabError> {
        let index = self.index(id)?;
        if self.active == Some(id) && self.tabs[index].session().residency.visibility().is_some() {
            return Ok(());
        }
        let visibility = self.next_serial()?;
        if let Some(old) = self.active {
            self.park(old)?;
        }
        self.active = Some(id);
        let thread = self.tabs[index].content.session_mut();
        if thread.connection.is_some() {
            let mut data = ResidentThread::default();
            if let TabTarget::Existing { id, .. } = &thread.target {
                data.host.selected = Some(id.clone());
            }
            thread.residency = Residency::Loading {
                visibility,
                data: Box::new(data),
            };
        }
        Ok(())
    }

    /// Capture/flush bookmarks and drafts before calling this. No run is stopped.
    pub fn park(&mut self, id: TabId) -> Result<(), TabError> {
        let i = self.index(id)?;
        self.tabs[i].content.session_mut().residency = Residency::Cold {
            reason: ColdReason::Parked,
        };
        Ok(())
    }

    pub fn observe_active_snapshot(&mut self, live: &crate::host::Snapshot) {
        let Some(index) = self.active.and_then(|id| self.index(id).ok()) else {
            return;
        };
        let session = self.tabs[index].content.session_mut();
        if matches!(&session.target, TabTarget::Existing { id, checkout } if *id == live.id && checkout.root == live.checkout)
        {
            session.outcomes.observe(live);
        }
    }

    /// Record that the person opened an earlier turn in `id`. Kept only while
    /// the tab stays open and never serialized.
    pub fn open_turn(&mut self, id: TabId, turn_id: String) -> bool {
        let Ok(index) = self.index(id) else {
            return false;
        };
        self.tabs[index]
            .content
            .session_mut()
            .opened_turns
            .insert(turn_id)
    }

    /// A view change invalidates its old read demand, including A -> B -> A.
    pub fn save_bookmark(&mut self, id: TabId, bookmark: BookmarkRecord) -> Result<(), TabError> {
        let i = self.index(id)?;
        validate_record(&self.tabs[i].session().target, &bookmark)?;
        let changed_view = self.tabs[i].session().bookmark.view() != bookmark.view();
        if changed_view && self.active == Some(id) && self.serial == u64::MAX {
            return Err(TabError::IdentityExhausted);
        }
        self.tabs[i].content.session_mut().bookmark = bookmark;
        if changed_view {
            self.park(id)?;
            if self.active == Some(id) {
                self.activate(id)?;
            }
        }
        Ok(())
    }

    /// The create acknowledgement changes content in place and invalidates reads.
    /// Mutation acknowledgements themselves must go to the operation ledger first.
    pub fn bind_created(&mut self, id: TabId, target: TabTarget) -> Result<(), TabError> {
        self.bind_creation(id, target, None)
    }

    /// Only an unbound draft may adopt a host-verified worktree. Existing threads never move.
    pub fn bind_worktree_created(
        &mut self,
        id: TabId,
        source: &CheckoutKey,
        target: TabTarget,
    ) -> Result<(), TabError> {
        self.bind_creation(id, target, Some(source))
    }

    fn bind_creation(
        &mut self,
        id: TabId,
        target: TabTarget,
        source: Option<&CheckoutKey>,
    ) -> Result<(), TabError> {
        let i = self.index(id)?;
        if !target.valid()
            || target.is_draft()
            || target.is_project_view()
            || !self.tabs[i].session().target.is_draft()
        {
            return Err(TabError::InvalidIdentity);
        }
        let original = self.tabs[i].session().target.checkout();
        if let Some(source) = source {
            if source != original
                || target.checkout().host != original.host
                || target.checkout().directory.is_none()
            {
                return Err(TabError::IdentityMismatch);
            }
        } else if original != target.checkout() {
            return Err(TabError::IdentityMismatch);
        }
        if self
            .tabs
            .iter()
            .any(|t| t.id != id && t.session().target.same_content(&target))
        {
            return Err(TabError::AlreadyOpen);
        }
        let bookmark = if source.is_some() {
            // Repository selections belong to the original root, even when paths happen to match.
            BookmarkRecord::capture(&bookmark::ThreadBookmark {
                view: self.tabs[i].session().bookmark.view(),
                ..Default::default()
            })
            .map_err(|_| TabError::InvalidIdentity)?
        } else {
            self.tabs[i].session().bookmark.clone()
        };
        validate_record(&target, &bookmark)?;
        if self.serial >= u64::MAX - 1 {
            return Err(TabError::IdentityExhausted);
        }
        let generation = self.next_serial()?;
        self.tabs[i].lifetime.generation = generation;
        let thread = self.tabs[i].content.session_mut();
        thread.opened_turns.clear();
        thread.outcomes = Default::default();
        thread.target = target;
        thread.bookmark = bookmark;
        thread.residency = Residency::Cold {
            reason: ColdReason::Replaced,
        };
        if self.active == Some(id) {
            self.activate(id)?;
        }
        Ok(())
    }

    pub fn move_draft(&mut self, id: TabId, checkout: CheckoutKey) -> Result<(), TabError> {
        let index = self.index(id)?;
        let TabTarget::Draft { id: draft, .. } = &self.tabs[index].session().target else {
            return Err(TabError::InvalidIdentity);
        };
        let target = TabTarget::Draft {
            id: draft.clone(),
            checkout,
        };
        validate_record(&target, &BookmarkRecord::default())?;
        if self.serial >= u64::MAX - 1 {
            return Err(TabError::IdentityExhausted);
        }
        let generation = self.next_serial()?;
        self.tabs[index].lifetime.generation = generation;
        let thread = self.tabs[index].content.session_mut();
        thread.opened_turns.clear();
        thread.outcomes = Default::default();
        thread.target = target;
        thread.bookmark = BookmarkRecord::default();
        self.park(id)?;
        if self.active == Some(id) {
            self.activate(id)?;
        }
        Ok(())
    }

    /// Creation owns its draft even after close; never selects another tab.
    pub fn bind_draft_target(
        &mut self,
        draft: &TabTarget,
        target: TabTarget,
    ) -> Result<(), TabError> {
        if let Some(tab) = self.tabs.iter().find(|tab| tab.session().target() == draft) {
            let id = tab.id;
            return if draft.checkout() == target.checkout() {
                self.bind_created(id, target)
            } else {
                self.bind_worktree_created(id, draft.checkout(), target)
            };
        }
        if !draft.is_draft()
            || target.is_draft()
            || !target.valid()
            || draft.checkout().host != target.checkout().host
        {
            return Err(TabError::InvalidIdentity);
        }
        if let Some(closed) = self.closed.iter_mut().find(|tab| &tab.target == draft) {
            closed.target = target;
            closed.bookmark = BookmarkRecord::default();
        }
        Ok(())
    }

    /// Call only after the UI's save succeeds. A save failure must leave it open.
    /// `draft_nonempty` includes images; it never copies a draft into this model.
    pub fn close(&mut self, id: TabId, draft_nonempty: bool) -> Result<(), TabError> {
        let i = self.index(id)?;
        if self.active == Some(id) && self.tabs.len() > 1 && self.serial == u64::MAX {
            return Err(TabError::IdentityExhausted);
        }
        let tab = self.tabs.remove(i);
        let (TabContent::Thread(thread) | TabContent::Worktrees(thread)) = tab.content;
        if !thread.target.is_draft() || draft_nonempty {
            // One entry per content: the newest close carries the current bookmark.
            self.closed
                .retain(|c| !c.target.same_content(&thread.target));
            self.closed.push(ClosedTab {
                index: i,
                target: thread.target,
                bookmark: thread.bookmark,
            });
            if self.closed.len() > CLOSED_TAB_LIMIT {
                self.closed.remove(0);
            }
        }
        self.select_after_removal(id, i)
    }

    fn select_after_removal(&mut self, removed: TabId, index: usize) -> Result<(), TabError> {
        if self.active == Some(removed) {
            self.active = None;
            if let Some(next) = self.tabs.get(index.min(self.tabs.len().saturating_sub(1))) {
                self.activate(next.id)?;
            }
        }
        Ok(())
    }

    /// Resolver must use a real catalog (archived is allowed), not a partial page.
    /// Missing entries are skipped. `Some(None)` means known but disconnected.
    pub fn reopen(
        &mut self,
        mut resolve: impl FnMut(&TabTarget) -> Option<Option<u64>>,
    ) -> Result<Option<TabId>, TabError> {
        if self.serial >= u64::MAX - 1 {
            return Err(TabError::IdentityExhausted);
        }
        while let Some(closed) = self.closed.pop() {
            let Some(connection) = resolve(&closed.target) else {
                continue;
            };
            if let Some(tab) = self
                .tabs
                .iter()
                .find(|t| t.session().target.same_content(&closed.target))
            {
                let id = tab.id;
                if tab.session().target.checkout() != closed.target.checkout() {
                    continue;
                }
                self.activate(id)?;
                return Ok(Some(id));
            }
            let id = self.insert(closed.target, closed.bookmark, closed.index, connection)?;
            self.activate(id)?;
            return Ok(Some(id));
        }
        Ok(None)
    }

    /// Zero-based destination, clamped at the end; active identity is unchanged.
    pub fn move_tab(&mut self, id: TabId, to: usize) -> Result<(), TabError> {
        let from = self.index(id)?;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to.min(self.tabs.len()), tab);
        Ok(())
    }
    pub fn next_tab(&mut self) -> Result<Option<TabId>, TabError> {
        self.cycle(true)
    }
    pub fn previous_tab(&mut self) -> Result<Option<TabId>, TabError> {
        self.cycle(false)
    }
    fn cycle(&mut self, forward: bool) -> Result<Option<TabId>, TabError> {
        if self.tabs.is_empty() {
            return Ok(None);
        }
        let i = self.active.and_then(|id| self.index(id).ok()).unwrap_or(0);
        let n = self.tabs.len();
        let id = self.tabs[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }]
        .id;
        self.activate(id)?;
        Ok(Some(id))
    }
    /// Cmd-1..8 select positions; Cmd-9 selects the last tab.
    pub fn jump(&mut self, number: usize) -> Result<Option<TabId>, TabError> {
        let index = match number {
            1..=8 => number - 1,
            9 => self.tabs.len().saturating_sub(1),
            _ => return Ok(None),
        };
        let Some(tab) = self.tabs.get(index) else {
            return Ok(None);
        };
        let id = tab.id;
        self.activate(id)?;
        Ok(Some(id))
    }

    /// Rename and archive are catalog facts: they do not close or reset a tab.
    /// Only confirmed deletion calls this; a missing summary delta is not deletion.
    pub fn thread_deleted(&mut self, key: &ThreadKey) -> Result<(), TabError> {
        self.remove_matching(|t| t.thread_key().as_ref() == Some(key))
    }
    /// Explicit project removal closes its checkouts, including supplied worktrees.
    /// Hiding a sidebar group is not project removal and need not call this.
    pub fn project_removed(&mut self, checkouts: &[CheckoutKey]) -> Result<(), TabError> {
        self.remove_matching(|t| checkouts.contains(t.checkout()))
    }
    fn remove_matching(&mut self, matches: impl Fn(&TabTarget) -> bool) -> Result<(), TabError> {
        if self.serial == u64::MAX {
            return Err(TabError::IdentityExhausted);
        }
        let old = self.active.and_then(|id| self.index(id).ok()).unwrap_or(0);
        let next = self
            .tabs
            .iter()
            .skip(old)
            .chain(self.tabs[..old].iter().rev())
            .find(|t| !matches(&t.session().target))
            .map(|t| t.id);
        self.closed.retain(|t| !matches(&t.target));
        self.tabs.retain(|t| !matches(&t.session().target));
        if self.active.is_some_and(|id| self.get(id).is_none()) {
            self.active = None;
            if let Some(id) = next {
                self.activate(id)?;
            }
        }
        Ok(())
    }

    pub fn disconnected(&mut self, host: &HostKey) {
        for tab in &mut self.tabs {
            let thread = tab.content.session_mut();
            if &thread.target.checkout().host == host {
                thread.connection = None;
                thread.residency = Residency::Cold {
                    reason: ColdReason::Disconnected,
                };
            }
        }
    }
    /// Call after identity validation, with a fresh connection generation. This
    /// never opens a hidden workspace; missing threads need confirmed deletion.
    pub fn reconnected(&mut self, host: &HostKey, generation: u64) -> Result<(), TabError> {
        if self.serial == u64::MAX {
            return Err(TabError::IdentityExhausted);
        }
        for tab in &mut self.tabs {
            let thread = tab.content.session_mut();
            if &thread.target.checkout().host == host {
                thread.connection = Some(generation);
                thread.residency = Residency::Cold {
                    reason: ColdReason::Disconnected,
                };
            }
        }
        if let Some(id) = self.active
            && self
                .get(id)
                .is_some_and(|t| &t.session().target.checkout().host == host)
        {
            self.activate(id)?;
        }
        Ok(())
    }

    pub fn accepts_lifetime(&self, lifetime: TabLifetime) -> bool {
        self.get(lifetime.tab)
            .is_some_and(|t| t.lifetime == lifetime)
    }
    pub fn read_stamp(&self, id: TabId) -> Option<ReadStamp> {
        if self.active != Some(id) {
            return None;
        }
        let t = self.get(id)?;
        Some(ReadStamp {
            lifetime: t.lifetime,
            visibility: t.session().residency.visibility()?,
            connection: t.session().connection?,
        })
    }
    /// Also check request generation, source revision and path/record on each job.
    pub fn accepts_read(&self, stamp: ReadStamp) -> bool {
        self.read_stamp(stamp.lifetime.tab) == Some(stamp)
    }
    pub fn resident_mut(&mut self, stamp: ReadStamp) -> Option<&mut ResidentThread> {
        if !self.accepts_read(stamp) {
            return None;
        }
        let index = self.index(stamp.lifetime.tab).ok()?;
        match &mut self.tabs[index].content.session_mut().residency {
            Residency::Loading { data, .. } | Residency::Ready { data, .. } => Some(data),
            Residency::Cold { .. } => None,
        }
    }
    pub fn finish_activation(&mut self, stamp: ReadStamp) -> bool {
        if !self.accepts_read(stamp) {
            return false;
        }
        let Ok(i) = self.index(stamp.lifetime.tab) else {
            return false;
        };
        let residency = &mut self.tabs[i].content.session_mut().residency;
        if matches!(residency, Residency::Loading { .. }) {
            let old = std::mem::replace(
                residency,
                Residency::Cold {
                    reason: ColdReason::Parked,
                },
            );
            if let Residency::Loading { visibility, data } = old {
                *residency = Residency::Ready { visibility, data };
            }
        }
        true
    }
}

fn content_for(session: Box<TabSession>) -> TabContent {
    // Project views share one content kind; the target says which view.
    if session.target.is_project_view() {
        TabContent::Worktrees(session)
    } else {
        TabContent::Thread(session)
    }
}

pub(crate) fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':' | b'.'))
}
fn validate_record(target: &TabTarget, bookmark: &BookmarkRecord) -> Result<(), TabError> {
    if !target.valid() {
        return Err(TabError::InvalidIdentity);
    }
    if !bookmark.valid()
        || (matches!(bookmark, BookmarkRecord::Worktrees { .. }) && !target.is_worktrees())
        || (matches!(bookmark, BookmarkRecord::Issues { .. }) && !target.is_issues())
    {
        return Err(TabError::InvalidBookmark);
    }
    let size = std::mem::size_of::<TabRecord>()
        + std::mem::size_of::<TabSession>()
        + target.retained_bytes()
        + bookmark.retained_bytes();
    if size > COLD_TAB_BYTES {
        return Err(TabError::BookmarkTooLarge);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
