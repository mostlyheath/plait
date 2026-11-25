//! Saved work per machine entry (#132). The machine a person selects in Plait
//! owns what they type while it is selected: unsent drafts, and thread work
//! keyed by thread ID and folder, with its waiting messages and approval choice.
//! Each machine's work is its own file under `desktop-machines/`, beside
//! `desktop.json`. Older builds never read or write these files.
use super::{DraftTab, Thread, drafts::validate_draft_tab, validate_thread};
use crate::{machines::MachineId, tabs::HostKey};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// One machine's file. Generous: drafts are small, and a full file is kept in
/// memory with a notice rather than refusing what a person types.
pub const FILE_LIMIT: usize = 8 * 1024 * 1024;
const DIRECTORY: &str = "desktop-machines";
const SCHEMA: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineWork {
    /// Unsent drafts, by draft ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drafts: BTreeMap<String, DraftTab>,
    /// Thread work, keyed by thread ID and folder: the same ID at another
    /// folder is another thread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<SavedThread>,
    /// `desktop.json`'s older records already copied in (this computer only),
    /// as `draft:<id>` and `thread:<id>`: each is copied once, so a discarded
    /// copy does not come back, and one an older build adds later still does.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub imported: std::collections::BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedThread {
    pub id: String,
    pub thread: Thread,
    /// The host that accepted each saved send, by request ID, recorded once
    /// when it was accepted and never changed. Receipts from any other host,
    /// such as one reinstalled under this machine entry, never settle it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub accepted_by: BTreeMap<String, HostKey>,
}

#[derive(Serialize, Deserialize)]
struct Document {
    schema_version: u32,
    machine: MachineId,
    work: MachineWork,
}

/// A draft with nothing to keep: no text, images or unanswered first send.
pub fn draft_is_empty(tab: &DraftTab) -> bool {
    tab.creation.is_none()
        && tab.draft.creation.is_none()
        && tab.draft.text.is_empty()
        && tab.draft.images.is_empty()
}

impl MachineWork {
    pub fn thread(&self, id: &str, checkout: &Path) -> Option<&SavedThread> {
        self.threads
            .iter()
            .find(|saved| saved.id == id && saved.thread.checkout == checkout)
    }

    /// Save thread work; the same ID at another folder is a separate record.
    /// Each send keeps the host recorded as accepting it; `accepted` adds
    /// hosts only for sends that have none yet, and sends that are gone drop theirs.
    pub fn put_thread(&mut self, id: &str, thread: Thread, accepted: &BTreeMap<String, HostKey>) {
        let index = match self
            .threads
            .iter()
            .position(|saved| saved.id == id && saved.thread.checkout == thread.checkout)
        {
            Some(index) => {
                self.threads[index].thread = thread;
                index
            }
            None => {
                self.threads.push(SavedThread {
                    id: id.to_owned(),
                    thread,
                    accepted_by: BTreeMap::new(),
                });
                self.threads.len() - 1
            }
        };
        let saved = &mut self.threads[index];
        let sends: std::collections::HashSet<&str> = saved
            .thread
            .pending
            .iter()
            .chain(&saved.thread.pending_submissions)
            .chain(&saved.thread.queued_submissions)
            .map(|pending| pending.request_id.as_str())
            .collect();
        saved
            .accepted_by
            .retain(|request, _| sends.contains(request.as_str()));
        for (request, host) in accepted {
            if sends.contains(request.as_str()) {
                saved
                    .accepted_by
                    .entry(request.clone())
                    .or_insert_with(|| host.clone());
            }
        }
    }

    /// Empty-draft cleanup: removes draft `id` only if its stored record is empty.
    pub fn discard_empty_draft(&mut self, id: &str) -> bool {
        let empty = self.drafts.get(id).is_some_and(draft_is_empty);
        if empty {
            self.drafts.remove(id);
        }
        empty
    }

    /// Copy `desktop.json`'s older records in, each once. A record is copied
    /// only if this machine has none of its own for it; the originals stay
    /// for older builds. Returns whether anything was copied.
    pub fn import_legacy(
        &mut self,
        drafts: &BTreeMap<String, DraftTab>,
        threads: &BTreeMap<String, Thread>,
    ) -> bool {
        let mut changed = false;
        for (id, tab) in drafts {
            if self.imported.insert(format!("draft:{id}")) {
                self.drafts.entry(id.clone()).or_insert_with(|| tab.clone());
                changed = true;
            }
        }
        for (id, thread) in threads {
            if self.imported.insert(format!("thread:{id}")) {
                if self.thread(id, &thread.checkout).is_none() {
                    self.put_thread(id, thread.clone(), &BTreeMap::new());
                }
                changed = true;
            }
        }
        changed
    }

