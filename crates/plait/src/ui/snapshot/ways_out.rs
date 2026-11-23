//! Ways out: which file and line Open in Editor, Show in Finder, and Open
//! Terminal Here pick, Settings → Editor, and its scenes.
//!
//! Test builds record the command an action would start instead of starting
//! it, so nothing here opens an editor, Finder, a terminal, or Rex.

use super::{Harness, SettingsTab, fixture_repository, isolated};
use crate::handoff;
use crate::ui::{
    Plait, Workspace, file_view::LINE_HEIGHT, settings::SettingsPage, settings_editor::Opens,
};
use gpui::*;
use plait_client::{
    handoff::{EditorChoice, Launch, TerminalChoice},
    model::DiffMode,
    state::Load,
};
use std::path::Path;

/// Records the line and the project with the file.
const RECORDING_EDITOR: &str = "ed {file} {line} {project}";

/// What the actions would have started since the last call.
fn started(harness: &mut Harness) -> Vec<Launch> {
    harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            std::mem::take(&mut plait.ways_out.started)
        })
        .expect("window update")
}

/// The one launch since the last call: the recording editor's arguments.
fn opened(harness: &mut Harness) -> Vec<String> {
    let mut launches = started(harness);
    assert_eq!(launches.len(), 1, "{launches:?}");
    let launch = launches.remove(0);
    assert_eq!(launch.program, "ed");
    launch.args
}

fn read<T>(harness: &mut Harness, read: impl FnOnce(&mut Plait) -> T) -> T {
    harness
        .window
        .update(&mut harness.cx, |plait, _, _| read(plait))
        .expect("window update")
}

/// Index of `path` in Changes' list.
fn change(plait: &Plait, path: &str) -> usize {
    let snapshot = plait
        .state
        .repository
        .snapshot()
        .ready()
        .expect("status read");
    let index = snapshot
        .changes
        .iter()
        .position(|change| change.path == Path::new(path));
    index.expect("a changed file")
}

