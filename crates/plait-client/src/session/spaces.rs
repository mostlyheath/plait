//! Saved work kept apart per machine (#132). Each namespace holds its own
//! records, and nothing in one namespace is read or changed through another.
//! The design: <https://github.com/mostlyheath/plait/issues/132#issuecomment-7153471382>.
use super::{DraftTab, Session, Thread};
use crate::{machines::MachineId, tabs::HostKey};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

/// Unsent drafts one namespace may hold. With [`THREAD_CAP`] and [`SPACE_CAP`]
/// this keeps a namespace's records well inside the 4 MiB journal; the file
/// size is still checked as a whole on every save.
pub const DRAFT_CAP: usize = 256;
/// Threads with saved work one namespace may hold.
pub const THREAD_CAP: usize = 256;
/// Namespaces one journal may hold: a verified and a provisional one for each
/// of the 32 machines a desktop can list.
pub const SPACE_CAP: usize = 64;

/// Whose saved work a record is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Namespace {
    /// A host identity verified on a connection.
    Host { host: HostKey },
    /// The machine selected while its host was not verified yet.
    Provisional { machine: MachineId },
}

/// The record, and its revision, that a record was copied or loaded from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub namespace: Namespace,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDraft {
    pub id: String,
    /// Advances, within its namespace, each time the record changes.
    pub revision: u64,
    /// In a host namespace: set while the record is an unedited copy of a
    /// provisional record. In a provisional one: the record it started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Origin>,
    pub tab: DraftTab,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedThread {
    pub id: String,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Origin>,
    /// Keyed by `id` and `thread.checkout`: the same ID at another path is
    /// another thread.
    pub thread: Thread,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub namespace: Namespace,
    /// The next record revision; never reused, even after a record is removed.
    #[serde(default)]
    pub next_revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<SavedDraft>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<SavedThread>,
    /// Records copied in, from before namespaces (`draft:` and `thread:` IDs)
    /// or from a provisional namespace (`copy:` origins), so none is copied twice.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub imported: BTreeSet<String>,
}

/// A source record copied by [`Session::migrate`], at the revision copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Copied {
    Draft {
        id: String,
        revision: u64,
    },
    Thread {
        id: String,
        checkout: std::path::PathBuf,
        revision: u64,
    },
}

impl Space {
    fn new(namespace: Namespace) -> Self {
        Self {
            namespace,
            next_revision: 1,
            drafts: Vec::new(),
            threads: Vec::new(),
            imported: BTreeSet::new(),
        }
    }

    fn take_revision(&mut self) -> u64 {
        let revision = self.next_revision.max(1);
        self.next_revision = revision + 1;
        revision
    }

    /// Whether `origin` was already copied here.
    fn copied_from(&self, origin: &Origin) -> bool {
        self.imported.contains(&origin.mark())
    }

    fn mark_copied(&mut self, origin: &Origin) {
        self.imported.insert(origin.mark());
    }

    fn is_empty(&self) -> bool {
        self.drafts.is_empty() && self.threads.is_empty() && self.imported.is_empty()
    }
}

impl Origin {
    fn of(namespace: &Namespace, revision: u64) -> Self {
        Self {
            namespace: namespace.clone(),
            revision,
        }
    }

    /// How a copied origin is recorded among a namespace's imported marks.
    fn mark(&self) -> String {
        format!(
            "copy:{}:{}",
            serde_json::to_string(&self.namespace).unwrap_or_default(),
            self.revision
        )
    }
}

/// A draft with nothing to keep: no text, images or unanswered first send.
pub fn draft_is_empty(tab: &DraftTab) -> bool {
    tab.creation.is_none()
        && tab.draft.creation.is_none()
        && tab.draft.text.is_empty()
        && tab.draft.images.is_empty()
}

impl Session {
    pub fn space(&self, namespace: &Namespace) -> Option<&Space> {
        self.spaces
            .iter()
            .find(|space| &space.namespace == namespace)
    }