    pub fn image_ids(&self) -> impl Iterator<Item = &str> {
        self.drafts
            .values()
            .flat_map(|tab| {
                tab.draft.images.iter().chain(
                    tab.creation
                        .iter()
                        .flat_map(|creation| &creation.message.images),
                )
            })
            .chain(self.threads.iter().flat_map(|saved| {
                saved.thread.draft.images.iter().chain(
                    saved
                        .thread
                        .pending
                        .iter()
                        .chain(&saved.thread.pending_submissions)
                        .flat_map(|pending| &pending.images),
                )
            }))
            .map(|image| image.id.as_str())
    }

    pub fn validate(&self) -> Result<(), String> {
        for (id, tab) in &self.drafts {
            validate_draft_tab(id, tab, FILE_LIMIT)?;
        }
        let mut keys = std::collections::HashSet::new();
        for saved in &self.threads {
            if !keys.insert((&saved.id, &saved.thread.checkout)) {
                return Err("A machine's saved threads repeat. Nothing was discarded.".into());
            }
            validate_thread(&saved.id, &saved.thread, FILE_LIMIT)?;
        }
        Ok(())
    }
}

fn directory(root: &Path) -> PathBuf {
    root.join(DIRECTORY)
}

fn path(root: &Path, machine: &MachineId) -> PathBuf {
    // Machine IDs are ASCII letters, digits, `-` and `_`: safe as a file name.
    directory(root).join(format!("{}.json", machine.as_str()))
}

/// Every machine's saved work. Blocking; call on a background executor.
pub fn read_all(root: &Path) -> Result<BTreeMap<MachineId, MachineWork>, String> {
    let directory = directory(root);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("Cannot read saved machine work: {error}")),
    };
    let mut all = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("Cannot read saved machine work: {e}"))?;
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        let Ok(machine) = MachineId::new(stem) else {
            continue;
        };
        let work = read(&entry.path(), &machine)?;
        all.insert(machine, work);
    }
    Ok(all)
}

fn read(path: &Path, machine: &MachineId) -> Result<MachineWork, String> {
    let io = |e: std::io::Error| format!("Cannot read saved work for {}: {e}", machine.as_str());
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(io)?;
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > FILE_LIMIT as u64
    {
        return Err(format!(
            "Saved work for {} must be a private file within its size limit; it was not changed.",
            machine.as_str()
        ));
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    let document: Document = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "Saved work for {} is invalid ({e}); it was not changed.",
            machine.as_str()
        )
    })?;
    if document.schema_version != SCHEMA || &document.machine != machine {
        return Err(format!(
            "Saved work for {} is from another version or machine; it was not changed.",
            machine.as_str()
        ));
    }
    document.work.validate()?;
    Ok(document.work)
}

/// A copy of each image a machine file refers to, outside `images/`. An older
/// build's sweep sees only `desktop.json` and may remove the image there; the
/// copy here brings it back on the next launch of this build.
fn backups(root: &Path) -> PathBuf {
    directory(root).join("images")
}

fn image_path(root: &Path, id: &str) -> PathBuf {
    root.join("images").join(id)
}

fn link_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Ok(());
    }
    fs::hard_link(from, to).or_else(|_| fs::copy(from, to).map(|_| ()))
}

/// Keep a backup of each image `work` refers to. Blocking.
pub(super) fn back_up_images(root: &Path, work: &MachineWork) -> Result<(), String> {
    let backups = backups(root);
    fs::create_dir_all(&backups).map_err(|e| format!("Cannot keep draft images: {e}"))?;
    for id in work.image_ids() {
        let image = image_path(root, id);
        if image.exists() {
            link_or_copy(&image, &backups.join(id))
                .map_err(|e| format!("Cannot keep draft images: {e}"))?;
        }
    }
    Ok(())
}

