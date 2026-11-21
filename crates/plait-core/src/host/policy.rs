//! Client side of the per-checkout command policy, and the wording that
//! depends on it. docs/HOST.md "Command policy" is the contract.
//!
//! A policy the desktop has not read is `None` everywhere here, and no
//! sentence then promises a sandbox.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How the `command` tool runs for runs in one checkout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPolicy {
    /// Opt-in per checkout: no checkout or Git writes, no network.
    Sandboxed,
    /// The default: commands run as the user, outside the sandbox.
    Trusted,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    CommandPolicy {
        checkout: PathBuf,
    },
    SetCommandPolicy {
        checkout: PathBuf,
        policy: CommandPolicy,
    },
}

/// The policy now in force for the canonical checkout. A policy name this
/// desktop does not know fails to load, so it is never shown as another one.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PolicyReply {
    pub checkout: PathBuf,
    pub policy: CommandPolicy,
}

impl CommandPolicy {
    /// Short label for the composer's context line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Sandboxed => "Sandboxed commands",
            Self::Trusted => "Trusted commands",
        }
    }

    /// What the policy means, in one line under its control.
    pub fn meaning(self) -> &'static str {
        match self {
            Self::Sandboxed => {
                "Commands read this checkout and write only private scratch and build caches. No network, no checkout or Git writes."
            }
            Self::Trusted => {
                "Commands run as you, outside the sandbox: full user access, network, and Git writes. They can change files you had already modified."
            }
        }
    }
}

/// What choosing Trusted means, stated before it is saved.
pub const TRUSTED_CONSEQUENCES: &[&str] = &[
    "Commands run as you, outside the sandbox, with full user access to your files, credentials, SSH agent, and keychain.",
    "They can use the network and write Git history: commit, reset, or push.",
    "They can change or delete files that were already modified before the run. Only the file tools keep protecting those.",
    "With automatic approval, they run without a prompt.",
];

/// When the change applies, shown with the confirmation.
pub const TRUSTED_SCOPE: &str = "Applies to this checkout on this host, from your next message. A run in progress keeps the policy it started with.";

/// The label on the composer's context line. Unknown is said, not guessed.
pub fn context_label(policy: Option<CommandPolicy>) -> &'static str {
    policy.map_or("Command policy not loaded", CommandPolicy::label)
}

/// The explanation of the application's default for edits and commands.
/// `ask`: each call waits for approval. It describes no one project: what a
/// command can reach is each project's own setting, so it promises no sandbox.
pub fn approval_detail(ask: bool) -> &'static str {
    if ask {
        "Each edit and command waits for Allow once."
    } else {
        "The agent edits files and runs commands without asking. What a command can reach is set in each project's settings. Applies from your next message."
    }
}

