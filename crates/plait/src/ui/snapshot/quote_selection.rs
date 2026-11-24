//! Quote a selection from a reply into the current draft.
use super::*;
use plait_client::tabs::bookmark::{TextPosition, ViewDetails};

const SELECTION: &str = "Check the queue.\n\nKeep the cursor.";
const DRAFT: &str = "Keep editing here";
const QUOTED_DRAFT: &str = "Keep editing here\n\n> Check the queue.\n> \n> Keep the cursor.\n\n";

fn select_reply(h: &mut Harness, first: &str, last: &str) {
    let at = |id: &str, block, byte| TextPosition {
        source: format!("message:{id}"),
        block,
        byte,
    };
    let details = ViewDetails {
        selection: Some((
            at(first, 0, 0),
            at(last, 1, "Keep the cursor.".len() as u32),
        )),
        ..Default::default()
    };
    h.update(|p, window, cx| {
        p.workspace.reading.restore_selection(THREAD, &details);
        window.focus(&p.focus, cx);
    });
    h.draw();
}

fn stage(h: &mut Harness, repository: &Path) {
    h.connect(repository, false);
    h.change_snapshot(|s| {
        s.status = ThreadStatus::Idle;
        s.messages = [
            ("question", MessageRole::User, "What should I check?"),
            ("reply", MessageRole::Assistant, SELECTION),
        ]
        .into_iter()
        .map(|(id, role, text)| Message {
            id: id.into(),
            role,
            text: text.into(),
            ..Default::default()
        })
        .collect();
        s.tools.clear();
        s.live_text.clear();
    });
    h.update(|p, window, cx| {
        p.preferences.value.hide_inspector = true;
        p.state.projects[0].machines[0].name = "studio".into();
        p.sync_composer(cx);
        p.workspace.composers[THREAD].update(cx, |composer, cx| {
            composer.restore_draft(DRAFT.into(), 7, Some((DRAFT.into(), 7)), cx);
            composer.set_selected_text_range(4..4, window, cx);
        });
    });
    select_reply(h, "reply", "reply");
}

pub(super) fn scenes(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut h = this_turn::harness(repository, output, width, height);
    stage(&mut h, repository);
    h.capture(&format!("quote-selection-button-{suffix}"));
    h.update(|p, window, cx| assert!(p.quote_reply_selection(None, window, cx)));
    h.capture(&format!("quote-selection-in-draft-{suffix}"));
}

#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn quote_selection_works_by_pointer_and_a_rebound_key() {
    if !isolated("quote_selection::quote_selection_works_by_pointer_and_a_rebound_key") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let repo = fixture_repository(&home.join("quote"));
    let mut h = this_turn::harness(&repo, None, 900., 600.);
    let button = |h: &mut Harness| {
        h.draw_a11y()
            .nodes
            .into_iter()
            .find(|n| n.role == Role::Button && n.label.as_deref() == Some("Quote selection"))
            .map(|n| n.bounds.center())
    };
    let verify = |h: &mut Harness| {
        h.update(|p, window, cx| {
            p.workspace.composers[THREAD].update(cx, |composer, cx| {
                assert_eq!(composer.saved_draft().0, QUOTED_DRAFT);
                assert_eq!(composer.saved_draft().1, 8);
                assert_eq!(composer.caret(), 4..4);
                assert!(composer.focus_handle(cx).is_focused(window));
                assert!(!composer.acknowledge(DRAFT, Some(7), cx));
            });
        });
    };

    stage(&mut h, &repo);
    h.update(|p, _, _| assert_eq!(p.selected_text().as_deref(), Some(SELECTION)));
    let position = button(&mut h).expect("quote control is visible");
    h.pointer(PlatformInput::MouseDown(MouseDownEvent {
        position,
        button: MouseButton::Left,
        click_count: 1,
        ..Default::default()
    }));
    h.pointer(PlatformInput::MouseUp(MouseUpEvent {
        position,
        button: MouseButton::Left,
        click_count: 1,
        ..Default::default()
    }));
    verify(&mut h);

    stage(&mut h, &repo);
    h.update(|p, _, cx| {
        p.preferences
            .value
            .keymap
            .insert("conversation.quote_selection".into(), "ctrl-alt-q".into());
        crate::ui::actions::install(cx, &p.preferences.value.keymap);
    });
    select_reply(&mut h, "question", "reply");
    assert!(
        button(&mut h).is_none(),
        "a selection crossing two messages"
    );
    h.keystroke("ctrl-alt-q");
    h.update(|p, _, cx| assert_eq!(p.workspace.composers[THREAD].read(cx).text(), DRAFT));
    select_reply(&mut h, "reply", "reply");
    assert!(button(&mut h).is_some());
    h.keystroke("ctrl-alt-q");
    verify(&mut h);
}
