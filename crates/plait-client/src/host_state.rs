use crate::ownership::HostKeyExt;
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    checkout_state::CheckoutCreationState,
    host::{ApprovalMode, ModelChoice, QueueMode, Request, Snapshot, Status, ThreadSummary},
    ownership::{CheckoutKey, HostKey, ReadStamp, ThreadKey},
    thread_view::SelectedThreadState,
};
use std::{path::PathBuf, sync::Arc};

/// Host connection, cross-checkout thread catalog, and default-model settings.
#[derive(Default)]
pub struct HostConnection {
    pub epoch: u64,
    pub connected: bool,
    pub has_checkout_identity: bool,
    pub connecting: bool,
    pub error: Option<String>,
    pub status: Option<Status>,
    pub catalog_revision: u64,
    /// The providers revision the host's workspace last reported. It changes when the host
    /// publishes new providers, such as keys read from the login shell after it started.
    pub providers_revision: u64,
    /// The providers revision the model catalog was loaded at, and a reload in flight.
    catalog_providers_revision: u64,
    catalog_refresh: Option<u64>,
    threads: Vec<ThreadSummary>,
    threads_revision: u64,
    busy_checkouts: std::sync::OnceLock<HashSet<PathBuf>>,
    pub settings_model: Option<ModelChoice>,
    pub saving: bool,
    pub save_message: Option<String>,
}

impl HostConnection {
    /// Starts a catalog reload when the catalog predates the host's providers, unless one is in
    /// flight. Returns the providers revision the reload will be current for.
    pub fn begin_catalog_refresh(&mut self) -> Option<u64> {
        let stale = self.providers_revision != self.catalog_providers_revision;
        if !self.connected || !stale || self.catalog_refresh.is_some() {
            return None;
        }
        self.catalog_refresh = Some(self.providers_revision);
        self.catalog_refresh
    }

    /// Ends the reload `begin_catalog_refresh` started; a loaded catalog is current for `at`.
    pub fn finish_catalog_refresh(&mut self, at: u64, loaded: bool) {
        if self.catalog_refresh != Some(at) {
            return;
        }
        self.catalog_refresh = None;
        if loaded {
            self.catalog_providers_revision = at;
        }
    }

    pub fn threads(&self) -> &[ThreadSummary] {
        &self.threads
    }
    pub fn checkout_has_run(&self, path: &std::path::Path) -> bool {
        self.busy_checkouts
            .get_or_init(|| {
                self.threads
                    .iter()
                    .filter(|t| t.status.busy())
                    .map(|t| t.checkout.clone())
                    .collect()
            })
            .contains(path)
    }
    pub fn threads_mut(&mut self) -> &mut Vec<ThreadSummary> {
        self.busy_checkouts.take();
        self.threads_revision = self
            .threads_revision
            .checked_add(1)
            .expect("thread revision exhausted");
        &mut self.threads
    }
    pub fn set_threads(&mut self, threads: Vec<ThreadSummary>) {
        *self.threads_mut() = threads;
    }
    pub fn threads_revision(&self) -> u64 {
        self.threads_revision
    }

    pub fn checkout_identity(
        &self,
        key: Option<&CheckoutKey>,
    ) -> Option<crate::host::git::WireIdentity> {
        key.filter(|_| self.has_checkout_identity)
            .map(|key| crate::host::git::WireIdentity {
                dev: key.device.to_string(),
                ino: key.inode.to_string(),
            })
    }

    /// The negotiated protocol; version 1 until a status reply says otherwise.
    pub fn protocol(&self) -> u32 {
        self.status
            .as_ref()
            .map_or(1, |status| status.protocol_version)
    }

    pub(crate) fn model_available(&self, choice: Option<&ModelChoice>, status: &Status) -> bool {
        choice.is_some_and(|choice| {
            status
                .models
                .iter()
                .any(|model| model.authenticated && model.choice() == *choice)
        })
    }

    pub fn available(&self, choice: Option<&ModelChoice>) -> bool {
        self.connected
            && self
                .status
                .as_ref()
                .is_some_and(|status| self.model_available(choice, status))
    }

    pub fn finish_save(&mut self, result: Result<ModelChoice, String>) {
        self.saving = false;
        match result {
            Ok(choice) => {
                if let Some(status) = &mut self.status {
                    status.default_model = Some(choice.clone());
                }
                self.settings_model = Some(choice);
                self.save_message =
                    Some("Default model saved. Existing threads keep their model.".into());
            }
            Err(error) => self.save_message = Some(format!("Save not confirmed: {error}")),
        }
    }
}

pub struct HostState {
    pub identity: Option<HostKey>,
    expected_checkouts: HashMap<ThreadKey, CheckoutKey>,
    /// Host connection, cross-checkout thread catalog, and default-model settings.
    pub connection: HostConnection,
    /// Active checkout path and per-checkout thread-creation state.
    pub creation: CheckoutCreationState,
    /// Read and control state for the single selected thread slot.
    pub thread: SelectedThreadState,
    /// Per-thread submission ledger, independent of which thread is selected.
    pub submissions: Submissions,
    pub undo: crate::operations::UndoLedger,
}

