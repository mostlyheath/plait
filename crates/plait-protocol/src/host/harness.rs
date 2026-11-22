//! Client side of version 4 harness choice: which agent harnesses can run a
//! thread here, and which one runs each thread. docs/HOST.md "Agent harnesses"
//! is the contract.
//!
//! New harnesses, states, and fields may be added, so unknown ones decode as
//! `Other` or are ignored rather than failing the reply.

use super::Model;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The agent harness that runs a thread. Pi is built into the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessId {
    #[default]
    Pi,
    Codex,
    /// Claude Code, run through Anthropic's Agent SDK once the person sets it up.
    Claude,
    /// A harness from a newer host. It can be shown, never chosen.
    #[serde(other, skip_serializing)]
    Other,
}

impl HarnessId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pi => "Pi",
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Other => "Another harness",
        }
    }
}

/// Which harness ran a thread, and the version of its latest run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ThreadHarness {
    pub id: HarnessId,
    /// `None` before the first run, and for Pi, which ships with the host.
    pub version: Option<String>,
    /// The harness's own permission mode, one of its `modes`. `None` for Pi,
    /// whose threads use Plait's approval mode.
    #[serde(default)]
    pub mode: Option<String>,
    /// The mode the harness reported using for the latest run, which its own
    /// configuration can override; `custom` when it matches none of its modes.
    #[serde(default)]
    pub reported: Option<String>,
}

/// One of a harness's own permission modes, named as the harness names it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HarnessMode {
    pub id: String,
    pub label: String,
    /// What the mode lets the harness do, in one sentence.
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Hosts from before version 4 reject it as `invalid_request`.
    Harnesses { protocol_version: u32 },
    /// Run this harness from `path` instead of the login shell's PATH; `None` returns
    /// to PATH. The reply is the whole list, checked again.
    SetHarnessPath {
        harness: HarnessId,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
        protocol_version: u32,
    },
    /// The mode new threads in this harness start in. The reply is the whole list.
    SetHarnessMode {
        harness: HarnessId,
        mode: String,
        protocol_version: u32,
    },
    /// Download what a `SetupRequired` harness needs; only Claude Code has
    /// setup. The reply is the whole list, that harness checked again, and a
    /// failed setup stays `SetupRequired` with a note saying why.
    SetUpHarness {
        harness: HarnessId,
        protocol_version: u32,
    },
    /// Install the version Plait is tested with into the host's own folder; the
    /// person's own copy still wins. Older hosts reject it as `invalid_request`.
    /// The reply is the whole list, that harness checked again.
    InstallHarness {
        harness: HarnessId,
        protocol_version: u32,
    },
}

impl Request {
    pub fn harnesses() -> Self {
        Self::Harnesses {
            protocol_version: 4,
        }
    }

    pub fn set_harness_path(harness: HarnessId, path: Option<PathBuf>) -> Self {
        Self::SetHarnessPath {
            harness,
            path,
            protocol_version: 4,
        }
    }

    pub fn set_harness_mode(harness: HarnessId, mode: String) -> Self {
        Self::SetHarnessMode {
            harness,
            mode,
            protocol_version: 4,
        }
    }

    pub fn set_up_harness(harness: HarnessId) -> Self {
        Self::SetUpHarness {
            harness,
            protocol_version: 4,
        }
    }

    pub fn install_harness(harness: HarnessId) -> Self {
        Self::InstallHarness {
            harness,
            protocol_version: 4,
        }
    }
}

/// Whether a harness can start a thread now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessState {
    Ready,
    NotInstalled,
    /// Older than Plait supports. A newer one is `Ready` with a note.
    UnsupportedVersion,
    SignedOut,
    /// Installed, but it could not be started or asked; `fix` says why.
    Unavailable,
    /// The harness needs a download the person starts with `set_up_harness`.
    SetupRequired,
    #[serde(other)]
    Other,
}

impl HarnessState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::NotInstalled => "Not installed",
            Self::UnsupportedVersion => "Version not supported",
            Self::SignedOut => "Signed out",
            Self::Unavailable => "Unavailable",
            Self::SetupRequired => "Needs setup",
            Self::Other => "Unknown",
        }
    }
}

/// What a harness's threads can do in Plait.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Capabilities {
    /// A message sent during a run joins it; otherwise it waits for the next run.
    pub steer: bool,
    /// Each edit and command can be a Plait approval.
    pub approvals: bool,
    pub thinking: bool,
    pub usage: bool,
}

