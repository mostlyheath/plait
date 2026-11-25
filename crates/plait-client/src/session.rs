//! Desktop recovery, separate from host conversations and model credentials.
//!
//! Open and flush only on a background executor. Queueing replaces an in-memory
//! snapshot; disk writes are serialized so an older save cannot replace newer work.

mod blobs;
mod drafts;
use crate::host::ModelChoice;
use crate::host_state::PendingSubmit;
pub use drafts::{DraftSettings, DraftTab, DraftTabCreation};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const FILE_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Draft {
    pub images: Vec<crate::images::ImageRef>,
    pub text: String,
    pub revision: u64,
    pub submitted: Option<(String, u64)>,
    /// A first-message creation the host may already have made. Sending the
    /// draft again retransmits it, so one thread and worktree are made, not two.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation: Option<DraftCreation>,
}

/// The admission request a draft is waiting on, and the approvals it was sent with.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftCreation {
    pub request: crate::host::admission::Request,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<crate::host::ApprovalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<crate::host::ApprovalMode>,
    /// The request's placement was Wait; see `Placement::journaled`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wait: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Thread {
    pub checkout: PathBuf,
    pub model: ModelChoice,
    pub title: String,
    pub draft: Draft,
    pub pending: Option<PendingSubmit>,
    /// Additional outstanding requests; the first stays in `pending` for older journals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_submissions: Vec<PendingSubmit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued_submissions: Vec<PendingSubmit>,
    /// This thread's approval choice. `None` follows the desktop default in
    /// preferences. Omitted when unset, so journals stay readable by older builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<crate::host::ApprovalMode>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    /// Independent drafts. Legacy fields below remain readable by schema-1 builds.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub draft_tabs: BTreeMap<String, DraftTab>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_draft: Option<(PathBuf, Draft)>,
    /// Drafts set aside in other checkouts; the current one retains the older journal field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub new_drafts: BTreeMap<PathBuf, Draft>,
    pub schema_version: u32,
    pub host_store: PathBuf,
    pub checkout: Option<PathBuf>,
    pub selected: Option<String>,
    pub threads: BTreeMap<String, Thread>,
}

impl Session {
    pub fn saved_thread(&self, summary: &crate::host::ThreadSummary) -> Option<&Thread> {
        self.threads
            .get(&summary.id)
            .filter(|saved| saved.checkout == summary.checkout)
    }

    pub fn accepts(&self, summary: &crate::host::ThreadSummary) -> bool {
        !self.threads.contains_key(&summary.id) || self.saved_thread(summary).is_some()
    }

    pub fn update_thread(&mut self, id: String, thread: Thread) -> Result<(), String> {
        if self
            .threads
            .get(&id)
            .is_some_and(|saved| saved.checkout != thread.checkout)
        {
            return Err("Thread identity conflicts with saved work. Its draft and submission were not changed. Start a new thread or check the host store.".into());
        }
        self.threads.insert(id, thread);
        Ok(())
    }

    pub fn empty(host_store: PathBuf) -> Self {
        Self {
            schema_version: 1,
            draft_tabs: BTreeMap::new(),
            new_draft: None,
            new_drafts: BTreeMap::new(),
            host_store,
            checkout: None,
            selected: None,
            threads: BTreeMap::new(),
        }
    }

