//! Disposable tab preferences, independent of the private draft/submission journal.
//! The UI reads/writes `tabs.json` beside desktop preferences, atomically and on
//! its background executor. Unknown versions must be left untouched on disk.
use super::*;
use crate::host::ThreadSummary;
use std::collections::HashSet;

pub const FILE_NAME: &str = "tabs.json";
pub const SCHEMA_VERSION: u32 = 1;
pub const FILE_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct SavedTab {
    id: TabId,
    target: TabTarget,
    bookmark: BookmarkRecord,
}
#[derive(Serialize)]
pub struct SavedWindow {
    schema_version: u32,
    active: Option<TabId>,
    tabs: Vec<SavedTab>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStatus {
    Restored,
    Invalid,
    TooLarge,
    UnknownVersion,
}
pub struct RestoredTabs {
    pub window: WindowTabs,
    /// For save policy/diagnostics only; never a blocking recovery error.
    pub status: RestoreStatus,
}

/// All inputs come from already validated services, never from the saved file.
/// Supply a complete catalog; a watch's newest-200 subset cannot prove absence.
pub struct RestoreContext<'a> {
    pub host: &'a HostKey,
    pub threads: &'a [ThreadSummary],
    pub checkouts: &'a [CheckoutKey],
    /// Drafts the desktop already knows are recoverable, including empty open
    /// drafts. No journal is read here; an empty list omits unsent draft tabs.
    pub drafts: &'a [TabTarget],
    pub connection: Option<u64>,
}
impl RestoreContext<'_> {
    pub fn contains(&self, target: &TabTarget) -> bool {
        let checkout = target.checkout();
        if &checkout.host != self.host || !self.checkouts.contains(checkout) {
            return false;
        }
        match target {
            TabTarget::Existing { id, .. } => self
                .threads
                .iter()
                .any(|t| &t.id == id && t.checkout == checkout.root),
            TabTarget::Draft { .. } => self.drafts.contains(target),
            TabTarget::Worktrees { .. } | TabTarget::Issues { .. } => true,
        }
    }
}

/// Encoding failure preserves the last good file; the UI must not replace it.
/// Every open thread is saved: a catalog that is loading, partial or stale must
/// not erase tabs, and `decode` validates on restore. Only drafts the caller
/// knows are recoverable are kept, including saved empty drafts.
pub fn encode(window: &WindowTabs, context: &RestoreContext<'_>) -> Result<Vec<u8>, TabError> {
    encode_document(&capture(window, context))
}

/// Copy small owned records on an event; serialize them on the background executor.
pub fn capture(window: &WindowTabs, context: &RestoreContext<'_>) -> SavedWindow {
    let tabs = window
        .tabs
        .iter()
        .filter(|t| {
            let target = &t.session().target;
            !target.is_draft() || context.drafts.contains(target)
        })
        .map(|tab| SavedTab {
            id: tab.id,
            target: tab.session().target.clone(),
            bookmark: tab.session().bookmark.clone(),
        })
        .collect::<Vec<_>>();
    let active = window.active.filter(|id| tabs.iter().any(|t| t.id == *id));
    SavedWindow {
        schema_version: SCHEMA_VERSION,
        active,
        tabs,
    }
}
impl SavedWindow {
    pub fn set_bookmark(&mut self, id: TabId, bookmark: BookmarkRecord) -> Result<(), TabError> {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            validate_record(&tab.target, &bookmark)?;
            tab.bookmark = bookmark;
        }
        Ok(())
    }
}
pub fn encode_document(document: &SavedWindow) -> Result<Vec<u8>, TabError> {
    let bytes = serde_json::to_vec(document).map_err(|_| TabError::InvalidBookmark)?;
    if bytes.len() > FILE_LIMIT {
        return Err(TabError::BookmarkTooLarge);
    }
    Ok(bytes)
}

