//! Per-machine saved work (#132). Which namespace each draft, thread composer
//! and send ledger saves to, what the window reads, and how work saved before a
//! host was verified is copied to it. The design:
//! <https://github.com/mostlyheath/plait/issues/132#issuecomment-7153471382>.
use super::Plait;
use gpui::*;
use plait_client::{
    host::ThreadSummary,
    session::{Copied, DraftTab, Namespace, Origin, Session, Thread},
    tabs::HostKey,
};
use std::{collections::HashSet, path::Path};

/// Where a draft, thread composer or send ledger saves. Set when it is created
/// or mounted and never changed afterwards, so a machine switch, a reconnect or
/// a change of locality cannot redirect its saves.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Binding {
    pub namespace: Namespace,
    /// The record its text was loaded from, when that is another namespace's.
    pub origin: Option<Origin>,
}

/// Saved-work state that is not itself saved.
#[derive(Default)]
pub(super) struct SavedWork {
    /// Where saved sends in memory are written; set when the first is loaded
    /// or made after a machine change.
    pub ledger: Option<Namespace>,
    /// Listed threads whose sends are in memory, with the folder they are for.
    /// Memory keys sends by thread ID alone, so a listed thread now at another
    /// folder conflicts: nothing of it is sent or saved.
    pub adopted: std::collections::HashMap<String, std::path::PathBuf>,
    pub origins: std::collections::HashMap<String, Origin>,
    /// Copies to provisional work's host namespace, awaiting a durable save
    /// before their sources are removed: (source, records copied).
    pub migrations: Vec<(Namespace, Vec<Copied>)>,
}

impl Plait {
    /// The selected machine's host identity, once verified on this connection.
    /// Not during a switch, a reconnect, or before the first check.
    pub(super) fn verified_host(&self) -> Option<&HostKey> {
        (self.tabs.identity.verified
            && self.tab_identity_current()
            && self.tabs.identity.machine.as_ref() == Some(&self.machines.active))
        .then_some(&self.tabs.host)
    }

    /// Where new work is saved now: the verified host's namespace, or the
    /// selected machine's provisional one.
    pub(super) fn write_namespace(&self) -> Namespace {
        match self.verified_host() {
            Some(host) => Namespace::Host { host: host.clone() },
            None => Namespace::Provisional {
                machine: self.machines.active.clone(),
            },
        }
    }

    /// The current machine's namespaces, most current first. Before a check,
    /// the host last verified for this machine is read too, read-only.
    pub(super) fn read_namespaces(&self) -> Vec<Namespace> {
        let provisional = Namespace::Provisional {
            machine: self.machines.active.clone(),
        };
        if let Some(host) = self.verified_host() {
            return vec![Namespace::Host { host: host.clone() }, provisional];
        }
        let mut read = vec![provisional];
        if let Some(host) = self
            .session
            .data
            .as_ref()
            .and_then(|data| data.last_verified.get(&self.machines.active))
        {
            read.push(Namespace::Host { host: host.clone() });
        }
        read
    }

    /// Whether `namespace` is one the current machine saves to.
    pub(super) fn writes_here(&self, namespace: &Namespace) -> bool {
        match namespace {
            Namespace::Provisional { machine } => *machine == self.machines.active,
            Namespace::Host { host } => self.verified_host() == Some(host),
        }
    }

    fn binding_from(&self, found: Option<(Namespace, u64)>) -> Binding {
        let namespace = self.write_namespace();
        let origin = found
            .filter(|(found, _)| *found != namespace)
            .map(|(namespace, revision)| Origin {
                namespace,
                revision,
            });
        Binding { namespace, origin }
    }

    /// Draft `id`'s saved record among the current machine's namespaces.
    pub(super) fn saved_draft_record(&self, id: &str) -> Option<(Namespace, u64, &DraftTab)> {
        let data = self.session.data.as_ref()?;
        self.read_namespaces().into_iter().find_map(|namespace| {
            let saved = data.draft_in(&namespace, id)?;
            Some((namespace, saved.revision, &saved.tab))
        })
    }

