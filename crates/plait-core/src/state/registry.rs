//! Repository job routing. Parked checkouts retain identity and write outcomes,
//! not a thread view or background read demand.
use super::*;
use crate::ownership::{CheckoutKey, HostKey};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CheckoutId(u64);

pub struct CheckoutRegistry {
    host: Option<HostKey>,
    active: CheckoutId,
    next: u64,
    keys: HashMap<CheckoutId, CheckoutKey>,
    parked: HashMap<CheckoutId, CheckoutRepositoryData>,
    redirects: HashMap<CheckoutId, CheckoutId>,
}

impl Default for CheckoutRegistry {
    fn default() -> Self {
        Self {
            host: HostKey::local(),
            active: CheckoutId(0),
            next: 1,
            keys: HashMap::new(),
            parked: HashMap::new(),
            redirects: HashMap::new(),
        }
    }
}

impl CheckoutRegistry {
    fn resolve(&self, mut id: CheckoutId) -> CheckoutId {
        while let Some(next) = self.redirects.get(&id) {
            id = *next;
        }
        id
    }
}

pub struct RepositoryJob {
    pub owner: CheckoutId,
    pub(super) expected: Option<CheckoutKey>,
    pub(super) host: Option<HostKey>,
    pub(super) view: u64,
    pub job: Job,
}

pub struct RepositoryReply {
    pub(super) identity: Option<CheckoutKey>,
    pub(super) reply: Reply,
}

impl RepositoryJob {
    /// Blocking. Runs on this Mac or on the checkout's host.
    pub fn run_on(&self, backend: &super::backend::Backend) -> RepositoryReply {
        match backend {
            super::backend::Backend::Local => self.run(),
            super::backend::Backend::Host(client) => self.run_remote(client),
        }
    }

    /// Blocking. The target is checked before any Git operation and again before
    /// its reply can be installed; a root replacement is never a cache hit.
    pub fn run(&self) -> RepositoryReply {
        if self.expected.is_none() && matches!(self.job, Job::Write { .. }) {
            return RepositoryReply {
                identity: None,
                reply: self
                    .job
                    .failure("Checkout identity is not verified.".into()),
            };
        }
        if let Some(key) = &self.expected
            && let Err(error) = key.verify()
        {
            return RepositoryReply {
                identity: None,
                reply: self.job.failure(error),
            };
        }
        let mut reply = match (&self.job, &self.expected) {
            (Job::Write { root, op }, Some(key)) => Reply::Write(git::write_checked(
                root,
                Some(crate::lease::DirIdentity {
                    dev: key.device,
                    ino: key.inode,
                }),
                op,
            )),
            _ => self.job.run(),
        };
        let identity = match &reply {
            Reply::Status(Ok(snapshot)) => {
                match CheckoutKey::resolve(self.host.clone(), &snapshot.root) {
                    Ok(key)
                        if self
                            .expected
                            .as_ref()
                            .is_none_or(|expected| expected == &key) =>
                    {
                        Some(key)
                    }
                    Ok(_) => {
                        reply = self.job.failure("Checkout identity changed.".into());
                        None
                    }
                    Err(error) => {
                        reply = self.job.failure(error);
                        None
                    }
                }
            }
            _ => None,
        };
        if let Some(key) = &self.expected
            && let Err(error) = key.verify()
        {
            reply = self.job.failure(error);
        }
        RepositoryReply { identity, reply }
    }
}

impl Job {
    pub(super) fn failure(&self, error: String) -> Reply {
        match self {
            Self::Status { .. } => Reply::Status(Err(error)),
            Self::Diff { .. } => Reply::Diff(Err(error)),
            Self::Files { .. } => Reply::Files(Err(error)),
            Self::Write { .. } => Reply::Write(Err(error)),
            Self::Log { .. } => Reply::Log(Err(error)),
            Self::Show { .. } => Reply::Show(Err(error)),
            Self::Branches { .. } => Reply::Branches(Err(error)),
        }
    }
}

impl AppState {
    pub fn checkout_id(&self) -> CheckoutId {
        self.checkouts.active
    }
    pub fn checkout_key(&self) -> Option<&CheckoutKey> {
        self.checkouts.keys.get(&self.checkouts.active)
    }