    pub fn new_draft_for(&self, checkout: &Path) -> Option<&Draft> {
        self.new_draft
            .as_ref()
            .filter(|(path, _)| path == checkout)
            .map(|(_, draft)| draft)
            .or_else(|| self.new_drafts.get(checkout))
    }
    pub fn set_new_draft(&mut self, checkout: PathBuf, draft: Draft) {
        if let Some((path, old)) = self.new_draft.take()
            && path != checkout
        {
            self.new_drafts.insert(path, old);
        }
        self.new_drafts.remove(&checkout);
        self.new_draft = Some((checkout, draft));
    }
    pub fn clear_new_draft(&mut self, checkout: &Path) {
        if self
            .new_draft
            .as_ref()
            .is_some_and(|(path, _)| path == checkout)
        {
            self.new_draft = None;
        }
        self.new_drafts.remove(checkout);
    }
    pub fn image_ids(&self) -> impl Iterator<Item = &str> {
        self.draft_tabs
            .values()
            .flat_map(|tab| {
                tab.draft.images.iter().chain(
                    tab.creation
                        .iter()
                        .flat_map(|creation| &creation.message.images),
                )
            })
            .chain(
                self.new_draft
                    .iter()
                    .flat_map(|(_, draft)| &draft.images)
                    .chain(self.new_drafts.values().flat_map(|draft| &draft.images))
                    .chain(self.threads.values().flat_map(|thread| {
                        thread.draft.images.iter().chain(
                            thread
                                .pending
                                .iter()
                                .chain(&thread.pending_submissions)
                                .flat_map(|pending| &pending.images),
                        )
                    })),
            )
            .map(|image| image.id.as_str())
    }

    fn validate(&self) -> Result<(), String> {
        self.validate_draft_tabs()?;
        if self.schema_version != 1 {
            return Err("Saved workspace uses an unsupported version. Update Plait; the file was not changed.".into());
        }
        if !self.host_store.is_absolute()
            || self
                .checkout
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            || self.threads.len() + self.new_drafts.len() > 512
        {
            return Err("Saved workspace has invalid paths or exceeds its thread limit.".into());
        }
        for (checkout, draft) in self
            .new_draft
            .iter()
            .map(|(path, draft)| (path, draft))
            .chain(self.new_drafts.iter())
        {
            if !checkout.is_absolute() || draft.text.len() > 256 * 1024 {
                return Err("Saved new-thread draft exceeds its limits.".into());
            }
            crate::images::validate_message(&draft.images)?;
        }
        for (id, thread) in &self.threads {
            crate::images::validate_message(&thread.draft.images)?;
            if !identifier(id)
                || !thread.checkout.is_absolute()
                || thread.title.len() > 1024
                || thread.draft.text.len() > 256 * 1024
                || thread
                    .draft
                    .submitted
                    .as_ref()
                    .is_some_and(|(text, _)| text.len() > crate::host::REQUEST_LIMIT)
            {
                return Err("Saved thread or draft exceeds its limits.".into());
            }
            if thread.queued_submissions.len() > 16
                || thread.pending_submissions.len() + usize::from(thread.pending.is_some()) > 16
            {
                return Err("Saved thread has too many pending submissions.".into());
            }
            let mut seen = std::collections::HashSet::new();
            for pending in thread
                .pending
                .iter()
                .chain(&thread.pending_submissions)
                .chain(&thread.queued_submissions)
            {
                crate::images::validate_message(&pending.images)?;
                let request =
                    crate::host::Request::submit_bound(id, &pending.request_id, &pending.text);
                if !seen.insert(&pending.request_id)
                    || !identifier(&pending.request_id)
                    || (pending.text.trim().is_empty() && pending.images.is_empty())
                    || serde_json::to_vec(&request)
                        .map_err(|e| e.to_string())?
                        .len()
                        + 1
                        > crate::host::REQUEST_LIMIT
                {
                    return Err("Saved submission is invalid. It was not sent.".into());
                }
            }
        }
        Ok(())
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub fn locations() -> Result<(PathBuf, PathBuf), String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("No absolute home directory is available.")?;
    let directory = match std::env::var_os("PLAIT_DESKTOP_STATE_DIR") {
        Some(path) => PathBuf::from(path),
        None => crate::preferences::default_path()
            .and_then(|path| path.parent().map(|p| p.join("session")))
            .ok_or("No desktop state directory is available.")?,
    };
    let host = std::env::var_os("PLAIT_HOST_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/plait"));
    if !directory.is_absolute() || !host.is_absolute() {
        return Err("Desktop state and host store paths must be absolute.".into());
    }
    Ok((directory, host))
}

