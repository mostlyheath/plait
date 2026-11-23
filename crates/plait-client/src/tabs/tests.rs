use super::*;
use crate::host::{ThreadStatus, ThreadSummary};
use bookmark::*;

pub(super) fn host() -> HostKey {
    HostKey {
        id: "host-one".into(),
        store: "/store".into(),
    }
}
pub(super) fn checkout(root: &str) -> CheckoutKey {
    CheckoutKey {
        host: host(),
        root: root.into(),
        directory: Some(DirectoryIdentity {
            device: 1,
            inode: 2,
        }),
    }
}
pub(super) fn target(id: &str) -> TabTarget {
    TabTarget::Existing {
        id: id.into(),
        checkout: checkout("/repo"),
    }
}
pub(super) fn draft(id: &str) -> TabTarget {
    TabTarget::Draft {
        id: id.into(),
        checkout: checkout("/repo"),
    }
}
pub(super) fn window() -> WindowTabs {
    WindowTabs::new(WindowId(7))
}
pub(super) fn open(w: &mut WindowTabs, id: &str) -> TabId {
    w.open(target(id), Some(1)).unwrap()
}
fn ids(w: &WindowTabs) -> Vec<TabId> {
    w.tabs().iter().map(TabRecord::id).collect()
}
fn bookmark(view: ThreadView) -> BookmarkRecord {
    BookmarkRecord::capture(&ThreadBookmark {
        view,
        ..Default::default()
    })
    .unwrap()
}
pub(super) fn summary(id: &str, status: ThreadStatus) -> ThreadSummary {
    ThreadSummary {
        id: id.into(),
        checkout: "/repo".into(),
        title: Some(format!("Thread {id}")),
        status,
        ..Default::default()
    }
}