fn live_preview_clipped(snapshot: &Snapshot) -> bool {
    snapshot.live_text.encode_utf16().take(16_383).count() >= 16_383
        || snapshot.live_reasoning.encode_utf16().take(16_383).count() >= 16_383
}

impl HostState {
    pub fn new(checkout: PathBuf) -> Self {
        Self {
            identity: HostKey::local(),
            expected_checkouts: HashMap::new(),
            connection: HostConnection::default(),
            creation: CheckoutCreationState::new(checkout),
            thread: SelectedThreadState::default(),
            submissions: Submissions::default(),
            undo: Default::default(),
        }
    }

    pub fn thread_key(&self, id: &str) -> ThreadKey {
        ThreadKey {
            host: self.identity.clone(),
            id: id.to_owned(),
        }
    }

    pub fn read_stamp(&self) -> ReadStamp {
        ReadStamp {
            connection: self.connection.epoch,
            selection: self.thread.watch_epoch,
            checkout: self.creation.generation,
            thread: self.thread.selected.as_ref().map(|id| self.thread_key(id)),
        }
    }

    pub fn accepts_read(&self, stamp: &ReadStamp) -> bool {
        self.read_stamp() == *stamp
    }

    pub fn bind_checkout(&mut self, id: &str, key: &CheckoutKey) -> Result<(), String> {
        let owner = self.thread_key(id);
        if let Some(expected) = self.expected_checkouts.get(&owner) {
            if expected != key {
                return Err("Checkout identity changed.".into());
            }
        } else {
            self.expected_checkouts.insert(owner, key.clone());
        }
        Ok(())
    }

    /// Refresh only identities captured by the completed background check.
    pub fn refresh_checkout(&mut self, old: &CheckoutKey, refreshed: &CheckoutKey) {
        for key in self.expected_checkouts.values_mut() {
            if key == old {
                *key = refreshed.clone();
            }
        }
    }

    /// Rebind all threads belonging to the explicitly reopened checkout.
    pub fn reopen_checkout(&mut self, key: &CheckoutKey) {
        for expected in self.expected_checkouts.values_mut() {
            if expected.host == key.host && expected.root == key.root {
                *expected = key.clone();
            }
        }
    }

    pub fn expected_checkout(&self, id: &str) -> Option<&CheckoutKey> {
        self.expected_checkouts.get(&self.thread_key(id))
    }

    /// Checkout navigation does not replace a live cross-checkout connection.
    pub fn change_checkout(&mut self, path: PathBuf) -> Result<bool, String> {
        if self.creation.creating || self.creation.pending_create.is_some() {
            return Err("Resolve the pending thread creation before changing checkout.".into());
        }
        let generation = self.creation.generation + 1;
        let live = self.protocol() >= 3 && self.connection.connected;
        if live {
            self.creation.checkout = path;
            self.select(None);
            self.thread.submitting = false;
        } else {
            self.submissions.disconnected();
            self.connection = HostConnection {
                epoch: self.connection.epoch,
                threads_revision: self
                    .connection
                    .threads_revision
                    .checked_add(1)
                    .expect("thread revision exhausted"),
                catalog_revision: self.connection.catalog_revision.wrapping_add(1),
                ..Default::default()
            };
            self.creation = CheckoutCreationState::new(path);
            self.thread = SelectedThreadState::default();
        }
        self.creation.generation = generation;
        Ok(live)
    }

    pub fn reconnect(&mut self) -> u64 {
        self.connection.epoch += 1;
        self.connection.connected = false;
        self.connection.has_checkout_identity = false;
        self.connection.connecting = true;
        self.connection.error = None;
        if self.connection.saving {
            self.connection.save_message =
                Some("Save not confirmed. Reconnect will reload the saved default.".into());
        }
        self.connection.saving = false;
        self.thread.disconnected();
        self.submissions.disconnected();
        self.creation.disconnected();
        self.connection.epoch
    }

    pub fn select(&mut self, id: Option<String>) -> u64 {
        self.thread.select(id)
    }

    pub fn apply_status(&mut self, epoch: u64, mut status: Status) -> bool {
        if epoch != self.connection.epoch {
            return false;
        }
        self.connection.connecting = false;
        if !(1..=crate::host::PROTOCOL_VERSION).contains(&status.protocol_version) {
            self.connection.connected = false;
            self.connection.error =
                Some("Unsupported host protocol. Update the host and desktop together.".into());
            return false;
        }
        self.connection.connected = true;
        if self.creation.pending_create.is_none()
            && !self
                .connection
                .model_available(self.creation.new_model.as_ref(), &status)
        {
            self.creation.new_model = status
                .default_model
                .clone()
                .filter(|choice| self.connection.model_available(Some(choice), &status));
        }
        status.models.sort_by_key(|model| !model.authenticated);
        if self.connection.status.as_ref().is_none_or(|old| {
            old.models.len() != status.models.len()
                || old.models.iter().zip(&status.models).any(|(a, b)| {
                    (&a.provider, &a.model_id, &a.name, a.authenticated)
                        != (&b.provider, &b.model_id, &b.name, b.authenticated)
                })
        }) {
            self.connection.catalog_revision = self.connection.catalog_revision.wrapping_add(1);
        }
        self.connection.settings_model = status.default_model.clone();
        self.connection.status = Some(status);
        true
    }