    pub fn open_checkout(&mut self, path: PathBuf) {
        if self.repository.path() == path {
            self.repository.reset_view();
            self.repository.refresh();
            return;
        }
        let registry = &mut self.checkouts;
        let owner = registry
            .parked
            .iter()
            .find(|(_, data)| data.path == path)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| {
                let id = CheckoutId(registry.next);
                registry.next += 1;
                id
            });
        let data = registry
            .parked
            .remove(&owner)
            .unwrap_or_else(|| CheckoutRepositoryData::new(path.clone()));
        let mut previous = std::mem::replace(&mut self.repository.data, data);
        previous.park();
        registry.parked.insert(registry.active, previous);
        registry.active = owner;
        self.repository.reset_view();
        self.repository.data.path = path;
        self.repository.refresh();
    }

    pub fn next_repository_job(&mut self) -> Option<RepositoryJob> {
        let registry = &mut self.checkouts;
        let (owner, job) = if let Some(job) = self.repository.next_job() {
            (registry.active, job)
        } else {
            registry.parked.iter_mut().find_map(|(owner, data)| {
                if data.running {
                    return None;
                }
                let root = data.root.clone()?;
                let op = data.write.queued.take()?;
                data.running = true;
                data.write.running = true;
                Some((*owner, Job::Write { root, op }))
            })?
        };
        Some(RepositoryJob {
            owner,
            expected: registry.keys.get(&owner).cloned(),
            host: registry.host.clone(),
            view: self.repository.view_generation,
            job,
        })
    }

    pub fn write_for(&mut self, owner: CheckoutId) -> Option<&mut WriteState> {
        let owner = self.checkouts.resolve(owner);
        if owner == self.checkouts.active {
            Some(&mut self.repository.data.write)
        } else {
            self.checkouts
                .parked
                .get_mut(&owner)
                .map(|data| &mut data.write)
        }
    }

    pub fn invalidate_checkout(&mut self, owner: CheckoutId) {
        let owner = self.checkouts.resolve(owner);
        if owner == self.checkouts.active {
            self.repository.refresh();
        } else if let Some(data) = self.checkouts.parked.get_mut(&owner) {
            data.park();
        }
    }

    pub fn request_write_for(&mut self, owner: CheckoutId, op: git::WriteOp) -> bool {
        let owner = self.checkouts.resolve(owner);
        if owner == self.checkouts.active {
            return self.repository.request_write(op);
        }
        let Some(data) = self.checkouts.parked.get_mut(&owner) else {
            return false;
        };
        if data.write.busy() || data.root.is_none() {
            return false;
        }
        data.write.note = None;
        data.write.undo = None;
        data.write.queued = Some(op);
        true
    }

    /// Returns whether the reply belongs to the checkout currently in front.
    pub fn complete_repository_job(&mut self, job: RepositoryJob, reply: RepositoryReply) -> bool {
        let registry = &mut self.checkouts;
        if let Some(key) = reply.identity {
            let existing = registry
                .keys
                .iter()
                .find(|(owner, known)| **owner != job.owner && **known == key)
                .map(|(owner, _)| *owner);
            if let Some(existing) = existing {
                // Before its first status, a provisional checkout cannot queue a write.
                // Rebind aliases to the existing writer before installing any read data.
                registry.redirects.insert(job.owner, existing);
                if job.owner == registry.active {
                    let data = registry
                        .parked
                        .remove(&existing)
                        .expect("known inactive checkout");
                    self.repository.data = data;
                    registry.active = existing;
                    self.repository.refresh();
                    return true;
                }
                registry.parked.remove(&job.owner);
                return false;
            }
            registry.keys.insert(job.owner, key);
        }
        if job.owner == registry.active {
            if job.view != self.repository.view_generation
                && matches!(job.job, Job::Diff { .. } | Job::Show { .. })
            {
                let view = std::mem::take(&mut self.repository.view);
                self.repository.complete(job.job, reply.reply);
                self.repository.view = view;
            } else {
                self.repository.complete(job.job, reply.reply);
            }
            true
        } else {
            if let Some(data) = registry.parked.remove(&job.owner) {
                let mut repository = RepositoryState {
                    data,
                    view: ThreadRepositoryView::default(),
                    view_generation: 0,
                };
                repository.complete(job.job, reply.reply);
                repository.data.park();
                registry.parked.insert(job.owner, repository.data);
            }
            false
        }
    }
}

impl CheckoutRepositoryData {
    fn park(&mut self) {
        self.epoch += 1;
        self.pending = None;
        self.prefetch = None;
        self.files_queued = false;
        self.snapshot = Load::Empty;
        self.files = Load::Empty;
        self.cache.clear();
        self.commits = CommitLogData {
            epoch: self.commits.epoch + 1,
            ..Default::default()
        };
        self.branches = Load::Empty;
        self.branch_request += 1;
        self.branches_queued = false;
        self.write.reselect = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str, inode: u64) -> CheckoutKey {
        CheckoutKey {
            host: None,
            root: path.into(),
            device: 1,
            inode,
        }
    }

