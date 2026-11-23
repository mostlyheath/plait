//! Folded earlier turns: scenes at both window sizes and desktop tests.
//! All content is labeled sample data; no host is contacted and no agent runs.

use super::{Harness, THREAD, fixture_repository, isolated};
use crate::ui::Plait;
use gpui::{prelude::*, *};
use plait_client::host::{
    Message, MessageRole, RecordChange, RecordKind, RecordPath, RecordState, Snapshot,
    ThreadStatus, Tool, ToolStatus, TurnRecord,
};
use std::{path::Path, sync::Arc};

const NOW_MS: u64 = 1_794_860_000_000;

/// Build a long sample thread with 6 finished earlier turns and 1 latest turn,
/// plus matching host turn records so ages and closing lines draw honestly.
fn long_thread(checkout: &Path) -> (Snapshot, Vec<TurnRecord>) {
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    let mut records = Vec::new();

    let topics = [
        (
            "r1",
            "Audit the retry backoff bounds\nCheck jitter and max delay.",
            "Sample reply 1: inspected retry backoff bounds and confirmed cap at 30 seconds. Unique token: alpha-marker.",
            "src/retry.rs",
            45 * 60_000,
        ),
        (
            "r2",
            "Add unit tests for empty diff headers",
            "Sample reply 2: added unit tests covering empty and binary diff headers. Unique token: bravo-marker.",
            "src/diff.rs",
            32 * 60_000,
        ),
        (
            "r3",
            "Simplify the sidebar unread counter",
            "Sample reply 3: removed duplicate counter branch and verified archived threads are excluded. Unique token: charlie-marker.",
            "src/navigation.rs",
            21 * 60_000,
        ),
        (
            "r4",
            "Guard against stale history pages on reconnect",
            "Sample reply 4: checked watch epoch before installing history pages. Unique token: delta-marker.",
            "src/history.rs",
            14 * 60_000,
        ),
        (
            "r5",
            "Preserve scroll offset when expanding a tool card",
            "Sample reply 5: remeasured only the toggled tool row in the virtualized list. Unique token: echo-marker.",
            "src/tool_card.rs",
            8 * 60_000,
        ),
        (
            "r6",
            "Document the new appearance toggle in the guide",
            "Sample reply 6: updated docs/GUIDE.md with the appearance option. Unique token: foxtrot-marker.",
            "docs/GUIDE.md",
            3 * 60_000,
        ),
    ];

    for (index, (req, prompt, reply, path, age_ms)) in topics.iter().enumerate() {
        let req_id = format!("request-{req}");
        let ans_id = format!("answer-{req}");
        let tool_id = format!("tool-{req}");
        messages.push(Message {
            id: req_id.clone(),
            role: MessageRole::User,
            text: (*prompt).into(),
            ..Default::default()
        });
        tools.push(Tool {
            id: tool_id,
            name: "edit".into(),
            status: ToolStatus::Done,
            input: format!(r#"{{"path":"{path}"}}"#),
            output: "Applied sample edit.".into(),
            after: Some(Some(req_id)),
        });
        messages.push(Message {
            id: ans_id,
            role: MessageRole::Assistant,
            text: (*reply).into(),
            ..Default::default()
        });
        records.push(TurnRecord {
            id: format!("rec-{}", index + 1),
            request_ids: vec![(*req).into()],
            request_ids_offset: 0,
            request_ids_total: 1,
            kind: RecordKind::Turn,
            undoes: None,
            state: RecordState::Ready,
            head_moved: false,
            after_restart: false,
            paths: vec![RecordPath {
                path: (*path).into(),
                change: RecordChange::Modified,
                added: Some(12 + index as u64 * 3),
                removed: Some(4 + index as u64),
            }],
            paths_offset: 0,
            paths_total: 1,
            not_covered: Vec::new(),
            not_covered_offset: 0,
            not_covered_total: 0,
            created_at: NOW_MS - age_ms,
        });
    }

    // Latest turn (always open).
    messages.push(Message {
        id: "request-r7".into(),
        role: MessageRole::User,
        text: "Verify the folded turns layout and keyboard access".into(),
        ..Default::default()
    });
    tools.push(Tool {
        id: "tool-r7".into(),
        name: "command".into(),
        status: ToolStatus::Done,
        input: r#"{"command":"cargo test -p plait-client folding"}"#.into(),
        output: "test result: ok. 8 passed; 0 failed".into(),
        after: Some(Some("request-r7".into())),
    });
    messages.push(Message {
        id: "answer-r7".into(),
        role: MessageRole::Assistant,
        text: "Sample latest reply: earlier finished turns now draw as their request line with age and closing line, while the latest turn stays open.".into(),
        ..Default::default()
    });
    records.push(TurnRecord {
        id: "rec-7".into(),
        request_ids: vec!["r7".into()],
        request_ids_offset: 0,
        request_ids_total: 1,
        kind: RecordKind::Turn,
        undoes: None,
        state: RecordState::Ready,
        head_moved: false,
        after_restart: false,
        paths: vec![RecordPath {
            path: "crates/plait/src/ui/live_conversation.rs".into(),
            change: RecordChange::Modified,
            added: Some(48),
            removed: Some(6),
        }],
        paths_offset: 0,
        paths_total: 1,
        not_covered: Vec::new(),
        not_covered_offset: 0,
        not_covered_total: 0,
        created_at: NOW_MS - 30_000,
    });
    // Host returns records newest first.
    records.reverse();

    let snapshot = Snapshot {
        id: THREAD.into(),
        checkout: checkout.to_path_buf(),
        status: ThreadStatus::Idle,
        messages,
        tools,
        ..Default::default()
    };
    (snapshot, records)
}

fn install_long_thread(harness: &mut Harness, repository: &Path) {
    let (snapshot, records) = long_thread(repository);
    harness.connect(repository, false);
    harness.update(move |plait, _, cx| {
        plait.host.connection.threads_mut()[0].status = ThreadStatus::Idle;
        if let Some(status) = plait.host.connection.status.as_mut() {
            status.protocol_version = 6;
        }
        plait.workspace.history.now = NOW_MS;
        plait.workspace.opened_turns.clear();
        let lines = plait_client::history::records::project(&snapshot, &records, true);
        plait.workspace.turn_records.data = Arc::new(records);
        plait.workspace.turn_records.loaded = true;
        plait.workspace.turn_records.lines = Arc::new(lines);
        plait.host.thread.snapshot = Some(Arc::new(snapshot));
        plait.workspace.formatted = Arc::new(plait_client::markdown::Transcript::update(
            &Default::default(),
            plait.host.thread.snapshot.as_ref().unwrap(),
        ));
        plait.refresh_live_list(None);
        plait.sync_composer(cx);
    });
    harness.draw();
}

fn type_find_query(plait: &mut Plait, query: &'static str, cx: &mut Context<Plait>) {
    let field = plait
        .workspace
        .conversation_find
        .field
        .clone()
        .expect("find is open");
    field.update(cx, |field, cx| field.set_text(query, cx));
}

/// Capture all four folded-turn scenes at both window sizes.
pub(super) fn walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    install_long_thread(harness, repository);
    // 1. Long thread with earlier turns folded.
    harness.update(|plait, _, _| {
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });
    harness.capture(&format!("folded-turns-long-{suffix}"));

    // 2. One earlier turn opened.
    harness.update(|plait, _, cx| {
        plait.open_folded_turn("request-r3", cx);
    });
    harness.capture(&format!("folded-turns-opened-{suffix}"));

    // 3. A folded turn holding a Find match (shown open).
    harness.update(|plait, window, cx| {
        plait.open_conversation_find(window, cx);
        type_find_query(plait, "delta-marker", cx);
    });
    harness.draw();
    harness.draw();
    harness.capture(&format!("folded-turns-find-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_conversation_find(window, cx);
    });

    // 4. Setting off: all earlier turns open.
    harness.update(|plait, _, cx| {
        plait.set_fold_earlier_turns(false, cx);
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });
    harness.capture(&format!("folded-turns-off-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.set_fold_earlier_turns(true, cx);
    });
}