    /// The negotiated protocol; version 1 until a status reply says otherwise.
    pub fn protocol(&self) -> u32 {
        self.connection.protocol()
    }

    pub fn available(&self, choice: Option<&ModelChoice>) -> bool {
        self.connection.available(choice)
    }

    pub fn accepts_watch(&self, epoch: u64, watch_epoch: u64, id: &str) -> bool {
        self.connection.epoch == epoch
            && self.thread.watch_epoch == watch_epoch
            && self.thread.selected.as_deref() == Some(id)
    }

    pub fn apply_snapshot(&mut self, epoch: u64, watch_epoch: u64, snapshot: Snapshot) -> bool {
        if !self.accepts_watch(epoch, watch_epoch, &snapshot.id) {
            return false;
        }
        let expected = self
            .connection
            .threads()
            .iter()
            .find(|thread| thread.id == snapshot.id)
            .map(|thread| (&thread.model, &thread.checkout))
            .or_else(|| {
                self.thread
                    .snapshot
                    .as_ref()
                    .map(|old| (&old.model, &old.checkout))
            });
        if expected.is_some_and(|(model, checkout)| {
            (self.protocol() < 3 && *model != snapshot.model) || *checkout != snapshot.checkout
        }) {
            self.connection.connected = false;
            self.connection.error =
                Some("Host changed this thread's identity. Reconnect to reload it.".into());
            return false;
        }
        if let Some(thread) = self
            .connection
            .threads_mut()
            .iter_mut()
            .find(|thread| thread.id == snapshot.id)
        {
            thread.status = snapshot.status;
            thread.model = snapshot.model.clone();
        }
        self.thread.live_preview_clipped = live_preview_clipped(&snapshot);
        self.thread.snapshot = Some(Arc::new(snapshot));
        true
    }

    pub fn finish_save(&mut self, result: Result<ModelChoice, String>) {
        self.connection.finish_save(result);
    }
}

/// The exact request to retry after an uncertain response. It is journaled before I/O.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingSubmit {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkout_wait: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<crate::images::ImageRef>,
    pub request_id: String,
    pub text: String,
    #[serde(default)]
    pub approval: Option<ApprovalMode>,
    #[serde(default)]
    pub when_busy: Option<QueueMode>,
    #[serde(default)]
    pub draft_revision: Option<u64>,
}

impl PendingSubmit {
    pub fn request(&self, thread_id: &str) -> Request {
        if self.checkout_wait {
            return Request::SubmitCheckout {
                identity: None,
                thread_id: thread_id.into(),
                request_id: self.request_id.clone(),
                text: self.text.clone(),
                approval: self.approval.unwrap_or(ApprovalMode::Ask),
                images: self.images.iter().map(|image| image.id.clone()).collect(),
            };
        }
        let mut request = match self.when_busy {
            Some(mode) => Request::submit_when_busy(
                thread_id.into(),
                self.request_id.clone(),
                self.text.clone(),
                self.approval.unwrap_or(ApprovalMode::Ask),
                mode,
            ),
            None => Request::submit(
                thread_id.into(),
                self.request_id.clone(),
                self.text.clone(),
                self.approval,
            ),
        };
        if !self.images.is_empty()
            && let Request::Submit {
                images,
                protocol_version,
                ..
            } = &mut request
        {
            *images = self.images.iter().map(|image| image.id.clone()).collect();
            *protocol_version = Some(6);
        }
        request
    }
}

#[derive(Default)]
pub struct Submissions {
    pending: HashMap<String, Vec<PendingSubmit>>,
    accepted: HashMap<String, Vec<PendingSubmit>>,
    /// Transient only: restored entries always require an explicit retry.
    sending: HashSet<String>,
    errors: HashMap<String, String>,
}

impl Submissions {
    /// Legacy clients allow one outstanding request per thread.
    pub fn begin(&mut self, thread_id: &str, text: String) -> Result<PendingSubmit, String> {
        if self.pending(thread_id).is_some() {
            return Err("Resolve the previous submission before sending another message.".into());
        }
        self.begin_with(thread_id, text, None, None, None)
    }

    pub fn begin_with(
        &mut self,
        thread_id: &str,
        text: String,
        approval: Option<ApprovalMode>,
        when_busy: Option<QueueMode>,
        draft_revision: Option<u64>,
    ) -> Result<PendingSubmit, String> {
        self.begin_images(
            thread_id,
            text,
            approval,
            when_busy,
            draft_revision,
            Vec::new(),
        )
    }

