//! Ways out of Plait: Open in Editor, Show in Finder, and Open Terminal Here.
//!
//! Each picks what is on screen (the file in Files, or the file selected in
//! Changes, and a line in it), builds the command from Settings → Editor
//! (crate::handoff), and starts it on the background executor. Plait
//! does not wait for it: only a command that cannot start, or exits at once
//! with an error, is reported, in the notice line.

use super::{Plait, Workspace, file_view::line_height, theme::*, thread_controls::TextTooltip};
use crate::handoff::{self};
use gpui::{prelude::*, *};
use plait_client::handoff::Launch;
use plait_client::{
    file_view::{self, Contents},
    model::Change,
    state::Load,
};
use std::path::PathBuf;

// Bound in the action table (actions.rs), like every application shortcut.
actions!(plait, [OpenInEditor, ShowInFinder, OpenTerminal]);

/// Said when there is nothing on screen to open.
const NO_FILE: &str = "Open a file in Files, or select one in Changes, to open it elsewhere.";

/// Long enough for `open` and `osascript` to hand off, or to fail because the
/// application is missing; short enough that nothing waits on an editor.
#[cfg(not(test))]
const WATCH: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(not(test))]
const WATCH_STEP: std::time::Duration = std::time::Duration::from_millis(50);

#[derive(Default)]
pub(super) struct WaysOut {
    /// The last failure put in the notice line, so a later start clears it
    /// and nothing else's notice.
    failure: Option<String>,
    /// Tests record what would start; no test starts a process.
    #[cfg(test)]
    pub started: Vec<Launch>,
}

/// The file a way out acts on, and the line to open it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    /// As the checkout names it, for hover text.
    pub display: String,
    pub absolute: PathBuf,
    /// One-based.
    pub line: usize,
}

/// A changed file that is no longer on disk has nothing to open.
pub(super) fn on_disk(change: &Change) -> bool {
    change.worktree != b'D' && !(change.index == b'D' && change.worktree == b' ')
}

/// The first line on screen in a list of `file_view::line_height()` rows,
/// zero-based. A scroll asked for but not yet drawn counts as done.
fn top_line(scroll: &UniformListScrollHandle) -> usize {
    let state = scroll.0.borrow();
    if let Some(deferred) = &state.deferred_scroll_to_item {
        return deferred.item_index;
    }
    let scrolled = -f32::from(state.base_handle.offset().y);
    (scrolled / line_height()).floor().max(0.) as usize
}