#[test]
fn opens_append_and_sidebar_activation_deduplicates_without_resetting() {
    let mut w = window();
    let a = open(&mut w, "a");
    let b = open(&mut w, "b");
    w.save_bookmark(a, bookmark(ThreadView::Files)).unwrap();
    assert_eq!(open(&mut w, "a"), a);
    assert_eq!(ids(&w), [a, b]);
    let stamp = w.read_stamp(a).unwrap();
    assert_eq!(open(&mut w, "a"), a);
    assert_eq!(w.read_stamp(a), Some(stamp));
    assert_eq!(
        w.get(a).unwrap().session().bookmark().view(),
        ThreadView::Files
    );
    assert_eq!(w.get(a).unwrap().content().view_commands().len(), 4);
}
#[test]
fn independent_drafts_and_creation_keeps_tab_but_changes_lifetime() {
    let mut w = window();
    let a = w.open(draft("draft-a"), Some(1)).unwrap();
    let b = w.open(draft("draft-b"), Some(1)).unwrap();
    assert_ne!(a, b);
    w.activate(a).unwrap();
    w.save_bookmark(a, bookmark(ThreadView::Changes)).unwrap();
    let old = w.read_stamp(a).unwrap();
    assert!(w.open_turn(a, "request-1".into()));
    w.bind_created(a, target("created")).unwrap();
    assert!(w.get(a).unwrap().session().opened_turns().is_empty());
    assert_eq!(w.active(), Some(a));
    assert!(!w.accepts_lifetime(old.lifetime));
    assert!(!w.accepts_read(old));
    assert_eq!(
        w.get(a).unwrap().session().bookmark().view(),
        ThreadView::Changes
    );
    assert_eq!(open(&mut w, "created"), a);
    assert!(w.open(draft("next-draft"), Some(1)).is_ok());
    assert_eq!(w.tabs().len(), 3);
}
#[test]
fn draft_creation_rejects_wrong_checkout_existing_thread_and_stale_ack() {
    let mut w = window();
    let a = w.open(draft("d"), Some(1)).unwrap();
    let b = open(&mut w, "b");
    assert_eq!(w.bind_created(a, target("b")), Err(TabError::AlreadyOpen));
    assert_eq!(
        w.bind_created(
            a,
            TabTarget::Existing {
                id: "c".into(),
                checkout: checkout("/other")
            }
        ),
        Err(TabError::IdentityMismatch)
    );
    assert_eq!(
        w.bind_created(b, target("c")),
        Err(TabError::InvalidIdentity)
    );
    w.close(a, true).unwrap();
    assert_eq!(w.bind_created(a, target("c")), Err(TabError::MissingTab));
}
#[test]
fn identical_thread_ids_on_other_hosts_are_distinct_but_checkout_rebinding_is_rejected() {
    let mut w = window();
    let a = open(&mut w, "a");
    let mut other = checkout("/other");
    assert_eq!(
        w.open(
            TabTarget::Existing {
                id: "a".into(),
                checkout: other.clone()
            },
            Some(1)
        ),
        Err(TabError::IdentityMismatch)
    );
    other.host.id = "other-host".into();
    let b = w
        .open(
            TabTarget::Existing {
                id: "a".into(),
                checkout: other,
            },
            Some(1),
        )
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(w.tabs().len(), 2);
}
#[test]
fn only_active_tab_retains_stage_one_scopes_and_old_reads_cannot_cross_a_b_a() {
    let mut w = window();
    let a = open(&mut w, "a");
    let old = w.read_stamp(a).unwrap();
    w.resident_mut(old).unwrap().repository.history.selected = Some("loaded-patch".into());
    assert!(w.finish_activation(old));
    assert!(matches!(
        w.get(a).unwrap().session().residency(),
        Residency::Ready { .. }
    ));
    let b = open(&mut w, "b");
    assert!(matches!(
        w.get(a).unwrap().session().residency(),
        Residency::Cold { .. }
    ));
    assert!(w.resident_mut(old).is_none());
    assert!(!w.finish_activation(old));
    w.activate(a).unwrap();
    assert!(!w.accepts_read(old));
    let new = w.read_stamp(a).unwrap();
    assert!(
        w.resident_mut(new)
            .unwrap()
            .repository
            .history
            .selected
            .is_none()
    );
    assert!(matches!(
        w.get(b).unwrap().session().residency(),
        Residency::Cold { .. }
    ));
    assert!(w.accepts_lifetime(old.lifetime));
}
#[test]
fn parking_and_view_changes_reject_old_results_even_when_tab_remains_active() {
    let mut w = window();
    let a = open(&mut w, "a");
    let old = w.read_stamp(a).unwrap();
    w.park(a).unwrap();
    assert_eq!(w.active(), Some(a));
    assert!(!w.accepts_read(old));
    w.activate(a).unwrap();
    let next = w.read_stamp(a).unwrap();
    w.save_bookmark(a, bookmark(ThreadView::Files)).unwrap();
    assert!(!w.accepts_read(next));
    assert!(w.read_stamp(a).is_some());
}
#[test]
fn close_inactive_preserves_focus_close_active_uses_right_then_left_and_last_is_empty() {
    let mut w = window();
    let a = open(&mut w, "a");
    let b = open(&mut w, "b");
    let c = open(&mut w, "c");
    w.activate(b).unwrap();
    let stamp = w.read_stamp(b).unwrap();
    w.close(a, false).unwrap();
    assert_eq!(w.active(), Some(b));
    assert!(w.accepts_read(stamp));
    w.close(b, false).unwrap();
    assert_eq!(w.active(), Some(c));
    assert!(!w.accepts_lifetime(stamp.lifetime));
    let d = open(&mut w, "d");
    w.close(d, false).unwrap();
    assert_eq!(w.active(), Some(c));
    w.close(c, false).unwrap();
    assert!(w.tabs().is_empty());
    assert_eq!(w.active(), None);
    assert_eq!(w.close(c, false), Err(TabError::MissingTab));
}
#[test]
fn closing_running_and_approval_tabs_does_not_change_host_summaries_or_archive() {
    for status in [ThreadStatus::Running, ThreadStatus::AwaitingApproval] {
        let catalog = vec![summary("a", status)];
        let before = catalog.clone();
        let mut w = window();
        let a = open(&mut w, "a");
        w.close(a, false).unwrap();
        assert_eq!(catalog, before);
        assert_eq!(w.closed_len(), 1);
    }
}
#[test]
fn reopen_restores_position_and_bookmark_with_fresh_identity() {
    let mut w = window();
    let a = open(&mut w, "a");
    let b = open(&mut w, "b");
    let c = open(&mut w, "c");
    w.activate(b).unwrap();
    w.save_bookmark(b, bookmark(ThreadView::History)).unwrap();
    let old = w.read_stamp(b).unwrap();
    w.close(b, false).unwrap();
    let reopened = w.reopen(|_| Some(Some(1))).unwrap().unwrap();
    assert_eq!(ids(&w), [a, reopened, c]);
    assert_eq!(w.active(), Some(reopened));
    assert_ne!(b, reopened);
    assert!(!w.accepts_read(old));
    assert!(!w.accepts_lifetime(old.lifetime));
    assert_eq!(
        w.get(reopened).unwrap().session().bookmark().view(),
        ThreadView::History
    );
}
#[test]
fn reopen_skips_missing_and_deduplicates_newly_opened_tabs_without_overwriting_them() {
    let mut w = window();
    let a = open(&mut w, "a");
    w.close(a, false).unwrap();
    let b = open(&mut w, "b");
    w.close(b, false).unwrap();
    let next_a = open(&mut w, "a");
    w.save_bookmark(next_a, bookmark(ThreadView::Files))
        .unwrap();
    assert_eq!(
        w.reopen(|t| (t == &target("a")).then_some(None)).unwrap(),
        Some(next_a)
    );
    assert_eq!(w.tabs().len(), 1);
    assert_eq!(
        w.get(next_a).unwrap().session().bookmark().view(),
        ThreadView::Files
    );
    assert_eq!(w.reopen(|_| Some(None)).unwrap(), None);
}
#[test]
fn closed_history_is_twenty_and_empty_drafts_are_not_retained() {
    let mut w = window();
    let d = w.open(draft("empty"), Some(1)).unwrap();
    w.close(d, false).unwrap();
    assert_eq!(w.closed_len(), 0);
    let d = w.open(draft("images-or-text"), Some(1)).unwrap();
    w.close(d, true).unwrap();
    assert_eq!(w.closed_len(), 1);
    for i in 0..25 {
        let a = open(&mut w, &format!("t{i}"));
        w.close(a, false).unwrap();
    }
    assert_eq!(w.closed_len(), CLOSED_TAB_LIMIT);
    for i in (5..25).rev() {
        let a = w.reopen(|_| Some(Some(1))).unwrap().unwrap();
        assert_eq!(
            w.get(a).unwrap().session().target(),
            &target(&format!("t{i}"))
        );
    }
    assert_eq!(w.reopen(|_| Some(None)).unwrap(), None);
}
#[test]
fn reorder_and_cycles_preserve_identity_and_number_nine_selects_last() {
    let mut w = window();
    assert_eq!(w.next_tab().unwrap(), None);
    assert_eq!(w.previous_tab().unwrap(), None);
    let tabs = (0..12)
        .map(|i| open(&mut w, &format!("t{i}")))
        .collect::<Vec<_>>();
    let stamp = w.read_stamp(tabs[11]).unwrap();
    w.move_tab(tabs[11], 0).unwrap();
    assert!(w.accepts_read(stamp));
    assert_eq!(w.active(), Some(tabs[11]));
    assert_eq!(w.previous_tab().unwrap(), Some(tabs[10]));
    assert_eq!(w.next_tab().unwrap(), Some(tabs[11]));
    assert_eq!(w.jump(1).unwrap(), Some(tabs[11]));
    assert_eq!(w.jump(8).unwrap(), Some(tabs[6]));
    assert_eq!(w.jump(9).unwrap(), Some(tabs[10]));
    assert_eq!(w.jump(0).unwrap(), None);
    assert_eq!(w.jump(10).unwrap(), None);
    w.move_tab(tabs[11], usize::MAX).unwrap();
    assert_eq!(ids(&w).last(), Some(&tabs[11]));
}
#[test]
fn jumping_past_short_window_does_nothing_and_single_tab_cycle_keeps_stamp() {
    let mut w = window();
    assert_eq!(w.jump(9).unwrap(), None);
    let a = open(&mut w, "a");
    let stamp = w.read_stamp(a).unwrap();
    assert_eq!(w.jump(2).unwrap(), None);
    assert_eq!(w.jump(9).unwrap(), Some(a));
    assert_eq!(w.next_tab().unwrap(), Some(a));
    assert_eq!(w.previous_tab().unwrap(), Some(a));
    assert!(w.accepts_read(stamp));
}
#[test]
fn deletion_purges_open_and_closed_tabs_without_archiving_or_reopen() {
    let mut w = window();
    let a = open(&mut w, "a");
    w.close(a, false).unwrap();
    let b = open(&mut w, "b");
    let a = open(&mut w, "a");
    w.thread_deleted(&target("a").thread_key().unwrap())
        .unwrap();
    assert!(w.get(a).is_none());
    assert_eq!(w.active(), Some(b));
    assert_eq!(w.closed_len(), 0);
    w.thread_deleted(&target("missing").thread_key().unwrap())
        .unwrap();
    assert_eq!(ids(&w), [b]);
}
#[test]
fn project_removal_purges_only_exact_checkouts_and_their_closed_history() {
    let mut w = window();
    let a = open(&mut w, "a");
    w.close(a, false).unwrap();
    let a = open(&mut w, "a");
    let b = w
        .open(
            TabTarget::Existing {
                id: "b".into(),
                checkout: checkout("/other"),
            },
            Some(1),
        )
        .unwrap();
    w.project_removed(&[checkout("/repo")]).unwrap();
    assert!(w.get(a).is_none());
    assert_eq!(ids(&w), [b]);
    assert_eq!(w.closed_len(), 0);
}
#[test]
fn disconnect_keeps_identity_bookmarks_and_order_but_reconnect_loads_only_active() {
    let mut w = window();
    let a = open(&mut w, "a");
    let b = open(&mut w, "b");
    let stamp = w.read_stamp(b).unwrap();
    w.save_bookmark(a, bookmark(ThreadView::Changes)).unwrap();
    w.disconnected(&host());
    assert_eq!(ids(&w), [a, b]);
    assert_eq!(w.active(), Some(b));
    assert!(!w.accepts_read(stamp));
    assert!(w.accepts_lifetime(stamp.lifetime));
    assert!(
        w.tabs()
            .iter()
            .all(|t| matches!(t.session().residency(), Residency::Cold { .. }))
    );
    w.reconnected(
        &HostKey {
            id: "wrong".into(),
            ..host()
        },
        2,
    )
    .unwrap();
    assert!(w.read_stamp(b).is_none());
    w.reconnected(&host(), 2).unwrap();
    assert_eq!(w.read_stamp(b).unwrap().connection, 2);
    assert!(!w.accepts_read(stamp));
    assert!(w.read_stamp(a).is_none());
    assert_eq!(
        w.get(a).unwrap().session().bookmark().view(),
        ThreadView::Changes
    );
}
#[test]
fn windows_do_not_accept_each_others_lifetimes() {
    let mut a = window();
    let mut b = WindowTabs::new(WindowId(8));
    let id = open(&mut a, "a");
    open(&mut b, "a");
    assert!(!b.accepts_lifetime(a.get(id).unwrap().lifetime()));
    assert!(!b.accepts_read(a.read_stamp(id).unwrap()));
}
#[test]
fn limits_and_invalid_operations_preserve_the_window() {
    let mut w = window();
    assert_eq!(w.open(target(""), None), Err(TabError::InvalidIdentity));
    for i in 0..300 {
        open(&mut w, &format!("t{i}"));
    }
    assert!(w.open(target("beyond-300"), Some(1)).is_ok());
    let before = ids(&w);
    assert_eq!(w.activate(TabId(0)), Err(TabError::MissingTab));
    assert_eq!(w.move_tab(TabId(0), 0), Err(TabError::MissingTab));
    w.serial = u64::MAX;
    assert_eq!(w.activate(before[0]), Err(TabError::IdentityExhausted));
    assert_eq!(ids(&w), before);
}
#[test]
fn cold_budget_counts_capacity_not_only_length() {
    let mut w = window();
    let mut oversized = target("a");
    if let TabTarget::Existing { id, .. } = &mut oversized {
        id.reserve(COLD_TAB_BYTES);
    }
    assert_eq!(w.open(oversized, None), Err(TabError::BookmarkTooLarge));
    let a = open(&mut w, "a");
    assert!(w.get(a).unwrap().cold_bytes() <= COLD_TAB_BYTES);
}
#[test]
fn bookmarks_round_trip_all_views_and_semantic_locations() {
    for view in [
        ThreadView::Conversation,
        ThreadView::Changes,
        ThreadView::History,
        ThreadView::Files,
    ] {
        let b = rich_bookmark(view);
        let saved = BookmarkRecord::capture(&b).unwrap();
        assert_eq!(saved.inline(), Some(b.clone()));
        let mut w = window();
        let a = open(&mut w, "a");
        w.save_bookmark(a, saved.clone()).unwrap();
        open(&mut w, "b");
        w.activate(a).unwrap();
        assert_eq!(w.get(a).unwrap().session().bookmark(), &saved);
        assert_eq!(decode_extended(&encode_extended(&b).unwrap()), Some(b));
    }
}
pub(super) fn rich_bookmark(view: ThreadView) -> ThreadBookmark {
    let position = TextPosition {
        source: "message-1".into(),
        block: 2,
        byte: 7,
    };
    let anchor = PatchAnchor {
        fingerprint: "sha-123".into(),
        hunk: 3,
        line: 6,
        offset_millipoints: 500,
        horizontal_millipoints: 0,
    };
    ThreadBookmark {
        view,
        conversation: ConversationBookmark {
            scroll: ConversationScroll::Anchored {
                row: "message-1".into(),
                offset_millipoints: 1234,
            },
            before: Some("cursor".into()),
            pages: 3,
            find: FindBookmark {
                open: true,
                query: "needle".into(),
                current: Some(position.clone()),
            },
        },
        changes: ChangesBookmark {
            path: Some("src/main.rs".into()),
            staged: true,
            scope: ChangeScope::Recorded {
                record: "r1".into(),
                original: "r0".into(),
                compare_current: true,
            },
            anchor: Some(anchor.clone()),
            selection: Some((3, 5)),
            list_offset_millipoints: 4200,
        },
        history: HistoryBookmark {
            commit: Some("a".repeat(40)),
            anchor: Some(anchor),
            list_offset_millipoints: 2500,
        },
        files: FileBookmark {
            path: Some("src/main.rs".into()),
            line: 90,
            column: 4,
            offset_millipoints: 800,
            horizontal_millipoints: 0,
            find: FindBookmark {
                open: true,
                query: "match".into(),
                current: None,
            },
        },
        draft: DraftBookmark {
            revision: 7,
            cursor: 5,
            selection_end: 12,
        },
        focus: FocusTarget::FileFind,
        panels: PanelBookmark {
            changed_files_collapsed: true,
            context_open: true,
            approval_open: true,
            enlarged_image: Some("image-id".into()),
            undo_conflict_record: Some("record-id".into()),
            selected_turn: Some("turn-id".into()),
        },
        details: ViewDetails {
            expanded: vec!["tool-1".into()],
            folded: vec!["group-2".into()],
            selection: Some((position.clone(), position)),
            commit_message: "Fix parser".into(),
        },
    }
}
#[test]
fn large_bookmarks_require_explicit_storage_and_keep_full_payload_in_codec() {
    let mut b = rich_bookmark(ThreadView::Changes);
    b.details.commit_message = "x".repeat(32 * 1024);
    assert_eq!(
        BookmarkRecord::capture(&b),
        Err(BookmarkError::NeedsStorage)
    );
    assert_eq!(decode_extended(&encode_extended(&b).unwrap()), Some(b));
    let stored = BookmarkRecord::stored(ThreadView::Changes, "opaque-123".into()).unwrap();
    assert_eq!(stored.inline(), None);
    assert!(stored.valid());
    for path in ["../escape", "..", ".", "/absolute", "", "a/b"] {
        assert_eq!(
            BookmarkRecord::stored(ThreadView::Files, path.into()),
            Err(BookmarkError::Invalid)
        );
    }
}
#[test]
fn invalid_bookmarks_are_rejected_without_replacing_saved_state() {
    let mut w = window();
    let a = open(&mut w, "a");
    let old = w.get(a).unwrap().session().bookmark().clone();
    let bad = BookmarkRecord::Inline {
        view: ThreadView::Files,
        json: "{}".into(),
    };
    assert_eq!(w.save_bookmark(a, bad), Err(TabError::InvalidBookmark));
    assert_eq!(w.get(a).unwrap().session().bookmark(), &old);
    for path in ["../x", "/x", "x/../y", "x//y", "./x", "x\0y"] {
        let mut b = ThreadBookmark::default();
        b.files.path = Some(path.into());
        assert_eq!(BookmarkRecord::capture(&b), Err(BookmarkError::Invalid));
    }
    assert!(decode_extended(&vec![b' '; EXTENDED_BOOKMARK_LIMIT + 1]).is_none());
    assert!(decode_extended(b"{\"conversation\":{\"pages\":65535}}").is_none());
}