#[core::prelude::v1::test]
fn find_opens_a_folded_match_and_folds_it_back_when_moving_away() {
    if !isolated("folded_turns::find_opens_a_folded_match_and_folds_it_back_when_moving_away") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-folded-find-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    install_long_thread(&mut harness, &repository);

    // Initially r2 and r4 are folded.
    harness.update(|plait, _, _| {
        assert!(
            plait
                .workspace
                .live_layout
                .folded_row_index("request-r2")
                .is_some()
        );
        assert!(
            plait
                .workspace
                .live_layout
                .folded_row_index("request-r4")
                .is_some()
        );
    });

    // Search for text inside folded turn r2.
    harness.update(|plait, window, cx| {
        plait.open_conversation_find(window, cx);
        type_find_query(plait, "bravo-marker", cx);
    });
    harness.draw();
    harness.draw();
    harness.update(|plait, _, _| {
        assert_eq!(plait.workspace.conversation_find.matches.len(), 1);
        assert!(
            plait
                .workspace
                .live_layout
                .folded_row_index("request-r2")
                .is_none(),
            "Find must open the folded turn holding the current match"
        );
        assert!(
            plait
                .workspace
                .live_layout
                .row_of_message
                .contains_key("answer-r2"),
            "the matched reply must be unfolded into live_layout"
        );
        assert!(
            plait
                .workspace
                .live_layout
                .folded_row_index("request-r4")
                .is_some(),
            "unrelated earlier turns stay folded"
        );
    });

    // Change query to text inside folded turn r4: r2 folds back, r4 opens.
    harness.update(|plait, _, cx| {
        type_find_query(plait, "delta-marker", cx);
    });
    harness.draw();
    harness.draw();
    harness.update(|plait, _, _| {
        assert!(
            plait
                .workspace
                .live_layout
                .folded_row_index("request-r2")
                .is_some(),
            "r2 was not manually opened so it folds back when the Find match leaves it"
        );
        assert!(
            plait
                .workspace
                .live_layout
                .folded_row_index("request-r4")
                .is_none(),
            "r4 opens because it holds the current Find match"
        );
    });

    // Closing Find folds r4 back too.
    harness.update(|plait, window, cx| plait.close_conversation_find(window, cx));
    harness.draw();
    harness.update(|plait, _, _| {
        assert!(
            plait
                .workspace
                .live_layout
                .folded_row_index("request-r4")
                .is_some()
        );
    });

    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[core::prelude::v1::test]
fn opening_a_folded_turn_keeps_the_activated_row_where_it_is_on_screen() {
    if !isolated(
        "folded_turns::opening_a_folded_turn_keeps_the_activated_row_where_it_is_on_screen",
    ) {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-folded-scroll-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::with_host(&repository, None, 900., 600., None);
    install_long_thread(&mut harness, &repository);

    // Case 1: scrolled up, opening a folded turn below the viewport top.
    harness.update(|plait, _, _| {
        plait.workspace.live_scroll.scroll_to(ListOffset {
            item_ix: 1,
            offset_in_item: px(10.),
        });
    });
    harness.draw();
    let before_top = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let ix = plait
                .workspace
                .live_layout
                .folded_row_index("request-r4")
                .expect("r4 folded");
            plait
                .workspace
                .live_scroll
                .bounds_for_item(ix)
                .expect("r4 visible")
                .top()
        })
        .unwrap();

    harness.update(|plait, _, cx| plait.open_folded_turn("request-r4", cx));
    harness.draw();

    let after_top = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let ix = *plait
                .workspace
                .live_layout
                .row_of_message
                .get("request-r4")
                .expect("r4 unfolded");
            plait
                .workspace
                .live_scroll
                .bounds_for_item(ix)
                .expect("r4 visible after open")
                .top()
        })
        .unwrap();
    assert!(
        (f32::from(after_top) - f32::from(before_top)).abs() < 1.0,
        "opening r4 jumped from {before_top:?} to {after_top:?}"
    );

    // Case 2: pinned to the bottom (ListAlignment::Bottom), opening a visible folded turn.
    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to_end());
    harness.draw();
    let mut pinned_top = px(0.);
    harness.update(|plait, _, cx| {
        let scroll = &plait.workspace.live_scroll;
        assert!(scroll.logical_scroll_top().item_ix >= plait.workspace.live_count);
        let ix = plait
            .workspace
            .live_layout
            .folded_row_index("request-r6")
            .unwrap();
        scroll.scroll_to(ListOffset::default());
        let tail = scroll
            .bounds_for_item(plait.workspace.live_count - 1)
            .unwrap()
            .bottom()
            - scroll.bounds_for_item(ix).unwrap().top();
        pinned_top = scroll.viewport_bounds().bottom() - tail;
        scroll.scroll_to_end();
        plait.open_folded_turn("request-r6", cx);
        assert!(
            plait.workspace.live_scroll.logical_scroll_top().item_ix < plait.workspace.live_count,
            "opening a folded turn unpins bottom alignment so the activated row stays put"
        );
    });
    harness.draw();
    harness.update(|plait, _, _| {
        let ix = *plait
            .workspace
            .live_layout
            .row_of_message
            .get("request-r6")
            .expect("r6 unfolded");
        let bounds = plait
            .workspace
            .live_scroll
            .bounds_for_item(ix)
            .expect("r6 stays in viewport");
        assert!(
            (bounds.top() + px(crate::ui::live_conversation::LIST_TOP_PADDING) - pinned_top).abs()
                < px(1.),
            "pinned row moved from {pinned_top:?} to {bounds:?}"
        );
    });

    // Leave two turns in a short bottom-aligned list, then open a reply
    // taller than the viewport. The initial blank space must survive layout.
    harness.change_snapshot(|snap| {
        snap.messages
            .retain(|m| m.id.ends_with("r1") || m.id.ends_with("r7"));
        snap.tools.clear();
        snap.messages[1].text = "Long reply paragraph.\n\n".repeat(100);
    });
    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to_end());
    harness.draw();
    let expected = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let scroll = &plait.workspace.live_scroll;
            scroll.scroll_to_end();
            let height = -scroll.scroll_px_offset_for_scrollbar().y;
            let top = scroll.viewport_bounds().bottom() - height;
            assert!(top > scroll.viewport_bounds().top() + px(20.));
            top
        })
        .unwrap();
    harness.update(|plait, _, cx| plait.open_folded_turn("request-r1", cx));
    harness.draw();
    harness.update(|plait, _, _| {
        let scroll = &plait.workspace.live_scroll;
        let top = scroll.bounds_for_item(0).unwrap().top()
            + px(crate::ui::live_conversation::LIST_TOP_PADDING);
        assert!(
            (top - expected).abs() < px(1.),
            "short transcript moved from {expected:?} to {top:?}"
        );
        assert!(scroll.max_offset_for_scrollbar().y > scroll.viewport_bounds().size.height);
    });

    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[core::prelude::v1::test]
