//! Commands offered when a draft starts with `/`: the app's own, then the
//! skills and prompt templates of the person's Pi setup.
//!
//! A local command names something the app already does. The composer only
//! reports the choice; its parent performs it. To add one: add a `CommandId`, a
//! row in `COMMANDS`, and an arm where the parent handles `ComposerEvent::Command`.
//! An agent command only inserts its text; the runtime expands it on send.

use gpui::SharedString;
use plait_client::fuzzy::fuzzy_match;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandId {
    NewThread,
    ShowChanges,
    ShowConversation,
    StopRun,
    OpenSettings,
    RefreshGit,
}

pub struct Command {
    pub id: CommandId,
    /// What the user types after `/`.
    pub name: &'static str,
    pub detail: &'static str,
    /// Offered only while the agent is working.
    pub needs_run: bool,
}

pub const COMMANDS: &[Command] = &[
    Command {
        id: CommandId::NewThread,
        name: "new",
        detail: "Start a new thread",
        needs_run: false,
    },
    Command {
        id: CommandId::ShowChanges,
        name: "changes",
        detail: "Show changed files and diffs",
        needs_run: false,
    },
    Command {
        id: CommandId::ShowConversation,
        name: "conversation",
        detail: "Show the conversation",
        needs_run: false,
    },
    Command {
        id: CommandId::StopRun,
        name: "stop",
        detail: "Stop the running agent",
        needs_run: true,
    },
    Command {
        id: CommandId::OpenSettings,
        name: "settings",
        detail: "Open settings",
        needs_run: false,
    },
    Command {
        id: CommandId::RefreshGit,
        name: "refresh",
        detail: "Refresh Git status",
        needs_run: false,
    },
];

/// A skill or prompt template from the person's Pi setup, with every string
/// the list draws made once, when the setup is read, not per keystroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCommand {
    /// What is typed after `/`: `skill:name`, or a template's name.
    pub name: SharedString,
    /// The name with its slash, as drawn and inserted.
    pub label: SharedString,
    /// One short line: a template's argument hint and the description.
    pub detail: SharedString,
    /// What assistive technology reads for the row.
    pub spoken: SharedString,
}

impl AgentCommand {
    pub fn new(name: String, detail: String) -> Self {
        let label = format!("/{name}");
        Self {
            spoken: format!("{label}: {detail}").into(),
            label: label.into(),
            name: name.into(),
            detail: detail.into(),
        }
    }
}

/// An item that matches the typed query.
pub(super) struct CommandMatch<'a, T> {
    pub command: &'a T,
    /// Matched byte offsets in the name, or in the detail when only it matched.
    pub name_matches: Vec<usize>,
    pub detail_matches: Vec<usize>,
}

/// Local commands matching `query`, best first; ties keep the table's order.
pub(super) fn matching(
    commands: &'static [Command],
    query: &str,
    busy: bool,
) -> Vec<CommandMatch<'static, Command>> {
    rank(
        commands.iter().filter(|command| busy || !command.needs_run),
        query,
        |command| (command.name, command.detail),
    )
}

/// Agent commands matching `query`, best first, leaving out any name a local
/// command already has: the app's own command wins a clash.
pub(super) fn matching_agent<'a>(
    commands: &'a [AgentCommand],
    query: &str,
) -> Vec<CommandMatch<'a, AgentCommand>> {
    rank(
        commands
            .iter()
            .filter(|command| COMMANDS.iter().all(|local| local.name != &*command.name)),
        query,
        |command| (&command.name, &command.detail),
    )
}

/// A name match always ranks above a match found only in the description.
fn rank<'a, T>(
    items: impl Iterator<Item = &'a T>,
    query: &str,
    text: impl Fn(&T) -> (&str, &str),
) -> Vec<CommandMatch<'a, T>> {
    let mut found: Vec<(bool, i32, CommandMatch<'a, T>)> = items
        .filter_map(|command| {
            let (name, detail) = text(command);
            let by_name = fuzzy_match(query, name);
            // A description is prose: only an unbroken run in it is a real match.
            let by_detail = by_name
                .is_none()
                .then(|| fuzzy_match(query, detail))
                .flatten()
                .filter(|found| {
                    found
                        .positions
                        .windows(2)
                        .all(|pair| pair[0] + 1 == pair[1])
                });
            let score = by_name.as_ref().or(by_detail.as_ref())?.score;
            Some((
                by_name.is_some(),
                score,
                CommandMatch {
                    command,
                    name_matches: by_name.map(|found| found.positions).unwrap_or_default(),
                    detail_matches: by_detail.map(|found| found.positions).unwrap_or_default(),
                },
            ))
        })
        .collect();
    found.sort_by_key(|(by_name, score, _)| {
        (std::cmp::Reverse(*by_name), std::cmp::Reverse(*score))
    });
    found.into_iter().map(|(_, _, found)| found).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(query: &str, busy: bool) -> Vec<&'static str> {
        matching(COMMANDS, query, busy)
            .iter()
            .map(|found| found.command.name)
            .collect()
    }

    #[test]
    fn an_empty_query_lists_every_available_command_in_table_order() {
        assert_eq!(
            names("", false),
            ["new", "changes", "conversation", "settings", "refresh"]
        );
        assert_eq!(names("", true).len(), COMMANDS.len());
        assert!(names("", true).contains(&"stop"));
    }

    #[test]
    fn typing_narrows_by_name_first_and_description_second() {
        assert_eq!(names("set", false), ["settings"]);
        // Scattered letters in a description do not count: "Show The diFFs".
        assert!(names("stff", false).is_empty());
        assert_eq!(names("st", false)[0], "settings");
        assert!(!names("st", false).contains(&"stop"));
        assert_eq!(names("st", true)[0], "stop");
        assert_eq!(names("c", false)[..2], ["changes", "conversation"]);
        // "git" is only in a description; "thread" too.
        assert_eq!(names("git", false), ["refresh"]);
        assert_eq!(names("thread", false), ["new"]);
        assert!(names("zzz", true).is_empty());
        let by_detail = &matching(COMMANDS, "git", false)[0];
        assert!(by_detail.name_matches.is_empty());
        assert_eq!(by_detail.detail_matches, [8, 9, 10]);
    }

    #[test]
    fn agent_commands_follow_the_same_ranking_and_never_shadow_a_local_one() {
        let agent = [
            AgentCommand::new("skill:release-notes".into(), "Write release notes".into()),
            AgentCommand::new("review".into(), "<path> · Review a change".into()),
            AgentCommand::new("new".into(), "A template named like a local command".into()),
        ];
        let names = |query: &str| -> Vec<String> {
            matching_agent(&agent, query)
                .iter()
                .map(|found| found.command.name.to_string())
                .collect()
        };
        assert_eq!(names(""), ["skill:release-notes", "review"]);
        assert_eq!(names("rev"), ["review"]);
        assert_eq!(names("rel"), ["skill:release-notes"]);
        assert_eq!(names("skill:"), ["skill:release-notes"]);
        assert_eq!(names("change"), ["review"]);
        assert!(names("new").is_empty());
        assert_eq!(agent[0].label, "/skill:release-notes");
        assert_eq!(agent[1].spoken, "/review: <path> · Review a change");
    }

    #[test]
    fn command_names_are_unique_single_words() {
        for (index, command) in COMMANDS.iter().enumerate() {
            assert!(!command.name.is_empty() && !command.detail.is_empty());
            assert!(command.name.chars().all(|ch| ch.is_ascii_lowercase()));
            assert!(
                COMMANDS[..index]
                    .iter()
                    .all(|other| other.name != command.name && other.id != command.id)
            );
        }
    }
}