#[test]
fn removing_many_project_tabs_selects_original_right_survivor() {
    let mut w = window();
    let a = open(&mut w, "a");
    let external = |id: &str| TabTarget::Existing {
        id: id.into(),
        checkout: checkout("/other"),
    };
    w.open(external("b"), Some(1)).unwrap();
    let c = open(&mut w, "c");
    open(&mut w, "d");
    let e = w.open(external("e"), Some(1)).unwrap();
    w.open(external("f"), Some(1)).unwrap();
    w.activate(c).unwrap();
    w.project_removed(&[checkout("/repo")]).unwrap();
    assert!(w.get(a).is_none());
    assert_eq!(w.active(), Some(e));
}

#[test]
fn mixed_transition_sequences_keep_unique_content_and_at_most_one_resident() {
    let mut w = window();
    let mut random = 7_u64;
    for _ in 0..2000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let name = format!("thread{}", random % 17);
        match (random >> 32) % 10 {
            0..=2 => {
                let _ = w.open(target(&name), Some(1));
            }
            3 => {
                if let Some(id) = w.active() {
                    w.close(id, false).unwrap();
                }
            }
            4 => {
                w.reopen(|_| Some(Some(1))).unwrap();
            }
            5 => {
                w.next_tab().unwrap();
            }
            6 => {
                w.previous_tab().unwrap();
            }
            7 => {
                w.jump((random % 11) as usize).unwrap();
            }
            8 => {
                if let Some(id) = w.active() {
                    w.move_tab(id, (random % 20) as usize).unwrap();
                }
            }
            _ => {
                w.thread_deleted(&target(&name).thread_key().unwrap())
                    .unwrap();
            }
        }
        assert_eq!(w.active().is_none(), w.tabs().is_empty());
        assert!(w.active().is_none_or(|id| w.get(id).is_some()));
        assert!(w.closed_len() <= CLOSED_TAB_LIMIT);
        for (index, tab) in w.tabs().iter().enumerate() {
            assert!(tab.cold_bytes() <= COLD_TAB_BYTES);
            assert!(w.tabs()[index + 1..].iter().all(|other| {
                !tab.session()
                    .target()
                    .same_content(other.session().target())
            }));
            if tab.session().residency.visibility().is_some() {
                assert_eq!(w.active(), Some(tab.id()));
            }
        }
    }
}