/// Bring back any image a machine file refers to that is missing from
/// `images/`, as after an older build swept it. Blocking.
pub(super) fn restore_images(root: &Path, machines: &BTreeMap<MachineId, MachineWork>) {
    let backups = backups(root);
    for id in machines.values().flat_map(|work| work.image_ids()) {
        let image = image_path(root, id);
        let backup = backups.join(id);
        if !image.exists() && backup.exists() {
            let _ = fs::create_dir_all(root.join("images"));
            let _ = link_or_copy(&backup, &image);
        }
    }
}

/// Remove backups no durable machine file refers to any more. Blocking.
pub(super) fn sweep_backups(root: &Path, referenced: &std::collections::HashSet<String>) {
    let Ok(entries) = fs::read_dir(backups(root)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|id| !referenced.contains(id))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Where work that could not be saved when Plait had to quit is kept.
pub fn recovery_directory(root: &Path) -> PathBuf {
    directory(root).join("recovery")
}

/// Recovery files left by a quit that could not save, if any.
pub fn recovery_files(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(recovery_directory(root))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default()
}

/// Keep one machine's unsaved work in its own new recovery file, with no
/// size bound: used only when a quit cannot be cancelled and the normal save
/// failed, so the work is never simply dropped. Blocking.
pub(super) fn write_recovery(
    root: &Path,
    machine: &MachineId,
    work: &MachineWork,
) -> Result<PathBuf, String> {
    let bytes = encode(machine, work)?;
    let directory = recovery_directory(root);
    fs::create_dir_all(&directory).map_err(|e| format!("Cannot keep unsaved work: {e}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = directory.join(format!("{}-{stamp}.json", machine.as_str()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| format!("Cannot keep unsaved work: {e}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| File::open(&directory)?.sync_all())
        .map_err(|e| format!("Cannot keep unsaved work: {e}"))?;
    Ok(path)
}

fn encode(machine: &MachineId, work: &MachineWork) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Document {
        schema_version: SCHEMA,
        machine: machine.clone(),
        work: work.clone(),
    })
    .map_err(|e| format!("Cannot encode saved machine work: {e}"))
}