struct Lease(File);

impl Drop for Lease {
    fn drop(&mut self) {
        // A forked child can keep the file description alive until exec. Only the
        // journal owns this lease, unlike host leases shared with command guardians.
        if let Err(error) = file_lock(&self.0, libc::LOCK_UN) {
            eprintln!("Plait workspace lock release failed: {error}");
        }
    }
}

fn file_lock(file: &File, operation: libc::c_int) -> io::Result<()> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

struct Saved {
    session: Session,
    _images: crate::images::Pins,
}
impl Saved {
    fn new(directory: &Path, session: Session) -> Arc<Self> {
        let pins = crate::images::Pins::new(directory, session.image_ids().map(str::to_owned));
        Arc::new(Self {
            session,
            _images: pins,
        })
    }
}
pub struct Journal {
    directory: PathBuf,
    _lease: Lease,
    queued: Mutex<(u64, Arc<Saved>)>,
    written: Mutex<u64>,
}

impl Journal {
    pub fn open(directory: &Path, host_store: PathBuf) -> Result<(Arc<Self>, Session), String> {
        let io = |e: std::io::Error| format!("Cannot open saved workspace: {e}");
        fs::create_dir_all(directory).map_err(io)?;
        let metadata = fs::symlink_metadata(directory).map_err(io)?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("Desktop state must be an owned directory, not a symlink.".into());
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(io)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join("desktop.lock"))
            .map_err(io)?;
        let metadata = lease.metadata().map_err(io)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err("Invalid desktop recovery lock.".into());
        }
        file_lock(&lease, libc::LOCK_EX | libc::LOCK_NB).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                "Another Plait client owns this saved workspace. Close it before sending work here."
                    .into()
            } else {
                format!("Cannot lock saved workspace: {error}")
            }
        })?;
        // Acquire the guard before reading so failed recovery also releases ownership.
        let lease = Lease(lease);
        let path = directory.join("desktop.json");
        let session = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => {
                let metadata = file.metadata().map_err(io)?;
                if !metadata.is_file()
                    || metadata.nlink() != 1
                    || metadata.uid() != unsafe { libc::geteuid() }
                    || metadata.mode() & 0o077 != 0
                    || metadata.len() > FILE_LIMIT as u64
                {
                    return Err(
                        "Saved workspace must be a private regular file within its size limit."
                            .into(),
                    );
                }
                let mut bytes = Vec::new();
                file.take(FILE_LIMIT as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(io)?;
                if bytes.len() > FILE_LIMIT {
                    return Err("Saved workspace exceeds its size limit.".into());
                }
                serde_json::from_slice::<Session>(&bytes).map_err(|e| {
                    format!("Saved workspace is invalid ({e}); the file was not changed.")
                })?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Session::empty(host_store.clone())
            }
            Err(e) => return Err(io(e)),
        };
        session.validate()?;
        if session.host_store != host_store {
            return Err("Saved workspace belongs to another host store. Use a separate PLAIT_DESKTOP_STATE_DIR; nothing was sent or overwritten.".into());
        }
        crate::images::saved(directory, session.image_ids().map(str::to_owned));
        let saved = Saved::new(directory, session.clone());
        let journal = Self {
            directory: directory.to_owned(),
            _lease: lease,
            queued: Mutex::new((0, saved)),
            written: Mutex::new(0),
        };
        crate::images::sweep(directory)?;
        Ok((Arc::new(journal), session))
    }

    /// No file access. Call after a state transition, never from render.
    pub fn queue(&self, session: Session) {
        let saved = Saved::new(&self.directory, session);
        let mut queued = self.queued.lock().unwrap();
        queued.0 += 1;
        queued.1 = saved;
    }

    /// Flush the newest queued state. Submission callers must await success first.
    pub fn flush(&self) -> Result<(), String> {
        let mut written = self.written.lock().unwrap();
        let (revision, saved) = self.queued.lock().unwrap().clone();
        if revision <= *written {
            return crate::images::sweep(&self.directory);
        }
        let session = &saved.session;
        session.validate()?;
        let bytes = session.encode()?;
        if bytes.len() > FILE_LIMIT {
            return Err("Saved workspace exceeds 4 MiB. Shorten drafts before sending; nothing was discarded.".into());
        }
        let path = self.directory.join("desktop.json");
        let temporary = self
            .directory
            .join(format!(".desktop.{}.{revision}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|e| format!("Cannot save workspace: {e}"))?;
        let result = (|| -> std::io::Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            File::open(&self.directory)?.sync_all()
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "Workspace save failed: {error}. Submission was not sent."
            ));
        }
        crate::images::saved(&self.directory, session.image_ids().map(str::to_owned));
        *written = revision;
        crate::images::sweep(&self.directory)
    }
}