#[test]
fn extended_payload_and_collection_limits_are_enforced() {
    let b = ThreadBookmark {
        details: ViewDetails {
            expanded: vec!["a".repeat(128); 4096],
            folded: vec!["b".repeat(128); 4096],
            commit_message: "x".repeat(256 * 1024),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(encode_extended(&b), Err(BookmarkError::TooLarge));
    let b = ThreadBookmark {
        details: ViewDetails {
            expanded: vec!["a".into(); 4097],
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(encode_extended(&b), Err(BookmarkError::Invalid));
}
#[test]
fn reopen_at_capacity_activates_an_already_open_thread() {
    let mut w = window();
    for i in 0..300 {
        open(&mut w, &format!("t{i}"));
    }
    let first = ids(&w)[0];
    w.close(first, false).unwrap();
    let again = open(&mut w, "t0");
    w.activate(ids(&w)[0]).unwrap();
    assert_eq!(w.reopen(|_| Some(Some(1))), Ok(Some(again)));
    assert_eq!(w.active(), Some(again));
    // Reopening beyond the old capacity also succeeds.
    let last = *ids(&w).last().unwrap();
    w.close(last, false).unwrap();
    open(&mut w, "full");
    assert!(w.reopen(|_| Some(Some(1))).unwrap().is_some());
    assert_eq!(w.closed_len(), 0);
}
#[test]
fn closing_a_thread_again_replaces_its_older_history_entry() {
    let mut w = window();
    let a = open(&mut w, "a");
    let b = open(&mut w, "b");
    w.close(b, false).unwrap();
    let b = open(&mut w, "b");
    w.save_bookmark(b, bookmark(ThreadView::History)).unwrap();
    w.close(b, false).unwrap();
    assert_eq!(w.closed_len(), 1);
    let b = w.reopen(|_| Some(Some(1))).unwrap().unwrap();
    assert_eq!(
        w.get(b).unwrap().session().bookmark().view(),
        ThreadView::History
    );
    // Nothing else was closed, so a second reopen must not just refocus b.
    w.activate(a).unwrap();
    assert_eq!(w.reopen(|_| Some(Some(1))), Ok(None));
    assert_eq!(w.active(), Some(a));
}

#[test]
fn verifying_a_session_host_preserves_order_bookmarks_and_closed_history() {
    let mut w = window();
    let provisional = HostKey {
        id: "session-1".into(),
        store: "/".into(),
    };
    let target = |id: &str| TabTarget::Existing {
        id: id.into(),
        checkout: CheckoutKey {
            host: provisional.clone(),
            root: "/checkout".into(),
            directory: None,
        },
    };
    let a = w.open(target("a"), Some(1)).unwrap();
    let bookmark = BookmarkRecord::capture(&rich_bookmark(ThreadView::Files)).unwrap();
    w.save_bookmark(a, bookmark.clone()).unwrap();
    let old = w.read_stamp(a).unwrap();
    let b = w.open(target("b"), Some(1)).unwrap();
    w.close(b, false).unwrap();
    let checkout = CheckoutKey {
        host: host(),
        root: "/checkout".into(),
        directory: Some(DirectoryIdentity {
            device: 1,
            inode: 2,
        }),
    };
    w.verify_session_host(&provisional, &host(), std::slice::from_ref(&checkout))
        .unwrap();
    assert_eq!(w.active(), Some(a));
    assert_eq!(w.get(a).unwrap().session().bookmark(), &bookmark);
    assert_eq!(w.get(a).unwrap().session().target().checkout(), &checkout);
    assert!(!w.accepts_read(old));
    let reopened = w
        .reopen(|target| (target.checkout() == &checkout).then_some(Some(1)))
        .unwrap()
        .unwrap();
    assert_ne!(reopened, b);
}

#[test]
fn worktree_creation_only_binds_the_expected_draft_and_never_steals_focus() {
    let mut w = window();
    let a = w.open(draft("draft"), Some(1)).unwrap();
    let other = open(&mut w, "other");
    let original = checkout("/repo");
    let target = TabTarget::Existing {
        id: "isolated".into(),
        checkout: checkout("/repo.worktrees/thread"),
    };
    assert_eq!(
        w.bind_worktree_created(a, &checkout("/wrong"), target.clone()),
        Err(TabError::IdentityMismatch)
    );
    let mut foreign = target.clone();
    if let TabTarget::Existing { checkout, .. } = &mut foreign {
        checkout.host.id = "different-host".into();
    }
    assert_eq!(
        w.bind_worktree_created(a, &original, foreign),
        Err(TabError::IdentityMismatch)
    );
    let mut unknown = target.clone();
    if let TabTarget::Existing { checkout, .. } = &mut unknown {
        checkout.directory = None;
    }
    assert_eq!(
        w.bind_worktree_created(a, &original, unknown),
        Err(TabError::IdentityMismatch)
    );
    assert_eq!(
        w.bind_worktree_created(other, &original, target.clone()),
        Err(TabError::InvalidIdentity)
    );
    w.bind_worktree_created(a, &original, target.clone())
        .unwrap();
    assert_eq!(w.active(), Some(other));
    assert_eq!(w.get(a).unwrap().session().target(), &target);
    assert_eq!(
        w.bind_worktree_created(a, &original, target),
        Err(TabError::InvalidIdentity)
    );
}

#[test]
fn three_hundred_drafts_close_in_every_order_without_sharing_identity() {
    for reverse in [false, true] {
        let mut w = window();
        let mut opened = Vec::new();
        for n in 0..300 {
            opened.push(w.open(draft(&format!("draft-{n}")), Some(1)).unwrap());
        }
        assert_eq!(w.tabs().len(), 300);
        assert_eq!(
            w.tabs()
                .iter()
                .filter(|t| t.session().residency.visibility().is_some())
                .count(),
            1
        );
        if reverse {
            opened.reverse();
        }
        for id in opened {
            w.close(id, true).unwrap();
        }
        assert_eq!(w.closed_len(), CLOSED_TAB_LIMIT);
        for _ in 0..CLOSED_TAB_LIMIT {
            assert!(w.reopen(|_| Some(Some(1))).unwrap().is_some());
        }
        assert_eq!(w.tabs().len(), CLOSED_TAB_LIMIT);
        assert!(w.reopen(|_| Some(Some(1))).unwrap().is_none());
    }
}

#[test]
fn closed_draft_creation_reopens_the_created_thread_without_stealing_focus() {
    let mut w = window();
    let a = w.open(draft("a"), Some(1)).unwrap();
    w.close(a, true).unwrap();
    let b = w.open(draft("b"), Some(1)).unwrap();
    w.bind_draft_target(&draft("a"), target("created-a"))
        .unwrap();
    assert_eq!(w.active(), Some(b));
    let reopened = w.reopen(|_| Some(Some(1))).unwrap().unwrap();
    assert_eq!(
        w.get(reopened).unwrap().session().target(),
        &target("created-a")
    );
}

#[test]
fn moving_a_draft_keeps_its_id_and_siblings_and_reserves_read_generations() {
    let mut w = window();
    let a = w.open(draft("a"), Some(1)).unwrap();
    let b = w.open(draft("b"), Some(1)).unwrap();
    let before = w.read_stamp(b).unwrap();
    let mut checkout = draft("a").checkout().clone();
    checkout.root = "/other-project".into();
    w.move_draft(b, checkout.clone()).unwrap();
    assert_eq!(w.active(), Some(b));
    assert_eq!(w.get(a).unwrap().session().target(), &draft("a"));
    assert!(
        matches!(w.get(b).unwrap().session().target(), TabTarget::Draft { id, checkout: root } if id == "b" && root == &checkout)
    );
    assert!(!w.accepts_read(before));
    let target = w.get(b).unwrap().session().target().clone();
    w.serial = u64::MAX - 1;
    assert_eq!(
        w.move_draft(b, draft("b").checkout().clone()),
        Err(TabError::IdentityExhausted)
    );
    assert_eq!(w.get(b).unwrap().session().target(), &target);
}

#[test]
fn worktrees_share_every_window_transition_without_becoming_threads() {
    let mut w = window();
    let thread = open(&mut w, "thread");
    let trees = TabTarget::Worktrees {
        checkout: checkout("/repo"),
    };
    let tab = w.open(trees.clone(), Some(1)).unwrap();
    assert_eq!(w.open(trees.clone(), Some(1)).unwrap(), tab);
    assert!(w.get(tab).unwrap().content().view_commands().is_empty());
    let old = w.read_stamp(tab).unwrap();
    w.previous_tab().unwrap();
    assert_eq!(w.active(), Some(thread));
    assert!(!w.accepts_read(old));
    w.jump(9).unwrap();
    assert_eq!(w.active(), Some(tab));
    assert!(!w.accepts_read(old));
    w.move_tab(tab, 0).unwrap();
    w.next_tab().unwrap();
    w.activate(tab).unwrap();
    let closed = w.read_stamp(tab).unwrap();
    w.close(tab, false).unwrap();
    assert!(!w.accepts_read(closed));
    let reopened = w.reopen(|_| Some(Some(2))).unwrap().unwrap();
    assert_ne!(reopened, tab);
    assert_eq!(w.get(reopened).unwrap().session().target(), &trees);
    w.disconnected(&host());
    assert!(w.read_stamp(reopened).is_none());
    w.reconnected(&host(), 3).unwrap();
    assert!(w.read_stamp(reopened).is_some());
    assert!(w.bind_created(reopened, target("another")).is_err());
    assert!(w.move_draft(reopened, checkout("/elsewhere")).is_err());
    w.thread_deleted(&ThreadKey {
        host: host(),
        id: "thread".into(),
    })
    .unwrap();
    assert_eq!(w.tabs().len(), 1);
    w.project_removed(&[checkout("/repo")]).unwrap();
    assert!(w.tabs().is_empty());
}

#[test]
fn reopen_checkout_updates_all_matching_tabs() {
    let mut w = window();
    let a = open(&mut w, "a");
    let b = w.open(draft("b"), Some(1)).unwrap();
    let stamp = w.read_stamp(b).unwrap();
    let mut resolved = checkout("/repo");
    resolved.directory.as_mut().unwrap().inode = 99;
    w.reopen_checkout(&resolved).unwrap();
    assert!(!w.accepts_read(stamp));
    assert_eq!(w.get(a).unwrap().session().target().checkout(), &resolved);
    assert_eq!(w.get(b).unwrap().session().target().checkout(), &resolved);
}

#[test]
fn work_view_belongs_to_the_thread_tab() {
    let mut tabs = window();
    let a = open(&mut tabs, "a");
    tabs.work_mut().unwrap().toggle(900., Some(true)).unwrap();
    let b = open(&mut tabs, "b");
    assert!(!tabs.work().unwrap().open);
    tabs.work_mut().unwrap().toggle(900., Some(false)).unwrap();
    let tree = tabs.work_mut().unwrap().tree.as_mut().unwrap();
    let divider = tree.layout(900., 500.).dividers[0].id;
    tree.resize_divider(divider, 500., 900., 500.).unwrap();
    for (id, width) in [(a, 420.), (b, 500.)] {
        tabs.activate(id).unwrap();
        let layout = tabs.work().unwrap().layout(900., 900., 500.).unwrap();
        assert!((layout.panes[0].rect.width - width).abs() < 0.001);
    }
    tabs.open(
        TabTarget::Worktrees {
            checkout: checkout("/repo"),
        },
        Some(1),
    )
    .unwrap();
    assert!(tabs.work_mut().is_none());
}