    pub fn begin_images(
        &mut self,
        thread_id: &str,
        text: String,
        approval: Option<ApprovalMode>,
        when_busy: Option<QueueMode>,
        draft_revision: Option<u64>,
        images: Vec<crate::images::ImageRef>,
    ) -> Result<PendingSubmit, String> {
        crate::images::validate_message(&images)?;
        if text.trim().is_empty() && images.is_empty() {
            return Err("Enter a message before sending.".into());
        }
        if self.all(thread_id).len() >= 16 {
            return Err("Resolve a pending message before sending another.".into());
        }
        if draft_revision.is_some()
            && self
                .all(thread_id)
                .iter()
                .any(|p| p.draft_revision == draft_revision)
        {
            return Err("This draft is already awaiting acceptance. Use Retry safely.".into());
        }
        let submission = PendingSubmit {
            checkout_wait: false,
            images,
            request_id: request_id(),
            text,
            approval,
            when_busy,
            draft_revision,
        };
        let request = submission.request(thread_id);
        let bytes = serde_json::to_vec(&request).map_err(|_| "Cannot encode this submission.")?;
        if bytes.len() + 1 > crate::host::REQUEST_LIMIT {
            return Err("Message exceeds the host's 64 KiB request limit. Shorten the draft before sending.".into());
        }
        self.pending
            .entry(thread_id.into())
            .or_default()
            .push(submission.clone());
        Ok(submission)
    }

    pub fn restore(&mut self, thread_id: String, pending: PendingSubmit) {
        let items = self.pending.entry(thread_id).or_default();
        if !items
            .iter()
            .any(|item| item.request_id == pending.request_id)
        {
            items.push(pending);
        }
    }

    pub fn all(&self, thread_id: &str) -> &[PendingSubmit] {
        self.pending.get(thread_id).map_or(&[], Vec::as_slice)
    }

    pub fn pending(&self, thread_id: &str) -> Option<&PendingSubmit> {
        self.all(thread_id).first()
    }

    pub fn get(&self, thread_id: &str, request_id: &str) -> Option<&PendingSubmit> {
        self.all(thread_id)
            .iter()
            .find(|p| p.request_id == request_id)
    }

    pub fn wait_for_checkout(&mut self, thread_id: &str, request_id: &str) {
        if let Some(pending) = self
            .pending
            .get_mut(thread_id)
            .and_then(|items| items.iter_mut().find(|p| p.request_id == request_id))
        {
            pending.checkout_wait = true;
        }
    }

    pub fn start_send(&mut self, thread_id: &str, request_id: &str) -> bool {
        self.get(thread_id, request_id).is_some() && self.sending.insert(request_id.into())
    }

    pub fn is_sending(&self, request_id: &str) -> bool {
        self.sending.contains(request_id)
    }

    pub fn uncertain(&mut self, request_id: &str, error: String) {
        self.sending.remove(request_id);
        self.errors.insert(request_id.into(), error);
    }

    pub fn error(&self, request_id: &str) -> Option<&str> {
        self.errors.get(request_id).map(String::as_str)
    }

    pub fn disconnected(&mut self) {
        self.sending.clear();
    }

    pub fn finish(&mut self, thread_id: &str, request_id: &str) -> Option<PendingSubmit> {
        let pending = self.pending.get_mut(thread_id)?;
        let index = pending.iter().position(|p| p.request_id == request_id)?;
        let finished = pending.remove(index);
        self.sending.remove(request_id);
        self.errors.remove(request_id);
        Some(finished)
    }

    pub fn accepted(&self, thread_id: &str) -> &[PendingSubmit] {
        self.accepted.get(thread_id).map_or(&[], Vec::as_slice)
    }

    pub fn restore_accepted(&mut self, thread_id: String, pending: PendingSubmit) {
        let items = self.accepted.entry(thread_id).or_default();
        if !items
            .iter()
            .any(|item| item.request_id == pending.request_id)
        {
            items.push(pending);
        }
    }

    pub fn confirm(&mut self, thread_id: &str, request_id: &str) -> Option<PendingSubmit> {
        let pending = self.finish(thread_id, request_id)?;
        let items = self.accepted.entry(thread_id.into()).or_default();
        items.push(pending.clone());
        // This cache is only for recovering exact queued text. Older host-owned
        // messages remain in history, not in the desktop journal.
        if items.len() > 16 {
            items.remove(0);
        }
        Some(pending)
    }

    /// Threads whose accepted Wait messages may have ended without starting.
    pub fn checkout_waits(&self) -> impl Iterator<Item = &str> {
        self.accepted
            .iter()
            .filter(|(_, items)| items.iter().any(|p| p.checkout_wait))
            .map(|(id, _)| id.as_str())
    }

    pub fn forget_accepted(&mut self, thread_id: &str, request_id: &str) {
        if let Some(items) = self.accepted.get_mut(thread_id) {
            items.retain(|p| p.request_id != request_id);
        }
    }

