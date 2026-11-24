//! Header and commit-bar actions derived from the last checkout status.
use crate::{git::WriteOp, model::Tracking};

const NOTHING_TO_PUSH: &str = "Nothing to push.";

/// What sending commits to the remote means for this branch right now.
#[derive(Debug, PartialEq, Eq)]
pub struct Sync {
    pub label: String,
    /// The write the Push button runs, if the branch can push at all.
    pub op: Option<WriteOp>,
    /// Why that write is unavailable now, in words.
    pub blocker: Option<&'static str>,
    /// There is an upstream to fetch from.
    pub fetch: bool,
    /// Such as `origin/main`, for the accessible description.
    pub upstream: Option<String>,
}

/// Counts come from the status read; nothing here runs Git.
pub fn sync(named: bool, tracking: Option<&Tracking>) -> Sync {
    let publish = |label: String| Sync {
        label,
        op: Some(WriteOp::Publish),
        blocker: None,
        fetch: false,
        upstream: None,
    };
    let Some(tracking) = tracking.filter(|_| named) else {
        return if named {
            publish("Not on the remote yet".into())
        } else {
            Sync {
                label: "No branch to push".into(),
                op: None,
                blocker: None,
                fetch: false,
                upstream: None,
            }
        };
    };
    let up = &tracking.upstream;
    if tracking.gone {
        return publish(format!("{up} was deleted"));
    }
    let upstream = Some(up.clone());
    let (ahead, behind) = (tracking.ahead, tracking.behind);
    let label = match (ahead, behind) {
        (0, 0) => "Up to date".to_owned(),
        (a, 0) => format!("{a} to push"),
        (0, b) => format!("{b} to pull"),
        (a, b) => format!("{a} to push, {b} to pull"),
    };
    // Only behind: the one useful move is a fast-forward pull.
    let (op, blocker) = match (ahead, behind) {
        (0, 0) => (WriteOp::Push, Some(NOTHING_TO_PUSH)),
        (0, _) => (WriteOp::Pull, None),
        (_, 0) => (WriteOp::Push, None),
        _ => (
            WriteOp::Push,
            Some(
                "This branch and its upstream have each moved on. Merge or rebase them in Terminal.",
            ),
        ),
    };
    Sync {
        label,
        op: Some(op),
        blocker,
        fetch: true,
        upstream,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitStep {
    Commit,
    Publish,
    Push,
    Pull,
}

pub fn next_git_step(
    changes: usize,
    staged: usize,
    sync: &Sync,
) -> Option<(GitStep, Option<&'static str>)> {
    if changes > 0 || staged > 0 {
        return Some((GitStep::Commit, None));
    }
    let step = match sync.op.as_ref()? {
        WriteOp::Publish => GitStep::Publish,
        WriteOp::Pull => GitStep::Pull,
        WriteOp::Push if sync.blocker != Some(NOTHING_TO_PUSH) => GitStep::Push,
        _ => return None,
    };
    Some((step, sync.blocker))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn push_follows_the_branch_and_its_upstream() {
        let up = |ahead, behind, gone| Tracking {
            upstream: "origin/main".into(),
            ahead,
            behind,
            gone,
        };
        let none = sync(false, None);
        assert_eq!((none.op, none.fetch), (None, false));
        let local = sync(true, None);
        assert_eq!(local.op, Some(WriteOp::Publish));
        assert_eq!(sync(true, Some(&up(0, 0, true))).op, Some(WriteOp::Publish));
        let even = sync(true, Some(&up(0, 0, false)));
        assert_eq!(even.label, "Up to date");
        assert_eq!(even.upstream.as_deref(), Some("origin/main"));
        assert!(even.blocker.is_some() && even.fetch);
        let ahead = sync(true, Some(&up(2, 0, false)));
        assert_eq!(ahead.label, "2 to push");
        assert_eq!((ahead.op, ahead.blocker), (Some(WriteOp::Push), None));
        let behind = sync(true, Some(&up(0, 3, false)));
        assert_eq!((behind.op, behind.blocker), (Some(WriteOp::Pull), None));
        let diverged = sync(true, Some(&up(1, 3, false)));
        assert_eq!(diverged.label, "1 to push, 3 to pull");
        assert!(diverged.blocker.unwrap().contains("Merge or rebase"));
    }

    #[test]
    fn next_step_uses_every_sync_row_and_prioritizes_local_changes() {
        use GitStep::*;
        let tracked = |ahead, behind, gone| {
            sync(
                true,
                Some(&Tracking {
                    upstream: "origin/main".into(),
                    ahead,
                    behind,
                    gone,
                }),
            )
        };
        for (row, expected) in [
            (sync(false, None), None),
            (sync(true, None), Some(Publish)),
            (tracked(0, 0, true), Some(Publish)),
            (tracked(0, 0, false), None),
            (tracked(2, 0, false), Some(Push)),
            (tracked(0, 2, false), Some(Pull)),
            (tracked(2, 2, false), Some(Push)),
        ] {
            assert_eq!(
                next_git_step(0, 0, &row),
                expected.map(|step| (step, row.blocker))
            );
            assert_eq!(next_git_step(1, 0, &row), Some((Commit, None)));
            assert_eq!(next_git_step(0, 1, &row), Some((Commit, None)));
        }
    }
}
