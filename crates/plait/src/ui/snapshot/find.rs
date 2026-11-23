//! Find in the open conversation: its scenes and desktop tests. Every message
//! here is labeled sample text; no host is contacted and no agent runs.

use super::{Harness, THREAD, fixture_repository, isolated, reading};
use crate::ui::{
    Plait, Workspace,
    selection::Owner,
    theme::{CHROME_HEIGHT, ROW_HEIGHT},
};
use gpui::{prelude::*, *};
use plait_client::{
    conversation_find::Matches,
    host::{Message, MessageRole, Snapshot, ThreadStatus},
};
use std::{ops::Range, path::Path, sync::Arc};

fn type_query(plait: &mut Plait, query: &'static str, cx: &mut Context<Plait>) {
    let field = plait
        .workspace
        .conversation_find
        .field
        .clone()
        .expect("find is open");
    field.update(cx, |field, cx| field.set_text(query, cx));
}

/// The reading fixture, at its top, with the thread finished.
fn reading_at_top(harness: &mut Harness, repository: &Path) {
    harness.connect(repository, false);
    harness.change_snapshot(reading);
    harness.update(|plait, _, cx| {
        plait.host.connection.threads_mut()[0].status = ThreadStatus::Idle;
        plait.sync_composer(cx);
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });
    // Find places a match by where its text was last drawn.
    harness.draw();
}

/// The bar over the conversation with matches highlighted, with none, and
/// with earlier messages not loaded.
pub(super) fn find_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    reading_at_top(harness, repository);
    harness.update(|plait, window, cx| {
        plait.open_conversation_find(window, cx);
        type_query(plait, "sample", cx);
    });
    // The first match is current; the second, further on, is the next.
    harness.update(|plait, _, cx| plait.step_conversation_find(true, cx));
    harness.draw();
    harness.capture(&format!("find-conversation-{suffix}"));
    harness.update(|plait, _, cx| type_query(plait, "no such words", cx));
    harness.capture(&format!("find-conversation-none-{suffix}"));
    // A version 3 host with earlier pages not yet read.
    harness.update(|plait, _, cx| {
        plait.workspace.history.enabled = true;
        let cache = Arc::make_mut(&mut plait.workspace.history.cache);
        cache.loaded = true;
        cache.older = Some(None);
        type_query(plait, "sample", cx);
    });
    harness.capture(&format!("find-conversation-unloaded-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_conversation_find(window, cx);
        plait.workspace.history = Default::default();
    });
}

/// Cmd-F opens the bar of the view on screen and no other; Escape closes it
/// and gives focus back to where it was.
#[core::prelude::v1::test]
fn find_opens_only_in_the_view_on_screen() {
    if !isolated("find::find_opens_only_in_the_view_on_screen") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-find-target-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    reading_at_top(&mut harness, &repository);
    harness.update(|plait, window, cx| {
        let composer = plait.workspace.composers[THREAD].clone();
        window.focus(&composer.focus_handle(cx), cx);
    });
    harness.draw();
    harness.keystroke("cmd-f");
    harness.update(|plait, window, cx| {
        assert!(plait.workspace.conversation_find.open);
        assert!(
            !plait.workspace.file_viewer.find.open,
            "the Files bar stays closed"
        );
        let field = plait.workspace.conversation_find.field.clone().unwrap();
        assert!(field.focus_handle(cx).is_focused(window));
        type_query(plait, "sample", cx);
    });
    harness.draw();
    harness.keystroke("escape");
    harness.update(|plait, window, cx| {
        assert!(!plait.workspace.conversation_find.open);
        assert!(plait.workspace.conversation_find.matches.is_empty());
        let composer = plait.workspace.composers[THREAD].clone();
        assert!(
            composer.focus_handle(cx).is_focused(window),
            "focus returns to the composer"
        );
    });
    // In the Files tab the same key opens the file's bar, not the conversation's,
    // though the composer had the keyboard when the file opened.
    harness.update(|plait, _, cx| plait.open_file("src/review.rs".into(), None, cx));
    harness.keystroke("cmd-f");
    harness.update(|plait, _, _| {
        assert!(plait.workspace.file_viewer.find.open);
        assert!(
            !plait.workspace.conversation_find.open,
            "the conversation's bar stays closed"
        );
    });
    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Press and release the left button at `position`.
fn click(harness: &mut Harness, position: Point<Pixels>) {
    harness.draw();
    harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }));
    harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 1,
        ..Default::default()
    }));
}