#[core::prelude::v1::test]
fn open_what_is_on_screen() {
    // Named from the snapshot module, as libtest names it.
    if !isolated("ways_out::open_what_is_on_screen") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-ways-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let long = (1..=200).map(|n| format!("line {n}\n")).collect::<String>();
    std::fs::write(repository.join("src/long.txt"), long).unwrap();
    let absolute = |path: &str| repository.join(path).to_string_lossy().into_owned();
    let root = repository.to_string_lossy().into_owned();
    let mut harness = Harness::new(&repository, None, 900., 600.);
    // In memory only: the recording editor is never saved.
    harness.update(|plait, _, _| {
        plait.preferences.value.open_files_with = EditorChoice::Custom(RECORDING_EDITOR.into());
    });

    // Nothing on screen to open: one sentence, and nothing starts.
    harness.update(|plait, _, cx| plait.open_in_editor(cx));
    assert!(started(&mut harness).is_empty());
    let notice = read(&mut harness, |plait| plait.notice.clone()).expect("a notice");
    assert!(notice.contains("Open a file in Files"), "{notice}");

    // Files, at the top, by the shortcut. Starting clears the failure.
    harness.update(|plait, _, cx| plait.open_file("src/long.txt".into(), None, cx));
    harness.draw();
    harness.keystroke("cmd-alt-e");
    assert_eq!(
        opened(&mut harness),
        [absolute("src/long.txt"), "1".into(), root.clone()]
    );
    assert_eq!(read(&mut harness, |plait| plait.notice.clone()), None);

    // The first line on screen after scrolling.
    harness.update(|plait, _, _| {
        let offset = point(px(0.), px(-LINE_HEIGHT * 40.));
        plait
            .workspace
            .file_viewer
            .scroll
            .0
            .borrow()
            .base_handle
            .set_offset(offset);
    });
    harness.draw();
    harness.update(|plait, _, cx| {
        assert_eq!(
            plait.editor_hover(),
            "Opens src/long.txt at line 41 with ed"
        );
        plait.open_in_editor(cx);
    });
    assert_eq!(opened(&mut harness)[1], "41");

    // A line asked for but not drawn yet counts as shown.
    harness.update(|plait, _, cx| {
        plait.open_file("src/long.txt".into(), Some(119), cx);
        plait.open_in_editor(cx);
    });
    assert_eq!(opened(&mut harness)[1], "120");
    harness.draw();

    // The current find match wins over the scroll position.
    harness.update(|plait, window, cx| {
        plait.open_find(window, cx);
        let field = plait.workspace.file_viewer.find.field.clone().unwrap();
        field.update(cx, |field, cx| field.set_text("line 150", cx));
    });
    harness.update(|plait, _, cx| plait.open_in_editor(cx));
    assert_eq!(opened(&mut harness)[1], "150");
    harness.update(|plait, window, cx| plait.close_find(window, cx));

    // Show in Finder and Open Terminal Here act on the same file and checkout.
    harness.update(|plait, _, cx| {
        plait.show_in_finder(cx);
        plait.open_terminal(cx);
    });
    let launches = started(&mut harness);
    assert_eq!(launches[0].program, "/usr/bin/open");
    assert_eq!(
        launches[0].args,
        ["-R".to_owned(), absolute("src/long.txt")]
    );
    assert_eq!(
        launches[1].args,
        ["-b".to_owned(), "com.apple.Terminal".into(), root.clone()]
    );

    // Changes: the first changed line of the diff on screen.
    harness.update(|plait, window, cx| {
        let index = change(plait, "src/review.rs");
        plait.select_file(index, DiffMode::Unstaged, window, cx);
    });
    harness.update(|plait, _, cx| {
        let Load::Ready(diff) = &plait.state.repository.diff() else {
            panic!("the diff is read");
        };
        assert_eq!(handoff::first_changed_line(&diff.lines), Some(3));
        plait.open_in_editor(cx);
    });
    assert_eq!(
        opened(&mut harness),
        [absolute("src/review.rs"), "3".into(), root.clone()]
    );
    // A picked line wins over the first change.
    harness.update(|plait, _, cx| {
        let Load::Ready(diff) = &plait.state.repository.diff() else {
            panic!("the diff is read");
        };
        let first = diff.lines.iter().position(|line| line.new_line == Some(1));
        plait.pick_line(first.expect("line 1 in the diff"), false, cx);
        plait.open_in_editor(cx);
    });
    assert_eq!(opened(&mut harness)[1], "1");
    // The staged side starts with a removed line: it opens where that was.
    harness.update(|plait, window, cx| {
        plait.workspace.line_pick = None;
        let index = change(plait, "src/review.rs");
        plait.select_file(index, DiffMode::Staged, window, cx);
    });
    harness.update(|plait, _, cx| plait.open_in_editor(cx));
    assert_eq!(opened(&mut harness)[1], "2");
    // A deleted file has nothing to open.
    harness.update(|plait, window, cx| {
        let index = change(plait, "README.md");
        plait.select_file(index, DiffMode::Unstaged, window, cx);
    });
    harness.update(|plait, _, cx| plait.open_in_editor(cx));
    assert!(started(&mut harness).is_empty());

    // The built-in Rex choices go through osascript.
    harness.update(|plait, window, cx| {
        plait.preferences.value.open_files_with = EditorChoice::RexNeovim;
        plait.preferences.value.open_terminals_with = TerminalChoice::Rex;
        let index = change(plait, "src/review.rs");
        plait.select_file(index, DiffMode::Unstaged, window, cx);
    });
    harness.update(|plait, _, cx| {
        assert_eq!(
            plait.editor_hover(),
            "Opens src/review.rs at line 3 in Neovim in a Rex tab"
        );
        plait.open_in_editor(cx);
        plait.open_terminal(cx);
    });
    let launches = started(&mut harness);
    let script = handoff::rex_editor_script(&absolute("src/review.rs"), 3, &root);
    assert_eq!(launches[0].program, "/usr/bin/osascript");
    assert_eq!(launches[0].args[5], script[2]);
    assert!(launches.iter().all(|launch| launch.rex));
    // A saved command that no longer reads fails with its reason.
    harness.update(|plait, _, cx| {
        plait.preferences.value.open_terminals_with = TerminalChoice::Custom("kitty {file}".into());
        plait.open_terminal(cx);
    });
    assert!(started(&mut harness).is_empty());
    let notice = read(&mut harness, |plait| plait.notice.clone()).expect("a notice");
    assert!(notice.contains("{project}"), "{notice}");

    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

#[core::prelude::v1::test]
fn settings_save_only_a_usable_command() {
    if !isolated("ways_out::settings_save_only_a_usable_command") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    let typed = |harness: &mut Harness, opens: Opens, text: &'static str| {
        harness.update(move |plait, _, cx| {
            let field = plait.editor_settings.get(opens).field.clone().unwrap();
            field.update(cx, |field, cx| field.set_text(text, cx));
        });
    };
    let choices = |harness: &mut Harness| {
        read(harness, |plait| {
            let value = &plait.preferences.value;
            (
                value.open_files_with.clone(),
                value.open_terminals_with.clone(),
            )
        })
    };
    harness.update(|plait, window, cx| {
        plait.open_page(Some(SettingsPage::Application), window, cx);
        plait.settings_tab = SettingsTab::Editor;
        plait.choose_command(Opens::Files, 1, cx);
    });
    harness.draw();
    assert_eq!(choices(&mut harness).0, EditorChoice::RexNeovim);

    // Custom with nothing usable keeps the saved choice and says why.
    harness.update(|plait, _, cx| plait.choose_command(Opens::Files, 2, cx));
    typed(&mut harness, Opens::Files, "zed '{file}");
    let error = read(&mut harness, |plait| {
        plait.editor_settings.files.error.clone()
    });
    assert!(error.unwrap().contains("single quote"));
    assert_eq!(choices(&mut harness).0, EditorChoice::RexNeovim);
    harness.draw();
    typed(&mut harness, Opens::Files, "zed {file}:{line}");
    assert_eq!(
        choices(&mut harness).0,
        EditorChoice::Custom("zed {file}:{line}".into())
    );
    assert!(read(&mut harness, |plait| plait
        .editor_settings
        .files
        .error
        .is_none()));

    // A terminal command may use only {project}.
    harness.update(|plait, _, cx| plait.choose_command(Opens::Terminals, 2, cx));
    typed(&mut harness, Opens::Terminals, "kitty {file}");
    assert_eq!(choices(&mut harness).1, TerminalChoice::Terminal);
    harness.update(|plait, _, cx| plait.choose_command(Opens::Terminals, 1, cx));
    assert_eq!(choices(&mut harness).1, TerminalChoice::Rex);
    harness.draw();

    // Both reached the file in this test's own home.
    let path = plait_client::preferences::default_path().expect("a preferences path");
    let saved = plait_client::preferences::load(&path).expect("saved preferences");
    assert_eq!(
        saved.open_files_with,
        EditorChoice::Custom("zed {file}:{line}".into())
    );
    assert_eq!(saved.open_terminals_with, TerminalChoice::Rex);

    // Reopened, Settings shows the saved command in its field.
    harness.update(|plait, window, cx| {
        plait.open_page(None, window, cx);
        plait.open_page(Some(SettingsPage::Application), window, cx);
        let files = &plait.editor_settings.files;
        assert!(files.custom && !plait.editor_settings.terminals.custom);
        assert_eq!(
            files.field.as_ref().unwrap().read(cx).text(),
            "zed {file}:{line}"
        );
    });
    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Settings → Editor with its ready choices, then with a custom command that
/// cannot be used and one that can. In memory only: scenes never save.
pub(super) fn ways_out_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|plait, window, cx| {
        plait.open_page(Some(SettingsPage::Application), window, cx);
        plait.settings_tab = SettingsTab::Editor;
    });
    harness.capture(&format!("settings-editor-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.preferences.value.open_terminals_with =
            TerminalChoice::Custom("/opt/homebrew/bin/wezterm start --cwd {project}".into());
        plait.show_editor_commands(cx);
        // Invalid, so choosing it saves nothing.
        plait.editor_settings.files.custom = true;
        let field = plait.editor_settings.files.field.clone().unwrap();
        field.update(cx, |field, cx| field.set_text("zed \"{file}:{line}", cx));
    });
    harness.capture(&format!("settings-editor-custom-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.preferences.value.open_terminals_with = TerminalChoice::default();
        plait.show_editor_commands(cx);
        plait.open_page(None, window, cx);
        plait.settings_tab = SettingsTab::default();
        plait.workspace.tab = Workspace::Conversation;
    });
}
