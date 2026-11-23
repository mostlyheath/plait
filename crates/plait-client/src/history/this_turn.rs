//! Projection shared by the turn inspector and the conversation's undo control.
use super::records::{Availability, File, Line, Lines};
use crate::host::ThreadStatus;

#[derive(Clone, Copy, Default)]
pub struct UndoContext<'a> {
    pub note: Option<&'a (String, &'static str, String)>,
    pub connected: bool,
    pub pending: bool,
    pub current: bool,
    pub busy: bool,
}

pub type Refusal = (&'static str, &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndoAction<'a> {
    pub label: &'static str,
    pub detail: &'a str,
    pub enabled: bool,
}

impl<'a> UndoContext<'a> {
    pub fn action(self, line: &'a Line) -> UndoAction<'a> {
        if let Some((label, detail)) = self.refusal(line) {
            return UndoAction {
                label,
                detail,
                enabled: false,
            };
        }
        if let Some((_, label, detail)) = self.note.filter(|(id, _, _)| id == &line.run) {
            return UndoAction {
                label,
                detail,
                enabled: matches!(*label, "Retry" | "Wait"),
            };
        }
        UndoAction {
            label: if line.undone { "Redo" } else { "Undo" },
            detail: &line.detail,
            enabled: true,
        }
    }

    pub fn refusal(self, line: &Line) -> Option<Refusal> {
        if !self.connected {
            return Some(("Offline", "Reconnect to the host to undo this run."));
        }
        if self.pending {
            return Some(("Working", "An undo is waiting for the host's reply."));
        }
        if !self.current {
            return Some(("Loading", "Waiting for the current run records."));
        }
        if self.busy {
            return Some((
                "Wait",
                "A run or another write is active in this checkout. Wait until it finishes.",
            ));
        }
        match line.availability {
            Availability::Recording => {
                Some(("Recording", "The host is still taking this run's record."))
            }
            Availability::Failed => Some((
                "Unavailable",
                "The record could not be made, so this run cannot be undone.",
            )),
            Availability::Unavailable => Some((
                "Unavailable",
                "The record needed to undo this run is no longer retained.",
            )),
            Availability::Ready => None,
        }
    }
}

#[derive(Default)]
pub struct Selection(Option<String>);
impl Selection {
    pub fn earlier(&mut self, run: &str) {
        self.0 = Some(run.to_owned());
    }
    pub fn latest(&mut self) {
        self.0 = None;
    }
    /// Forgets an earlier turn the records no longer hold, so it cannot
    /// come back into view by itself when they hold it again.
    pub fn retain(&mut self, lines: &Lines) {
        if self
            .0
            .as_ref()
            .is_some_and(|run| !lines.runs.contains_key(run))
        {
            self.0 = None;
        }
    }
    pub fn is_earlier(&self) -> bool {
        self.0.is_some()
    }
}

pub struct Panel<'a> {
    pub heading: &'static str,
    pub state: String,
    pub line: Option<&'a Line>,
    pub files: &'a [File],
    pub more: usize,
    pub action: Option<UndoAction<'a>>,
}

pub fn panel<'a>(
    lines: &'a Lines,
    selection: &Selection,
    status: ThreadStatus,
    undo: UndoContext<'a>,
) -> Option<Panel<'a>> {
    let newest = lines.newest.as_ref();
    let earlier = selection
        .0
        .as_ref()
        .filter(|id| Some(*id) != newest)
        .and_then(|id| lines.runs.get(id));
    let working = earlier.is_none() && status.busy();
    let line = earlier.or_else(|| {
        let id = if working {
            lines
                .active_run
                .as_ref()
                .filter(|_| lines.live_status.is_some_and(|s| s.busy()))
        } else {
            newest
        }?;
        lines.runs.get(id)
    });
    if !working && line.is_none() {
        return None;
    }
    // A historical record contains steps and paths, but no terminal thread status.
    let terminal = earlier.is_none() && lines.active_run.as_ref() == newest;
    let word = if working {
        "Working"
    } else if line.is_some_and(|l| l.undone) {
        "Undone"
    } else if terminal {
        match status {
            ThreadStatus::Stopped => "Stopped",
            ThreadStatus::Interrupted => "Interrupted",
            ThreadStatus::Error => "Failed",
            _ => "Finished",
        }
    } else {
        ""
    };
    let state = match (word, line.and_then(|l| l.steps)) {
        ("Finished" | "", Some(n)) => {
            let steps = format!("{n} {}", if n == 1 { "step" } else { "steps" });
            if word.is_empty() {
                steps
            } else {
                format!("{word} · {steps}")
            }
        }
        _ => word.to_owned(),
    };
    let ready = line.filter(|l| l.availability == Availability::Ready);
    Some(Panel {
        heading: if earlier.is_some() {
            "Earlier turn"
        } else {
            "This turn"
        },
        state,
        line,
        files: ready.map_or(&[], |l| l.files.as_slice()),
        more: ready.map_or(0, |l| l.paths_total.saturating_sub(l.files.len())),
        // A turn still running has no closing line, and so no Undo yet. A
        // note about an undo of it that needs attention is still shown.
        action: line
            .filter(|l| !working || undo.note.is_some_and(|(run, _, _)| run == &l.run))
            .map(|l| undo.action(l)),
    })
}