/// The composer's text and caret, and whether it or the root has the keyboard.
fn composer_state(harness: &mut Harness) -> ((String, Range<usize>), bool, bool) {
    harness.draw();
    harness
        .window
        .update(&mut harness.cx, |plait, window, cx| {
            let composer = plait.workspace.composers[THREAD].clone();
            let composer = composer.read(cx);
            (
                (composer.text().to_owned(), composer.caret()),
                composer.focus_handle(cx).is_focused(window),
                plait.focus.is_focused(window),
            )
        })
        .unwrap()
}

/// A tab that hides the conversation takes the keyboard out of its composer,
/// so shortcuts and Find work there whether the tab was chosen with a shortcut
/// or a click. Showing the conversation again gives the composer the keyboard
/// back, with its draft and caret as they were.
#[core::prelude::v1::test]
fn the_keyboard_leaves_a_hidden_composer_and_returns_to_it() {
    if !isolated("find::the_keyboard_leaves_a_hidden_composer_and_returns_to_it") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-find-focus-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    reading_at_top(&mut harness, &repository);
    // A file for Find in Files, then back to the conversation to type.
    harness.update(|plait, _, cx| plait.open_file("src/review.rs".into(), None, cx));
    harness.update(|plait, window, cx| plait.show_tab(Workspace::Conversation, window, cx));
    harness.update(|plait, window, cx| {
        let composer = plait.workspace.composers[THREAD].clone();
        composer.update(cx, |composer, cx| {
            composer.type_text("Sample draft", window, cx)
        });
    });
    harness.draw();
    harness.keystroke("left");
    let (draft, composer, _) = composer_state(&mut harness);
    assert_eq!(draft, ("Sample draft".into(), 11..11), "typed, caret moved");
    assert!(composer);
    let tab = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |plait, _, _| plait.workspace.tab)
            .unwrap()
    };
    // By shortcut: Changes, where the composer is not drawn, so the root
    // takes the keyboard and the next shortcut still reaches the app.
    harness.keystroke("cmd-shift-2");
    assert_eq!(tab(&mut harness), Workspace::Changes);
    let (_, composer, root) = composer_state(&mut harness);
    assert!(
        !composer && root,
        "the hidden composer gives up the keyboard"
    );
    harness.keystroke("cmd-shift-3");
    assert_eq!(tab(&mut harness), Workspace::History);
    // By click: Files, the last tab, just inside its right edge. Right of it
    // are the segment's inset, a gap, and the inspector toggle.
    let files_tab = point(
        px(900. - 12. - ROW_HEIGHT - 12. - 3. - 10.),
        px(CHROME_HEIGHT + 18.),
    );
    click(&mut harness, files_tab);
    assert_eq!(tab(&mut harness), Workspace::Files);
    let find_open = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |plait, window, cx| {
                let find = &plait.workspace.file_viewer.find;
                find.open
                    && find
                        .field
                        .as_ref()
                        .unwrap()
                        .focus_handle(cx)
                        .is_focused(window)
            })
            .unwrap()
    };
    harness.keystroke("cmd-f");
    assert!(
        find_open(&mut harness),
        "Cmd-F opens the Files bar and focuses it"
    );
    harness.keystroke("escape");
    assert!(!find_open(&mut harness));
    // Back to the conversation: the composer has the keyboard, draft, and caret.
    harness.keystroke("cmd-shift-1");
    assert_eq!(tab(&mut harness), Workspace::Conversation);
    assert_eq!(composer_state(&mut harness), (draft.clone(), true, false));
    // A click on Files straight from the composer leaves Find working too.
    click(&mut harness, files_tab);
    assert_eq!(tab(&mut harness), Workspace::Files);
    harness.keystroke("cmd-f");
    assert!(find_open(&mut harness));
    harness.keystroke("escape");
    harness.keystroke("cmd-shift-1");
    assert_eq!(composer_state(&mut harness).0, draft);
    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A result for an older query, or for rows that have since changed, is not