    /// Draft `id`'s saved text for `checkout`, from the current machine.
    pub(super) fn saved_draft(&self, id: &str, checkout: &Path) -> Option<&DraftTab> {
        self.saved_draft_record(id)
            .map(|(_, _, tab)| tab)
            .filter(|tab| tab.checkout == checkout)
    }

    /// The current machine's saved drafts, each once, most current first.
    pub(super) fn saved_drafts(&self) -> Vec<(String, &DraftTab)> {
        let Some(data) = &self.session.data else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        self.read_namespaces()
            .iter()
            .filter_map(|namespace| data.space(namespace))
            .flat_map(|space| space.drafts.iter())
            .filter(|draft| seen.insert(draft.id.clone()))
            .map(|draft| (draft.id.clone(), &draft.tab))
            .collect()
    }

    pub(super) fn saved_thread_record(
        &self,
        summary: &ThreadSummary,
    ) -> Option<(Namespace, u64, &Thread)> {
        let data = self.session.data.as_ref()?;
        self.read_namespaces().into_iter().find_map(|namespace| {
            let saved = data.thread_in(&namespace, &summary.id, &summary.checkout)?;
            Some((namespace, saved.revision, &saved.thread))
        })
    }

    /// A listed thread's saved work, from the current machine only.
    pub(super) fn saved_thread(&self, summary: &ThreadSummary) -> Option<&Thread> {
        self.saved_thread_record(summary)
            .map(|(_, _, thread)| thread)
    }

    /// Bind the open draft, if it is not bound yet, to where it saves now.
    pub(super) fn bind_draft(&mut self) {
        if self.draft.binding.is_some() {
            return;
        }
        let Some(id) = self.draft.id.clone() else {
            return;
        };
        let found = self
            .saved_draft_record(&id)
            .map(|(namespace, revision, _)| (namespace, revision));
        self.draft.binding = Some(self.binding_from(found));
    }

    /// Bind thread `id`'s composer as it mounts, and return its saved text.
    pub(super) fn bind_thread_composer(&mut self, id: &str) -> Option<Thread> {
        let summary = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .cloned()?;
        let found = self
            .saved_thread_record(&summary)
            .map(|(namespace, revision, thread)| (namespace, revision, thread.clone()));
        let binding = self.binding_from(
            found
                .as_ref()
                .map(|(namespace, revision, _)| (namespace.clone(), *revision)),
        );
        self.workspace
            .bindings
            .entry(id.to_owned())
            .or_insert(binding);
        found.map(|(_, _, thread)| thread)
    }

    /// The namespace saved sends are written to, bound on first use.
    pub(super) fn ledger_namespace(&mut self) -> Namespace {
        if self.saved_work.ledger.is_none() {
            self.saved_work.ledger = Some(self.write_namespace());
        }
        self.saved_work.ledger.clone().unwrap()
    }

    /// Load saved sends for listed threads, from the current machine's record
    /// at the listed path. Another machine's or another path's sends are never
    /// loaded, so nothing reconciles or rewrites them.
    pub(super) fn adopt_saved_sends(&mut self) {
        let ledger = self.ledger_namespace();
        let found: Vec<(String, Namespace, u64, Thread)> = self
            .host
            .connection
            .threads()
            .iter()
            .filter(|summary| !self.saved_work.adopted.contains_key(&summary.id))
            .filter_map(|summary| {
                self.saved_thread_record(summary)
                    .map(|(namespace, revision, thread)| {
                        (summary.id.clone(), namespace, revision, thread.clone())
                    })
            })
            .collect();
        for (id, namespace, revision, thread) in found {
            for pending in thread.pending.iter().chain(&thread.pending_submissions) {
                self.host.submissions.restore(id.clone(), pending.clone());
            }
            for pending in &thread.queued_submissions {
                self.host
                    .submissions
                    .restore_accepted(id.clone(), pending.clone());
            }
            if namespace != ledger {
                self.saved_work.origins.insert(
                    id.clone(),
                    Origin {
                        namespace,
                        revision,
                    },
                );
            }
            self.saved_work.adopted.insert(id, thread.checkout.clone());
        }
    }

