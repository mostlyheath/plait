//! Manual removal keeps the confirmed owner and request through a lost reply.
use crate::{
    host::worktree::{Checkout, Operation, Request, Stage},
    tabs::HostKey,
};
use std::path::PathBuf;

pub const RECOVERY_HINT: &str = "Use Remove worktree again on that checkout, or open its project's Worktrees tab and choose Refresh.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    RemoveIfPushed,
    KeepLocal,
}
impl Choice {
    pub fn label(self) -> &'static str {
        match self {
            Self::RemoveIfPushed => "Remove if pushed",
            Self::KeepLocal => "Keep local commits only",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub if_pushed: bool,
    pub keep_local: bool,
}
impl Choices {
    pub fn for_checkout(row: &Checkout) -> Self {
        if row.main
            || row.missing
            || row.removed
            || row.creation_id.is_none()
            || row.identity.is_none()
            || row.thread_id.is_none()
            || row.branch.as_ref().is_none_or(|branch| branch.is_empty())
            || row.preservation.changed != Some(0)
            || row.preservation.exclusive != Some(0)
        {
            return Self::default();
        }
        let if_pushed = row.preservation.refusal.is_none() && row.preservation.unpushed == Some(0);
        // Keeping local commits waives publication, so it is offered only when
        // publication is what refuses: then the confirmation states unpushed
        // or unknown commits. A published worktree needs no waiver, and one
        // sent anyway would remove commits made after this read without asking.
        Self {
            if_pushed,
            keep_local: row.preservation.can_keep_local
                && !if_pushed
                && row.preservation.unpushed != Some(0),
        }
    }
    pub fn any(self) -> bool {
        self.if_pushed || self.keep_local
    }
    pub fn offered(self) -> Vec<Choice> {
        [Choice::RemoveIfPushed, Choice::KeepLocal]
            .into_iter()
            .filter(|choice| self.allows(*choice))
            .collect()
    }
    fn allows(self, choice: Choice) -> bool {
        match choice {
            Choice::RemoveIfPushed => self.if_pushed,
            Choice::KeepLocal => self.keep_local,
        }
    }
}

pub fn confirmation_detail(
    path: &std::path::Path,
    branch: &str,
    facts: Option<&crate::git::worktree::inventory::Preservation>,
) -> String {
    let path = crate::display_text::escape(&path.display().to_string());
    let branch = crate::display_text::escape(branch);
    let publication = match facts.and_then(|f| f.unpushed) {
        Some(0) => format!("The host reports no unpushed commits on branch {branch}."),
        Some(1) => format!("1 commit is not pushed; it stays on branch {branch}."),
        Some(n) => format!("{n} commits are not pushed; they stay on branch {branch}."),
        None => format!("Publication is unknown; commits stay on branch {branch}."),
    };
    let counts = facts.map_or(String::new(), |facts| {
        let count = |n: Option<u64>| n.map_or("Unknown".into(), |n| n.to_string());
        format!(
            "\nChanged files: {}. Commits kept only here: {}.",
            count(facts.changed),
            count(facts.exclusive)
        )
    });
    format!(
        "Delete {path}?\n{publication}{counts}\nThe branch, thread history and private build cache stay. Uncommitted or ignored files, hidden index edits and commits kept only here block removal. The host checks again."
    )
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub host: HostKey,
    pub path: PathBuf,
    pub request: Request,
}
#[derive(Default)]
pub struct Removal {
    pub asking: Option<Attempt>,
    pub pending: Option<Attempt>,
    pub message: Option<String>,
    choices: Choices,
}
impl Removal {
    pub fn ask(&mut self, row: &Checkout, host: HostKey, id: String) -> bool {
        let choices = Choices::for_checkout(row);
        let (Some(creation_id), Some(identity)) = (row.creation_id.clone(), row.identity.clone())
        else {
            return false;
        };
        self.start(
            Attempt {
                host,
                path: row.path.clone(),
                request: Request::RemoveWorktree {
                    request_id: id,
                    creation_id,
                    identity,
                    allow_unpushed: false,
                },
            },
            choices,
        )
    }
    /// The sidebar's older-host path has ownership but no live inventory.
    /// Keep its explicit choices; the host still applies every removal guard.
    pub fn ask_record(
        &mut self,
        path: PathBuf,
        record: &crate::navigation::WorktreeOf,
        host: HostKey,
        id: String,
    ) -> bool {
        let (Some(creation_id), Some(identity)) =
            (record.creation_id.clone(), record.identity.clone())
        else {
            return false;
        };
        if path == record.main || record.branch.is_empty() {
            return false;
        }
        self.start(
            Attempt {
                host,
                path,
                request: Request::RemoveWorktree {
                    request_id: id,
                    creation_id,
                    identity,
                    allow_unpushed: false,
                },
            },
            Choices {
                if_pushed: true,
                keep_local: true,
            },
        )
    }
    fn start(&mut self, attempt: Attempt, choices: Choices) -> bool {
        if self.asking.is_some() || self.pending.is_some() || !choices.any() {
            return false;
        }
        self.asking = Some(attempt);
        self.choices = choices;
        true
    }
    pub fn choices(&self) -> Choices {
        self.choices
    }
    pub fn answer(&mut self, choice: Option<Choice>) -> Option<Attempt> {
        let mut attempt = self.asking.take()?;
        let choices = std::mem::take(&mut self.choices);
        let choice = choice?;
        if !choices.allows(choice) {
            return None;
        }
        let Request::RemoveWorktree { allow_unpushed, .. } = &mut attempt.request else {
            return None;
        };
        *allow_unpushed = choice == Choice::KeepLocal;
        self.message = None;
        self.pending = Some(attempt.clone());
        Some(attempt)
    }
    pub fn finish(&mut self, attempt: &Attempt, result: Result<Operation, String>) -> bool {
        if self.pending.as_ref() != Some(attempt) {
            return false;
        }
        self.message = Some(match result {
            Ok(operation) => {
                self.pending = None;
                operation.error.map_or_else(
                    || {
                        if operation.state == Stage::Removed {
                            format!(
                                "Worktree removed: {}. Its branch and history remain.",
                                attempt.path.display()
                            )
                        } else {
                            "The host did not remove this worktree.".into()
                        }
                    },
                    |error| error.message,
                )
            }
            Err(error) => format!("{error} Removal is unconfirmed. {RECOVERY_HINT}"),
        });
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git::worktree::{Identity, inventory::Preservation},
        host::HostError,
    };
    fn row() -> Checkout {
        Checkout {
            build_directories: Vec::new(),
            build_output: None,
            path: "/repo.worktrees/tree".into(),
            main: false,
            branch: Some("plait/tree".into()),
            head: None,
            base: None,
            thread_id: Some("thread".into()),
            identity: Some(Identity {
                dev: "1".into(),
                ino: "2".into(),
            }),
            missing: false,
            removed: false,
            preservation: Preservation {
                build_output: None,
                changed: Some(0),
                unpushed: Some(0),
                exclusive: Some(0),
                refusal: None,
                can_keep_local: true,
            },
            creation_id: Some("creation".into()),
            last_used: None,
        }
    }
    fn host() -> HostKey {
        HostKey {
            id: "host".into(),
            store: "/store".into(),
        }
    }
    #[test]
    fn asks_once_cancels_and_preserves_the_same_request_after_a_lost_reply() {
        let mut removal = Removal::default();
        assert!(removal.ask(&row(), host(), "id".into()));
        assert!(!removal.ask(&row(), host(), "other".into()));
        assert!(removal.answer(None).is_none());
        assert!(removal.pending.is_none());
        assert!(removal.ask(&row(), host(), "id".into()));
        let attempt = removal.answer(Some(Choice::RemoveIfPushed)).unwrap();
        assert!(removal.finish(&attempt, Err("Connection lost.".into())));
        assert_eq!(removal.pending.as_ref(), Some(&attempt));
        assert_eq!(
            removal.message.as_deref(),
            Some(
                "Connection lost. Removal is unconfirmed. Use Remove worktree again on that checkout, or open its project's Worktrees tab and choose Refresh."
            )
        );
        assert!(removal.finish(
            &attempt,
            Ok(Operation {
                request_id: "creation".into(),
                thread_id: "thread".into(),
                state: Stage::Removed,
                worktree: None,
                error: None
            })
        ));
        assert!(removal.pending.is_none());
        assert!(removal.message.as_ref().unwrap().contains("removed"));
        assert!(!removal.finish(&attempt, Err("late".into())));
    }
    #[test]
    fn refusal_before_and_after_confirmation_uses_the_hosts_words() {
        let mut removal = Removal::default();
        let mut row = row();
        row.preservation.refusal = Some("New work appeared.".into());
        row.preservation.can_keep_local = false;
        assert!(!removal.ask(&row, host(), "id".into()));
        row.preservation.refusal = None;
        row.preservation.can_keep_local = true;
        assert!(removal.ask(&row, host(), "id".into()));
        let attempt = removal.answer(Some(Choice::RemoveIfPushed)).unwrap();
        removal.finish(
            &attempt,
            Ok(Operation {
                request_id: "creation".into(),
                thread_id: "thread".into(),
                state: Stage::Ready,
                worktree: None,
                error: Some(HostError {
                    retry_after_seconds: None,
                    code: "busy".into(),
                    message: "New work appeared.".into(),
                    outcome: None,
                }),
            }),
        );
        assert_eq!(removal.message.as_deref(), Some("New work appeared."));
        assert!(removal.pending.is_none());
    }