pub fn title(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(72)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("plait-session-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }
    fn thread() -> Thread {
        Thread {
            checkout: "/repo".into(),
            model: ModelChoice {
                provider: "p".into(),
                model_id: "m".into(),
            },
            title: "Fix the parser".into(),
            draft: Draft {
                images: Vec::new(),
                text: "unfinished 🙂".into(),
                revision: 7,
                submitted: None,
                creation: None,
            },
            pending: Some(PendingSubmit {
                checkout_wait: false,
                images: Vec::new(),
                request_id: "request-1".into(),
                text: "Do the task".into(),
                approval: None,
                when_busy: None,
                draft_revision: None,
            }),
            pending_submissions: Vec::new(),
            queued_submissions: Vec::new(),
            approval: None,
        }
    }

    fn image_file(directory: &Path, variant: u8) -> crate::images::ImageRef {
        let mut bytes = crate::images::TEST_PNG_HEADER.to_vec();
        bytes.push(variant);
        let image = crate::images::inspect(&bytes).unwrap();
        fs::create_dir_all(directory.join("images")).unwrap();
        fs::write(directory.join("images").join(&image.id), bytes).unwrap();
        image
    }

    #[test]
    fn startup_sweeps_unreferenced_originals_and_preserves_saved_work() {
        let directory = root("image-sweep");
        let (journal, mut saved) = Journal::open(&directory, "/host".into()).unwrap();
        let kept = image_file(&directory, 0);
        let pending = image_file(&directory, 1);
        let mut thread = thread();
        thread.draft.images.push(kept.clone());
        thread
            .pending
            .as_mut()
            .unwrap()
            .images
            .push(pending.clone());
        saved.threads.insert("thread".into(), thread);
        journal.queue(saved);
        journal.flush().unwrap();
        drop(journal);
        let orphan = image_file(&directory, 2);
        let (journal, _) = Journal::open(&directory, "/host".into()).unwrap();
        assert!(directory.join("images").join(kept.id).exists());
        assert!(directory.join("images").join(pending.id).exists());
        assert!(!directory.join("images").join(orphan.id).exists());
        drop(journal);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_upload_keeps_originals_after_an_earlier_attempt_is_acknowledged() {
        let directory = root("image-upload-pin");
        let (journal, mut saved) = Journal::open(&directory, "/host".into()).unwrap();
        let image = image_file(&directory, 0);
        let mut thread = thread();
        thread.pending.as_mut().unwrap().images.push(image.clone());
        saved.threads.insert("thread".into(), thread);
        journal.queue(saved.clone());
        journal.flush().unwrap();
        let upload = crate::images::Pins::new(&directory, [image.id.clone()]);
        saved.threads.get_mut("thread").unwrap().pending = None;
        journal.queue(saved);
        journal.flush().unwrap();
        assert!(directory.join("images").join(&image.id).exists());
        drop(upload);
        crate::images::sweep(&directory).unwrap();
        assert!(!directory.join("images").join(image.id).exists());
        drop(journal);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn collection_waits_for_recovery_and_preserves_a_closed_journal() {
        let directory = root("image-recovery-guard");
        let image = image_file(&directory, 0);
        crate::images::sweep(&directory).unwrap();
        assert!(directory.join("images").join(&image.id).exists());
        let pin = crate::images::Pins::new(&directory, [image.id.clone()]);
        let (journal, mut saved) = Journal::open(&directory, "/host".into()).unwrap();
        saved.new_draft = Some((
            "/repo".into(),
            Draft {
                images: vec![image.clone()],
                ..Default::default()
            },
        ));
        journal.queue(saved);
        journal.flush().unwrap();
        drop(pin);
        drop(journal);
        crate::images::sweep(&directory).unwrap();
        assert!(directory.join("images").join(image.id).exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cleanup_preserves_preparation_and_newer_queued_references() {
        let directory = root("image-pins");
        let (journal, mut saved) = Journal::open(&directory, "/host".into()).unwrap();
        let image = image_file(&directory, 0);
        let pin = crate::images::Pins::new(&directory, [image.id.clone()]);
        journal.flush().unwrap();
        assert!(directory.join("images").join(&image.id).exists());
        saved.new_draft = Some((
            "/repo".into(),
            Draft {
                images: vec![image.clone()],
                ..Default::default()
            },
        ));
        journal.queue(saved.clone());
        drop(pin);
        crate::images::sweep(&directory).unwrap();
        assert!(directory.join("images").join(&image.id).exists());
        journal.flush().unwrap();
        saved.new_draft = None;
        journal.queue(saved);
        // The durable draft keeps its bytes until the replacement is saved.
        crate::images::sweep(&directory).unwrap();
        assert!(directory.join("images").join(&image.id).exists());
        journal.flush().unwrap();
        assert!(!directory.join("images").join(image.id).exists());
        drop(journal);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn thread_approval_is_optional_and_round_trips() {
        let plain = serde_json::to_string(&thread()).unwrap();
        // An unset choice is not written, so the record matches older journals.
        let fields: serde_json::Value = serde_json::from_str(&plain).unwrap();
        assert!(fields.get("approval").is_none(), "{plain}");
        assert!(
            serde_json::from_str::<Thread>(&plain)
                .unwrap()
                .approval
                .is_none()
        );
        let chosen = Thread {
            approval: Some(crate::host::ApprovalMode::Ask),
            ..thread()
        };
        let written = serde_json::to_string(&chosen).unwrap();
        assert_eq!(serde_json::from_str::<Thread>(&written).unwrap(), chosen);
    }

    #[test]
    fn capture_cannot_reassign_a_draft_or_pending_request_to_a_new_identity() {
        let mut saved = Session::empty("/host".into());
        let original = thread();
        saved
            .update_thread("thread".into(), original.clone())
            .unwrap();
        let mut changed = original.clone();
        changed.checkout = "/another-checkout".into();
        let summary = crate::host::ThreadSummary {
            id: "thread".into(),
            checkout: changed.checkout.clone(),
            model: changed.model.clone(),
            ..Default::default()
        };
        assert!(!saved.accepts(&summary));
        assert!(saved.saved_thread(&summary).is_none());
        assert!(saved.update_thread("thread".into(), changed).is_err());
        assert_eq!(saved.threads["thread"], original);
        let mut edited = original;
        edited.draft.text = "A later draft".into();
        saved
            .update_thread("thread".into(), edited.clone())
            .unwrap();
        assert_eq!(saved.threads["thread"], edited);
    }

    #[test]
    fn shared_lock_permissions_are_rejected_without_replacement() {
        let root = root("shared-lock");
        fs::create_dir_all(&root).unwrap();
        let lock = root.join("desktop.lock");
        fs::write(&lock, b"existing").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Journal::open(&root, "/host".into()).is_err());
        assert_eq!(fs::read(&lock).unwrap(), b"existing");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicated_descriptor_does_not_extend_journal_ownership() {
        let root = root("inherited-descriptor");
        let (journal, mut state) = Journal::open(&root, "/host".into()).unwrap();
        state.threads.insert("thread-1".into(), thread());
        journal.queue(state.clone());
        journal.flush().unwrap();
        let owner = journal.clone();
        // dup and fork share a flock's open file description, even with close-on-exec.
        let inherited = journal._lease.0.try_clone().unwrap();
        assert_ne!(
            unsafe { libc::fcntl(inherited.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(journal);
        assert!(Journal::open(&root, "/host".into()).is_err());
        drop(owner);
        let (reopened, saved) = Journal::open(&root, "/host".into()).unwrap();
        assert_eq!(saved, state);
        drop(inherited);
        assert!(Journal::open(&root, "/host".into()).is_err());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fork_before_exec_does_not_delay_journal_reopen() {
        use std::{os::unix::net::UnixStream, os::unix::process::CommandExt, process::Command};
        let root = root("fork-reopen");
        let (journal, _) = Journal::open(&root, "/host".into()).unwrap();
        let (mut parent, child) = UnixStream::pair().unwrap();
        let timeout = Some(std::time::Duration::from_secs(10));
        parent.set_read_timeout(timeout).unwrap();
        child.set_read_timeout(timeout).unwrap();
        let spawner = std::thread::spawn(move || {
            let fd = child.as_raw_fd();
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "exit 0"]);
            // Only async-signal-safe I/O runs between fork and exec. The handshake
            // holds the inherited descriptor without timing sleeps or a live child leak.
            unsafe {
                command.pre_exec(move || {
                    let mut byte = 0_u8;
                    if libc::write(fd, (&byte as *const u8).cast(), 1) != 1
                        || libc::read(fd, (&mut byte as *mut u8).cast(), 1) != 1
                    {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                });
            }
            command.spawn().unwrap().wait().unwrap()
        });
        parent.read_exact(&mut [0]).unwrap();
        drop(journal);
        let reopened = Journal::open(&root, "/host".into());
        let released = parent.write_all(&[1]);
        let status = spawner.join().unwrap();
        released.unwrap();
        assert!(status.success());
        drop(reopened.unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_retains_drafts_pending_ids_and_model_identity() {
        let root = root("roundtrip");
        let (journal, mut state) = Journal::open(&root, "/host".into()).unwrap();
        state.checkout = Some("/repo".into());
        state.selected = Some("thread-1".into());
        state.threads.insert("thread-1".into(), thread());
        journal.queue(state.clone());
        journal.flush().unwrap();
        drop(journal);
        let (_, saved) = Journal::open(&root, "/host".into()).unwrap();
        assert_eq!(saved, state);
        assert_eq!(
            fs::metadata(root.join("desktop.json")).unwrap().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_updates_coalesce_and_old_flushes_cannot_regress_state() {
        let root = root("ordering");
        let (journal, mut state) = Journal::open(&root, "/host".into()).unwrap();
        for i in 0..50 {
            state.selected = Some(format!("thread-{i}"));
            journal.queue(state.clone());
        }
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let journal = journal.clone();
                std::thread::spawn(move || journal.flush().unwrap())
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        drop(journal);
        assert_eq!(
            Journal::open(&root, "/host".into()).unwrap().1.selected,
            Some("thread-49".into())
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn second_client_and_different_host_cannot_overwrite_saved_work() {
        let root = root("ownership");
        let (journal, state) = Journal::open(&root, "/host".into()).unwrap();
        journal.queue(state);
        journal.flush().unwrap();
        assert!(Journal::open(&root, "/host".into()).is_err());
        drop(journal);
        assert!(Journal::open(&root, "/other-host".into()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_future_and_oversized_files_are_not_reset() {
        let root = root("invalid");
        let (journal, state) = Journal::open(&root, "/host".into()).unwrap();
        journal.queue(state.clone());
        journal.flush().unwrap();
        drop(journal);
        let path = root.join("desktop.json");
        let mut future = state;
        future.schema_version = 2;
        for bytes in [
            b"not json".to_vec(),
            serde_json::to_vec(&future).unwrap(),
            vec![b' '; FILE_LIMIT + 1],
        ] {
            fs::write(&path, &bytes).unwrap();
            assert!(Journal::open(&root, "/host".into()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_and_invalid_submissions_fail_without_replacing_state() {
        let root = root("links");
        let (journal, mut state) = Journal::open(&root, "/host".into()).unwrap();
        journal.queue(state.clone());
        journal.flush().unwrap();
        let before = fs::read(root.join("desktop.json")).unwrap();
        let mut thread = thread();
        thread.pending.as_mut().unwrap().request_id = "bad\nrequest".into();
        state.threads.insert("thread-1".into(), thread);
        journal.queue(state);
        assert!(journal.flush().is_err());
        assert_eq!(fs::read(root.join("desktop.json")).unwrap(), before);
        drop(journal);
        fs::remove_file(root.join("desktop.json")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("desktop.json")).unwrap();
        assert!(Journal::open(&root, "/host".into()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn titles_are_plain_whitespace_normalized_and_unicode_safe() {
        assert_eq!(title("  Fix\n the\tparser 🙂  "), "Fix the parser 🙂");
        assert_eq!(title(&"🙂".repeat(100)).chars().count(), 72);
    }
    #[test]
    fn model_changes_keep_saved_work_and_old_journals_load() {
        let mut state = Session::empty("/host".into());
        let mut saved = thread();
        state.update_thread("thread".into(), saved.clone()).unwrap();
        saved.model.model_id = "changed".into();
        let summary = crate::host::ThreadSummary {
            id: "thread".into(),
            checkout: saved.checkout.clone(),
            model: saved.model.clone(),
            ..Default::default()
        };
        assert!(state.saved_thread(&summary).is_some());
        state.update_thread("thread".into(), saved).unwrap();
        let mut old = serde_json::to_value(&state).unwrap();
        let pending = old["threads"]["thread"]["pending"].as_object_mut().unwrap();
        pending.remove("approval");
        pending.remove("when_busy");
        pending.remove("draft_revision");
        let loaded: Session = serde_json::from_value(old).unwrap();
        loaded.validate().unwrap();
        assert!(loaded.threads["thread"].pending_submissions.is_empty());
        assert!(
            loaded.threads["thread"]
                .pending
                .as_ref()
                .unwrap()
                .when_busy
                .is_none()
        );
    }

    #[test]
    fn journal_preserves_multiple_pending_and_accepted_queue_without_sending() {
        let directory = root("multiple-pending");
        let (journal, mut state) = Journal::open(&directory, "/host".into()).unwrap();
        let mut saved = thread();
        let mut second = saved.pending.clone().unwrap();
        second.request_id = "request-2".into();
        second.when_busy = Some(crate::host::QueueMode::Steer);
        second.approval = Some(crate::host::ApprovalMode::Auto);
        second.draft_revision = Some(8);
        saved.pending_submissions.push(second.clone());
        let mut queued = second;
        queued.request_id = "request-3".into();
        saved.queued_submissions.push(queued);
        state.update_thread("thread".into(), saved).unwrap();
        journal.queue(state.clone());
        journal.flush().unwrap();
        drop(journal);
        let (_, restored) = Journal::open(&directory, "/host".into()).unwrap();
        assert_eq!(restored, state);
        let mut duplicate = restored.clone();
        let saved = duplicate.threads.get_mut("thread").unwrap();
        saved
            .pending_submissions
            .push(saved.pending.clone().unwrap());
        assert!(duplicate.validate().is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