    /// Whether listed thread `summary` is at another folder than the sends in
    /// memory for its ID.
    pub(super) fn ledger_conflict(&self, summary: &ThreadSummary) -> bool {
        self.saved_work
            .adopted
            .get(&summary.id)
            .is_some_and(|checkout| *checkout != summary.checkout)
    }

    /// Leaving a machine: its saved sends stay saved in its namespace, and the
    /// next machine starts a ledger of its own.
    pub(super) fn forget_saved_work(&mut self) {
        self.host.submissions = Default::default();
        self.saved_work.ledger = None;
        self.saved_work.adopted.clear();
        self.saved_work.origins.clear();
    }

    /// Once the selected machine's host is verified: record it as the
    /// machine's last verified host, copy older records in if this is this
    /// computer, and copy its provisional work across. Sources stay until the
    /// copy is durable.
    pub(super) fn migrate_saved_work(&mut self, next: &mut Session) -> Result<(), String> {
        let Some(host) = self.verified_host().cloned() else {
            return Ok(());
        };
        let machine = self.machines.active.clone();
        if next.last_verified.get(&machine) != Some(&host) {
            next.last_verified.insert(machine.clone(), host.clone());
        }
        let target = Namespace::Host { host };
        if machine == plait_client::machines::MachineId::default() {
            next.import_legacy(&target)?;
        }
        let source = Namespace::Provisional { machine };
        let copied = next.migrate(&source, &target)?;
        if !copied.is_empty() {
            self.saved_work.migrations.push((source, copied));
        }
        Ok(())
    }

    /// The first `count` copies are durable: remove what they copied from.
    pub(super) fn complete_migrations(&mut self, count: usize, cx: &mut Context<Self>) {
        let count = count.min(self.saved_work.migrations.len());
        if count == 0 {
            return;
        }
        let done: Vec<_> = self.saved_work.migrations.drain(..count).collect();
        let Some(data) = &mut self.session.data else {
            return;
        };
        for (source, copied) in &done {
            data.remove_copied(source, copied);
        }
        if let Some(journal) = &self.session.journal {
            journal.queue(data.clone());
        }
        self.schedule_session_save(false, cx);
    }

    /// Why the open draft cannot be sent here, on any send path: the selected
    /// machine's host is not verified yet, or the draft saves to another
    /// machine. A draft not bound yet would bind to this machine.
    pub(super) fn draft_send_refusal(&self) -> Option<String> {
        if self.verified_host().is_none() {
            return Some("Checking which machine this is…".into());
        }
        match &self.draft.binding {
            Some(binding) if !self.writes_here(&binding.namespace) => Some(
                "This draft was written for another machine. Switch back to it to send.".into(),
            ),
            _ => None,
        }
    }
}

#[cfg(test)]
impl Plait {
    /// Scenes that start the tab state over: nothing mounted stays bound to
    /// the identity before, as a parked workspace would not.
    pub(super) fn reset_tabs_for_test(&mut self) {
        self.tabs = Default::default();
        self.workspace.bindings.clear();
        self.draft.binding = None;
    }

    /// The current machine's saved thread work, by thread ID, for scenes.
    pub(super) fn test_saved_threads(&self) -> std::collections::BTreeMap<String, Thread> {
        let Some(data) = &self.session.data else {
            return Default::default();
        };
        let mut threads = std::collections::BTreeMap::new();
        for namespace in self.read_namespaces() {
            for saved in data.space(&namespace).into_iter().flat_map(|s| &s.threads) {
                threads
                    .entry(saved.id.clone())
                    .or_insert_with(|| saved.thread.clone());
            }
        }
        threads
    }

    /// The current machine's saved drafts, by draft ID, for scenes.
    pub(super) fn test_saved_drafts(&self) -> std::collections::BTreeMap<String, DraftTab> {
        self.saved_drafts()
            .into_iter()
            .map(|(id, tab)| (id, tab.clone()))
            .collect()
    }
}