/// The versions Plait checks a harness against.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Supported {
    /// The oldest version Plait runs.
    pub min: String,
    /// The version Plait's message types come from; newer ones run untested.
    pub tested: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Harness {
    pub id: HarnessId,
    pub name: String,
    pub state: HarnessState,
    /// What to do when the state is not `Ready`, or a caveat when it is.
    pub note: Option<String>,
    pub version: Option<String>,
    /// `None` for Pi, which ships with the host.
    pub supported: Option<Supported>,
    /// The executable used, when the harness is the person's own install.
    pub path: Option<PathBuf>,
    pub capabilities: Capabilities,
    /// What protects the checkout in this harness's threads, in one line.
    pub protection: String,
    /// `None` for Pi, whose models are in the status catalog.
    pub models: Option<Vec<Model>>,
    /// The harness's own permission modes; empty for Pi.
    #[serde(default)]
    pub modes: Vec<HarnessMode>,
    /// The mode new threads start in; `None` for Pi.
    #[serde(default)]
    pub default_mode: Option<String>,
    /// The version the host can install itself, when it has a build for its platform.
    #[serde(default)]
    pub installable: Option<String>,
    /// The copy that runs is the one the host installed, not the person's own.
    #[serde(default)]
    pub managed: bool,
}

impl Harness {
    pub fn mode(&self, id: &str) -> Option<&HarnessMode> {
        self.modes.iter().find(|mode| mode.id == id)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Harnesses {
    pub harnesses: Vec<Harness>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_version_4() {
        assert_eq!(
            serde_json::to_value(Request::harnesses()).unwrap(),
            serde_json::json!({"op": "harnesses", "protocol_version": 4})
        );
        assert_eq!(
            serde_json::to_value(Request::set_harness_path(
                HarnessId::Codex,
                Some("/opt/codex".into())
            ))
            .unwrap(),
            serde_json::json!({"op": "set_harness_path", "harness": "codex", "path": "/opt/codex", "protocol_version": 4})
        );
        assert_eq!(
            serde_json::to_value(Request::set_harness_path(HarnessId::Codex, None)).unwrap(),
            serde_json::json!({"op": "set_harness_path", "harness": "codex", "protocol_version": 4})
        );
        assert_eq!(
            serde_json::to_value(Request::set_harness_mode(HarnessId::Codex, "ask".into()))
                .unwrap(),
            serde_json::json!({"op": "set_harness_mode", "harness": "codex", "mode": "ask", "protocol_version": 4})
        );
        assert_eq!(
            serde_json::to_value(Request::set_up_harness(HarnessId::Claude)).unwrap(),
            serde_json::json!({"op": "set_up_harness", "harness": "claude", "protocol_version": 4})
        );
        assert_eq!(
            serde_json::to_value(Request::install_harness(HarnessId::Codex)).unwrap(),
            serde_json::json!({"op": "install_harness", "harness": "codex", "protocol_version": 4})
        );
    }

    #[test]
    fn a_thread_harness_reads_its_mode_and_older_snapshots_have_none() {
        let older: ThreadHarness =
            serde_json::from_str(r#"{"id":"codex","version":"0.169.0"}"#).unwrap();
        assert_eq!((older.mode, older.reported), (None, None));
        let current: ThreadHarness = serde_json::from_str(
            r#"{"id":"codex","version":"0.169.0","mode":"ask","reported":"read_only"}"#,
        )
        .unwrap();
        assert_eq!(current.mode.as_deref(), Some("ask"));
        assert_eq!(current.reported.as_deref(), Some("read_only"));
        let claude: ThreadHarness = serde_json::from_str(
            r#"{"id":"claude","version":"2.1.326","mode":"auto","reported":null}"#,
        )
        .unwrap();
        assert_eq!(claude.id, HarnessId::Claude);
        assert_eq!(claude.id.label(), "Claude Code");
        assert_eq!(
            (claude.mode.as_deref(), claude.reported),
            (Some("auto"), None)
        );
    }

    /// The Claude Code entry as the host sends it before setup, and after.
    const CLAUDE_MODES: &str = r#"[
        {"id":"ask","label":"Ask","detail":"Claude Code asks before each edit and command your Claude Code settings do not already allow."},
        {"id":"accept_edits","label":"Accept edits","detail":"Claude Code edits files in this checkout without asking, and asks before other commands."},
        {"id":"plan","label":"Plan","detail":"Claude Code reads and plans without changing anything, and asks before it starts on the plan."},
        {"id":"auto","label":"Auto","detail":"Claude Code's own safety check approves or blocks each request, and asks only when it cannot decide."},
        {"id":"bypass","label":"Bypass","detail":"Claude Code never asks and has no sandbox: it can change any file and run any command."}]"#;

    fn claude_entry(state: &str, note: &str, version: &str, path: &str, models: &str) -> String {
        format!(
            r#"{{"id":"claude","name":"Claude Code","state":"{state}","note":{note},"version":{version},
               "supported":{{"min":"2.1.290","tested":"2.1.326"}},"path":{path},
               "capabilities":{{"steer":true,"approvals":true,"thinking":true,"usage":true}},
               "protection":"The thread's Claude Code mode and your Claude Code settings.",
               "modes":{CLAUDE_MODES},"default_mode":"auto","models":{models}}}"#
        )
    }