/// Total for arbitrary bytes. Bad entries are dropped independently; absent or
/// invalid active identity selects the first surviving tab. No reads are begun.
pub fn decode(bytes: &[u8], window: WindowId, context: &RestoreContext<'_>) -> RestoredTabs {
    let mut restored = RestoredTabs {
        window: WindowTabs::new(window),
        status: RestoreStatus::Restored,
    };
    if bytes.len() > FILE_LIMIT {
        restored.status = RestoreStatus::TooLarge;
        return restored;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        restored.status = RestoreStatus::Invalid;
        return restored;
    };
    let Some(version) = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    else {
        restored.status = RestoreStatus::Invalid;
        return restored;
    };
    if version != u64::from(SCHEMA_VERSION) {
        restored.status = RestoreStatus::UnknownVersion;
        return restored;
    }
    let Some(tabs) = value.get("tabs").and_then(serde_json::Value::as_array) else {
        restored.status = RestoreStatus::Invalid;
        return restored;
    };
    let active = value
        .get("active")
        .and_then(serde_json::Value::as_u64)
        .map(TabId);
    let mut ids = HashSet::new();
    for entry in tabs.iter().take(MAX_RESTORED_TABS) {
        let Ok(mut saved) = serde_json::from_value::<SavedTab>(entry.clone()) else {
            continue;
        };
        let checkout = match &mut saved.target {
            TabTarget::Existing { checkout, .. }
            | TabTarget::Draft { checkout, .. }
            | TabTarget::Worktrees { checkout }
            | TabTarget::Issues { checkout } => checkout,
        };
        if let Some(current) = context.checkouts.iter().find(|current| {
            current.host == checkout.host
                && current.root == checkout.root
                && match (checkout.directory, current.directory) {
                    // Device numbers can change when a volume is mounted again.
                    (Some(saved), Some(current)) => saved.inode == current.inode,
                    (None, None) => true,
                    _ => false,
                }
        }) {
            *checkout = current.clone();
        }
        if let TabTarget::Draft { id, checkout } = &saved.target
            && id == "draft"
            && let Some(target) = context.drafts.iter().find(|t| t.checkout() == checkout)
        {
            saved.target = target.clone();
        }
        if saved.id.0 == 0
            || ids.contains(&saved.id)
            || !context.contains(&saved.target)
            || restored
                .window
                .tabs
                .iter()
                .any(|t| t.session().target.same_content(&saved.target))
        {
            continue;
        }
        let Ok(id) = restored.window.insert(
            saved.target,
            saved.bookmark,
            restored.window.tabs.len(),
            context.connection,
        ) else {
            continue;
        };
        ids.insert(saved.id);
        if active == Some(saved.id) {
            restored.window.active = Some(id);
        }
        if let Some(tab) = restored.window.tabs.last_mut() {
            tab.content.session_mut().residency = Residency::Cold {
                reason: ColdReason::Restored,
            };
        }
    }
    if restored.window.active.is_none() {
        restored.window.active = restored.window.tabs.first().map(|t| t.id);
    }
    restored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ThreadStatus;
    use crate::tabs::tests::{checkout, draft, host, open, rich_bookmark, summary, target, window};
    use serde_json::{Value, json};

    struct Catalog {
        host: HostKey,
        threads: Vec<ThreadSummary>,
        checkouts: Vec<CheckoutKey>,
        drafts: Vec<TabTarget>,
    }
    impl Catalog {
        fn new(count: usize) -> Self {
            Self {
                host: host(),
                threads: (0..count)
                    .map(|i| summary(&format!("t{i}"), ThreadStatus::Idle))
                    .collect(),
                checkouts: vec![checkout("/repo")],
                drafts: Vec::new(),
            }
        }
        fn context(&self) -> RestoreContext<'_> {
            RestoreContext {
                host: &self.host,
                threads: &self.threads,
                checkouts: &self.checkouts,
                drafts: &self.drafts,
                connection: Some(2),
            }
        }
    }
    fn source(count: usize) -> (WindowTabs, Catalog) {
        let mut w = window();
        for i in 0..count {
            open(&mut w, &format!("t{i}"));
        }
        (w, Catalog::new(count))
    }
    fn value(w: &WindowTabs, c: &Catalog) -> Value {
        serde_json::from_slice(&encode(w, &c.context()).unwrap()).unwrap()
    }
    fn read(v: &Value, c: &Catalog) -> RestoredTabs {
        decode(&serde_json::to_vec(v).unwrap(), WindowId(99), &c.context())
    }

    #[test]
    fn pure_round_trip_restores_order_active_and_all_bookmarks_without_hydrating() {
        let (mut w, c) = source(4);
        let ids = w.tabs().iter().map(TabRecord::id).collect::<Vec<_>>();
        for (id, view) in ids.iter().zip([
            ThreadView::Conversation,
            ThreadView::Changes,
            ThreadView::History,
            ThreadView::Files,
        ]) {
            w.save_bookmark(*id, BookmarkRecord::capture(&rich_bookmark(view)).unwrap())
                .unwrap();
        }
        w.move_tab(ids[3], 0).unwrap();
        w.activate(ids[1]).unwrap();
        let old = w.read_stamp(ids[1]).unwrap();
        let mut restored = read(&value(&w, &c), &c);
        assert_eq!(restored.status, RestoreStatus::Restored);
        assert_eq!(restored.window.tabs().len(), 4);
        for (old, new) in w.tabs().iter().zip(restored.window.tabs()) {
            assert_eq!(old.session().target(), new.session().target());
            assert_eq!(old.session().bookmark(), new.session().bookmark());
        }
        let active = restored.window.active().unwrap();
        assert_eq!(
            restored.window.get(active).unwrap().session().target(),
            &target("t1")
        );
        assert!(restored.window.tabs().iter().all(|t| matches!(
            t.session().residency(),
            Residency::Cold {
                reason: ColdReason::Restored
            }
        )));
        assert_eq!(restored.window.closed_len(), 0);
        assert!(!restored.window.accepts_read(old));
        restored.window.activate(active).unwrap();
        assert!(restored.window.read_stamp(active).is_some());
        assert_eq!(
            restored
                .window
                .tabs()
                .iter()
                .filter(|t| t.session().residency.visibility().is_some())
                .count(),
            1
        );
    }

    #[test]
    fn legacy_draft_tab_restores_as_one_imported_identity() {
        let mut w = window();
        w.open(draft("draft"), None).unwrap();
        let mut catalog = Catalog::new(0);
        catalog.drafts = vec![draft("draft")];
        let saved = value(&w, &catalog);
        catalog.drafts = vec![draft("legacy-0")];
        let restored = read(&saved, &catalog);
        assert_eq!(restored.window.tabs().len(), 1);
        assert_eq!(
            restored.window.tabs()[0].session().target(),
            &draft("legacy-0")
        );
        assert_eq!(
            restored.window.active(),
            Some(restored.window.tabs()[0].id())
        );
    }

    #[test]
    fn malformed_torn_unknown_old_and_oversized_documents_are_valid_empty_windows() {
        let c = Catalog::new(1);
        for bytes in [
            b"".as_slice(),
            b"null",
            b"{",
            b"[]",
            b"{}",
            b"{\"schema_version\":1}",
            b"{\"schema_version\":1,\"tabs\":false}",
            b"\xff",
        ] {
            let r = decode(bytes, WindowId(99), &c.context());
            assert_eq!(r.status, RestoreStatus::Invalid);
            assert!(r.window.tabs().is_empty());
            assert_eq!(r.window.active(), None);
        }
        for version in [0, 2, u64::MAX] {
            let r = read(&json!({"schema_version":version,"tabs":[]}), &c);
            assert_eq!(r.status, RestoreStatus::UnknownVersion);
            assert!(r.window.tabs().is_empty());
        }
        let r = decode(&vec![b' '; FILE_LIMIT + 1], WindowId(99), &c.context());
        assert_eq!(r.status, RestoreStatus::TooLarge);
    }

    #[test]
    fn restored_checkout_refreshes_device_only_for_matching_inode() {
        let (w, mut catalog) = source(1);
        let saved = value(&w, &catalog);
        catalog.checkouts[0].directory.as_mut().unwrap().device = 99;
        let restored = read(&saved, &catalog);
        assert_eq!(restored.window.tabs().len(), 1);
        assert_eq!(
            restored.window.tabs()[0].session().target().checkout(),
            &catalog.checkouts[0]
        );
        catalog.checkouts[0].directory.as_mut().unwrap().inode = 99;
        assert!(read(&saved, &catalog).window.tabs().is_empty());
    }

    #[test]
    fn duplicate_ids_and_threads_are_dropped_independently_and_bad_active_falls_back() {
        let (w, c) = source(3);
        let mut v = value(&w, &c);
        let mut same_thread = v["tabs"][0].clone();
        same_thread["id"] = json!(999);
        let same_id = v["tabs"][1].clone();
        v["tabs"].as_array_mut().unwrap().extend([
            same_thread,
            same_id,
            json!({"id":4}),
            json!(null),
        ]);
        v["active"] = json!(987654);
        let r = read(&v, &c);
        assert_eq!(r.window.tabs().len(), 3);
        assert_eq!(r.window.active(), Some(r.window.tabs()[0].id()));
        v["active"] = json!(null);
        assert_eq!(read(&v, &c).window.active(), Some(r.window.tabs()[0].id()));
    }

    #[test]
    fn missing_threads_wrong_host_store_root_and_inode_drop_only_the_affected_tabs() {
        let (w, mut c) = source(3);
        let v = value(&w, &c);
        c.threads.remove(1);
        let r = read(&v, &c);
        assert_eq!(r.window.tabs().len(), 2);
        c.threads[0].checkout = "/other".into();
        assert_eq!(read(&v, &c).window.tabs().len(), 1);
        for field in ["id", "store"] {
            let mut bad = v.clone();
            bad["tabs"][2]["target"]["checkout"]["host"][field] = json!("/wrong");
            assert!(read(&bad, &c).window.tabs().is_empty());
        }
        let mut bad = v.clone();
        bad["tabs"][2]["target"]["checkout"]["directory"]["inode"] = json!(999);
        assert!(read(&bad, &c).window.tabs().is_empty());
        let mut bad = v.clone();
        bad["tabs"][2]["target"]["checkout"]["root"] = json!("relative");
        assert!(read(&bad, &c).window.tabs().is_empty());
        c.host.id = "new-host".into();
        assert!(read(&v, &c).window.tabs().is_empty());
    }

    #[test]
    fn corrupted_bookmark_unknown_kind_zero_id_and_huge_fields_do_not_poison_siblings() {
        let (w, c) = source(2);
        let base = value(&w, &c);
        let mut variants = Vec::new();
        let mut v = base.clone();
        v["tabs"][0]["bookmark"]["json"] = json!("{");
        variants.push(v);
        let mut v = base.clone();
        v["tabs"][0]["bookmark"]["json"] = json!("x".repeat(bookmark::INLINE_BOOKMARK_LIMIT + 1));
        variants.push(v);
        let mut v = base.clone();
        v["tabs"][0]["target"]["kind"] = json!("browser");
        variants.push(v);
        let mut v = base.clone();
        v["tabs"][0]["id"] = json!(0);
        variants.push(v);
        let mut v = base.clone();
        v["tabs"][0]["target"]["id"] = json!("x".repeat(129));
        variants.push(v);
        let mut v = base;
        v["tabs"][0]["bookmark"] = json!({"storage":"stored","view":"files","blob":"../escape"});
        variants.push(v);
        for v in variants {
            let r = read(&v, &c);
            assert_eq!(r.window.tabs().len(), 1);
            assert_eq!(r.window.tabs()[0].session().target(), &target("t1"));
        }
    }

    #[test]
    fn restore_caps_input_entries_and_later_bad_active_selects_first_survivor() {
        let c = Catalog::new(MAX_RESTORED_TABS + 3);
        let (w, _) = source(1);
        let mut v = value(&w, &c);
        let template = v["tabs"][0].clone();
        v["tabs"] = Value::Array(
            (0..MAX_RESTORED_TABS + 3)
                .map(|i| {
                    let mut t = template.clone();
                    t["id"] = json!(i + 1);
                    t["target"]["id"] = json!(format!("t{i}"));
                    t
                })
                .collect(),
        );
        v["active"] = json!(MAX_RESTORED_TABS + 2);
        let r = read(&v, &c);
        assert_eq!(r.window.tabs().len(), MAX_RESTORED_TABS);
        assert_eq!(r.window.active(), Some(r.window.tabs()[0].id()));
        assert!(
            r.window
                .tabs()
                .iter()
                .all(|t| t.cold_bytes() <= COLD_TAB_BYTES)
        );
    }

    #[test]
    fn drafts_require_external_recoverable_identity_and_closed_history_is_not_saved() {
        let (mut w, mut c) = source(1);
        let a = w.active().unwrap();
        w.close(a, false).unwrap();
        let d = w.open(draft("d"), Some(1)).unwrap();
        assert!(value(&w, &c)["tabs"].as_array().unwrap().is_empty());
        c.drafts.push(draft("d"));
        let saved = value(&w, &c);
        let r = read(&saved, &c);
        assert_eq!(r.window.tabs().len(), 1);
        assert_eq!(r.window.closed_len(), 0);
        assert!(r.window.tabs()[0].session().target().is_draft());
        assert_eq!(w.active(), Some(d));
        c.drafts.clear();
        assert!(read(&saved, &c).window.tabs().is_empty());
    }

    #[test]
    fn archived_and_known_offline_threads_restore_without_fake_execution() {
        let (w, mut c) = source(1);
        c.threads[0].archived = Some(true);
        let bytes = encode(&w, &c.context()).unwrap();
        let context = RestoreContext {
            connection: None,
            ..c.context()
        };
        let mut r = decode(&bytes, WindowId(99), &context);
        let id = r.window.active().unwrap();
        r.window.activate(id).unwrap();
        assert_eq!(r.window.tabs().len(), 1);
        assert!(r.window.read_stamp(id).is_none());
        assert!(!r.window.tabs()[0].session().connected());
    }

    #[test]
    fn extended_bookmark_references_round_trip_without_reading_a_file_or_journal() {
        let (mut w, c) = source(1);
        let id = w.active().unwrap();
        let record = BookmarkRecord::stored(ThreadView::Files, "private-blob-1".into()).unwrap();
        w.save_bookmark(id, record.clone()).unwrap();
        let v = value(&w, &c);
        let r = read(&v, &c);
        assert_eq!(r.window.tabs()[0].session().bookmark(), &record);
        assert!(!v.to_string().contains("draft_revision"));
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
    }

    #[test]
    fn truncation_and_byte_mutations_never_panic_or_violate_window_invariants() {
        let (w, c) = source(2);
        let bytes = encode(&w, &c.context()).unwrap();
        for end in 0..bytes.len() {
            let r = decode(&bytes[..end], WindowId(99), &c.context());
            assert!(r.window.tabs().is_empty());
        }
        for offset in (0..bytes.len()).step_by(7) {
            for byte in [0, b'{', b']', b'9', 255] {
                let mut changed = bytes.clone();
                changed[offset] = byte;
                let r = decode(&changed, WindowId(99), &c.context());
                assert!(r.window.tabs().len() <= MAX_RESTORED_TABS);
                assert!(
                    r.window
                        .active()
                        .is_none_or(|id| r.window.get(id).is_some())
                );
            }
        }
    }

    #[test]
    fn encode_keeps_tabs_the_current_catalog_does_not_list() {
        // A save made while the listing is empty, partial or reconnecting must
        // not erase tabs; decode alone decides what can be restored.
        let (w, full) = source(2);
        let mut partial = Catalog::new(1);
        let lagging = value(&w, &partial);
        partial.threads.clear();
        partial.checkouts.clear();
        let empty = value(&w, &partial);
        for saved in [lagging, empty] {
            assert_eq!(saved["tabs"].as_array().unwrap().len(), 2);
            let r = read(&saved, &full);
            assert_eq!(r.window.tabs().len(), 2);
            assert_eq!(r.window.tabs()[1].session().target(), &target("t1"));
        }
    }

    #[test]
    fn full_window_fits_file_bound_and_contains_no_loaded_or_submission_data() {
        let (mut w, c) = source(300);
        let id = w.active().unwrap();
        let stamp = w.read_stamp(id).unwrap();
        w.resident_mut(stamp).unwrap().repository.history.selected =
            Some("resident-only-sentinel".into());
        let bytes = encode(&w, &c.context()).unwrap();
        assert!(bytes.len() < FILE_LIMIT);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("resident-only-sentinel"));
        let r = decode(&bytes, WindowId(99), &c.context());
        assert_eq!(r.window.tabs().len(), 300);
    }
    #[test]
    fn worktrees_round_trip_and_unknown_content_is_dropped_independently() {
        let (mut w, c) = source(1);
        let target = TabTarget::Worktrees {
            checkout: checkout("/repo"),
        };
        let id = w.open(target.clone(), Some(2)).unwrap();
        let bookmark = BookmarkRecord::Worktrees {
            selected: Some("/repo.worktrees/topic".into()),
            size_sort: true,
        };
        w.save_bookmark(id, bookmark.clone()).unwrap();
        let mut saved = value(&w, &c);
        let mut unknown = saved["tabs"][0].clone();
        unknown["id"] = json!(999);
        unknown["target"]["kind"] = json!("future_content");
        saved["tabs"].as_array_mut().unwrap().push(unknown);
        let restored = read(&saved, &c);
        assert_eq!(restored.status, RestoreStatus::Restored);
        assert_eq!(restored.window.tabs().len(), 2);
        assert_eq!(
            restored
                .window
                .get(restored.window.active().unwrap())
                .unwrap()
                .session()
                .bookmark(),
            &bookmark
        );
        assert_eq!(
            restored
                .window
                .get(restored.window.active().unwrap())
                .unwrap()
                .session()
                .target(),
            &target
        );
    }
    #[test]
    fn issues_round_trip_and_reject_a_mismatched_bookmark() {
        let (mut w, c) = source(1);
        let target = TabTarget::Issues {
            checkout: checkout("/repo"),
        };
        let id = w.open(target.clone(), Some(2)).unwrap();
        let bookmark = BookmarkRecord::Issues {
            selected: Some("2f1c0d8e-issue".into()),
        };
        w.save_bookmark(id, bookmark.clone()).unwrap();
        let restored = read(&value(&w, &c), &c);
        assert_eq!(restored.status, RestoreStatus::Restored);
        let tab = restored
            .window
            .get(restored.window.active().unwrap())
            .unwrap();
        assert_eq!(tab.session().target(), &target);
        assert_eq!(tab.session().bookmark(), &bookmark);
        // An Issues bookmark belongs only to an Issues tab, and its ID is bounded.
        let other = w
            .open(
                TabTarget::Worktrees {
                    checkout: checkout("/repo"),
                },
                Some(2),
            )
            .unwrap();
        assert!(w.save_bookmark(other, bookmark).is_err());
        assert!(
            w.save_bookmark(
                id,
                BookmarkRecord::Issues {
                    selected: Some("bad id/../x".into())
                }
            )
            .is_err()
        );
    }
}