impl Plait {
    pub(super) fn on_ways_out_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_action(cx.listener(|this, _: &OpenInEditor, _, cx| this.open_in_editor(cx)))
            .on_action(cx.listener(|this, _: &ShowInFinder, _, cx| this.show_in_finder(cx)))
            .on_action(cx.listener(|this, _: &OpenTerminal, _, cx| this.open_terminal(cx)))
    }

    /// What is on screen: the file in Files, or the file selected in Changes.
    pub(super) fn way_out_target(&self) -> Option<Target> {
        if !self.host_is_local() || self.settings_open {
            return None;
        }
        if self.panel_is_open() {
            use plait_client::panel::PanelKind;
            return match self.panel_kind()? {
                PanelKind::Files => self.files_target(),
                PanelKind::Changes if !self.panel_recorded() => self.changes_target(),
                PanelKind::Changes => {
                    let file = self.recorded_file()?;
                    if file.label == "Deleted"
                        || !file
                            .path
                            .components()
                            .all(|part| matches!(part, std::path::Component::Normal(_)))
                    {
                        return None;
                    }
                    Some(Target {
                        display: file.display_path.clone(),
                        absolute: self.file_root().join(&file.path),
                        line: self
                            .workspace
                            .turn_review
                            .diff
                            .as_ref()
                            .and_then(|diff| handoff::first_changed_line(&diff.lines))
                            .unwrap_or(1),
                    })
                }
                PanelKind::History => None,
            };
        }
        if let Some(kind) = self.focused_panel_kind() {
            use plait_client::panes::PaneKind;
            return match kind {
                PaneKind::Changes if !self.panel_recorded() => self.changes_target(),
                PaneKind::Files => self.files_target(),
                _ => None,
            };
        }
        match self.workspace.tab {
            Workspace::Files => self.files_target(),
            Workspace::Changes => self.changes_target(),
            Workspace::Conversation | Workspace::History => None,
        }
    }

    /// The file in Files, at the current find match, or else at the first
    /// line on screen.
    fn files_target(&self) -> Option<Target> {
        let viewer = &self.workspace.file_viewer;
        let path = viewer.path.as_ref()?;
        let text = match viewer.opened.as_deref().map(|opened| &opened.contents) {
            Some(Contents::Text(text)) => Some(text),
            _ => None,
        };
        let found = viewer
            .find
            .open
            .then(|| viewer.find.matches.get(viewer.find.current))
            .flatten();
        let index = match (found, text) {
            (Some(found), Some(text)) => file_view::line_of(&text.lines, found.start),
            _ => viewer.reveal.unwrap_or_else(|| top_line(&viewer.scroll)),
        };
        let count = text.map_or(1, |text| text.lines.len().max(1));
        Some(Target {
            display: path.to_string_lossy().into_owned(),
            absolute: viewer.root.join(path),
            line: index.min(count - 1) + 1,
        })
    }

    /// The file selected in Changes, at the picked line when lines are
    /// picked, or else at the diff's first change.
    fn changes_target(&self) -> Option<Target> {
        if self.workspace.turn_review.target.is_some() && !self.recorded_current_available() {
            return None;
        }
        let repo = &self.state.repository;
        let key = repo.selected_diff()?;
        let snapshot = repo.snapshot().ready()?;
        let change = snapshot
            .changes
            .get(key.file)
            .filter(|change| on_disk(change))?;
        let line = match &repo.diff() {
            Load::Ready(diff) => match self.current_pick(diff.lines.len()) {
                Some(pick) => handoff::line_near(&diff.lines, *pick.range().start()),
                None => handoff::first_changed_line(&diff.lines),
            },
            _ => None,
        };
        Some(Target {
            display: change.display_path(),
            absolute: snapshot.root.join(&change.path),
            line: line.unwrap_or(1),
        })
    }

    /// Hover text for an Editor control: what it opens, where, and with what.
    pub(super) fn editor_hover(&self) -> String {
        if !self.host_is_local() {
            return super::REMOTE_UNAVAILABLE.into();
        }
        let Some(target) = self.way_out_target() else {
            return NO_FILE.into();
        };
        let choice = &self.preferences.value.open_files_with;
        let line = if choice.takes_line() {
            format!(" at line {}", target.line)
        } else {
            String::new()
        };
        format!("Opens {}{line} {}", target.display, choice.describe())
    }

    pub(super) fn finder_hover(&self) -> String {
        if !self.host_is_local() {
            return super::REMOTE_UNAVAILABLE.into();
        }
        self.way_out_target().map_or_else(
            || NO_FILE.into(),
            |target| format!("Shows {} in Finder", target.display),
        )
    }

    pub(super) fn open_in_editor(&mut self, cx: &mut Context<Self>) {
        if !self.host_is_local() {
            return self.way_out_failed(super::REMOTE_UNAVAILABLE.into(), cx);
        }
        let Some(target) = self.way_out_target() else {
            return self.way_out_failed(NO_FILE.into(), cx);
        };
        let launch = handoff::open_in_editor(
            &self.preferences.value.open_files_with,
            &target.absolute,
            target.line,
            &self.file_root(),
        );
        self.hand_off(launch, cx);
    }

    pub(super) fn show_in_finder(&mut self, cx: &mut Context<Self>) {
        if !self.host_is_local() {
            return self.way_out_failed(super::REMOTE_UNAVAILABLE.into(), cx);
        }
        let Some(target) = self.way_out_target() else {
            return self.way_out_failed(NO_FILE.into(), cx);
        };
        self.hand_off(handoff::show_in_finder(&target.absolute), cx);
    }

    /// A terminal at the open checkout, whatever is on screen.
    pub(super) fn open_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.host_is_local() {
            return self.way_out_failed(super::REMOTE_UNAVAILABLE.into(), cx);
        }
        let choice = &self.preferences.value.open_terminals_with;
        let launch = handoff::open_terminal(choice, &self.file_root());
        self.hand_off(launch, cx);
    }

    pub(super) fn hand_off(&mut self, launch: Result<Launch, String>, cx: &mut Context<Self>) {
        if !self.host_is_local() {
            return self.way_out_failed(super::REMOTE_UNAVAILABLE.into(), cx);
        }
        match launch {
            Ok(launch) => {
                // A new attempt replaces the last failure's sentence.
                if self.notice.is_some() && self.notice == self.ways_out.failure.take() {
                    self.notice = None;
                }
                self.start_process(launch, cx);
                cx.notify();
            }
            Err(error) => self.way_out_failed(error, cx),
        }
    }

    fn way_out_failed(&mut self, sentence: String, cx: &mut Context<Self>) {
        self.ways_out.failure = Some(sentence.clone());
        self.notice = Some(sentence);
        cx.notify();
    }

    #[cfg(test)]
    fn start_process(&mut self, launch: Launch, _: &mut Context<Self>) {
        self.ways_out.started.push(launch);
    }

    /// Start the command off the UI thread and report only an early failure.
    #[cfg(not(test))]
    fn start_process(&mut self, launch: Launch, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let task = cx
            .background_executor()
            .spawn(async move { watch(launch, executor).await });
        cx.spawn(async move |this, cx| {
            if let Err(sentence) = task.await {
                let _ = this.update(cx, |this, cx| this.way_out_failed(sentence, cx));
            }
        })
        .detach();
    }
}