    /// Both accepted messages and still-queued inputs acknowledge the exact request.
    pub fn observe(&mut self, snapshot: &Snapshot) -> Vec<PendingSubmit> {
        let accepted = self
            .all(&snapshot.id)
            .iter()
            .filter(|pending| {
                snapshot
                    .queued
                    .iter()
                    .any(|q| q.request_id == pending.request_id)
                    || snapshot
                        .messages
                        .iter()
                        .any(|m| m.id == format!("request-{}", pending.request_id))
            })
            .map(|pending| pending.request_id.clone())
            .collect::<Vec<_>>();
        let confirmed = accepted
            .into_iter()
            .filter_map(|id| self.confirm(&snapshot.id, &id))
            .collect();
        if let Some(items) = self.accepted.get_mut(&snapshot.id) {
            items.retain(|pending| {
                !snapshot
                    .messages
                    .iter()
                    .any(|m| m.id == format!("request-{}", pending.request_id))
            });
        }
        confirmed
    }
}

pub fn request_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "desktop-{}-{time}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_clipping_uses_utf16_units() {
        let mut value = snapshot("a");
        value.live_text = "a".repeat(16_382);
        assert!(!live_preview_clipped(&value));
        value.live_text.push('a');
        assert!(live_preview_clipped(&value));
        value.live_text.clear();
        value.live_reasoning = "😀".repeat(8192);
        assert!(live_preview_clipped(&value));
    }

    #[test]
    fn catalog_revision_changes_only_for_displayed_model_changes() {
        let mut state = HostState::new(PathBuf::from("/repo"));
        let model = crate::host::Model {
            provider: "p".into(),
            model_id: "m".into(),
            name: "Model".into(),
            authenticated: true,
            ..Default::default()
        };
        assert!(state.apply_status(0, status(None, vec![model.clone()])));
        let revision = state.connection.catalog_revision;
        assert!(state.apply_status(0, status(None, vec![model.clone()])));
        assert_eq!(state.connection.catalog_revision, revision);
        let mut other = model.clone();
        other.model_id = "other".into();
        assert!(state.apply_status(0, status(None, vec![model.clone(), other.clone()])));
        let revision = state.connection.catalog_revision;
        let mut metadata = model.clone();
        metadata.context_window = Some(1234);
        assert!(state.apply_status(0, status(None, vec![metadata, other.clone()])));
        assert_eq!(state.connection.catalog_revision, revision);
        assert!(state.apply_status(0, status(None, vec![other.clone(), model.clone()])));
        let reordered = state.connection.catalog_revision;
        assert_ne!(reordered, revision);
        other.authenticated = false;
        assert!(state.apply_status(0, status(None, vec![model, other])));
        assert_ne!(state.connection.catalog_revision, reordered);
    }

    #[test]
    fn a_provider_authenticated_after_startup_reaches_the_catalog_without_a_restart() {
        let mut state = HostState::new(PathBuf::from("/repo"));
        let mut model = crate::host::Model {
            provider: "lab".into(),
            model_id: "a".into(),
            name: "A".into(),
            authenticated: false,
            ..Default::default()
        };
        let choice = model.choice();
        // The desktop's first status came before the host read the login shell.
        assert!(state.apply_status(0, status(None, vec![model.clone()])));
        assert!(
            !state
                .connection
                .model_available(Some(&choice), state.connection.status.as_ref().unwrap())
        );
        assert_eq!(
            state.connection.begin_catalog_refresh(),
            None,
            "nothing newer yet"
        );

        // The workspace reports a newer provider registry: one reload starts, not two.
        state.connection.providers_revision = 1;
        assert_eq!(state.connection.begin_catalog_refresh(), Some(1));
        assert_eq!(state.connection.begin_catalog_refresh(), None);
        model.authenticated = true;
        assert!(state.apply_status(0, status(None, vec![model.clone()])));
        state.connection.finish_catalog_refresh(1, true);
        assert!(
            state
                .connection
                .model_available(Some(&choice), state.connection.status.as_ref().unwrap())
        );
        // Opening the picker now finds the catalog current.
        assert_eq!(state.connection.begin_catalog_refresh(), None);

        // A failed reload is tried again; a restarted host counts from zero again.
        state.connection.providers_revision = 2;
        assert_eq!(state.connection.begin_catalog_refresh(), Some(2));
        state.connection.finish_catalog_refresh(2, false);
        assert_eq!(state.connection.begin_catalog_refresh(), Some(2));
        state.connection.finish_catalog_refresh(2, true);
        state.connection.providers_revision = 0;
        assert_eq!(state.connection.begin_catalog_refresh(), Some(0));
    }

    fn status(default: Option<ModelChoice>, models: Vec<crate::host::Model>) -> Status {
        Status {
            protocol_version: 1,
            default_model: default,
            models,
        }
    }

    fn snapshot(id: &str) -> Snapshot {
        serde_json::from_value(serde_json::json!({
            "id": id, "checkout": "/repo",
            "model": {"provider":"p", "model_id":"saved"},
            "status":"idle", "messages":[], "live_text":"", "tools":[],
            "pending_approval":null, "error":null, "truncated":false
        }))
        .unwrap()
    }

    #[test]
    fn checkout_navigation_retires_reads_but_keeps_host_acknowledgements() {
        let mut state = HostState::new("/repo".into());
        let mut connected = status(None, vec![]);
        connected.protocol_version = 6;
        state.apply_status(0, connected);
        state.select(Some("a".into()));
        let read = state.read_stamp();
        let pending = state
            .submissions
            .begin("a", "original input".into())
            .unwrap();
        state.submissions.start_send("a", &pending.request_id);
        state.change_checkout("/other".into()).unwrap();
        state.select(Some("b".into()));
        assert_eq!(state.connection.epoch, 0);
        assert!(state.submissions.is_sending(&pending.request_id));
        assert!(
            state
                .submissions
                .confirm("a", &pending.request_id)
                .is_some()
        );
        assert!(state.submissions.accepted("b").is_empty());
        assert!(!state.accepts_read(&read));
        state.change_checkout("/repo".into()).unwrap();
        state.select(Some("a".into()));
        assert!(!state.accepts_read(&read));
        assert!(!state.apply_snapshot(read.connection, read.selection, snapshot("a")));
        let key = CheckoutKey {
            host: None,
            root: "/repo".into(),
            device: 1,
            inode: 2,
        };
        state.bind_checkout("a", &key).unwrap();
        let mut replaced = key.clone();
        replaced.inode += 1;
        assert!(state.bind_checkout("a", &replaced).is_err());
        assert_eq!(state.expected_checkout("a"), Some(&key));
    }

    #[test]
    fn negotiated_protocol_accepts_versions_one_through_seven_only() {
        let mut state = HostState::new("/repo".into());
        assert_eq!(state.protocol(), 1);
        for (version, accepted) in [
            (1, true),
            (2, true),
            (3, true),
            (4, true),
            (5, true),
            (6, true),
            (7, true),
            (8, false),
            (0, false),
        ] {
            state.reconnect();
            let mut reply = status(None, Vec::new());
            reply.protocol_version = version;
            assert_eq!(state.apply_status(state.connection.epoch, reply), accepted);
            assert_eq!(state.connection.connected, accepted);
            if accepted {
                assert_eq!(state.protocol(), version);
            }
        }
        // Submissions are sized as their largest negotiated form.
        let request = crate::host::Request::submit_bound("t", "r", "hi");
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(wire["protocol_version"], 3);
        assert_eq!(wire["approval"], "auto");
        assert_eq!(wire["when_busy"], "follow_up");
        let legacy = crate::host::Request::submit("t".into(), "r".into(), "hi".into(), None);
        assert_eq!(
            serde_json::to_value(&legacy).unwrap(),
            serde_json::json!({"op":"submit","thread_id":"t","request_id":"r","text":"hi"})
        );
    }

    #[test]
    fn snapshots_replace_state_and_keep_exact_saved_model() {
        let mut state = HostState::new("/repo".into());
        let watch = state.select(Some("a".into()));
        assert!(state.apply_snapshot(0, watch, snapshot("a")));
        assert!(!state.apply_snapshot(0, watch, snapshot("b")));
        assert_eq!(
            state.thread.snapshot.as_ref().unwrap().model.model_id,
            "saved"
        );
        let mut wrong_model = snapshot("a");
        wrong_model.model.model_id = "new-default".into();
        assert!(!state.apply_snapshot(0, watch, wrong_model));
        assert_eq!(
            state.thread.snapshot.as_ref().unwrap().model.model_id,
            "saved"
        );
        let new_watch = state.select(Some("b".into()));
        assert!(!state.apply_snapshot(0, watch, snapshot("a")));
        assert!(state.apply_snapshot(0, new_watch, snapshot("b")));
        assert_eq!(state.thread.snapshot.as_ref().unwrap().id, "b");
    }

    #[test]
    fn reconnect_preserves_uncertain_create_and_submit_ids() {
        let mut state = HostState::new("/repo".into());
        let original = state
            .submissions
            .begin("thread", "One task".into())
            .unwrap();
        state.creation.pending_create = Some((
            "create-once".into(),
            ModelChoice {
                provider: "p".into(),
                model_id: "saved".into(),
            },
        ));
        state.reconnect();
        state.apply_status(state.connection.epoch, status(None, vec![]));
        assert_eq!(state.submissions.pending("thread"), Some(&original));
        assert_eq!(
            state.creation.pending_create.as_ref().unwrap().0,
            "create-once"
        );
        assert!(!state.thread.submitting);
        assert!(!state.creation.creating);
    }

    #[test]
    fn stale_replies_and_unavailable_catalog_do_not_select_a_model() {
        let mut state = HostState::new("/repo".into());
        let old = state.reconnect();
        let current = state.reconnect();
        assert!(!state.apply_status(old, status(None, vec![])));
        assert!(state.apply_status(
            current,
            status(
                Some(ModelChoice {
                    provider: "p".into(),
                    model_id: "missing".into()
                }),
                vec![]
            )
        ));
        assert!(state.creation.new_model.is_none());
        assert!(!state.available(state.creation.new_model.as_ref()));
        let watch = state.select(Some("old".into()));
        state.select(Some("new".into()));
        assert!(!state.accepts_watch(current, watch, "old"));
        state.reconnect();
        assert!(!state.accepts_watch(current, state.thread.watch_epoch, "new"));
    }

    #[test]
    fn saved_defaults_require_acknowledgement_and_do_not_replace_new_choice() {
        let choice = ModelChoice {
            provider: "p".into(),
            model_id: "m".into(),
        };
        let mut state = HostState::new("/repo".into());
        state.apply_status(
            0,
            status(
                Some(choice.clone()),
                vec![crate::host::Model {
                    image_input: None,
                    provider: "p".into(),
                    model_id: "m".into(),
                    name: "Model".into(),
                    authenticated: true,
                    reasoning: None,
                    thinking_levels: None,
                    context_window: None,
                    input: None,
                }],
            ),
        );
        assert_eq!(state.creation.new_model, Some(choice.clone()));
        state.connection.saving = true;
        state.finish_save(Err("disk full".into()));
        assert_eq!(
            state.connection.status.as_ref().unwrap().default_model,
            Some(choice.clone())
        );
        assert!(
            state
                .connection
                .save_message
                .as_ref()
                .unwrap()
                .starts_with("Save not confirmed")
        );
        let other = ModelChoice {
            provider: "p".into(),
            model_id: "other".into(),
        };
        state.finish_save(Ok(other.clone()));
        assert_eq!(
            state.connection.status.as_ref().unwrap().default_model,
            Some(other)
        );
        assert_eq!(state.creation.new_model, Some(choice));
    }

    #[test]
    fn oversized_encoded_submission_is_rejected_without_pending_work() {
        let mut state = Submissions::default();
        assert!(state.begin("thread", "\\".repeat(40_000)).is_err());
        assert!(state.pending("thread").is_none());
    }

    #[test]
    fn uncertain_submission_keeps_its_id_and_text() {
        let mut state = Submissions::default();
        let first = state.begin("thread", "Run one task".into()).unwrap();
        assert!(state.begin("thread", "Run another task".into()).is_err());
        assert_eq!(state.pending("thread"), Some(&first));
        assert!(state.finish("thread", "stale-reply").is_none());
        assert_eq!(state.pending("thread"), Some(&first));
        assert_eq!(state.finish("thread", &first.request_id), Some(first));
        assert!(state.pending("thread").is_none());
    }

    #[test]
    fn switching_threads_does_not_discard_an_uncertain_submission() {
        let mut state = Submissions::default();
        let first = state.begin("a", "Message A".into()).unwrap();
        let second = state.begin("b", "Message B".into()).unwrap();
        assert_ne!(first.request_id, second.request_id);
        state.finish("b", &second.request_id);
        assert_eq!(state.pending("a"), Some(&first));
        assert!(state.begin("c", " \n".into()).is_err());
    }
    #[test]
    fn many_pending_messages_keep_original_modes_and_acknowledge_out_of_order() {
        let mut state = Submissions::default();
        let a = state
            .begin_with(
                "t",
                "first".into(),
                Some(ApprovalMode::Ask),
                Some(QueueMode::FollowUp),
                Some(1),
            )
            .unwrap();
        let b = state
            .begin_with(
                "t",
                "second".into(),
                Some(ApprovalMode::Auto),
                Some(QueueMode::Steer),
                Some(2),
            )
            .unwrap();
        assert!(state.start_send("t", &a.request_id));
        assert!(!state.start_send("t", &a.request_id));
        state.uncertain(&a.request_id, "transport closed".into());
        assert_eq!(state.confirm("t", &b.request_id), Some(b));
        assert_eq!(state.pending("t"), Some(&a));
        assert!(
            state
                .begin_with("t", "edited".into(), None, Some(QueueMode::Steer), Some(1))
                .is_err()
        );
        let wire = serde_json::to_value(a.request("t")).unwrap();
        assert_eq!(wire["request_id"], a.request_id);
        assert_eq!(wire["approval"], "ask");
        assert_eq!(wire["when_busy"], "follow_up");
        assert!(state.start_send("t", &a.request_id));
        state.disconnected();
        assert!(!state.is_sending(&a.request_id));
        assert_eq!(state.pending("t"), Some(&a));
    }

    #[test]
    fn snapshot_queue_acknowledges_pending_and_keeps_full_local_text() {
        let mut state = Submissions::default();
        let a = state
            .begin_with(
                "a",
                "full text".into(),
                Some(ApprovalMode::Auto),
                Some(QueueMode::FollowUp),
                Some(1),
            )
            .unwrap();
        let mut current = snapshot("a");
        current.queued.push(crate::host::Queued {
            id: format!("request-{}", a.request_id),
            request_id: a.request_id.clone(),
            text: "tail".into(),
            mode: QueueMode::FollowUp,
            approval: ApprovalMode::Auto,
        });
        assert_eq!(state.observe(&current), vec![a.clone()]);
        assert!(state.pending("a").is_none());
        assert_eq!(state.accepted("a"), std::slice::from_ref(&a));
        assert!(state.observe(&current).is_empty());
        current.queued.clear();
        current.messages.push(crate::host::Message {
            id: format!("request-{}", a.request_id),
            ..Default::default()
        });
        state.observe(&current);
        assert!(state.accepted("a").is_empty());
        state.restore("a".into(), a.clone());
        state.restore("a".into(), a.clone());
        assert_eq!(state.all("a"), &[a]);
    }

    #[test]
    fn v3_watch_accepts_model_change_but_never_a_checkout_change() {
        let mut state = HostState::new("/repo".into());
        let mut catalog = status(None, Vec::new());
        catalog.protocol_version = 3;
        state.apply_status(0, catalog);
        let epoch = state.select(Some("a".into()));
        assert!(state.apply_snapshot(0, epoch, snapshot("a")));
        let mut changed = snapshot("a");
        changed.model.model_id = "other".into();
        assert!(state.apply_snapshot(0, epoch, changed.clone()));
        changed.checkout = "/different".into();
        assert!(!state.apply_snapshot(0, epoch, changed));
    }
    #[test]
    fn pending_limit_is_bounded_and_cancelled_cache_is_forgotten() {
        let mut state = Submissions::default();
        for revision in 0..16 {
            state
                .begin_with(
                    "t",
                    format!("message {revision}"),
                    Some(ApprovalMode::Ask),
                    Some(QueueMode::FollowUp),
                    Some(revision),
                )
                .unwrap();
        }
        assert!(
            state
                .begin_with("t", "overflow".into(), None, None, Some(16))
                .is_err()
        );
        assert_eq!(state.all("t").len(), 16);
        let request = state.pending("t").unwrap().request_id.clone();
        state.confirm("t", &request).unwrap();
        state.forget_accepted("t", &request);
        assert!(state.accepted("t").is_empty());
        assert!(
            state
                .begin_with("t", "next".into(), None, None, Some(17))
                .is_ok()
        );
    }

    #[test]
    fn cached_busy_checkouts_follow_catalog_changes() {
        let repo = std::path::Path::new("/repo");
        let mut connection = HostConnection::default();
        connection.set_threads(vec![ThreadSummary {
            checkout: repo.into(),
            status: crate::host::ThreadStatus::Running,
            ..Default::default()
        }]);
        assert!(connection.checkout_has_run(repo));
        connection.threads_mut()[0].status = crate::host::ThreadStatus::Stopped;
        assert!(!connection.checkout_has_run(repo));
    }

    #[test]
    fn thread_revision_tracks_mutation_and_checkout_reset() {
        let mut state = HostState::new("/repo".into());
        let initial = state.connection.threads_revision();
        state
            .connection
            .threads_mut()
            .push(ThreadSummary::default());
        let mutated = state.connection.threads_revision();
        assert!(mutated > initial);
        state.connection.set_threads(Vec::new());
        let replaced = state.connection.threads_revision();
        assert!(replaced > mutated);
        state.change_checkout("/other".into()).unwrap();
        assert!(state.connection.threads_revision() > replaced);
    }

    #[test]
    fn scoped_host_state_preserves_defaults_and_selection_transitions() {
        let mut state = HostState::new("/repo".into());
        assert_eq!(state.connection.epoch, 0);
        assert!(!state.connection.connected);
        assert!(!state.connection.connecting);
        assert!(state.connection.threads().is_empty());
        assert_eq!(state.creation.checkout, PathBuf::from("/repo"));
        assert_eq!(state.thread.watch_epoch, 0);
        assert!(state.thread.selected.is_none());

        state.thread.controlling = true;
        state.thread.decided_approvals.insert("a1".into());
        let watch = state.select(Some("t1".into()));
        assert_eq!(watch, 1);
        assert_eq!(state.thread.selected.as_deref(), Some("t1"));
        assert!(!state.thread.controlling);
        assert!(state.thread.decided_approvals.contains("a1"));

        state.connection.saving = true;
        state.thread.submitting = true;
        state.creation.creating = true;
        let epoch = state.reconnect();
        assert_eq!(epoch, 1);
        assert_eq!(state.thread.watch_epoch, 2);
        assert!(!state.thread.submitting);
        assert!(!state.creation.creating);
        assert!(state.thread.decided_approvals.is_empty());
        assert!(!state.connection.saving);
        assert!(state.connection.save_message.is_some());
    }
}

#[cfg(test)]
mod checkout_capability_tests {
    use super::*;
    #[test]
    fn identity_is_sent_only_when_supported() {
        let key = CheckoutKey {
            host: None,
            root: "/repo".into(),
            device: 7,
            inode: 9,
        };
        let mut connection = HostConnection::default();
        let request = |connection: &HostConnection| Request::ListThreads {
            checkout: key.root.clone(),
            identity: connection.checkout_identity(Some(&key)),
        };
        assert_eq!(
            serde_json::to_value(request(&connection)).unwrap(),
            serde_json::json!({"op":"list_threads","checkout":"/repo"})
        );
        connection.has_checkout_identity = true;
        assert_eq!(
            serde_json::to_value(request(&connection)).unwrap()["identity"],
            serde_json::json!({"dev":"7","ino":"9"})
        );
    }
}
