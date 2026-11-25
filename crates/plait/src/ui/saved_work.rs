//! Saved work per machine entry (#132). What a person types while a machine
//! is selected belongs to that machine from the start: each draft, thread
//! composer and send ledger is bound to the machine selected when it is made
//! or mounted, and saves only to that machine's file.
use super::Plait;
use plait_client::{
    host::ThreadSummary,
    machines::MachineId,
    session::{DraftTab, MachineWork, Thread},
    tabs::HostKey,
};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

/// Where a draft, thread composer or send ledger saves: set when it is made
/// or mounted, and never changed, so a switch or reconnect cannot redirect it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Binding {
    pub machine: MachineId,
}

#[derive(Default)]
pub(super) struct SavedWork {
    /// Each machine's saved work, in memory first; the journal writes changed
    /// machines' files in the background.
    pub machines: BTreeMap<MachineId, MachineWork>,
    /// The machine whose sends are in memory.
    pub ledger: Option<MachineId>,
    /// Listed threads whose sends are in memory, with the folder they are for.
    /// Memory keys sends by thread ID alone, so the same ID now at another
    /// folder conflicts: nothing of it is sent or saved.
    pub adopted: HashMap<String, PathBuf>,
    /// The host that accepted each send in memory, by request ID: loaded with
    /// the saved send, or recorded when a host's reply accepted it. Never
    /// replaced, so a replacement host cannot take over an earlier Wait.
    pub accepted_by: HashMap<String, HostKey>,
    /// The catalog revision and record count adoption last checked, so it
    /// scans again only when either changes.
    pub adoption_checked: Option<(u64, usize)>,
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

    /// The selected machine's saved work.
    pub(super) fn current_work(&self) -> Option<&MachineWork> {
        self.saved_work.machines.get(&self.machines.active)
    }

    /// Draft `id`, saved on the selected machine.
    pub(super) fn saved_draft_record(&self, id: &str) -> Option<&DraftTab> {
        self.current_work()?.drafts.get(id)
    }

    /// Draft `id`'s saved text for `checkout`, from the selected machine.
    pub(super) fn saved_draft(&self, id: &str, checkout: &Path) -> Option<&DraftTab> {
        self.saved_draft_record(id)
            .filter(|tab| tab.checkout == checkout)
    }

    /// The selected machine's saved drafts.
    pub(super) fn saved_drafts(&self) -> Vec<(String, &DraftTab)> {
        self.current_work()
            .into_iter()
            .flat_map(|work| work.drafts.iter())
            .map(|(id, tab)| (id.clone(), tab))
            .collect()
    }

    /// A listed thread's saved work, from the selected machine only.
    pub(super) fn saved_thread(&self, summary: &ThreadSummary) -> Option<&Thread> {
        self.current_work()?
            .thread(&summary.id, &summary.checkout)
            .map(|saved| &saved.thread)
    }

    /// Bind the open draft, if it is not bound yet, to the selected machine.
    pub(super) fn bind_draft(&mut self) {
        if self.draft.binding.is_none() && self.draft.id.is_some() {
            self.draft.binding = Some(Binding {
                machine: self.machines.active.clone(),
            });
        }
    }

