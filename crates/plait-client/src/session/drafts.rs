//! Additive schema-1 draft identities. Legacy fields remain complete snapshots.
use super::{Draft, Session, identifier};
use crate::host::{ApprovalMode, ModelChoice, ThinkingLevel, harness::HarnessId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Older builds refuse a journal whose threads and legacy drafts exceed 512;
/// compatibility copies leave 128 of those records for the older build's own use.
const LEGACY_RECORDS: usize = 384;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DraftSettings {
    pub harness: Option<HarnessId>,
    pub model: Option<ModelChoice>,
    pub mode: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub approval: Option<ApprovalMode>,
}

/// A first send that is never replayed on launch; explicit retry reuses this ID.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftTabCreation {
    pub request_id: String,
    /// Never `Wait` in a saved file; `wait` carries it. See `Placement::journaled`.
    pub placement: crate::host::admission::Placement,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wait: bool,
    pub harness: HarnessId,
    pub model: ModelChoice,
    pub mode: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub approval: ApprovalMode,
    pub message_approval: Option<ApprovalMode>,
    pub thread_approval: Option<ApprovalMode>,
    pub message: Draft,
    pub needs_choice: bool,
}

impl DraftTabCreation {
    /// Import the reviewed schema-one creation field without allocating a new request.
    pub fn from_legacy(draft: &Draft, checkout: &Path) -> Option<Self> {
        let saved = draft.creation.as_ref()?;
        let crate::host::admission::Request::CreateAdmittedThread {
            request_id,
            checkout: root,
            placement,
            harness,
            provider,
            model_id,
            approval,
            mode,
            thinking,
            source_thread_id: None,
        } = &saved.request
        else {
            return None;
        };
        if root != checkout {
            return None;
        }
        let mut message = draft.clone();
        message.creation = None;
        Some(Self {
            request_id: request_id.clone(),
            placement: *placement,
            wait: saved.wait,
            harness: *harness,
            model: ModelChoice {
                provider: provider.clone(),
                model_id: model_id.clone(),
            },
            mode: mode.clone(),
            thinking: *thinking,
            approval: *approval,
            message_approval: saved.approval,
            thread_approval: saved.choice,
            message,
            needs_choice: false,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftTab {
    /// Owns this checkout's backwards-compatible draft snapshot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub legacy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation: Option<DraftTabCreation>,
    pub checkout: PathBuf,
    pub draft: Draft,
    #[serde(default)]
    pub settings: DraftSettings,
}

impl Session {
    pub fn draft_tab(&self, id: &str, checkout: &Path) -> Option<&DraftTab> {
        self.draft_tabs
            .get(id)
            .filter(|tab| tab.checkout == checkout)
    }

    pub fn set_draft_tab(&mut self, id: String, mut tab: DraftTab) {
        tab.legacy = self.draft_tabs.get(&id).is_some_and(|old| old.legacy)
            || !self
                .draft_tabs
                .values()
                .any(|old| old.checkout == tab.checkout && old.legacy);
        if tab.legacy {
            self.set_new_draft(tab.checkout.clone(), tab.draft.clone());
        }
        self.draft_tabs.insert(id, tab);
    }

    pub fn clear_draft_tab(&mut self, id: &str) {
        let Some(tab) = self.draft_tabs.remove(id) else {
            return;
        };
        if !tab.legacy {
            return;
        }
        // Keep an intact nonempty sibling visible to an older build.
        self.clear_new_draft(&tab.checkout);
        let next = self
            .draft_tabs
            .iter()
            .filter(|(_, other)| other.checkout == tab.checkout)
            .min_by_key(|(_, other)| other.draft.text.is_empty() && other.draft.images.is_empty())
            .map(|(id, _)| id.clone());
        if let Some(id) = next {
            let other = self.draft_tabs.get_mut(&id).unwrap();
            other.legacy = true;
            let other = other.clone();
            self.set_new_draft(other.checkout, other.draft);
        }
    }

    /// The file form. Older schema-1 builds read only the legacy fields, sweep
    /// images no legacy field names, and rewrite only what they know; so each
    /// other draft with text, images or an unanswered first send is also copied
    /// into `new_drafts` under a key no folder can have. Copies are left out rather than cost the file its size
    /// limit or an older build its record bound, with room for its own threads.
    pub(super) fn encode(&self) -> Result<Vec<u8>, String> {
        let encode = |session: &Session| {
            serde_json::to_vec(session).map_err(|e| format!("Cannot encode workspace: {e}"))
        };
        let plain = encode(self)?;
        let room = LEGACY_RECORDS.saturating_sub(self.threads.len() + self.new_drafts.len());
        let mut copied = self.clone();
        for (id, tab) in &self.draft_tabs {
            if (tab.draft.text.is_empty() && tab.draft.images.is_empty() && tab.creation.is_none())
                || self.new_draft_for(&tab.checkout) == Some(&tab.draft)
            {
                continue;
            }
            // An unanswered first send keeps its request and captured message,
            // so a retry after a downgrade reaches the thread the host made.
            let creation = tab.creation.as_ref().and_then(|creation| {
                let text = serde_json::to_string(creation).ok()?;
                (text.len() <= 256 * 1024).then(|| Draft {
                    text,
                    images: creation.message.images.clone(),
                    ..Default::default()
                })
            });
            if copied.new_drafts.len() - self.new_drafts.len() + 1 + usize::from(creation.is_some())
                > room
            {
                break;
            }
            let key = |suffix: &str| {
                let mut key = tab.checkout.clone().into_os_string();
                key.push("\0");
                key.push(id);
                key.push(suffix);
                PathBuf::from(key)
            };
            copied.new_drafts.insert(key(""), tab.draft.clone());
            if let Some(creation) = creation {
                copied.new_drafts.insert(key("\0creation"), creation);
            }
        }
        if copied.new_drafts.len() == self.new_drafts.len() {
            return Ok(plain);
        }
        let bytes = encode(&copied)?;
        Ok(if bytes.len() <= super::FILE_LIMIT {
            bytes
        } else {
            plain
        })
    }

    /// Return copies written by `encode` to `draft_tabs`. Older builds never
    /// edit them, so a live entry with the same id and checkout wins.
    fn import_copies(&mut self) {
        let copies: Vec<_> = self
            .new_drafts
            .keys()
            .filter(|key| key.as_os_str().as_encoded_bytes().contains(&0))
            .cloned()
            .collect();
        let mut creations = Vec::new();
        for key in copies {
            let draft = self.new_drafts.remove(&key).unwrap();
            let key = String::from_utf8_lossy(key.as_os_str().as_encoded_bytes()).into_owned();
            let mut parts = key.splitn(3, '\0');
            let (Some(checkout), Some(id)) = (parts.next(), parts.next()) else {
                continue;
            };
            let (checkout, id) = (PathBuf::from(checkout), id.to_owned());
            if parts.next().is_some() {
                creations.push((checkout, id, draft));
                continue;
            }
            // Equal content is not a duplicate here: two sent drafts can both be empty.
            if self.draft_tab(&id, &checkout).is_some() {
                continue;
            }
            let id = if identifier(&id) && !self.draft_tabs.contains_key(&id) {
                id
            } else {
                self.legacy_id()
            };
            self.draft_tabs.insert(
                id,
                DraftTab {
                    legacy: false,
                    creation: DraftTabCreation::from_legacy(&draft, &checkout),
                    checkout,
                    draft,
                    settings: DraftSettings::default(),
                },
            );
        }
        // Attach only to a draft that came back; one sent or discarded stays gone.
        for (checkout, id, copy) in creations {
            if let Ok(creation) = serde_json::from_str::<DraftTabCreation>(&copy.text)
                && let Some(tab) = self.draft_tabs.get_mut(&id)
                && tab.checkout == checkout
                && tab.creation.is_none()
            {
                tab.creation = Some(creation);
            }
        }
    }

    fn legacy_id(&self) -> String {
        (0..)
            .map(|n| format!("legacy-{n}"))
            .find(|id| !self.draft_tabs.contains_key(id))
            .unwrap()
    }

    /// A downgrade may edit only a legacy field. Import that version as another
    /// draft rather than choosing one version and losing the other person's text.
    pub fn import_legacy_drafts(&mut self) {
        self.import_copies();
        let legacy: Vec<_> = self
            .new_draft
            .iter()
            .map(|(p, d)| (p.clone(), d.clone()))
            .chain(self.new_drafts.iter().map(|(p, d)| (p.clone(), d.clone())))
            .collect();
        for (checkout, draft) in legacy {
            if self
                .draft_tabs
                .values()
                .any(|tab| tab.checkout == checkout && tab.draft == draft)
            {
                continue;
            }
            let id = self.legacy_id();
            for tab in self
                .draft_tabs
                .values_mut()
                .filter(|t| t.checkout == checkout)
            {
                tab.legacy = false;
            }
            self.draft_tabs.insert(
                id,
                DraftTab {
                    legacy: true,
                    creation: DraftTabCreation::from_legacy(&draft, &checkout),
                    checkout,
                    draft,
                    settings: DraftSettings::default(),
                },
            );
        }
    }

    pub(super) fn validate_draft_tabs(&self) -> Result<(), String> {
        if self.draft_tabs.len() > 10_000 {
            return Err("Saved workspace exceeds 10,000 drafts. Nothing was discarded.".into());
        }
        for (id, tab) in &self.draft_tabs {
            validate_draft_tab(id, tab)?;
        }
        Ok(())
    }
}

/// One saved draft's limits, wherever it is kept.
pub(super) fn validate_draft_tab(id: &str, tab: &DraftTab) -> Result<(), String> {
    if !identifier(id)
        || !tab.checkout.is_absolute()
        || tab.checkout.as_os_str().len() > 4096
        || tab.draft.text.len() > 256 * 1024
        || tab
            .draft
            .submitted
            .as_ref()
            .is_some_and(|(text, _)| text.len() > crate::host::REQUEST_LIMIT)
        || tab
            .settings
            .mode
            .as_ref()
            .is_some_and(|mode| mode.len() > 128)
        || tab
            .settings
            .model
            .as_ref()
            .is_some_and(|m| m.provider.len() > 1024 || m.model_id.len() > 1024)
    {
        return Err("Saved new-thread draft exceeds its limits. Nothing was discarded.".into());
    }
    crate::images::validate_message(&tab.draft.images)?;
    if let Some(creation) = &tab.creation {
        if !identifier(&creation.request_id)
            || creation.message.text.len() > 256 * 1024
            || creation.model.provider.len() > 1024
            || creation.model.model_id.len() > 1024
            || creation.mode.as_ref().is_some_and(|mode| mode.len() > 128)
            || (creation.message.text.trim().is_empty() && creation.message.images.is_empty())
            || creation.harness == HarnessId::Other
        {
            return Err("Saved draft creation is invalid. It was not sent.".into());
        }
        crate::images::validate_message(&creation.message.images)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // The schema-1 reader before independent drafts: no knowledge of draft_tabs.
    #[derive(Deserialize)]
    struct OldSession {
        #[serde(default)]
        new_draft: Option<(PathBuf, Draft)>,
        #[serde(default)]
        new_drafts: BTreeMap<PathBuf, Draft>,
        schema_version: u32,
        host_store: PathBuf,
        checkout: Option<PathBuf>,
        selected: Option<String>,
        threads: BTreeMap<String, super::super::Thread>,
    }
    impl OldSession {
        fn decode(bytes: &[u8]) -> Result<Self, String> {
            if bytes.len() > super::super::FILE_LIMIT {
                return Err("too large".into());
            }
            let old: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            if old.schema_version != 1
                || !old.host_store.is_absolute()
                || old.checkout.as_ref().is_some_and(|p| !p.is_absolute())
                || old.threads.len() + old.new_drafts.len() > 512
            {
                return Err("invalid workspace".into());
            }
            for (path, draft) in old
                .new_draft
                .iter()
                .map(|(p, d)| (p, d))
                .chain(old.new_drafts.iter())
            {
                if !path.is_absolute() || draft.text.len() > 256 * 1024 {
                    return Err("invalid draft".into());
                }
                crate::images::validate_message(&draft.images)?;
            }
            Ok(old)
        }
    }
    fn tab(text: &str) -> DraftTab {
        DraftTab {
            legacy: false,
            creation: None,
            checkout: "/project".into(),
            draft: Draft {
                text: text.into(),
                revision: 4,
                ..Default::default()
            },
            settings: DraftSettings::default(),
        }
    }
    #[test]
    fn unknown_saved_draft_harness_is_not_replaced_by_preferences() {
        let mut session = Session::empty("/host".into());
        session.set_draft_tab("draft".into(), tab("saved"));
        let mut value = serde_json::to_value(session).unwrap();
        value["draft_tabs"]["draft"]["settings"]["harness"] = "antigravity".into();
        let restored: Session = serde_json::from_value(value).unwrap();
        restored.validate_draft_tabs().unwrap();
        let preferences: crate::preferences::Preferences =
            serde_json::from_str(r#"{"default_harness":"antigravity"}"#).unwrap();
        assert_eq!(preferences.default_harness, HarnessId::Pi);
        assert_eq!(
            restored.draft_tabs["draft"].settings.harness,
            Some(HarnessId::Other)
        );
        assert_eq!(restored.draft_tabs["draft"].draft.text, "saved");
    }

    #[test]
    fn additive_shape_keeps_old_reader_and_imports_old_and_downgrade_edits() {
        let mut session = Session::empty("/host".into());
        session.set_new_draft("/project".into(), tab("legacy").draft);
        let old_bytes = serde_json::to_vec(&session).unwrap();
        let mut upgraded: Session = serde_json::from_slice(&old_bytes).unwrap();
        upgraded.import_legacy_drafts();
        assert_eq!(upgraded.draft_tabs.len(), 1);
        assert_eq!(upgraded.draft_tabs["legacy-0"].draft.text, "legacy");
        upgraded.set_draft_tab("second".into(), tab("second message"));
        let bytes = serde_json::to_vec(&upgraded).unwrap();
        let old = OldSession::decode(&bytes).unwrap();
        assert_eq!(old.new_draft.unwrap().1.text, "legacy");
        assert!(old.selected.is_none());
        assert!(old.threads.is_empty());
        let mut restored: Session = serde_json::from_slice(&bytes).unwrap();
        restored.import_legacy_drafts();
        assert_eq!(restored, upgraded, "mirrors must not duplicate drafts");
        restored.new_draft.as_mut().unwrap().1.text = "edited by an older build".into();
        restored.import_legacy_drafts();
        assert_eq!(restored.draft_tabs.len(), 3);
        assert!(
            restored
                .draft_tabs
                .values()
                .any(|t| t.draft.text == "second message")
        );
        assert!(
            restored
                .draft_tabs
                .values()
                .any(|t| t.draft.text == "edited by an older build")
        );
    }
    #[test]
    fn legacy_worktree_creation_imports_and_survives_a_sibling_draft() {
        use crate::host::admission::{Placement, Request};
        let request = Request::CreateAdmittedThread {
            request_id: "keep-this-request".into(),
            checkout: "/project".into(),
            placement: Placement::Worktree,
            harness: HarnessId::Pi,
            provider: "provider".into(),
            model_id: "model".into(),
            approval: ApprovalMode::Ask,
            mode: None,
            thinking: None,
            source_thread_id: None,
        };
        let mut draft = tab("Keep this first message").draft;
        draft.creation = Some(super::super::DraftCreation {
            request: request.clone(),
            approval: Some(ApprovalMode::Ask),
            choice: None,
            wait: false,
        });
        let mut session = Session::empty("/host".into());
        session.set_new_draft("/project".into(), draft);
        session.import_legacy_drafts();
        session.set_draft_tab("sibling".into(), tab("Independent message"));
        let creation = session.draft_tabs["legacy-0"].creation.as_ref().unwrap();
        assert_eq!(creation.request_id, "keep-this-request");
        assert_eq!(creation.placement, Placement::Worktree);
        assert_eq!(creation.message.text, "Keep this first message");
        assert!(creation.message.creation.is_none());
        let bytes = serde_json::to_vec(&session).unwrap();
        let old = OldSession::decode(&bytes).unwrap();
        assert_eq!(old.new_draft.unwrap().1.creation.unwrap().request, request);
        let mut restored: Session = serde_json::from_slice(&bytes).unwrap();
        restored.import_legacy_drafts();
        assert_eq!(restored, session);
    }

    #[test]
    fn clearing_one_draft_keeps_its_siblings_and_legacy_mirror() {
        let mut session = Session::empty("/host".into());
        session.set_draft_tab("a".into(), tab("first"));
        session.set_draft_tab("b".into(), tab("second"));
        session.clear_draft_tab("b");
        assert_eq!(
            session.new_draft_for(Path::new("/project")).unwrap().text,
            "first"
        );
        assert_eq!(session.draft_tabs.len(), 1);
        session.clear_draft_tab("a");
        assert!(session.new_draft.is_none() && session.new_drafts.is_empty());
    }
    #[test]
    fn malformed_additions_fail_totally_without_touching_legacy_text() {
        let mut session = Session::empty("/host".into());
        session.set_draft_tab("a".into(), tab("saved text"));
        let bytes = serde_json::to_vec(&session).unwrap();
        for end in 0..bytes.len() {
            assert!(serde_json::from_slice::<Session>(&bytes[..end]).is_err());
        }
        let mut value = serde_json::to_value(&session).unwrap();
        for invalid in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!({"a":false}),
        ] {
            value["draft_tabs"] = invalid;
            assert!(serde_json::from_value::<Session>(value.clone()).is_err());
            assert_eq!(
                OldSession::decode(&serde_json::to_vec(&value).unwrap())
                    .unwrap()
                    .new_draft
                    .unwrap()
                    .1
                    .text,
                "saved text"
            );
        }
    }
    fn isolated(name: &str) -> bool {
        if std::env::var("PLAIT_DRAFT_JOURNAL_TEST").ok().as_deref() == Some(name) {
            return true;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("PLAIT_DRAFT_JOURNAL_TEST", name)
            .status()
            .unwrap();
        assert!(status.success());
        false
    }
    #[test]
    fn full_journal_and_interrupted_write_keep_previous_drafts_readable() {
        if !isolated(
            "session::drafts::tests::full_journal_and_interrupted_write_keep_previous_drafts_readable",
        ) {
            return;
        }
        let root = std::env::temp_dir().join(format!("plait-many-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (journal, mut session) = super::super::Journal::open(&root, "/host".into()).unwrap();
        for n in 0..300 {
            session.set_draft_tab(format!("draft-{n}"), tab(&format!("Message {n}")));
        }
        journal.queue(session.clone());
        journal.flush().unwrap();
        let good = std::fs::read(root.join("desktop.json")).unwrap();
        assert!(OldSession::decode(&good).is_ok());
        for n in 0..20 {
            session.set_draft_tab(format!("large-{n}"), tab(&"x".repeat(256 * 1024)));
        }
        journal.queue(session);
        assert!(journal.flush().unwrap_err().contains("4 MiB"));
        assert_eq!(std::fs::read(root.join("desktop.json")).unwrap(), good);
        std::fs::write(root.join(".desktop.interrupted.tmp"), b"{\"draft_tabs\":").unwrap();
        drop(journal);
        let (_, restored) = super::super::Journal::open(&root, "/host".into()).unwrap();
        assert_eq!(restored.draft_tabs.len(), 300);
        assert_eq!(restored.draft_tabs["draft-299"].draft.text, "Message 299");
        std::fs::remove_dir_all(root).unwrap();
    }
    /// preview.19's journal: no `draft_tabs`, no `Draft.creation`, and image
    /// retention counted only from the fields below. It rewrites what it knows.
    #[derive(Serialize, Deserialize)]
    struct ReleasedDraft {
        images: Vec<crate::images::ImageRef>,
        text: String,
        revision: u64,
        submitted: Option<(String, u64)>,
    }
    #[derive(Serialize, Deserialize)]
    struct ReleasedSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_draft: Option<(PathBuf, ReleasedDraft)>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        new_drafts: BTreeMap<PathBuf, ReleasedDraft>,
        schema_version: u32,
        host_store: PathBuf,
        checkout: Option<PathBuf>,
        selected: Option<String>,
        threads: BTreeMap<String, serde_json::Value>,
    }
    impl ReleasedSession {
        // preview.19's `Session::set_new_draft`, verbatim.
        fn set_new_draft(&mut self, checkout: PathBuf, draft: ReleasedDraft) {
            if let Some((path, old)) = self.new_draft.take()
                && path != checkout
            {
                self.new_drafts.insert(path, old);
            }
            self.new_drafts.remove(&checkout);
            self.new_draft = Some((checkout, draft));
        }
        fn image_ids(&self) -> Vec<String> {
            let drafts = self.new_draft.iter().map(|(_, d)| d);
            drafts
                .chain(self.new_drafts.values())
                .flat_map(|d| d.images.iter().map(|image| image.id.clone()))
                .collect()
        }
    }
    fn image(c: char) -> crate::images::ImageRef {
        crate::images::ImageRef {
            id: std::iter::repeat_n(c, 64).collect(),
            media_type: "image/png".into(),
            size: 10,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn released_build_keeps_every_draft_through_open_and_rewrite() {
        if !isolated(
            "session::drafts::tests::released_build_keeps_every_draft_through_open_and_rewrite",
        ) {
            return;
        }
        let root = std::env::temp_dir().join(format!("plait-downgrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (journal, mut session) = super::super::Journal::open(&root, "/host".into()).unwrap();
        // The first tab owns the legacy field and is empty; its siblings have work.
        session.set_draft_tab("a".into(), tab(""));
        let mut b = tab("B text");
        b.draft.images = vec![image('b')];
        b.settings.mode = Some("plan".into());
        session.set_draft_tab("b".into(), b);
        let mut c = tab("C text");
        c.checkout = "/other".into();
        session.set_draft_tab("c".into(), c);
        session.set_draft_tab("d".into(), tab("D text"));
        journal.queue(session);
        journal.flush().unwrap();
        drop(journal);

        let bytes = std::fs::read(root.join("desktop.json")).unwrap();
        assert!(OldSession::decode(&bytes).is_ok());
        let mut released: ReleasedSession = serde_json::from_slice(&bytes).unwrap();
        assert!(
            released.image_ids().contains(&image('b').id),
            "the released build's sweep would delete this draft's image"
        );
        // The released build edits its one visible draft and saves what it knows.
        released.set_new_draft(
            "/project".into(),
            ReleasedDraft {
                images: Vec::new(),
                text: "typed in the released build".into(),
                revision: 9,
                submitted: None,
            },
        );
        std::fs::write(
            root.join("desktop.json"),
            serde_json::to_vec(&released).unwrap(),
        )
        .unwrap();

        let (_, mut restored) = super::super::Journal::open(&root, "/host".into()).unwrap();
        restored.import_legacy_drafts();
        let texts = |checkout: &str| {
            let mut texts: Vec<_> = restored
                .draft_tabs
                .values()
                .filter(|t| t.checkout == Path::new(checkout))
                .map(|t| t.draft.text.clone())
                .collect();
            texts.sort();
            texts
        };
        assert_eq!(
            texts("/project"),
            ["B text", "D text", "typed in the released build"]
        );
        assert_eq!(texts("/other"), ["C text"]);
        assert_eq!(restored.draft_tabs["b"].draft.images, [image('b')]);
        assert!(
            restored
                .new_drafts
                .keys()
                .all(|key| key.to_str().is_some_and(|k| !k.contains('\0'))),
            "compatibility copies stay out of memory"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compatibility_copies_never_cost_the_size_or_record_limits() {
        let copies = |bytes: &[u8]| {
            let old = OldSession::decode(bytes).unwrap();
            assert!(old.threads.len() + old.new_drafts.len() <= LEGACY_RECORDS);
            old.new_drafts.len()
        };
        let mut many = Session::empty("/host".into());
        for n in 0..500 {
            many.set_draft_tab(format!("d{n}"), tab(&format!("Message {n}")));
        }
        assert_eq!(copies(&many.encode().unwrap()), LEGACY_RECORDS);
        let mut restored: Session = serde_json::from_slice(&many.encode().unwrap()).unwrap();
        restored.import_legacy_drafts();
        assert_eq!(restored, many, "copies of live drafts add nothing");

        // Twelve near-maximal drafts fit once, not twice: write them once.
        let mut large = Session::empty("/host".into());
        for n in 0..12 {
            large.set_draft_tab(format!("d{n}"), tab(&"x".repeat(250 * 1024)));
        }
        let bytes = large.encode().unwrap();
        assert!(bytes.len() <= super::super::FILE_LIMIT);
        assert_eq!(bytes, serde_json::to_vec(&large).unwrap());
    }
    #[test]
    fn released_build_keeps_an_unanswered_first_send_and_its_request() {
        if !isolated(
            "session::drafts::tests::released_build_keeps_an_unanswered_first_send_and_its_request",
        ) {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("plait-downgrade-send-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (journal, mut session) = super::super::Journal::open(&root, "/host".into()).unwrap();
        session.set_draft_tab("owner".into(), tab("Owner text"));
        // Sent, unanswered, then the composer was emptied: only the capture has work.
        let mut sent = tab("");
        sent.creation = Some(DraftTabCreation {
            request_id: "keep-this-request".into(),
            placement: crate::host::admission::Placement::Worktree,
            wait: false,
            harness: HarnessId::Pi,
            model: ModelChoice {
                provider: "provider".into(),
                model_id: "model".into(),
            },
            mode: None,
            thinking: None,
            approval: ApprovalMode::Ask,
            message_approval: Some(ApprovalMode::Ask),
            thread_approval: None,
            message: Draft {
                text: "Sent text".into(),
                revision: 7,
                images: vec![image('e')],
                ..Default::default()
            },
            needs_choice: false,
        });
        let expected = sent.creation.clone();
        session.set_draft_tab("sent".into(), sent);
        journal.queue(session);
        journal.flush().unwrap();
        drop(journal);

        let bytes = std::fs::read(root.join("desktop.json")).unwrap();
        let mut released: ReleasedSession = serde_json::from_slice(&bytes).unwrap();
        assert!(released.image_ids().contains(&image('e').id));
        released.set_new_draft(
            "/project".into(),
            ReleasedDraft {
                images: Vec::new(),
                text: "Owner edited".into(),
                revision: 9,
                submitted: None,
            },
        );
        std::fs::write(
            root.join("desktop.json"),
            serde_json::to_vec(&released).unwrap(),
        )
        .unwrap();

        let (_, mut restored) = super::super::Journal::open(&root, "/host".into()).unwrap();
        restored.import_legacy_drafts();
        let sent = restored.draft_tab("sent", Path::new("/project")).unwrap();
        assert_eq!(sent.creation, expected, "retry must reuse the request");
        let owners: Vec<_> = restored
            .draft_tabs
            .values()
            .filter(|t| t.creation.is_none())
            .map(|t| t.draft.text.as_str())
            .collect();
        assert_eq!(owners, ["Owner edited"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
