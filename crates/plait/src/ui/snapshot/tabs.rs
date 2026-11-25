//! Injected tab states; no fixture data reaches the shipped view.
use super::*;
use plait_client::tabs::{CheckoutKey, TabTarget};

pub(super) fn show_tabs(plait: &mut Plait) {
    plait.preferences.value.thread_tabs = Some(plait_client::preferences::ThreadTabs::Show);
    for project in &mut plait.state.projects {
        project.machines[0].name = "studio".into();
    }
}

pub(super) fn install(harness: &mut Harness, count: usize) {
    install_projects(harness, count, false);
}

fn install_projects(harness: &mut Harness, count: usize, projects: bool) {
    harness.update(move |plait, _, _| {
        show_tabs(plait);
        plait.reset_tabs_for_test();
        let template = plait
            .host
            .connection
            .threads()
            .first()
            .cloned()
            .unwrap_or_default();
        plait.host.connection.threads_mut().clear();
        for index in 0..count {
            let mut thread = template.clone();
            thread.id = if index == 0 {
                THREAD.into()
            } else {
                format!("tab-thread-{index}")
            };
            thread.title = Some(match index {
                0 => "Improve retry handling".into(),
                1 => "Run the parser tests".into(),
                2 => "Review dependency update".into(),
                _ => format!("Task {}: refine checkout handling", index + 1),
            });
            thread.status = match index % 7 {
                0 | 3 | 5 => ThreadStatus::Idle,
                1 => ThreadStatus::Running,
                2 => ThreadStatus::AwaitingApproval,
                4 => ThreadStatus::Error,
                _ => ThreadStatus::Stopped,
            };
            if projects && count == 3 && index > 0 {
                thread.checkout = thread.checkout.parent().unwrap().join(if index == 1 {
                    "tools"
                } else {
                    "runtime"
                });
            }
            if index == 6 {
                thread.status = ThreadStatus::Interrupted;
            }
            if index == 7 {
                thread.status = ThreadStatus::Stopped;
            }
            if count == 20 && (index == 10 || index == 11) {
                thread.status = ThreadStatus::Running;
            }
            thread.pending_approval = Some(thread.status == ThreadStatus::AwaitingApproval);
            if index == 3 {
                plait
                    .preferences
                    .value
                    .navigation
                    .unread
                    .insert(thread.id.clone());
            }
            let target = TabTarget::Existing {
                id: thread.id.clone(),
                checkout: CheckoutKey {
                    host: plait.tabs.host.clone(),
                    root: thread.checkout.clone(),
                    directory: None,
                },
            };
            plait
                .tabs
                .model
                .open(target, Some(plait.host.connection.epoch))
                .unwrap();
            plait.host.connection.threads_mut().push(thread);
        }
        if let Some(first) = plait.tabs.model.tabs().first() {
            let id = first.id();
            plait.tabs.model.activate(id).unwrap();
        }
        plait.refresh_tab_rows();
    });
}