/// Start `launch`, then look at it for a moment without blocking a thread:
/// an exit with an error then is reported; after that it is left to run.
#[cfg(not(test))]
async fn watch(launch: Launch, executor: BackgroundExecutor) -> Result<(), String> {
    let mut child = handoff::start(&launch)?;
    let steps = WATCH.as_millis() / WATCH_STEP.as_millis();
    for _ in 0..steps {
        match handoff::check(&mut child, &launch) {
            handoff::Outcome::Running => executor.timer(WATCH_STEP).await,
            handoff::Outcome::Done => return Ok(()),
            handoff::Outcome::Failed(sentence) => return Err(sentence),
        }
    }
    handoff::release(child);
    Ok(())
}

/// A control that opens the file on screen in the editor. Its hover text is
/// worked out only when it shows.
pub(super) fn editor_button(
    id: &'static str,
    name: &str,
    shortcut: String,
    local: bool,
    compact: bool,
    cx: &mut Context<Plait>,
) -> super::theme::Control {
    let plait = cx.weak_entity();
    let label = format!("Open {name} in your editor");
    let editor = if compact {
        super::theme::icon_button(id, "open-external", label)
    } else {
        super::theme::button(id, "Editor").aria_label(label)
    };
    editor
        .disabled(!local)
        .aria_keyshortcuts(shortcut)
        .tooltip(move |_, cx| {
            let text = plait
                .upgrade()
                .map(|plait| plait.read(cx).editor_hover())
                .unwrap_or_default();
            cx.new(|_| TextTooltip(text)).into()
        })
        .on_activate(cx, |this, _, cx| this.open_in_editor(cx))
}

/// A control that shows the file on screen in Finder.
pub(super) fn finder_button(
    id: &'static str,
    name: &str,
    local: bool,
    cx: &mut Context<Plait>,
) -> super::theme::Control {
    let plait = cx.weak_entity();
    super::theme::button(id, "Finder")
        .disabled(!local)
        .aria_label(format!("Show {name} in Finder"))
        .tooltip(move |_, cx| {
            let text = plait
                .upgrade()
                .map(|plait| plait.read(cx).finder_hover())
                .unwrap_or_default();
            cx.new(|_| TextTooltip(text)).into()
        })
        .on_activate(cx, |this, _, cx| this.show_in_finder(cx))
}