    #[test]
    fn claude_code_decodes_before_and_after_setup() {
        let before: Harness = serde_json::from_str(&claude_entry(
            "setup_required",
            r#""Set up Claude Code threads to download Anthropic's Agent SDK 0.3.326 from the npm registry to this Mac.""#,
            "null",
            "null",
            "[]",
        ))
        .unwrap();
        assert_eq!(
            (before.id, before.state),
            (HarnessId::Claude, HarnessState::SetupRequired)
        );
        assert_eq!(before.state.label(), "Needs setup");
        assert!(
            before
                .note
                .unwrap()
                .starts_with("Set up Claude Code threads")
        );
        assert!(before.version.is_none() && before.path.is_none());
        assert_eq!(before.modes.len(), 5);
        assert_eq!(before.default_mode.as_deref(), Some("auto"));
        assert!(before.models.unwrap().is_empty());

        let after: Harness = serde_json::from_str(&claude_entry(
            "ready",
            "null",
            r#""2.1.326""#,
            r#""/opt/sample/bin/claude""#,
            r#"[{"provider":"claude","model_id":"default","name":"Default","authenticated":true,
                 "reasoning":true,"thinking_levels":["low","medium","high"],"context_window":null,"input":["text"]}]"#,
        ))
        .unwrap();
        assert_eq!(after.state, HarnessState::Ready);
        assert_eq!(after.version.as_deref(), Some("2.1.326"));
        let models = after.models.as_ref().unwrap();
        assert_eq!(
            (models[0].provider.as_str(), models[0].model_id.as_str()),
            ("claude", "default")
        );
        assert_eq!(after.mode("bypass").unwrap().label, "Bypass");
    }

    #[test]
    fn a_reply_decodes_and_tolerates_later_additions() {
        let reply: Harnesses = serde_json::from_str(
            r#"{"harnesses":[
                {"id":"pi","name":"Pi","state":"ready","note":null,"version":null,"supported":null,"path":null,
                 "capabilities":{"steer":true,"approvals":true,"thinking":true,"usage":true},
                 "protection":"Plait's sandbox and approvals","models":null},
                {"id":"codex","name":"Codex","state":"ready","note":"Newer than tested.","version":"0.170.0",
                 "supported":{"min":"0.169.0","tested":"0.169.0"},"path":"/bin/codex",
                 "capabilities":{"steer":true,"approvals":true,"thinking":true,"usage":true,"later":1},
                 "protection":"Codex's sandbox","models":[{"provider":"codex","model_id":"m","name":"M","authenticated":true}],
                 "modes":[{"id":"ask","label":"Ask","detail":"Asks."},{"id":"full_access","label":"Full access","detail":"Never asks."}],
                 "default_mode":"full_access"},
                {"id":"someday","name":"Someday","state":"warming_up","note":null,"version":null,"supported":null,
                 "path":null,"capabilities":{"steer":false,"approvals":false,"thinking":false,"usage":false},
                 "protection":"","models":[],"later":true}],
              "later":true}"#,
        )
        .unwrap();
        let [pi, codex, other] = &reply.harnesses[..] else {
            panic!("three harnesses")
        };
        assert_eq!((pi.id, pi.state), (HarnessId::Pi, HarnessState::Ready));
        assert!(pi.models.is_none());
        assert_eq!(codex.supported.as_ref().unwrap().tested, "0.169.0");
        assert_eq!(codex.models.as_ref().unwrap()[0].model_id, "m");
        assert_eq!(codex.default_mode.as_deref(), Some("full_access"));
        assert_eq!(codex.mode("ask").unwrap().label, "Ask");
        assert!(pi.modes.is_empty() && pi.default_mode.is_none());
        assert_eq!(
            (other.id, other.state),
            (HarnessId::Other, HarnessState::Other)
        );
    }
}