pub(super) fn walk(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut h = Harness::new(repository, output, width, height);
    h.connect(repository, false);
    for (name, count, light) in [
        ("one", 1, true),
        ("three", 3, true),
        ("states", 8, true),
        ("eight", 8, false),
        ("twenty", 20, false),
        ("overflow-open", 20, true),
    ] {
        install_projects(&mut h, count, true);
        h.update(move |p, _, _| {
            p.tabs.list_open = name == "overflow-open";
            if name == "twenty" {
                let id = p.tabs.model.tabs()[12].id();
                p.tabs.model.activate(id).unwrap();
                p.refresh_tab_rows();
            }
        });
        h.capture(&format!("tabs-{name}-{suffix}"));
        if light {
            h.update(|p, _, cx| {
                p.themes.forced = Some(true);
                p.apply_theme(cx);
            });
            h.capture(&format!("light-tabs-{name}-{suffix}"));
            h.update(|p, _, cx| {
                p.themes.forced = Some(false);
                p.apply_theme(cx);
            });
        }
    }
    install(&mut h, 3);
    h.update(|p, _, _| {
        p.tabs.list_open = false;
        p.sidebar = false;
    });
    h.capture(&format!("tabs-no-sidebar-{suffix}"));
    h.update(|p, _, _| {
        p.reset_tabs_for_test();
        p.host.select(None);
        p.draft.open = false;
        p.sidebar = true;
    });
    h.capture(&format!("tabs-empty-{suffix}"));
    h.connect(repository, false);
    install(&mut h, 3);
    for (name, view) in [
        ("conversation", super::super::Workspace::Conversation),
        ("changes", super::super::Workspace::Changes),
        ("history", super::super::Workspace::History),
        ("files", super::super::Workspace::Files),
    ] {
        h.update(|p, w, cx| {
            p.sidebar = true;
            p.show_tab(view, w, cx);
            if name == "files" {
                p.open_file("src/review.rs".into(), Some(2), cx);
            }
        });
        h.capture(&format!("tabs-all-views-restored-{name}-{suffix}"));
    }
    h.update(|p, _, _| {
        p.workspace.tab = super::super::Workspace::Conversation;
        p.workspace.history = Default::default();
        p.workspace.formatted = Default::default();
        p.host.thread.snapshot = None;
        p.refresh_live_list(None);
    });
    h.capture(&format!("tabs-cold-loading-{suffix}"));
    h.update(|p, _, _| {
        p.notice = Some(
            "The saved view is unavailable. Showing this thread from its latest messages.".into(),
        )
    });
    h.capture(&format!("tabs-bookmark-evicted-{suffix}"));
    h.connect(repository, false);
    install(&mut h, 3);
    h.update(|p, _, _| {
        p.notice = None;
        let first = p.tabs.model.tabs()[0].id();
        p.tabs.model.move_tab(first, 2).unwrap();
        p.tabs.model.activate(first).unwrap();
        p.workspace.tab = super::super::Workspace::Conversation;
        p.host.connection.connected = false;
        p.tabs.model.disconnected(&p.tabs.host);
    });
    h.capture(&format!("tabs-restored-offline-{suffix}"));
    h.update(|p, _, _| {
        p.notice =
            Some("Tabs not saved: storage is unavailable. Previous saved tabs are intact.".into())
    });
    h.capture(&format!("tabs-save-failed-{suffix}"));
    h.update(|p, w, cx| {
        p.notice = None;
        p.open_draft(w, cx);
        p.draft.composer.as_ref().unwrap().update(cx, |c, cx| {
            c.restore_draft(
                "Continue checking the checkout after restart.".into(),
                3,
                None,
                cx,
            )
        });
    });
    h.capture(&format!("tabs-recovered-draft-{suffix}"));
    super::many_drafts::scenes(&mut h, suffix);
}

pub(super) fn exercise() {
    let scratch = std::env::temp_dir().join(format!("plait-tab-actions-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 3);
    // Use offline data so selection does not start a real host watch.
    h.update(|p, _, _| {
        p.host.connection.connected = false;
    });
    h.draw();
    h.keystroke("cmd-2");
    h.update(|p, _, _| {
        assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[1].id()));
    });
    h.keystroke("cmd-shift-]");
    h.update(|p, _, _| {
        assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[2].id()));
    });
    h.keystroke("ctrl-shift-left");
    h.update(|p, _, _| {
        assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[1].id()));
    });
    h.keystroke("cmd-w");
    h.update(|p, _, _| {
        assert_eq!(p.tabs.model.tabs().len(), 2);
        assert_eq!(p.tabs.model.closed_len(), 1);
    });
    h.keystroke("cmd-shift-t");
    h.update(|p, _, _| {
        assert_eq!(p.tabs.model.tabs().len(), 3);
        assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[1].id()));
    });
    h.keystroke("cmd-shift-\\");
    h.update(|p, _, _| assert!(p.tabs.list_open));
    h.keystroke("down");
    h.keystroke("enter");
    h.update(|p, _, _| assert!(!p.tabs.list_open));
    h.keystroke("cmd-9");
    h.update(|p, _, _| {
        assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[2].id()));
    });
    h.keystroke("cmd-w");
    h.keystroke("cmd-w");
    h.keystroke("cmd-w");
    h.update(|p, _, _| {
        assert!(p.tabs.model.tabs().is_empty());
        assert!(p.host.thread.selected.is_none());
    });
    let _ = std::fs::remove_dir_all(scratch);
}