    fn space_mut(&mut self, namespace: &Namespace) -> Result<&mut Space, String> {
        if let Some(index) = self
            .spaces
            .iter()
            .position(|space| &space.namespace == namespace)
        {
            return Ok(&mut self.spaces[index]);
        }
        if self.spaces.len() >= SPACE_CAP {
            return Err("Saved work is kept for too many machines. Nothing was discarded.".into());
        }
        self.spaces.push(Space::new(namespace.clone()));
        Ok(self.spaces.last_mut().unwrap())
    }

    pub fn draft_in(&self, namespace: &Namespace, id: &str) -> Option<&SavedDraft> {
        self.space(namespace)?.drafts.iter().find(|d| d.id == id)
    }

    pub fn thread_in(
        &self,
        namespace: &Namespace,
        id: &str,
        checkout: &Path,
    ) -> Option<&SavedThread> {
        self.space(namespace)?
            .threads
            .iter()
            .find(|t| t.id == id && t.thread.checkout == checkout)
    }

    /// The first of `read` holding draft `id`, and where it was found.
    pub fn find_draft<'a>(
        &'a self,
        read: &'a [Namespace],
        id: &str,
    ) -> Option<(&'a Namespace, &'a SavedDraft)> {
        read.iter()
            .find_map(|namespace| self.draft_in(namespace, id).map(|d| (namespace, d)))
    }

    pub fn find_thread<'a>(
        &'a self,
        read: &'a [Namespace],
        id: &str,
        checkout: &Path,
    ) -> Option<(&'a Namespace, &'a SavedThread)> {
        read.iter().find_map(|namespace| {
            self.thread_in(namespace, id, checkout)
                .map(|t| (namespace, t))
        })
    }

    /// Save draft `id` in `namespace`. It never touches another namespace. A
    /// new record starts from `base`; an edit in a host namespace ends its
    /// standing as an unedited copy. Only the namespace's cap refuses a save,
    /// and then nothing is changed.
    pub fn put_draft(
        &mut self,
        namespace: &Namespace,
        id: &str,
        tab: DraftTab,
        base: Option<Origin>,
    ) -> Result<(), String> {
        let host = matches!(namespace, Namespace::Host { .. });
        let space = self.space_mut(namespace)?;
        if let Some(index) = space.drafts.iter().position(|d| d.id == id) {
            if space.drafts[index].tab == tab {
                return Ok(());
            }
            let revision = space.take_revision();
            let saved = &mut space.drafts[index];
            saved.tab = tab;
            saved.revision = revision;
            if host {
                saved.base = None;
            }
            return Ok(());
        }
        if space.drafts.len() >= DRAFT_CAP {
            return Err(format!(
                "This machine already has {DRAFT_CAP} saved drafts. Send or discard one; nothing was discarded."
            ));
        }
        let revision = space.take_revision();
        space.drafts.push(SavedDraft {
            id: id.to_owned(),
            revision,
            base,
            tab,
        });
        Ok(())
    }

    /// Save thread `id` at `thread.checkout` in `namespace`, as [`Self::put_draft`] does.
    pub fn put_thread(
        &mut self,
        namespace: &Namespace,
        id: &str,
        thread: Thread,
        base: Option<Origin>,
    ) -> Result<(), String> {
        let host = matches!(namespace, Namespace::Host { .. });
        let space = self.space_mut(namespace)?;
        if let Some(index) = space
            .threads
            .iter()
            .position(|t| t.id == id && t.thread.checkout == thread.checkout)
        {
            if space.threads[index].thread == thread {
                return Ok(());
            }
            let revision = space.take_revision();
            let saved = &mut space.threads[index];
            saved.thread = thread;
            saved.revision = revision;
            if host {
                saved.base = None;
            }
            return Ok(());
        }
        if space.threads.len() >= THREAD_CAP {
            return Err(format!(
                "This machine already has saved work for {THREAD_CAP} threads. Nothing was discarded."
            ));
        }
        let revision = space.take_revision();
        space.threads.push(SavedThread {
            id: id.to_owned(),
            revision,
            base,
            thread,
        });
        Ok(())
    }

    /// An explicit discard, or a sent draft: removes draft `id` from
    /// `namespace` only.
    pub fn discard_draft(&mut self, namespace: &Namespace, id: &str) {
        if let Some(space) = self.spaces.iter_mut().find(|s| &s.namespace == namespace) {
            space.drafts.retain(|d| d.id != id);
        }
        self.drop_empty_spaces();
    }

    /// Empty-draft cleanup: removes draft `id` from `namespace` only if the
    /// record stored there is empty. Returns whether it was removed.
    pub fn discard_empty_draft(&mut self, namespace: &Namespace, id: &str) -> bool {
        let empty = self
            .draft_in(namespace, id)
            .is_some_and(|saved| draft_is_empty(&saved.tab));
        if empty {
            self.discard_draft(namespace, id);
        }
        empty
    }

    fn drop_empty_spaces(&mut self) {
        self.spaces.retain(|space| !space.is_empty());
    }

    /// Copy every record of provisional `from` into host namespace `to`. A
    /// record already copied at the same revision is skipped. A copy replaces
    /// the record in `to` only while that record is an unedited copy of
    /// `from`, or when the source started from it unchanged; otherwise both
    /// are kept. Nothing in `from` is removed here: see [`Self::remove_copied`].
    pub fn migrate(&mut self, from: &Namespace, to: &Namespace) -> Result<Vec<Copied>, String> {
        let Some(source) = self.space(from).cloned() else {
            return Ok(Vec::new());
        };
        let mut copied = Vec::new();
        for draft in &source.drafts {
            self.migrate_draft(from, to, draft)?;
            copied.push(Copied::Draft {
                id: draft.id.clone(),
                revision: draft.revision,
            });
        }
        for thread in &source.threads {
            self.migrate_thread(from, to, thread)?;
            copied.push(Copied::Thread {
                id: thread.id.clone(),
                checkout: thread.thread.checkout.clone(),
                revision: thread.revision,
            });
        }
        Ok(copied)
    }

    /// Whether `target` may be replaced by a copy of `source`.
    fn replaceable(
        target_base: &Option<Origin>,
        target_revision: u64,
        from: &Namespace,
        to: &Namespace,
        source_base: &Option<Origin>,
    ) -> bool {
        target_base
            .as_ref()
            .is_some_and(|base| &base.namespace == from)
            || source_base.as_ref() == Some(&Origin::of(to, target_revision))
    }

    fn migrate_draft(
        &mut self,
        from: &Namespace,
        to: &Namespace,
        draft: &SavedDraft,
    ) -> Result<(), String> {
        let origin = Origin::of(from, draft.revision);
        let target = self.space_mut(to)?;
        if target.copied_from(&origin) {
            return Ok(());
        }
        let Some(index) = target.drafts.iter().position(|d| d.id == draft.id) else {
            if target.drafts.len() >= DRAFT_CAP {
                return Err(
                    "The verified machine's saved drafts are full; the unverified copy is kept."
                        .into(),
                );
            }
            let revision = target.take_revision();
            target.drafts.push(SavedDraft {
                id: draft.id.clone(),
                revision,
                base: Some(origin.clone()),
                tab: draft.tab.clone(),
            });
            target.mark_copied(&origin);
            return Ok(());
        };
        let existing = &target.drafts[index];
        if existing.tab == draft.tab {
            target.mark_copied(&origin);
            return Ok(());
        }
        if Self::replaceable(&existing.base, existing.revision, from, to, &draft.base) {
            let revision = target.take_revision();
            let saved = &mut target.drafts[index];
            saved.tab = draft.tab.clone();
            saved.revision = revision;
            saved.base = Some(origin.clone());
            target.mark_copied(&origin);
            return Ok(());
        }
        // Both changed: keep both, the copy as another draft.
        if target.drafts.len() >= DRAFT_CAP {
            return Err(
                "The verified machine's saved drafts are full; the unverified copy is kept.".into(),
            );
        }
        let revision = target.take_revision();
        target.drafts.push(SavedDraft {
            id: crate::host_state::request_id(),
            revision,
            base: Some(origin.clone()),
            tab: draft.tab.clone(),
        });
        target.mark_copied(&origin);
        Ok(())
    }

    fn migrate_thread(
        &mut self,
        from: &Namespace,
        to: &Namespace,
        thread: &SavedThread,
    ) -> Result<(), String> {
        let origin = Origin::of(from, thread.revision);
        let target = self.space_mut(to)?;
        if target.copied_from(&origin) {
            return Ok(());
        }
        let Some(index) = target
            .threads
            .iter()
            .position(|t| t.id == thread.id && t.thread.checkout == thread.thread.checkout)
        else {
            if target.threads.len() >= THREAD_CAP {
                return Err(
                    "The verified machine's saved threads are full; the unverified copy is kept."
                        .into(),
                );
            }
            let revision = target.take_revision();
            target.threads.push(SavedThread {
                id: thread.id.clone(),
                revision,
                base: Some(origin.clone()),
                thread: thread.thread.clone(),
            });
            target.mark_copied(&origin);
            return Ok(());
        };
        let existing = &target.threads[index];
        if existing.thread == thread.thread {
            target.mark_copied(&origin);
            return Ok(());
        }
        if Self::replaceable(&existing.base, existing.revision, from, to, &thread.base) {
            let revision = target.take_revision();
            let saved = &mut target.threads[index];
            saved.thread = thread.thread.clone();
            saved.revision = revision;
            saved.base = Some(origin.clone());
            target.mark_copied(&origin);
            return Ok(());
        }
        // Both changed: the verified record keeps its text and gains any send
        // it lacks; the copy's text is kept as a draft in the same folder.
        let mut merged = existing.thread.clone();
        merge_sends(&mut merged, &thread.thread);
        let text = &thread.thread.draft;
        let keep_text = (!text.text.is_empty() || !text.images.is_empty()) && *text != merged.draft;
        if keep_text && target.drafts.len() >= DRAFT_CAP {
            return Err(
                "The verified machine's saved drafts are full; the unverified copy is kept.".into(),
            );
        }
        let revision = target.take_revision();
        let saved = &mut target.threads[index];
        saved.thread = merged;
        saved.revision = revision;
        saved.base = None;
        if keep_text {
            let mut draft = text.clone();
            draft.creation = None;
            let revision = target.take_revision();
            target.drafts.push(SavedDraft {
                id: crate::host_state::request_id(),
                revision,
                base: Some(origin.clone()),
                tab: DraftTab {
                    legacy: false,
                    creation: None,
                    checkout: thread.thread.checkout.clone(),
                    draft,
                    settings: Default::default(),
                },
            });
        }
        target.mark_copied(&origin);
        Ok(())
    }

    /// Remove from `from` the records [`Self::migrate`] copied, once that copy is
    /// durable. A record changed since it was copied stays, for the next copy.
    pub fn remove_copied(&mut self, from: &Namespace, copied: &[Copied]) {
        if let Some(space) = self.spaces.iter_mut().find(|s| &s.namespace == from) {
            space.drafts.retain(|d| {
                !copied.contains(&Copied::Draft {
                    id: d.id.clone(),
                    revision: d.revision,
                })
            });
            space.threads.retain(|t| {
                !copied.contains(&Copied::Thread {
                    id: t.id.clone(),
                    checkout: t.thread.checkout.clone(),
                    revision: t.revision,
                })
            });
        }
        self.drop_empty_spaces();
    }

    /// Copy records saved before namespaces into `to`, never moving them: older
    /// builds keep reading them. Each is copied once. Returns whether any was.
    pub fn import_legacy(&mut self, to: &Namespace) -> Result<bool, String> {
        let drafts: Vec<(String, DraftTab)> = self
            .draft_tabs
            .iter()
            .map(|(id, tab)| (id.clone(), tab.clone()))
            .collect();
        let threads: Vec<(String, Thread)> = self
            .threads
            .iter()
            .map(|(id, t)| (id.clone(), t.clone()))
            .collect();
        let mut changed = false;
        for (id, tab) in drafts {
            let key = format!("draft:{id}");
            if self.space(to).is_some_and(|s| s.imported.contains(&key)) {
                continue;
            }
            let target = self.space_mut(to)?;
            let id = if target.drafts.iter().any(|d| d.id == id) {
                crate::host_state::request_id()
            } else {
                id
            };
            if target.drafts.len() >= DRAFT_CAP {
                return Err(
                    "This machine's saved drafts are full; older drafts are still kept.".into(),
                );
            }
            let revision = target.take_revision();
            target.drafts.push(SavedDraft {
                id,
                revision,
                base: None,
                tab,
            });
            target.imported.insert(key);
            changed = true;
        }
        for (id, thread) in threads {
            let key = format!("thread:{id}");
            if self.space(to).is_some_and(|s| s.imported.contains(&key)) {
                continue;
            }
            let target = self.space_mut(to)?;
            if !target
                .threads
                .iter()
                .any(|t| t.id == id && t.thread.checkout == thread.checkout)
            {
                if target.threads.len() >= THREAD_CAP {
                    return Err(
                        "This machine's saved threads are full; older work is still kept.".into(),
                    );
                }
                let revision = target.take_revision();
                target.threads.push(SavedThread {
                    id,
                    revision,
                    base: None,
                    thread,
                });
            }
            target.imported.insert(key);
            changed = true;
        }
        Ok(changed)
    }

    pub(super) fn validate_spaces(&self) -> Result<(), String> {
        if self.spaces.len() > SPACE_CAP {
            return Err(
                "Saved workspace keeps work for too many machines. Nothing was discarded.".into(),
            );
        }
        let mut namespaces = std::collections::HashSet::new();
        for space in &self.spaces {
            if !namespaces.insert(&space.namespace)
                || space.drafts.len() > DRAFT_CAP
                || space.threads.len() > THREAD_CAP
            {
                return Err(
                    "Saved per-machine work exceeds its limits. Nothing was discarded.".into(),
                );
            }
            let mut drafts = std::collections::HashSet::new();
            for draft in &space.drafts {
                if !drafts.insert(&draft.id) || draft.revision >= space.next_revision {
                    return Err(
                        "Saved per-machine drafts are inconsistent. Nothing was discarded.".into(),
                    );
                }
                super::drafts::validate_draft_tab(&draft.id, &draft.tab)?;
            }
            let mut threads = std::collections::HashSet::new();
            for thread in &space.threads {
                if !threads.insert((&thread.id, &thread.thread.checkout))
                    || thread.revision >= space.next_revision
                {
                    return Err(
                        "Saved per-machine threads are inconsistent. Nothing was discarded.".into(),
                    );
                }
                super::validate_thread(&thread.id, &thread.thread)?;
            }
        }
        Ok(())
    }
}