    /// Bind thread `id`'s composer as it mounts, and return its saved work.
    pub(super) fn bind_thread_composer(&mut self, id: &str) -> Option<Thread> {
        let summary = self
            .host
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == id)
            .cloned()?;
        let machine = self.machines.active.clone();
        self.workspace
            .bindings
            .entry(id.to_owned())
            .or_insert(Binding { machine });
        self.saved_thread(&summary).cloned()
    }

    /// The machine whose sends are in memory, bound on first use.
    pub(super) fn ledger_machine(&mut self) -> MachineId {
        self.saved_work
            .ledger
            .get_or_insert_with(|| self.machines.active.clone())
            .clone()
    }

    /// Load saved sends for listed threads, from the selected machine's record
    /// at the listed folder. Another machine's or another folder's sends are
    /// never loaded, so nothing reconciles or rewrites them.
    pub(super) fn adopt_saved_sends(&mut self) {
        if self.ledger_machine() != self.machines.active {
            return;
        }
        let checked = (
            self.host.connection.threads_revision(),
            self.current_work().map_or(0, |work| work.threads.len()),
        );
        if self.saved_work.adoption_checked == Some(checked) {
            return;
        }
        self.saved_work.adoption_checked = Some(checked);
        let found: Vec<(String, Thread, BTreeMap<String, HostKey>)> = self
            .host
            .connection
            .threads()
            .iter()
            .filter(|summary| !self.saved_work.adopted.contains_key(&summary.id))
            .filter_map(|summary| {
                self.current_work()?
                    .thread(&summary.id, &summary.checkout)
                    .map(|saved| {
                        (
                            summary.id.clone(),
                            saved.thread.clone(),
                            saved.accepted_by.clone(),
                        )
                    })
            })
            .collect();
        for (id, thread, accepted_by) in found {
            self.saved_work.accepted_by.extend(accepted_by);
            for pending in thread.pending.iter().chain(&thread.pending_submissions) {
                self.host.submissions.restore(id.clone(), pending.clone());
            }
            for pending in &thread.queued_submissions {
                self.host
                    .submissions
                    .restore_accepted(id.clone(), pending.clone());
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

    /// Whether host receipts may settle thread `summary`'s saved Waits: the
    /// selected machine's host is verified, the sends in memory are this
    /// machine's and this folder's, and they were accepted by that host.
    pub(super) fn may_reconcile(&self, summary: &ThreadSummary) -> bool {
        self.verified_host().is_some()
            && self.saved_work.ledger.as_ref() == Some(&self.machines.active)
            && !self.ledger_conflict(summary)
    }

    /// The accepting hosts in memory, for saving with a thread's sends.
    pub(super) fn accepting_hosts(&self) -> BTreeMap<String, HostKey> {
        self.saved_work
            .accepted_by
            .iter()
            .map(|(request, host)| (request.clone(), host.clone()))
            .collect()
    }

    /// Leaving a machine: its saved sends stay in its file, and the next
    /// machine starts a ledger of its own.
    pub(super) fn forget_saved_work(&mut self) {
        self.host.submissions = Default::default();
        self.saved_work.ledger = None;
        self.saved_work.adopted.clear();
        self.saved_work.accepted_by.clear();
        self.saved_work.adoption_checked = None;
        // Approval choices are keyed by thread ID too; they are saved in the
        // machine's file and must not follow the same ID to another machine.
        self.workspace.thread_controls.approvals.clear();
    }

    /// Why the open draft cannot be sent here, on any send path: the selected
    /// machine's host is not verified yet, or the draft is another machine's.
    pub(super) fn draft_send_refusal(&self) -> Option<String> {
        if self.verified_host().is_none() {
            return Some("Checking which machine this is…".into());
        }
        match &self.draft.binding {
            Some(binding) if binding.machine != self.machines.active => Some(
                "This draft was written for another machine. Switch back to it to send.".into(),
            ),
            _ => None,
        }
    }

    /// Store `works`, each the full new work of one machine: in memory now,
    /// and queued for a debounced background write of only those files.
    /// Returns whether any changed, and whether an image reference went away.
    pub(super) fn store_machine_work(
        &mut self,
        works: BTreeMap<MachineId, MachineWork>,
    ) -> (bool, bool) {
        let mut changed = false;
        let mut removed_image = false;
        for (machine, work) in works {
            let old = self.saved_work.machines.get(&machine);
            if old == Some(&work) {
                continue;
            }
            if let Some(old) = old {
                removed_image |= old
                    .image_ids()
                    .any(|id| !work.image_ids().any(|next| next == id));
            }
            if let Some(journal) = &self.session.journal {
                journal.queue_machine(&machine, work.clone());
            }
            self.saved_work.machines.insert(machine, work);
            changed = true;
        }
        (changed, removed_image)
    }
}

/// `machine`'s work to change in this capture, copied once from `saved`.
pub(super) fn touch<'a>(
    works: &'a mut BTreeMap<MachineId, MachineWork>,
    saved: &BTreeMap<MachineId, MachineWork>,
    machine: &MachineId,
) -> &'a mut MachineWork {
    works
        .entry(machine.clone())
        .or_insert_with(|| saved.get(machine).cloned().unwrap_or_default())
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

    /// The selected machine's saved thread work, by thread ID, for scenes.
    pub(super) fn test_saved_threads(&self) -> BTreeMap<String, Thread> {
        self.current_work()
            .into_iter()
            .flat_map(|work| &work.threads)
            .map(|saved| (saved.id.clone(), saved.thread.clone()))
            .collect()
    }

    /// The selected machine's saved drafts, by draft ID, for scenes.
    pub(super) fn test_saved_drafts(&self) -> BTreeMap<String, DraftTab> {
        self.current_work()
            .map(|work| work.drafts.clone())
            .unwrap_or_default()
    }
}