/// Replace one machine's file atomically. Blocking; the journal serializes it.
pub(super) fn write(root: &Path, machine: &MachineId, work: &MachineWork) -> Result<(), String> {
    work.validate()?;
    let bytes = encode(machine, work)?;
    if bytes.len() > FILE_LIMIT {
        return Err(format!(
            "Saved work for this machine exceeds {} MiB. It is kept while Plait stays open; shorten or send drafts.",
            FILE_LIMIT / (1024 * 1024)
        ));
    }
    let directory = directory(root);
    fs::create_dir_all(&directory).map_err(|e| format!("Cannot save machine work: {e}"))?;
    let metadata = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Saved machine work needs an owned directory.".into());
    }
    let target = path(root, machine);
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = directory.join(format!(
        ".{}.{}.{}.tmp",
        machine.as_str(),
        std::process::id(),
        SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &target)?;
        File::open(&directory)?.sync_all()
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Saving machine work failed: {error}. Nothing was discarded."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host::ModelChoice,
        host_state::PendingSubmit,
        session::{Draft, DraftSettings, Journal, Session},
    };
    use std::os::unix::fs::PermissionsExt;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("plait-machines-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
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
            queued_submissions: waits
                .iter()
                .map(|id| PendingSubmit {
                    checkout_wait: true,
                    images: Vec::new(),
                    request_id: (*id).into(),
                    text: "Wait".into(),
                    approval: None,
                    when_busy: None,
                    draft_revision: None,
                })
                .collect(),
            approval: Some(crate::host::ApprovalMode::Auto),
        }
    }

    fn remote() -> MachineId {
        MachineId::new("workstation").unwrap()
    }

    fn huge(work: &MachineWork) -> MachineWork {
        let mut huge = work.clone();
        for n in 0..40 {
            huge.drafts
                .insert(format!("big-{n}"), tab(&"x".repeat(250 * 1024)));
        }
        huge
    }

    /// The same thread ID at another folder is another record; the host that
    /// accepted a send is recorded once and never replaced.
    #[test]
    fn threads_are_keyed_by_id_and_folder() {
        let mut work = MachineWork::default();
        let host = |id: &str| HostKey {
            id: id.into(),
            store: "/s".into(),
        };
        let accepted = |id: &str| [("w1".to_owned(), host(id))].into_iter().collect();
        work.put_thread("t", thread("/repo", "here", &["w1"]), &accepted("first"));
        work.put_thread("t", thread("/elsewhere", "there", &[]), &BTreeMap::new());
        assert_eq!(work.threads.len(), 2);
        work.put_thread(
            "t",
            thread("/repo", "edited", &["w1"]),
            &accepted("replacement"),
        );
        let saved = work.thread("t", Path::new("/repo")).unwrap();
        assert_eq!(saved.thread.draft.text, "edited");
        assert_eq!(saved.accepted_by["w1"], host("first"));
        // A settled send drops its host.
        work.put_thread("t", thread("/repo", "edited", &[]), &BTreeMap::new());
        assert!(
            work.thread("t", Path::new("/repo"))
                .unwrap()
                .accepted_by
                .is_empty()
        );
        work.validate().unwrap();
    }

    /// Empty-draft cleanup checks the stored record.
    #[test]
    fn empty_cleanup_checks_the_stored_record() {
        let mut work = MachineWork::default();
        work.drafts.insert("full".into(), tab("keep me"));
        work.drafts.insert("empty".into(), tab(""));
        assert!(!work.discard_empty_draft("full"));
        assert!(work.discard_empty_draft("empty"));
        assert_eq!(work.drafts["full"].draft.text, "keep me");
    }

    /// Each machine's file round-trips, and a file over its limit is refused
    /// without touching the one on disk.
    #[test]
    fn machine_files_round_trip_and_refuse_oversize() {
        let root = root("files");
        let mut work = MachineWork::default();
        work.drafts.insert("d".into(), tab("saved"));
        work.put_thread("t", thread("/repo", "text", &["w1"]), &BTreeMap::new());
        write(&root, &remote(), &work).unwrap();
        assert_eq!(read_all(&root).unwrap()[&remote()], work);
        assert!(
            write(&root, &remote(), &huge(&work))
                .unwrap_err()
                .contains("exceeds")
        );
        assert_eq!(read_all(&root).unwrap()[&remote()], work);
        fs::remove_dir_all(root).unwrap();
    }

    /// The journal writes only changed machines' files, reports a failed one,
    /// and loads the last durable file again on reopen.
    #[test]
    fn the_journal_writes_each_machine_file_and_reloads_it() {
        let root = root("journal");
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        let mut work = MachineWork::default();
        work.drafts.insert("d".into(), tab("typed on the remote"));
        journal.queue_machine(&remote(), work.clone());
        journal.flush().unwrap();
        journal.queue_machine(&remote(), huge(&work));
        assert!(journal.flush().is_err());
        drop(journal);
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        assert_eq!(journal.take_machines()[&remote()], work);
        fs::remove_dir_all(root).unwrap();
    }

    /// A quit that cannot be cancelled keeps work the normal save refused in a
    /// recovery file, whole, beside the journal.
    #[test]
    fn unsaved_work_is_kept_in_a_recovery_file() {
        let root = root("recovery");
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        let work = huge(&MachineWork::default());
        journal.queue_machine(&remote(), work.clone());
        assert!(journal.flush().is_err());
        let kept = journal.keep_unsaved().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(journal.recovery_files(), kept);
        let document: Document = serde_json::from_slice(&fs::read(&kept[0]).unwrap()).unwrap();
        assert_eq!(document.machine, remote());
        assert_eq!(document.work, work);
        fs::remove_dir_all(root).unwrap();
    }

    /// `desktop.json`'s older records are copied once into this computer's
    /// file when the journal opens, and stay in place for older builds.
    #[test]
    fn older_records_are_copied_once_and_left_in_place() {
        let root = root("legacy");
        let (journal, mut session) = Journal::open(&root, "/host".into()).unwrap();
        journal.take_machines();
        session
            .draft_tabs
            .insert("old".into(), tab("older build's draft"));
        session
            .threads
            .insert("t".into(), thread("/repo", "older text", &["w1"]));
        journal.queue(session.clone());
        journal.flush().unwrap();
        drop(journal);
        let (journal, reopened) = Journal::open(&root, "/host".into()).unwrap();
        let mut local = journal
            .take_machines()
            .remove(&MachineId::default())
            .unwrap();
        assert!(local.imported.contains("draft:old"));
        assert_eq!(local.drafts["old"].draft.text, "older build's draft");
        assert!(local.thread("t", Path::new("/repo")).is_some());
        assert_eq!(reopened.draft_tabs, session.draft_tabs);
        // Copied once: a discard here is not undone by a reopen.
        local.drafts.remove("old");
        journal.queue_machine(&MachineId::default(), local);
        journal.flush().unwrap();
        drop(journal);
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        assert!(
            !journal.take_machines()[&MachineId::default()]
                .drafts
                .contains_key("old")
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// An older build's draft kept only in `new_draft` or `new_drafts` is
    /// copied on the first reopen; `desktop.json` is left exactly as it was.
    #[test]
    fn a_legacy_only_new_draft_is_copied_on_the_first_reopen() {
        let root = root("legacy-only");
        fs::create_dir_all(&root).unwrap();
        let older = serde_json::json!({
            "schema_version": 1, "host_store": "/host", "checkout": null, "selected": null,
            "threads": {},
            "new_draft": ["/project", {"text": "only in new_draft", "revision": 2}],
            "new_drafts": {"/other": {"text": "only in new_drafts", "revision": 1}}
        });
        let bytes = serde_json::to_vec(&older).unwrap();
        let path = root.join("desktop.json");
        fs::write(&path, &bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let (journal, session) = Journal::open(&root, "/host".into()).unwrap();
        let local = journal
            .take_machines()
            .remove(&MachineId::default())
            .unwrap();
        let mut texts: Vec<_> = local
            .drafts
            .values()
            .map(|tab| tab.draft.text.clone())
            .collect();
        texts.sort();
        assert_eq!(texts, ["only in new_draft", "only in new_drafts"]);
        assert!(session.draft_tabs.is_empty());
        journal.flush().unwrap();
        drop(journal);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        assert_eq!(
            journal.take_machines()[&MachineId::default()].drafts.len(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// After a rollback, an older build's sweep removes images only machine
    /// files refer to; the next launch of this build brings them back.
    #[test]
    fn images_survive_a_rollback_sweep() {
        let root = root("rollback-images");
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        let mut bytes = crate::images::TEST_PNG_HEADER.to_vec();
        bytes.push(7);
        let image = crate::images::inspect(&bytes).unwrap();
        fs::create_dir_all(root.join("images")).unwrap();
        fs::write(root.join("images").join(&image.id), &bytes).unwrap();
        let mut work = MachineWork::default();
        let mut draft = tab("with an image");
        draft.draft.images.push(image.clone());
        work.drafts.insert("d".into(), draft);
        journal.queue_machine(&remote(), work);
        journal.flush().unwrap();
        drop(journal);
        // An older build's startup sweep: desktop.json names no image.
        fs::remove_file(root.join("images").join(&image.id)).unwrap();
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        assert_eq!(
            fs::read(root.join("images").join(&image.id)).unwrap(),
            bytes
        );
        assert_eq!(
            journal.take_machines()[&remote()].drafts["d"].draft.images,
            [image]
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The 256 KiB draft limit is desktop.json's; a machine file takes a
    /// longer draft, bounded by its file size.
    #[test]
    fn machine_files_take_drafts_past_the_legacy_limit() {
        let root = root("long-draft");
        let mut work = MachineWork::default();
        work.drafts
            .insert("long".into(), tab(&"x".repeat(1024 * 1024)));
        write(&root, &remote(), &work).unwrap();
        assert_eq!(read_all(&root).unwrap()[&remote()], work);
        let mut legacy = Session::empty("/host".into());
        legacy
            .draft_tabs
            .insert("long".into(), tab(&"x".repeat(1024 * 1024)));
        assert!(legacy.validate().is_err());
        fs::remove_dir_all(root).unwrap();
    }

    /// Older builds read only `desktop.json`, which keeps its old fields; this
    /// build reads a `desktop.json` an older build wrote.
    #[test]
    fn desktop_json_keeps_its_old_form() {
        let value = serde_json::to_value(Session::empty("/host".into())).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "checkout",
                "host_store",
                "schema_version",
                "selected",
                "threads"
            ]
        );
        let older = serde_json::json!({
            "schema_version": 1, "host_store": "/host", "checkout": null,
            "selected": null, "threads": {}
        });
        let loaded: Session = serde_json::from_value(older).unwrap();
        assert!(loaded.draft_tabs.is_empty());
    }
}