    fn status(state: &mut AppState, identity: CheckoutKey) {
        let job = state.next_repository_job().unwrap();
        assert!(matches!(job.job, Job::Status { .. }));
        let mut snapshot = super::super::tests::snapshot().as_ref().clone_for_test();
        snapshot.root = identity.root.clone();
        state.complete_repository_job(
            job,
            RepositoryReply {
                identity: Some(identity),
                reply: Reply::Status(Ok(Arc::new(snapshot))),
            },
        );
        // These tests control completion order without running fixture Git reads.
        state.repository.data.pending = None;
    }

    use super::super::tests::CloneForTest;

    #[test]
    fn retiring_a_loading_view_reissues_its_request() {
        let mut state = AppState::new("/a".into());
        status(&mut state, key("/a", 1));
        state.repository.select(0, DiffMode::Staged);
        let old = state.next_repository_job().unwrap();
        state.repository.invalidate_view();
        state.complete_repository_job(
            old,
            RepositoryReply {
                identity: None,
                reply: Reply::Diff(Err("old view".into())),
            },
        );
        let fresh = state.next_repository_job().unwrap();
        assert!(matches!(fresh.job, Job::Diff { .. }));
        state.complete_repository_job(
            fresh,
            RepositoryReply {
                identity: None,
                reply: Reply::Diff(Err("current result".into())),
            },
        );
        assert!(matches!(state.repository.diff(), Load::Error(error) if error == "current result"));
    }

    #[test]
    fn inactive_write_acknowledges_its_checkout_and_queues_no_reads() {
        let mut state = AppState::new("/a".into());
        status(&mut state, key("/a", 1));
        let owner = state.checkout_id();
        assert!(state.request_write_for(owner, git::WriteOp::Stage(vec!["a".into()])));
        let write = state.next_repository_job().unwrap();
        state.open_checkout("/b".into());
        status(&mut state, key("/b", 2));
        assert!(!state.complete_repository_job(
            write,
            RepositoryReply {
                identity: None,
                reply: Reply::Write(Err("A refused".into())),
            }
        ));
        assert!(state.repository.write().note.is_none());
        assert_eq!(
            state.write_for(owner).unwrap().note.as_ref().unwrap().0,
            "A refused"
        );
        assert!(state.next_repository_job().is_none());
        state.open_checkout("/a".into());
        assert_eq!(state.checkout_id(), owner);
        assert_eq!(
            state.repository.write().note.as_ref().unwrap().0,
            "A refused"
        );
    }

    #[test]
    fn returning_to_a_checkout_cannot_accept_its_old_read() {
        let mut state = AppState::new("/a".into());
        status(&mut state, key("/a", 1));
        state.repository.select(1, DiffMode::Unstaged);
        let old = state.next_repository_job().unwrap();
        state.open_checkout("/b".into());
        status(&mut state, key("/b", 2));
        state.open_checkout("/a".into());
        state.complete_repository_job(
            old,
            RepositoryReply {
                identity: None,
                reply: Reply::Diff(Err("old A".into())),
            },
        );
        assert!(matches!(state.repository.diff(), Load::Empty));
        assert!(matches!(
            state.next_repository_job().unwrap().job,
            Job::Status { .. }
        ));
    }

    #[test]
    fn same_checkout_views_share_cache_but_not_delayed_selection() {
        let mut state = AppState::new("/a".into());
        status(&mut state, key("/a", 1));
        state.repository.select(0, DiffMode::Staged);
        let old = state.next_repository_job().unwrap();
        let a = std::mem::take(&mut state.repository.view);
        state.repository.invalidate_view();
        state.repository.select(1, DiffMode::Unstaged);
        state.complete_repository_job(
            old,
            RepositoryReply {
                identity: None,
                reply: Reply::Diff(Err("old view".into())),
            },
        );
        assert_eq!(a.changes.selected.unwrap().path, PathBuf::from("a"));
        assert_eq!(
            state
                .repository
                .view
                .changes
                .selected
                .as_ref()
                .unwrap()
                .path,
            PathBuf::from("b")
        );
        assert!(matches!(state.repository.diff(), Load::Loading));
        assert_eq!(
            state.repository.snapshot().ready().unwrap().root,
            PathBuf::from("/a")
        );
    }

    #[test]
    fn aliases_rejoin_the_existing_writer_before_scheduling() {
        let mut state = AppState::new("/a".into());
        status(&mut state, key("/a", 1));
        let owner = state.checkout_id();
        assert!(state.request_write_for(owner, git::WriteOp::Stage(vec!["a".into()])));
        let write = state.next_repository_job().unwrap();
        state.open_checkout("/alias".into());
        status(&mut state, key("/a", 1));
        assert_eq!(state.checkout_id(), owner);
        assert!(state.repository.write().busy());
        assert!(state.next_repository_job().is_none());
        assert!(state.complete_repository_job(
            write,
            RepositoryReply {
                identity: None,
                reply: Reply::Write(Err("A refused".into())),
            }
        ));
        assert!(!state.repository.write().busy());
    }
}