/// Add to `into` each pending or queued send of `from` it lacks.
fn merge_sends(into: &mut Thread, from: &Thread) {
    let known: BTreeSet<String> = into
        .pending
        .iter()
        .chain(&into.pending_submissions)
        .chain(&into.queued_submissions)
        .map(|p| p.request_id.clone())
        .collect();
    for pending in from.pending.iter().chain(&from.pending_submissions) {
        if !known.contains(&pending.request_id) {
            if into.pending.is_none() {
                into.pending = Some(pending.clone());
            } else {
                into.pending_submissions.push(pending.clone());
            }
        }
    }
    for pending in &from.queued_submissions {
        if !known.contains(&pending.request_id) {
            into.queued_submissions.push(pending.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host::ModelChoice,
        host_state::PendingSubmit,
        session::{Draft, DraftSettings},
    };
    use std::collections::BTreeMap;

    fn host(id: &str) -> Namespace {
        Namespace::Host {
            host: HostKey {
                id: id.into(),
                store: "/store".into(),
            },
        }
    }

    fn provisional(machine: &str) -> Namespace {
        Namespace::Provisional {
            machine: MachineId::new(machine).unwrap(),
        }
    }

    fn tab(text: &str) -> DraftTab {
        DraftTab {
            legacy: false,
            creation: None,
            checkout: "/project".into(),
            draft: Draft {
                text: text.into(),
                revision: 1,
                ..Default::default()
            },
            settings: DraftSettings::default(),
        }
    }

    fn wait(id: &str) -> PendingSubmit {
        PendingSubmit {
            checkout_wait: true,
            images: Vec::new(),
            request_id: id.into(),
            text: "Wait for the checkout".into(),
            approval: None,
            when_busy: None,
            draft_revision: None,
        }
    }

    fn thread(checkout: &str, text: &str, waits: &[&str]) -> Thread {
        Thread {
            checkout: checkout.into(),
            model: ModelChoice {
                provider: "p".into(),
                model_id: "m".into(),
            },
            title: "Saved".into(),
            draft: Draft {
                text: text.into(),
                revision: 1,
                ..Default::default()
            },
            pending: None,
            pending_submissions: Vec::new(),
            queued_submissions: waits.iter().map(|id| wait(id)).collect(),
            approval: None,
        }
    }

    fn session() -> Session {
        Session::empty("/host".into())
    }

    /// The same draft ID, or thread ID and folder, in two namespaces is two
    /// records; and the same thread ID at another folder is another record.
    #[test]
    fn namespaces_and_folders_keep_records_apart() {
        let (a, b) = (host("zaphod"), host("arthur"));
        let mut saved = session();
        saved.put_draft(&a, "d", tab("for zaphod"), None).unwrap();
        saved.put_draft(&b, "d", tab("for arthur"), None).unwrap();
        assert_eq!(
            saved.draft_in(&a, "d").unwrap().tab.draft.text,
            "for zaphod"
        );
        assert_eq!(
            saved.draft_in(&b, "d").unwrap().tab.draft.text,
            "for arthur"
        );
        saved
            .put_thread(&a, "t", thread("/repo", "here", &["w1"]), None)
            .unwrap();
        saved
            .put_thread(&a, "t", thread("/elsewhere", "there", &[]), None)
            .unwrap();
        assert_eq!(saved.space(&a).unwrap().threads.len(), 2);
        assert_eq!(
            saved
                .thread_in(&a, "t", Path::new("/repo"))
                .unwrap()
                .thread
                .queued_submissions
                .len(),
            1
        );
        assert!(saved.thread_in(&b, "t", Path::new("/repo")).is_none());
        // Discarding in one namespace leaves the other's record.
        saved.discard_draft(&b, "d");
        assert!(saved.draft_in(&a, "d").is_some());
        saved.validate().unwrap();
    }

    /// Revisions only advance within a namespace, even across removals, and
    /// an unchanged save is not a new revision.
    #[test]
    fn revisions_advance_and_are_never_reused() {
        let p = provisional("workstation");
        let mut saved = session();
        saved.put_draft(&p, "d", tab("one"), None).unwrap();
        let first = saved.draft_in(&p, "d").unwrap().revision;
        saved.put_draft(&p, "d", tab("one"), None).unwrap();
        assert_eq!(saved.draft_in(&p, "d").unwrap().revision, first);
        saved.put_draft(&p, "e", tab("two"), None).unwrap();
        saved.discard_draft(&p, "d");
        saved.put_draft(&p, "d", tab("three"), None).unwrap();
        assert!(saved.draft_in(&p, "d").unwrap().revision > first + 1);
    }

    /// Provisional work is copied to the host, kept at the source until the
    /// copy is durable, and copied again when it changes before removal.
    #[test]
    fn migration_copies_first_and_removes_only_unchanged_sources() {
        let (p, h) = (provisional("workstation"), host("workstation-host"));
        let mut saved = session();
        saved
            .put_draft(&p, "d", tab("typed before the check"), None)
            .unwrap();
        saved
            .put_thread(&p, "t", thread("/repo", "thread text", &["w1"]), None)
            .unwrap();
        let copied = saved.migrate(&p, &h).unwrap();
        assert_eq!(copied.len(), 2);
        // Both exist until the copy is durable.
        assert!(saved.draft_in(&p, "d").is_some());
        assert_eq!(
            saved.draft_in(&h, "d").unwrap().tab.draft.text,
            "typed before the check"
        );
        let copy = saved.thread_in(&h, "t", Path::new("/repo")).unwrap();
        assert_eq!(copy.thread.queued_submissions[0].request_id, "w1");
        // Migrating again copies nothing twice.
        saved.migrate(&p, &h).unwrap();
        assert_eq!(saved.space(&h).unwrap().drafts.len(), 1);
        // An edit at the source before removal survives removal and replaces
        // the unedited copy on the next migration.
        saved
            .put_draft(&p, "d", tab("edited after the copy"), None)
            .unwrap();
        saved.remove_copied(&p, &copied);
        assert_eq!(
            saved.draft_in(&p, "d").unwrap().tab.draft.text,
            "edited after the copy"
        );
        assert!(saved.thread_in(&p, "t", Path::new("/repo")).is_none());
        let copied = saved.migrate(&p, &h).unwrap();
        assert_eq!(
            saved.draft_in(&h, "d").unwrap().tab.draft.text,
            "edited after the copy"
        );
        assert_eq!(saved.space(&h).unwrap().drafts.len(), 1);
        saved.remove_copied(&p, &copied);
        assert!(saved.space(&p).is_none());
        saved.validate().unwrap();
    }

    /// Work copied from the last verified host and edited provisionally
    /// replaces that host's record when the host did not change meanwhile.
    #[test]
    fn migration_replaces_a_record_the_source_started_from() {
        let (p, h) = (provisional("workstation"), host("workstation-host"));
        let mut saved = session();
        saved
            .put_draft(&h, "d", tab("saved earlier"), None)
            .unwrap();
        let base = Origin::of(&h, saved.draft_in(&h, "d").unwrap().revision);
        saved
            .put_draft(&p, "d", tab("edited before the check"), Some(base))
            .unwrap();
        saved.migrate(&p, &h).unwrap();
        let drafts = &saved.space(&h).unwrap().drafts;
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].tab.draft.text, "edited before the check");
    }

    /// When both copies changed, both are kept: a draft as another draft, a
    /// thread's text as a draft in its folder, and every send.
    #[test]
    fn migration_keeps_both_when_both_changed() {
        let (p, h) = (provisional("workstation"), host("workstation-host"));
        let mut saved = session();
        saved
            .put_draft(&h, "d", tab("host's own edit"), None)
            .unwrap();
        saved
            .put_draft(&p, "d", tab("provisional edit"), None)
            .unwrap();
        saved
            .put_thread(&h, "t", thread("/repo", "host text", &["w1"]), None)
            .unwrap();
        saved
            .put_thread(&p, "t", thread("/repo", "provisional text", &["w2"]), None)
            .unwrap();
        saved.migrate(&p, &h).unwrap();
        let space = saved.space(&h).unwrap();
        let texts: Vec<_> = space
            .drafts
            .iter()
            .map(|d| d.tab.draft.text.as_str())
            .collect();
        assert!(texts.contains(&"host's own edit"));
        assert!(texts.contains(&"provisional edit"));
        assert!(texts.contains(&"provisional text"));
        let thread = &saved.thread_in(&h, "t", Path::new("/repo")).unwrap().thread;
        assert_eq!(thread.draft.text, "host text");
        let waits: Vec<_> = thread
            .queued_submissions
            .iter()
            .map(|w| w.request_id.as_str())
            .collect();
        assert_eq!(waits, ["w1", "w2"]);
        // And not again.
        saved.migrate(&p, &h).unwrap();
        assert_eq!(saved.space(&h).unwrap().drafts.len(), 3);
        saved.validate().unwrap();
    }

    /// Records from before namespaces are copied, never moved, and only once.
    #[test]
    fn legacy_records_are_copied_once_and_left_for_older_builds() {
        let h = host("local-host");
        let mut saved = session();
        saved
            .draft_tabs
            .insert("old-draft".into(), tab("older build's draft"));
        saved
            .threads
            .insert("old-thread".into(), thread("/repo", "older text", &["w1"]));
        let legacy = (saved.draft_tabs.clone(), saved.threads.clone());
        assert!(saved.import_legacy(&h).unwrap());
        assert_eq!((saved.draft_tabs.clone(), saved.threads.clone()), legacy);
        assert_eq!(
            saved.draft_in(&h, "old-draft").unwrap().tab.draft.text,
            "older build's draft"
        );
        assert!(
            saved
                .thread_in(&h, "old-thread", Path::new("/repo"))
                .is_some()
        );
        // A discard here does not bring the copy back.
        saved.discard_draft(&h, "old-draft");
        assert!(!saved.import_legacy(&h).unwrap());
        assert!(saved.draft_in(&h, "old-draft").is_none());
        saved.validate().unwrap();
    }

    /// Cleanup removes only an empty stored record, in its own namespace.
    #[test]
    fn empty_cleanup_checks_the_stored_record() {
        let p = provisional("workstation");
        let mut saved = session();
        saved.put_draft(&p, "full", tab("keep me"), None).unwrap();
        saved.put_draft(&p, "empty", tab(""), None).unwrap();
        assert!(!saved.discard_empty_draft(&p, "full"));
        assert!(!saved.discard_empty_draft(&host("other"), "empty"));
        assert!(saved.discard_empty_draft(&p, "empty"));
        assert_eq!(
            saved.draft_in(&p, "full").unwrap().tab.draft.text,
            "keep me"
        );
    }

    /// A full namespace refuses a new record and changes nothing; edits to
    /// existing records still save.
    #[test]
    fn caps_refuse_new_records_without_evicting() {
        let p = provisional("workstation");
        let mut saved = session();
        for n in 0..DRAFT_CAP {
            saved
                .put_draft(&p, &format!("d{n}"), tab("x"), None)
                .unwrap();
        }
        assert!(saved.put_draft(&p, "one-more", tab("x"), None).is_err());
        assert_eq!(saved.space(&p).unwrap().drafts.len(), DRAFT_CAP);
        saved.put_draft(&p, "d0", tab("edited"), None).unwrap();
        saved.validate().unwrap();
        let mut many = session();
        for n in 0..SPACE_CAP {
            many.put_draft(&provisional(&format!("m{n}")), "d", tab("x"), None)
                .unwrap();
        }
        assert!(
            many.put_draft(&provisional("too-many"), "d", tab("x"), None)
                .is_err()
        );
    }

    /// The fields older builds read, as they were at the last release
    /// without namespaces. Unknown fields are ignored, as serde does there.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct OlderSession {
        #[serde(default)]
        draft_tabs: BTreeMap<String, DraftTab>,
        #[serde(default)]
        new_draft: Option<(std::path::PathBuf, Draft)>,
        #[serde(default)]
        new_drafts: BTreeMap<std::path::PathBuf, Draft>,
        schema_version: u32,
        host_store: std::path::PathBuf,
        checkout: Option<std::path::PathBuf>,
        selected: Option<String>,
        threads: BTreeMap<String, Thread>,
    }

    /// A file this build writes still parses for an older build, which sees
    /// its legacy records unchanged; a file an older build wrote loads here.
    #[test]
    fn files_stay_readable_in_both_directions() {
        let h = host("local-host");
        let mut saved = session();
        saved
            .threads
            .insert("old-thread".into(), thread("/repo", "older text", &[]));
        saved.put_draft(&h, "d", tab("new draft"), None).unwrap();
        saved.last_verified.insert(
            MachineId::default(),
            HostKey {
                id: "local-host".into(),
                store: "/store".into(),
            },
        );
        let bytes = saved.encode().unwrap();
        let older: OlderSession = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(older.threads["old-thread"].draft.text, "older text");
        assert_eq!(older.schema_version, 1);
        // An older build's file: no namespaces, loads, and imports on demand.
        let mut written_by_older: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let object = written_by_older.as_object_mut().unwrap();
        object.remove("spaces");
        object.remove("last_verified");
        let loaded: Session = serde_json::from_value(written_by_older).unwrap();
        loaded.validate().unwrap();
        assert!(loaded.spaces.is_empty() && loaded.last_verified.is_empty());
        // Nothing new is written when there is nothing per machine.
        let plain = serde_json::to_value(session()).unwrap();
        assert!(plain.get("spaces").is_none() && plain.get("last_verified").is_none());
    }
}