/// The empty workspace's invitation to start a thread.
pub fn idle_detail(ask: bool, policy: Option<CommandPolicy>) -> &'static str {
    match (ask, policy) {
        (true, Some(CommandPolicy::Trusted)) => {
            "Start a thread in this checkout. Every edit and command waits for your approval. Commands you allow run with full user access and network."
        }
        (true, _) => {
            "Start a thread in this checkout. Every edit and command waits for your approval."
        }
        (false, Some(CommandPolicy::Sandboxed)) => {
            "Start a thread in this checkout. The agent edits and runs sandboxed commands without asking; files you had already changed stay protected."
        }
        (false, Some(CommandPolicy::Trusted)) => {
            "Start a thread in this checkout. The agent edits and runs commands without asking. Commands in this project are trusted: full user access, network, and Git writes."
        }
        (false, None) => {
            "Start a thread in this checkout. The agent edits and runs commands without asking, under this project's command policy."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_match_the_documented_json() {
        let read = Request::CommandPolicy {
            checkout: "/absolute/checkout".into(),
        };
        assert_eq!(
            serde_json::to_string(&read).unwrap(),
            r#"{"op":"command_policy","checkout":"/absolute/checkout"}"#
        );
        let set = Request::SetCommandPolicy {
            checkout: "/absolute/checkout".into(),
            policy: CommandPolicy::Trusted,
        };
        assert_eq!(
            serde_json::to_string(&set).unwrap(),
            r#"{"op":"set_command_policy","checkout":"/absolute/checkout","policy":"trusted"}"#
        );
        let reply: PolicyReply =
            serde_json::from_str(r#"{"checkout":"/canonical/checkout","policy":"sandboxed"}"#)
                .unwrap();
        assert_eq!(reply.checkout, PathBuf::from("/canonical/checkout"));
        assert_eq!(reply.policy, CommandPolicy::Sandboxed);
        // Key order and added fields do not matter.
        let newer: PolicyReply = serde_json::from_str(
            r#"{"changed_at":1,"checkout":"/c","policy":"trusted","source":"desktop"}"#,
        )
        .unwrap();
        assert_eq!(newer.policy, CommandPolicy::Trusted);
    }

    #[test]
    fn an_unknown_policy_is_never_read_as_a_known_one() {
        assert!(
            serde_json::from_str::<PolicyReply>(r#"{"checkout":"/c","policy":"container"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<PolicyReply>(r#"{"checkout":"/c"}"#).is_err());
    }

    #[test]
    fn an_old_host_is_recognized_by_its_rejection() {
        use crate::host::{Error, HostError, providers::needs_host_update};
        let old = Error::Rejected(HostError {
            retry_after_seconds: None,
            code: "invalid_request".into(),
            message: "Unknown operation or field.".into(),
        });
        assert!(needs_host_update(&old));
        let missing = Error::Rejected(HostError {
            retry_after_seconds: None,
            code: "not_found".into(),
            message: "checkout is not inside a Git working tree.".into(),
        });
        assert!(!needs_host_update(&missing));
    }

    const SANDBOX_PROMISES: &[&str] = &[
        "cannot write the checkout",
        "reach the network",
        "sandboxed commands",
        "stay protected",
    ];

    #[test]
    fn copy_promises_a_sandbox_only_when_the_policy_is_sandboxed() {
        for ask in [true, false] {
            // The application's default describes no project, so it never does.
            for promise in SANDBOX_PROMISES {
                let text = approval_detail(ask);
                assert!(!text.contains(promise), "{text}");
            }
            for policy in [None, Some(CommandPolicy::Trusted)] {
                let text = idle_detail(ask, policy);
                for promise in SANDBOX_PROMISES {
                    assert!(!text.contains(promise), "{policy:?}: {text}");
                }
            }
        }
        let sandboxed = Some(CommandPolicy::Sandboxed);
        assert!(idle_detail(false, sandboxed).contains("sandboxed commands"));
        // The project's own setting is where the sandbox is described.
        assert!(CommandPolicy::Sandboxed.meaning().contains("No network"));
    }

    #[test]
    fn trusted_copy_says_what_commands_can_do() {
        let trusted = Some(CommandPolicy::Trusted);
        for text in [
            CommandPolicy::Trusted.meaning(),
            idle_detail(false, trusted),
            idle_detail(true, trusted),
        ] {
            assert!(text.contains("full user access"), "{text}");
            assert!(text.contains("network"), "{text}");
        }
        assert!(
            CommandPolicy::Trusted
                .meaning()
                .contains("already modified")
        );
        // Asking is the same promise in every policy: nothing runs without Allow once.
        assert!(approval_detail(true).starts_with("Each edit and command waits"));
        for policy in [None, Some(CommandPolicy::Sandboxed), trusted] {
            assert!(idle_detail(true, policy).contains("waits for your approval"));
        }
        // Without asking, the default points at the project for what commands reach.
        assert!(approval_detail(false).contains("each project's settings"));
    }

    #[test]
    fn the_context_label_names_the_policy_or_says_it_is_unknown() {
        assert_eq!(
            context_label(Some(CommandPolicy::Sandboxed)),
            "Sandboxed commands"
        );
        assert_eq!(
            context_label(Some(CommandPolicy::Trusted)),
            "Trusted commands"
        );
        assert_eq!(context_label(None), "Command policy not loaded");
        assert!(CommandPolicy::Sandboxed.meaning().contains("No network"));
        assert!(
            CommandPolicy::Trusted
                .meaning()
                .contains("outside the sandbox")
        );
    }

    #[test]
    fn the_confirmation_states_every_consequence_in_the_contract() {
        let all = TRUSTED_CONSEQUENCES.join(" ");
        for required in [
            "full user access",
            "network",
            "Git",
            "already modified",
            "automatic approval",
            "without a prompt",
        ] {
            assert!(all.contains(required), "{required}");
        }
        assert!(TRUSTED_SCOPE.contains("next message"));
    }
}