    #[test]
    fn every_offered_choice_freezes_its_publication_exception() {
        for (published, local) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut row = row();
            row.preservation.refusal = (!published).then(|| "Publication is unknown.".into());
            row.preservation.unpushed = published.then_some(0);
            row.preservation.can_keep_local = local;
            let choices = Choices::for_checkout(&row);
            let expected: Vec<_> = [
                published.then_some(Choice::RemoveIfPushed),
                (local && !published).then_some(Choice::KeepLocal),
            ]
            .into_iter()
            .flatten()
            .collect();
            assert_eq!(choices.offered(), expected);
            assert_eq!(choices.any(), published || local);
            for choice in [Choice::RemoveIfPushed, Choice::KeepLocal] {
                let mut removal = Removal::default();
                assert_eq!(removal.ask(&row, host(), "request".into()), choices.any());
                let attempt = removal.answer(Some(choice));
                assert_eq!(attempt.is_some(), expected.contains(&choice));
                if let Some(attempt) = attempt {
                    assert!(
                        matches!(&attempt.request, Request::RemoveWorktree {request_id,allow_unpushed,..} if request_id == "request" && *allow_unpushed == (choice == Choice::KeepLocal))
                    );
                    assert!(removal.finish(&attempt, Err("Reply lost.".into())));
                    assert_eq!(removal.pending.as_ref(), Some(&attempt));
                    assert!(!removal.ask(&row, host(), "other".into()));
                }
            }
        }
    }
    /// `allow_unpushed:true` only ever follows a confirmation that stated
    /// unpushed commits or unknown publication.
    #[test]
    fn keeping_local_commits_is_offered_only_beside_the_words_that_need_it() {
        for unpushed in [Some(0), Some(1), Some(3), None] {
            for refused in [false, true] {
                for local in [false, true] {
                    let mut row = row();
                    row.preservation.unpushed = unpushed;
                    row.preservation.refusal = refused.then(|| "Publication refusal.".into());
                    row.preservation.can_keep_local = local;
                    let detail = confirmation_detail(
                        &row.path,
                        row.branch.as_deref().unwrap(),
                        Some(&row.preservation),
                    );
                    let offered = Choices::for_checkout(&row).offered();
                    if offered.contains(&Choice::KeepLocal) {
                        assert!(
                            detail.contains("not pushed")
                                || detail.contains("Publication is unknown"),
                            "{unpushed:?} {refused} {local}: {detail}"
                        );
                    }
                    assert!(
                        !(offered.contains(&Choice::KeepLocal)
                            && offered.contains(&Choice::RemoveIfPushed)),
                        "two choices that both remove a published worktree"
                    );
                }
            }
        }
        let mut removal = Removal::default();
        assert!(removal.ask(&row(), host(), "id".into()));
        assert!(removal.answer(Some(Choice::KeepLocal)).is_none());
        assert!(removal.pending.is_none());
    }
    #[test]
    fn local_choice_never_bypasses_ownership_or_preservation_guards() {
        type Mutation = fn(&mut Checkout);
        let unsafe_rows: [Mutation; 11] = [
            |r| r.main = true,
            |r| r.missing = true,
            |r| r.removed = true,
            |r| r.creation_id = None,
            |r| r.identity = None,
            |r| r.thread_id = None,
            |r| r.branch = None,
            |r| r.preservation.changed = Some(1),
            |r| r.preservation.changed = None,
            |r| r.preservation.exclusive = Some(1),
            |r| r.preservation.exclusive = None,
        ];
        for change in unsafe_rows {
            let mut row = row();
            change(&mut row);
            assert!(!Choices::for_checkout(&row).any());
            assert!(!Removal::default().ask(&row, host(), "request".into()));
        }
    }
    #[test]
    fn confirmation_names_real_counts_and_the_branch_that_survives() {
        let mut row = row();
        row.preservation.unpushed = Some(2);
        let detail = confirmation_detail(
            &row.path,
            row.branch.as_deref().unwrap(),
            Some(&row.preservation),
        );
        assert!(detail.contains("2 commits are not pushed; they stay on branch plait/tree."));
        assert!(detail.contains("Changed files: 0. Commits kept only here: 0."));
        let detail = confirmation_detail(&row.path, "plait/tree", None);
        assert!(detail.contains("Publication is unknown"));
        assert!(!detail.contains("Changed files: 0"));
        assert_eq!(Choice::RemoveIfPushed.label(), "Remove if pushed");
        assert_eq!(Choice::KeepLocal.label(), "Keep local commits only");
    }
}