pub(super) fn cold_workspaces() {
    let scratch = std::env::temp_dir().join(format!("plait-cold-tabs-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 3);
    h.update(|p, _, cx| {
        p.host.connection.connected = false;
        p.workspace.composers[THREAD].update(cx, |c, cx| {
            c.restore_draft("Draft survives parking".into(), 9, None, cx)
        });
    });
    for view in [
        super::super::Workspace::Conversation,
        super::super::Workspace::Changes,
        super::super::Workspace::History,
        super::super::Workspace::Files,
    ] {
        let old = h
            .window
            .update(&mut h.cx, |p, w, cx| {
                p.show_tab(view, w, cx);
                p.workspace.thread_files.collapsed = true;
                p.workspace.thread_controls.usage_open = true;
                p.workspace
                    .commit_message
                    .update(cx, |f, cx| f.set_text("Preserve this commit message", cx));
                p.restore_find_bookmark(
                    super::super::file_view::FindIn::Conversation,
                    &plait_client::tabs::bookmark::FindBookmark {
                        open: true,
                        query: "queue".into(),
                        current: None,
                    },
                    cx,
                );
                (
                    p.host.read_stamp(),
                    p.tabs
                        .model
                        .read_stamp(p.tabs.model.active().unwrap())
                        .unwrap(),
                )
            })
            .unwrap();
        h.draw();
        h.keystroke("cmd-2");
        h.update(|p, _, _| {
            assert!(!p.host.accepts_read(&old.0));
            assert!(!p.tabs.model.accepts_read(old.1));
            assert_eq!(p.workspace.composers.len(), 1);
            assert!(p.workspace.formatted.messages.is_empty());
            assert!(p.workspace.history.cache.entries.is_empty());
            assert!(p.workspace.host_watch.is_none());
        });
        h.draw();
        h.keystroke("cmd-1");
        h.update(|p, _, cx| {
            assert_eq!(p.selected_view(), view);
            assert_eq!(
                p.workspace.composers[THREAD].read(cx).saved_draft().0,
                "Draft survives parking"
            );
            assert!(p.workspace.thread_files.collapsed && p.workspace.thread_controls.usage_open);
            assert_eq!(
                p.workspace.commit_message.read(cx).text(),
                "Preserve this commit message"
            );
            assert_eq!(
                p.workspace
                    .conversation_find
                    .field
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .text(),
                "queue"
            );
            assert!(!p.host.accepts_read(&old.0));
        });
    }
    h.draw();
    h.keystroke("cmd-w");
    h.draw();
    h.keystroke("cmd-shift-t");
    h.update(|p, _, cx| {
        assert_eq!(
            p.workspace.composers[THREAD].read(cx).saved_draft().0,
            "Draft survives parking"
        )
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

pub(super) fn restore_preferences() {
    use plait_client::{
        session::Journal,
        tabs::{CheckoutKey, HostKey, persistence, storage},
    };
    let scratch = std::env::temp_dir().join(format!("plait-tab-restore-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let saved_path = scratch.join("desktop/tabs.json");
    let (journal, data) = Journal::open(&scratch.join("session"), "/tab-test-host".into()).unwrap();
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 3);
    let host = HostKey {
        id: "verified-test-host".into(),
        store: "/tab-test-host".into(),
    };
    let checkout = CheckoutKey {
        host: host.clone(),
        root: repository.clone(),
        directory: super::super::tab_identity::directory(&repository),
    };
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.install_tab_identity(host.clone(), vec![checkout.clone()], cx);
        p.session.journal = Some(journal.clone());
        p.session.data = Some(data);
        p.tabs.identity.catalog_ready = true;
        p.tabs.persistence.requested = true;
        p.tabs.persistence.loaded = true;
        p.tabs.persistence.restored = true;
        p.tabs.persistence.writable = true;
        p.tabs.persistence.path = Some(saved_path.clone());
        p.tabs.persistence.window = Some(w.window_handle());
        p.workspace.composers[THREAD].update(cx, |c, cx| {
            c.restore_draft("Saved unsent draft".into(), 5, None, cx)
        });
        p.capture_session(cx);
        p.workspace.tab = super::super::Workspace::Files;
        p.workspace.file_viewer.path = Some("src/review.rs".into());
    });
    h.draw();
    let revision = h
        .window
        .update(&mut h.cx, |p, _, _| p.tabs.persistence.save_revision())
        .unwrap();
    h.pointer(PlatformInput::Scroll(ScrollEvent {
        position: point(px(500.), px(200.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(100.))),
        ..Default::default()
    }));
    h.update(|p, _, _| assert!(p.tabs.persistence.save_revision() > revision));
    let previous = h
        .window
        .update(&mut h.cx, |p, w, cx| {
            let lifetime = p
                .tabs
                .model
                .get(p.tabs.model.active().unwrap())
                .unwrap()
                .lifetime();
            let save = p.flush_tab_preferences(Some(w), cx).unwrap();
            (lifetime, save)
        })
        .unwrap();
    previous.1.write().unwrap();
    let bytes = storage::read(&saved_path).unwrap().unwrap();
    // A delayed writer cannot replace the most recently requested view.
    let old = h
        .window
        .update(&mut h.cx, |p, w, cx| {
            p.workspace.tab = super::super::Workspace::History;
            p.flush_tab_preferences(Some(w), cx).unwrap()
        })
        .unwrap();
    let newest = h
        .window
        .update(&mut h.cx, |p, w, cx| {
            p.workspace.tab = super::super::Workspace::Files;
            p.flush_tab_preferences(Some(w), cx).unwrap()
        })
        .unwrap();
    newest.write().unwrap();
    old.write().unwrap();
    assert_eq!(storage::read(&saved_path).unwrap().unwrap(), bytes);
    h.update(|p, w, cx| {
        p.tabs.persistence.restored = false;
        p.tabs.persistence.edited = false;
        p.tabs.persistence.bytes = Some(bytes);
        p.host.connection.epoch += 1;
        p.try_restore_tabs(w, cx);
        assert!(!p.tabs.persistence.restored);
        assert!(!p.tab_identity_current());
        p.install_tab_identity(host.clone(), vec![checkout.clone()], cx);
        p.try_restore_tabs(w, cx);
        assert_eq!(p.tabs.model.tabs().len(), 3);
        assert_eq!(p.selected_view(), super::super::Workspace::Conversation);
        assert!(!p.panel_is_open());
        assert_eq!(p.workspace.file_viewer.path, Some("src/review.rs".into()));
        assert!(!p.tabs.model.accepts_lifetime(previous.0));
        assert_eq!(p.workspace.composers.len(), 1);
        assert_eq!(
            p.workspace.composers[THREAD].read(cx).saved_draft().0,
            "Saved unsent draft"
        );
    });
    let saved = storage::read(&saved_path).unwrap().unwrap();
    h.update(|p, w, cx| {
        // Choosing a tab before restore completes keeps that choice in front.
        let chosen = p.tabs.model.tabs()[1].id();
        p.tabs.model.activate(chosen).unwrap();
        p.tabs.persistence.restored = false;
        p.tabs.persistence.edited = true;
        p.tabs.persistence.bytes = Some(saved.clone());
        p.try_restore_tabs(w, cx);
        assert_eq!(p.tabs.model.active(), Some(chosen));
        assert_eq!(p.tabs.model.tabs().len(), 3);
        // Closing every tab before restore finishes is also an explicit choice.
        let ids: Vec<_> = p.tabs.model.tabs().iter().map(|t| t.id()).collect();
        for id in ids {
            p.tabs.model.close(id, false).unwrap();
        }
        p.tabs.persistence.restored = false;
        p.tabs.persistence.bytes = Some(saved);
        p.try_restore_tabs(w, cx);
        assert!(p.tabs.model.tabs().is_empty());
    });
    for damaged in [b"{".as_slice(), b"{\"schema_version\":1,\"tabs\":false}"] {
        h.update(|p, w, cx| {
            p.tabs.persistence.restored = false;
            p.tabs.persistence.edited = false;
            p.tabs.persistence.bytes = Some(damaged.to_vec());
            p.try_restore_tabs(w, cx);
            assert!(p.tabs.model.tabs().is_empty());
            assert!(p.host.thread.selected.is_none());
        });
    }
    let future = b"{\"schema_version\":99,\"tabs\":[]}";
    storage::write(&saved_path, future).unwrap();
    h.update(|p, w, cx| {
        p.tabs.persistence.restored = false;
        p.tabs.persistence.writable = true;
        p.tabs.persistence.bytes = Some(future.to_vec());
        p.try_restore_tabs(w, cx);
        assert!(p.tabs.model.tabs().is_empty());
        assert!(!p.tabs.persistence.writable);
        assert!(p.flush_tab_preferences(Some(w), cx).is_none());
    });
    assert_eq!(storage::read(&saved_path).unwrap().unwrap(), future);
    // A nonempty draft restores even though no host thread has been created for it.
    let draft_path = scratch.join("desktop/draft-tabs.json");
    h.update(|p, w, cx| {
        p.tabs.persistence.writable = true;
        p.tabs.persistence.path = Some(draft_path.clone());
        p.open_draft(w, cx);
        p.draft.composer.as_ref().unwrap().update(cx, |c, cx| {
            c.restore_draft("Unsent first message".into(), 7, None, cx)
        });
        p.capture_session(cx);
    });
    h.window
        .update(&mut h.cx, |p, w, cx| {
            p.flush_tab_preferences(Some(w), cx).unwrap()
        })
        .unwrap()
        .write()
        .unwrap();
    let draft_bytes = storage::read(&draft_path).unwrap().unwrap();
    h.update(|p, w, cx| {
        p.tabs.persistence.restored = false;
        p.tabs.persistence.edited = false;
        p.tabs.persistence.bytes = Some(draft_bytes);
        p.try_restore_tabs(w, cx);
        assert_eq!(p.tabs.model.tabs().len(), 1);
        assert!(p.draft.open);
        assert!(p.workspace.composers.is_empty());
        assert_eq!(
            p.draft.composer.as_ref().unwrap().read(cx).saved_draft().0,
            "Unsent first message"
        );
    });
    // A snapshot remains decodable without a live connection or loaded workspace.
    let checkouts = vec![checkout];
    let summaries = h
        .window
        .update(&mut h.cx, |p, _, _| p.host.connection.threads().to_vec())
        .unwrap();
    let context = persistence::RestoreContext {
        host: &host,
        threads: &summaries,
        checkouts: &checkouts,
        drafts: &[],
        connection: None,
    };
    assert_eq!(
        persistence::decode(future, super::super::tabs::fresh_window_id(), &context).status,
        persistence::RestoreStatus::UnknownVersion
    );
    drop(h);
    drop(journal);
    std::fs::remove_dir_all(scratch).unwrap();
}

pub(super) fn journal_limit() {
    let scratch = std::env::temp_dir().join(format!("plait-tab-cap-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let directory = scratch.join("session");
    let (journal, data) =
        plait_client::session::Journal::open(&directory, "/tab-test-host".into()).unwrap();
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 20);
    h.update(|p, _, cx| {
        p.host.connection.connected = false;
        p.session.journal = Some(journal.clone());
        p.session.data = Some(data);
        p.capture_session(cx);
    });
    journal.flush().unwrap();
    let last_good = std::fs::read(directory.join("desktop.json")).unwrap();
    h.update(|p, w, cx| {
        fill_journal(p);
        journal.queue(p.session.data.clone().unwrap());
        p.capture_session(cx);
        let next = p.tabs.model.tabs()[1].id();
        p.request_tab(super::super::tab_lifecycle::Intent::Activate(next), w, cx);
    });
    super::undo::wait(&mut h, |p| !p.tabs.lifecycle.busy);
    h.update(|p, _, cx| {
        assert_eq!(p.tabs.model.active(), Some(p.tabs.model.tabs()[0].id()));
        assert_eq!(p.host.thread.selected.as_deref(), Some(THREAD));
        assert_eq!(p.workspace.composers.len(), 1);
        assert!(p.notice.as_deref().unwrap().contains("could not be saved"));
        // Kept in memory although the file could not take it: the 19 listed
        // threads and the fillers; the open thread had nothing to keep.
        assert_eq!(p.test_saved_threads().len(), 39);
        assert_eq!(
            p.test_saved_threads()["tab-thread-19"].draft.text.len(),
            256 * 1024
        );
        assert!(p.workspace.composers[THREAD].read(cx).text().is_empty());
    });
    assert!(journal.flush().unwrap_err().contains("exceeds"));
    assert_eq!(
        std::fs::read(directory.join("desktop.json")).unwrap(),
        last_good
    );
    drop(h);
    drop(journal);
    std::fs::remove_dir_all(scratch).unwrap();
}

pub(super) fn unloaded_views() {
    use plait_client::tabs::bookmark::{ConversationScroll, ThreadBookmark};
    let scratch = std::env::temp_dir().join(format!("plait-tab-views-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        let stamp = p.host.read_stamp();
        p.show_tab(super::super::Workspace::Changes, w, cx);
        assert!(
            p.host.accepts_read(&stamp),
            "opening a panel retains its conversation"
        );
        let mut saved = ThreadBookmark::default();
        saved.conversation.scroll = ConversationScroll::Anchored {
            row: "message:older".into(),
            offset_millipoints: 12000,
        };
        saved.conversation.before = Some("older-page".into());
        saved.changes.path = Some("src/review.rs".into());
        saved.history.commit = Some("a".repeat(40));
        saved.files.path = Some("src/review.rs".into());
        saved.files.line = 2;
        p.workspace.restore = Some(saved.clone());
        p.workspace.restore_pending = [true; 4];
        let captured = p.capture_tab_bookmark(Some(w), cx);
        assert_eq!(captured.conversation.scroll, saved.conversation.scroll);
        assert_eq!(captured.conversation.before, saved.conversation.before);
        assert_eq!(captured.changes, saved.changes);
        assert_eq!(captured.history, saved.history);
        assert_eq!(captured.files.path, saved.files.path);
        assert_eq!(captured.files.line, saved.files.line);
        p.open_file("src/review.rs".into(), Some(0), cx);
        assert!(!p.workspace.restore_pending[3]);
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A parked tab's late line counts never fill the changed-files card in front.
pub(super) fn parked_counts() {
    let scratch = std::env::temp_dir().join(format!("plait-tab-counts-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 3);
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.workspace.thread_files = Default::default();
        p.workspace
            .thread_files
            .inject(THREAD, repository.clone(), Vec::new());
        p.workspace
            .thread_files
            .set_paths(vec!["src/review.rs".into()]);
        // Counting needs a live host; the reply is Git's, read in the background.
        p.host.connection.connected = true;
        p.host.connection.status.as_mut().unwrap().protocol_version = 3;
        p.thread_files_status(cx);
        p.host.connection.connected = false;
        let next = p.tabs.model.tabs()[1].id();
        p.request_tab(super::super::tab_lifecycle::Intent::Activate(next), w, cx);
        assert_eq!(p.tabs.model.active(), Some(next));
        assert_eq!(p.workspace.thread_files.counted().count(), 0);
    });
    // Git answers within this; the control without switching sees the count arrive.
    std::thread::sleep(Duration::from_millis(500));
    h.settle();
    h.update(|p, _, _| {
        assert_eq!(p.workspace.thread_files.counted().count(), 0);
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A New thread tab in front shows its thread once created, whichever view is open.
pub(super) fn created_in_front() {
    let scratch = std::env::temp_dir().join(format!("plait-tab-created-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 1);
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.open_draft(w, cx);
        assert!(p.draft.open);
        let mut created = p.host.connection.threads()[0].clone();
        created.id = "created-thread".into();
        created.checkout = p.host.creation.checkout.clone();
        p.host.connection.threads_mut().push(created);
        p.draft.sending = Some(super::super::new_thread::Sending {
            generation: p.draft.generation,
            request_id: "create-1".into(),
            text: "First message".into(),
            images: Vec::new(),
            revision: 1,
            approval: None,
            choice: None,
        });
        p.show_tab(super::super::Workspace::Changes, w, cx);
        super::many_drafts::set_pending(p, "create-1");
        let reply = plait_client::host::ThreadReply {
            thread: Snapshot {
                id: "created-thread".into(),
                checkout: p.host.creation.checkout.clone(),
                model: choice(),
                ..Default::default()
            },
        };
        super::many_drafts::finish(p, "create-1", Ok(reply), w, cx);
        let active = p.tabs.model.get(p.tabs.model.active().unwrap()).unwrap();
        assert!(matches!(
            active.session().target(),
            TabTarget::Existing { id, .. } if id == "created-thread"
        ));
        assert_eq!(p.host.thread.selected.as_deref(), Some("created-thread"));
        assert_eq!(p.selected_view(), super::super::Workspace::Changes);
        assert!(p.workspace.composers.contains_key("created-thread"));
    });
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The tab file does not depend on the draft journal staying under its limit.
pub(super) fn journal_limit_saves_tabs() {
    use plait_client::{
        session::Journal,
        tabs::{CheckoutKey, HostKey, storage},
    };
    let scratch = std::env::temp_dir().join(format!("plait-tab-full-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let saved_path = scratch.join("desktop/tabs.json");
    let (journal, data) = Journal::open(&scratch.join("session"), "/tab-test-host".into()).unwrap();
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 20);
    let host = HostKey {
        id: "verified-test-host".into(),
        store: "/tab-test-host".into(),
    };
    let checkout = CheckoutKey {
        host: host.clone(),
        root: repository.clone(),
        directory: super::super::tab_identity::directory(&repository),
    };
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.install_tab_identity(host.clone(), vec![checkout.clone()], cx);
        p.session.journal = Some(journal.clone());
        p.session.data = Some(data);
        p.capture_session(cx);
        p.tabs.identity.catalog_ready = true;
        p.tabs.persistence.requested = true;
        p.tabs.persistence.loaded = true;
        p.tabs.persistence.restored = true;
        p.tabs.persistence.writable = true;
        p.tabs.persistence.path = Some(saved_path.clone());
        p.tabs.persistence.window = Some(w.window_handle());
        fill_journal(p);
        journal.queue(p.session.data.clone().unwrap());
    });
    assert!(journal.flush().unwrap_err().contains("exceeds"));
    h.window
        .update(&mut h.cx, |p, w, cx| {
            p.flush_tab_preferences(Some(w), cx).unwrap()
        })
        .unwrap()
        .write()
        .unwrap();
    assert!(storage::read(&saved_path).unwrap().is_some());
    drop(h);
    drop(journal);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Saving an unchanged large view neither adds a stored record nor rewrites the tab file.
pub(super) fn stored_views() {
    use plait_client::{
        session::Journal,
        tabs::{CheckoutKey, HostKey},
    };
    let scratch = std::env::temp_dir().join(format!("plait-tab-blobs-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let saved_path = scratch.join("desktop/tabs.json");
    let session = scratch.join("session");
    let (journal, data) = Journal::open(&session, "/tab-test-host".into()).unwrap();
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 3);
    let host = HostKey {
        id: "verified-test-host".into(),
        store: "/tab-test-host".into(),
    };
    let checkout = CheckoutKey {
        host: host.clone(),
        root: repository.clone(),
        directory: super::super::tab_identity::directory(&repository),
    };
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.install_tab_identity(host.clone(), vec![checkout.clone()], cx);
        p.session.journal = Some(journal.clone());
        p.session.data = Some(data);
        p.tabs.identity.catalog_ready = true;
        p.tabs.persistence.requested = true;
        p.tabs.persistence.loaded = true;
        p.tabs.persistence.restored = true;
        p.tabs.persistence.writable = true;
        p.tabs.persistence.path = Some(saved_path.clone());
        p.tabs.persistence.window = Some(w.window_handle());
        // A long commit message does not fit an inline bookmark.
        p.workspace
            .commit_message
            .update(cx, |f, cx| f.set_text(&"x".repeat(9000), cx));
    });
    let views = || {
        std::fs::read_dir(session.join("tabs"))
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with("view-"))
                    .count()
            })
            .unwrap_or(0)
    };
    for _ in 0..3 {
        h.window
            .update(&mut h.cx, |p, w, cx| {
                p.flush_tab_preferences(Some(w), cx).unwrap()
            })
            .unwrap()
            .write()
            .unwrap();
    }
    assert_eq!(views(), 1);
    // A changed view replaces the record it supersedes.
    h.update(|p, _, cx| {
        p.workspace
            .commit_message
            .update(cx, |f, cx| f.set_text(&"y".repeat(9000), cx));
    });
    h.window
        .update(&mut h.cx, |p, w, cx| {
            p.flush_tab_preferences(Some(w), cx).unwrap()
        })
        .unwrap()
        .write()
        .unwrap();
    assert_eq!(views(), 1);
    drop(h);
    drop(journal);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Saved tabs restored while Settings is open stay beneath it.
pub(super) fn restore_beneath_settings() {
    use plait_client::{
        session::Journal,
        tabs::{CheckoutKey, HostKey, storage},
    };
    let scratch = std::env::temp_dir().join(format!("plait-tab-covered-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let saved_path = scratch.join("desktop/tabs.json");
    let (journal, data) = Journal::open(&scratch.join("session"), "/tab-test-host".into()).unwrap();
    let mut h = Harness::new(&repository, None, 900., 600.);
    h.connect(&repository, false);
    install(&mut h, 3);
    let host = HostKey {
        id: "verified-test-host".into(),
        store: "/tab-test-host".into(),
    };
    let checkout = CheckoutKey {
        host: host.clone(),
        root: repository.clone(),
        directory: super::super::tab_identity::directory(&repository),
    };
    h.update(|p, w, cx| {
        p.host.connection.connected = false;
        p.install_tab_identity(host.clone(), vec![checkout.clone()], cx);
        p.session.journal = Some(journal.clone());
        p.session.data = Some(data);
        p.tabs.identity.catalog_ready = true;
        p.tabs.persistence.requested = true;
        p.tabs.persistence.loaded = true;
        p.tabs.persistence.restored = true;
        p.tabs.persistence.writable = true;
        p.tabs.persistence.path = Some(saved_path.clone());
        p.tabs.persistence.window = Some(w.window_handle());
    });
    h.window
        .update(&mut h.cx, |p, w, cx| {
            p.flush_tab_preferences(Some(w), cx).unwrap()
        })
        .unwrap()
        .write()
        .unwrap();
    let bytes = storage::read(&saved_path).unwrap().unwrap();
    h.update(|p, w, cx| {
        p.tabs.persistence.restored = false;
        p.tabs.persistence.edited = false;
        p.tabs.persistence.bytes = Some(bytes);
        p.show_settings(true, w, cx);
        p.try_restore_tabs(w, cx);
        assert_eq!(p.tabs.model.tabs().len(), 3);
        assert!(p.settings_open);
        assert!(p.settings_focus.contains_focused(w, cx));
    });
    drop(h);
    drop(journal);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Fill the current machine's saved thread work past the journal's size limit.
fn fill_journal(p: &mut Plait) {
    let summaries = p.host.connection.threads().to_vec();
    let thread = |checkout: &std::path::Path| plait_client::session::Thread {
        checkout: checkout.to_owned(),
        model: summaries[0].model.clone(),
        title: String::new(),
        draft: plait_client::session::Draft {
            text: "x".repeat(256 * 1024),
            ..Default::default()
        },
        pending: None,
        pending_submissions: Vec::new(),
        queued_submissions: Vec::new(),
        approval: None,
    };
    let machine = p.machines.active.clone();
    let mut work = p
        .saved_work
        .machines
        .get(&machine)
        .cloned()
        .unwrap_or_default();
    for summary in summaries.iter().skip(1) {
        work.put_thread(&summary.id, thread(&summary.checkout), &Default::default());
    }
    // Unlisted threads, to pass the machine file's size limit.
    for n in 0..20 {
        work.put_thread(
            &format!("filler-{n}"),
            thread(&summaries[0].checkout),
            &Default::default(),
        );
    }
    p.store_machine_work([(machine, work)].into_iter().collect());
}