/// shown; the search runs again for what is current.
#[core::prelude::v1::test]
fn a_stale_conversation_search_is_dropped() {
    if !isolated("find::a_stale_conversation_search_is_dropped") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-find-stale-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    reading_at_top(&mut harness, &repository);
    harness.update(|plait, window, cx| {
        plait.open_conversation_find(window, cx);
        type_query(plait, "sample", cx);
    });
    let shown = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |plait, _, _| {
                (*plait.workspace.conversation_find.matches).clone()
            })
            .unwrap()
    };
    let sample = shown(&mut harness);
    assert!(sample.len() > 1, "the fixture says sample more than once");
    let invented = || Matches::default();
    // Started for "sample", then the query changed before it landed. The
    // window update flushes the field's event without running the search.
    let stale = harness
        .window
        .update(&mut harness.cx, |plait, _, cx| {
            let stale = plait.found_now("sample", invented());
            type_query(plait, "fixture", cx);
            stale
        })
        .unwrap();
    harness
        .window
        .update(&mut harness.cx, |plait, _, cx| plait.land_search(stale, cx))
        .unwrap();
    assert_eq!(
        shown(&mut harness),
        sample,
        "an older query's result is dropped"
    );
    harness.settle();
    let fixture = shown(&mut harness);
    assert!(!fixture.is_empty() && fixture != sample);
    // Started over one transcript, landing after a reply replaced it.
    let stale = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let stale = plait.found_now("fixture", invented());
            plait.workspace.formatted = Arc::new(plait_client::markdown::Transcript::update(
                &plait.workspace.formatted,
                plait.host.thread.snapshot.as_ref().unwrap(),
            ));
            stale
        })
        .unwrap();
    harness
        .window
        .update(&mut harness.cx, |plait, _, cx| plait.land_search(stale, cx))
        .unwrap();
    assert_eq!(
        shown(&mut harness),
        fixture,
        "an older transcript's result is dropped"
    );
    harness.settle();
    assert_eq!(shown(&mut harness), fixture, "and the search ran again");
    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Sixty messages; three of them say "needle", far apart.
fn haystack(snapshot: &mut Snapshot) {
    snapshot.status = ThreadStatus::Idle;
    snapshot.live_text.clear();
    snapshot.tools.clear();
    snapshot.pending_approval = None;
    let filler = "Sample filler for a long conversation. ".repeat(12);
    snapshot.messages = (0..60)
        .map(|index| {
            let user = index % 2 == 0;
            let text = match index {
                3 | 30 | 57 => format!("{filler}\n\nThe needle is in message {index}.\n\n{filler}"),
                _ if user => format!("Sample request {index}."),
                _ => format!("{filler}\n\n{filler}"),
            };
            Message {
                id: format!("hay-{index}"),
                role: if user {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                text,
                ..Default::default()
            }
        })
        .collect();
}

/// Where the current match is, and whether it is drawn inside the list.
fn current_in_view(harness: &mut Harness) -> (String, bool) {
    // One frame draws the match's row, the next places the match in view.
    harness.draw();
    harness.draw();
    harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let find = &plait.workspace.conversation_find;
            let hit = find.matches.hits[find.current].clone();
            let owner = Owner::Message(hit.message.clone().into());
            let point = plait.text_point(&owner, hit.block, hit.range.start);
            let view = plait.workspace.live_scroll.viewport_bounds();
            (
                hit.message,
                point.is_some_and(|point| view.contains(&point)),
            )
        })
        .unwrap()
}

/// Typing starts from the view; next and previous move the list to each
/// match's row and wrap at the ends.
#[core::prelude::v1::test]
fn next_and_previous_bring_each_match_into_view() {
    if !isolated("find::next_and_previous_bring_each_match_into_view") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-find-step-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.change_snapshot(haystack);
    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to_end());
    harness.draw();
    harness.update(|plait, window, cx| {
        plait.open_conversation_find(window, cx);
        type_query(plait, "NEEDLE", cx);
    });
    harness.update(|plait, _, _| assert_eq!(plait.workspace.conversation_find.matches.len(), 3));
    // At the end of the conversation, the first match from the view is the last one.
    assert_eq!(current_in_view(&mut harness), ("hay-57".into(), true));
    let step = |harness: &mut Harness, forward: bool| {
        harness.update(|plait, _, cx| plait.step_conversation_find(forward, cx));
        current_in_view(harness)
    };
    assert_eq!(
        step(&mut harness, true),
        ("hay-3".into(), true),
        "wraps to the first"
    );
    assert_eq!(step(&mut harness, true), ("hay-30".into(), true));
    assert_eq!(step(&mut harness, false), ("hay-3".into(), true));
    assert_eq!(
        step(&mut harness, false),
        ("hay-57".into(), true),
        "wraps to the last"
    );
    // A new reply keeps the current match and leaves the list where it is.
    let top = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |plait, _, _| {
                plait.workspace.live_scroll.logical_scroll_top()
            })
            .unwrap()
    };
    assert_eq!(step(&mut harness, false), ("hay-30".into(), true));
    let before = top(&mut harness);
    harness.change_snapshot(|snapshot| {
        snapshot.messages.push(Message {
            id: "hay-60".into(),
            role: MessageRole::Assistant,
            text: "One more needle.".into(),
            ..Default::default()
        })
    });
    harness.draw();
    harness.update(|plait, _, _| {
        let find = &plait.workspace.conversation_find;
        assert_eq!(find.matches.len(), 4);
        assert_eq!(find.matches.hits[find.current].message, "hay-30");
    });
    assert_eq!(top(&mut harness), before);
    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}