fn folded_bookmarks_and_copy_use_loaded_transcript_rows() {
    if !isolated("folded_turns::folded_bookmarks_and_copy_use_loaded_transcript_rows") {
        return;
    }
    use plait_client::tabs::bookmark::{
        ConversationScroll, TextPosition, ThreadBookmark, ViewDetails,
    };
    let scratch = std::env::temp_dir().join(format!("plait-folded-reading-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::with_host(&repository, None, 900., 600., None);
    install_long_thread(&mut harness, &repository);
    harness.update(|plait, _, cx| {
        for key in ["message:answer-r3", "tool:tool-r3"] {
            let mut bookmark = ThreadBookmark::default();
            bookmark.conversation.scroll = ConversationScroll::Anchored {
                row: key.into(),
                offset_millipoints: 64000,
            };
            plait.workspace.restore = Some(bookmark);
            plait.workspace.restore_pending[0] = true;
            plait.restore_ready_bookmarks(cx);
            let top = plait.workspace.live_scroll.logical_scroll_top();
            assert_eq!((top.item_ix, top.offset_in_item), (2, px(0.)));
            assert!(!plait.workspace.restore_pending[0]);
        }
        plait.open_folded_turn("request-r1", cx);
        let pos = |id: &str, byte| TextPosition {
            source: format!("message:{id}"),
            block: 0,
            byte,
        };
        let details = ViewDetails {
            selection: Some((pos("answer-r1", 5), pos("request-r7", 6))),
            ..Default::default()
        };
        plait.workspace.reading.restore_selection(THREAD, &details);
        plait.workspace.expanded_live_tools.insert("tool-r3".into());
        let copy = |plait: &Plait| {
            plait.resolve_selection();
            plait.selected_text().unwrap()
        };
        let folded = copy(plait);
        for n in 2..=6 {
            plait.open_folded_turn(&format!("request-r{n}"), cx);
        }
        assert_eq!(folded, copy(plait));
        assert!(folded.contains("bravo-marker"));
    });
    drop(harness);
    let _ = std::fs::remove_dir_all(scratch);
}

/// Deliver a real key press (KeyDown + KeyUp), as GPUI's button activation
/// listens for Enter/Space release after press.
fn press_key(harness: &mut Harness, key: &'static str) {
    let keystroke = Keystroke::parse(key).expect("keystroke");
    harness.pointer(PlatformInput::KeyDown(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    }));
    harness.pointer(PlatformInput::KeyUp(KeyUpEvent { keystroke }));
}

#[core::prelude::v1::test]
fn disclosure_has_accessible_name_and_opens_with_enter_and_space() {
    if !isolated("folded_turns::disclosure_has_accessible_name_and_opens_with_enter_and_space") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-folded-a11y-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::with_host(&repository, None, 1380., 860., None);
    install_long_thread(&mut harness, &repository);
    harness.update(|plait, _, _| {
        plait.sidebar = false;
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });

    #[cfg(debug_assertions)]
    {
        let frame = harness.draw_a11y();
        frame.assert_no_duplicates("folded turns");
        assert!(
            frame.has_node(Role::Button, |node| {
                node.label.as_deref().is_some_and(|label| {
                    label.contains("Turn hidden:")
                        && label.contains("Audit the retry backoff bounds")
                        && label.contains("Show turn")
                })
            }),
            "folded request disclosure must expose an accessible name stating whether the turn is shown"
        );
    }

    for (key, request, reply) in [
        ("enter", "request-r2", "answer-r2"),
        ("space", "request-r1", "answer-r1"),
    ] {
        harness.update(|plait, window, cx| {
            assert!(
                plait
                    .workspace
                    .live_layout
                    .folded_row_index(request)
                    .is_some()
            );
            window.focus(&plait.workspace.composers[THREAD].focus_handle(cx), cx);
        });
        harness.draw();
        let reached = (0..64).any(|_| {
            harness.keystroke("shift-tab");
            harness.draw();
            harness
                .window
                .update(&mut harness.cx, |_, window, _| {
                    window.context_stack().iter().any(|context| {
                        context.get("folded-turn").map(|id| id.as_ref()) == Some(request)
                    })
                })
                .unwrap()
        });
        assert!(
            reached,
            "Shift-Tab must reach {request} before pressing {key}"
        );
        press_key(&mut harness, key);
        harness.draw();
        harness.update(|plait, _, _| {
            let layout = &plait.workspace.live_layout;
            assert!(
                layout.folded_row_index(request).is_none(),
                "{key} must open {request}"
            );
            assert!(layout.row_of_message.contains_key(reply));
        });
    }

    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[core::prelude::v1::test]
fn opened_turns_stay_open_per_tab_until_closed() {
    if !isolated("folded_turns::opened_turns_stay_open_per_tab_until_closed") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-folded-tabs-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    install_long_thread(&mut harness, &repository);

    harness.update(|plait, _, cx| {
        let tab_id = plait.tabs.model.active().expect("active tab");
        plait.open_folded_turn("request-r2", cx);
        assert!(
            plait
                .tabs
                .model
                .get(tab_id)
                .unwrap()
                .session()
                .opened_turns()
                .contains("request-r2")
        );
        // Closing the tab discards its in-memory opened_turns set.
        plait.tabs.model.close(tab_id, false).unwrap();
        assert!(plait.tabs.model.get(tab_id).is_none());
    });

    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[core::prelude::v1::test]
fn scenes_render_at_wide_and_narrow_sizes() {
    if !isolated("folded_turns::scenes_render_at_wide_and_narrow_sizes") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-folded-scenes-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    // Writes its images when asked to, so these scenes can be looked at alone.
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(std::path::PathBuf::from);
    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        let mut harness = Harness::new(&repository, output.as_deref(), width, height);
        walk(&mut harness, &repository, suffix);
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
