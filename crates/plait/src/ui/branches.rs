//! Creating and switching local branches.
//!
//! The branch name in Changes and in the Git inspector is a control that opens
//! the palette over the open checkout's local branches. plait-core reads them
//! when the palette opens and runs the chosen write on the background
//! executor; its result appears under the commit box like any other Git write.

use super::{
    Plait, Workspace,
    commands::Command,
    commits::{age, now},
    palette::{Item, Options, Palette, Typed},
    theme::*,
    thread_controls::{TextTooltip, locked},
};
use gpui::{prelude::*, *};
use plait_core::{
    git::{BRANCH_LIMIT, Branch, WriteOp},
    state::Load,
};
use std::rc::Rc;

/// "Create branch <text>" when the text names no listed branch. Typing part
/// of a listed name is looking for that branch, so the best match keeps the
/// selection, unless the palette was opened to make a new branch.
pub(super) fn create_item(
    text: &str,
    items: &[Item],
    enabled: bool,
    create: bool,
) -> Option<(Item, bool)> {
    if text.is_empty() || items.iter().any(|item| item.title.as_ref() == text) {
        return None;
    }
    let lowered = text.to_lowercase();
    // Only a branch Enter could switch to; the current one cannot be chosen.
    let partial = items
        .iter()
        .any(|item| item.enabled && item.title.to_lowercase().contains(&lowered));
    let item = Item::new(
        format!("create:{text}"),
        format!("Create branch {text}"),
        Command::CreateBranch(text.to_owned()),
    )
    .detail("from the current commit")
    .glyph("plus", colors().mint)
    .enabled(enabled);
    Some((item, create || !partial))
}

/// One local branch as a palette row. The current branch is marked and cannot
/// be chosen: there is nothing to switch to.
fn branch_item(branch: &Branch, enabled: bool, now: i64) -> Item {
    let item = Item::new(
        format!("branch:{}", branch.name),
        branch.name.clone(),
        Command::SwitchBranch(branch.name.clone()),
    )
    .glyph("branch", colors().subtle)
    .hint(age(branch.time, now))
    .enabled(enabled && !branch.current);
    let item = match &branch.elsewhere {
        Some(path) => item.detail(format!(
            "in worktree {}",
            home_relative(std::path::Path::new(path))
        )),
        None => item,
    };
    if branch.current {
        item.badge("Current", colors().mint)
    } else {
        item
    }
}

impl Plait {
    /// Why a branch cannot be created or switched to now, if anything stops
    /// it: a short form for the palette's footer, and the full sentence.
    fn branch_refusal(&self) -> Option<(&'static str, &'static str)> {
        if self.state.repository.write().busy() {
            Some((
                "Git is still working.",
                "Git is still working. Try again when it finishes.",
            ))
        } else if self.running_here() {
            Some((
                "A thread is running in this checkout.",
                "A thread is running in this checkout. Switching branches would change the files it is editing.",
            ))
        } else {
            None
        }
    }

    /// The full reason, for hover text and the result line.
    pub(super) fn branch_blocker(&self) -> Option<&'static str> {
        self.branch_refusal().map(|(_, full)| full)
    }

    /// The palette's footer: why nothing can be chosen, else `note`.
    pub(super) fn branch_note(&self, note: Option<SharedString>) -> Option<SharedString> {
        self.branch_refusal()
            .map(|(short, _)| short.into())
            .or(note)
    }

    /// The branch name beside its icon, as a control that opens the branch
    /// palette. Changes and the Git inspector share it. While a branch write
    /// would be refused it stays readable in hint color, hover says why, and
    /// pressing it opens the palette with the same reason.
    pub(super) fn branch_control(
        &self,
        compact: bool,
        branch: &str,
        cx: &mut Context<Self>,
    ) -> Control {
        let blocker = self.branch_blocker();
        let shortcut = self.shortcut_label("git.switch_branch");
        let hover = match shortcut.as_str() {
            "" => "Switch or create a branch".to_owned(),
            keys => format!("Switch or create a branch  {keys}"),
        };
        let control = button(
            if compact {
                "inspector-branch"
            } else {
                "changes-branch"
            },
            "",
        )
        .h(px(24.))
        .px_1()
        .gap_1()
        .min_w_0()
        // Buttons keep their size; a long branch name truncates instead.
        .flex_shrink(1.)
        .text_size(px(13.))
        .text_color(rgb(colors().text))
        .aria_label(format!("Branch {branch}"))
        .aria_description(blocker.unwrap_or("Switch to another branch or create one."))
        .aria_keyshortcuts(self.shortcut_aria("git.switch_branch"))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(branch.to_owned()),
        )
        .child(icon("down").size(px(12.)).text_color(rgb(colors().subtle)))
        .on_activate(cx, |this, window, cx| this.open_branches(false, window, cx));
        match blocker {
            Some(_) => locked(control, blocker),
            None => control.tooltip(move |_, cx| cx.new(|_| TextTooltip(hover.clone())).into()),
        }
    }

    /// Open the branch palette. `create` starts the selection on the Create
    /// item as soon as a name is typed. The list is read afresh each time.
    pub(super) fn open_branches(
        &mut self,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.repository.request_branches();
        self.pump(cx);
        let blocker = self.branch_blocker();
        let (items, empty, note) = self.branch_items();
        let options = Options {
            name: if create {
                "New branch"
            } else {
                "Switch branch"
            }
            .into(),
            placeholder: if create {
                "Name the new branch…"
            } else {
                "Switch to a branch, or type a new name…"
            }
            .into(),
            empty: empty.into(),
            initial: None,
            mark: None,
            aside: None,
            note: self.branch_note(note),
        };
        let enabled = blocker.is_none();
        let typed: Typed =
            Rc::new(move |text, items| create_item(text.trim(), items, enabled, create));
        let palette = cx.new(|cx| {
            let mut palette = Palette::new(options, items, window, cx);
            palette.set_typed(typed, cx);
            palette
        });
        self.present(palette, None, window, cx);
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.branches = true;
        }
    }

    /// Rows for the branch palette, what to say with none, and a footer note.
    pub(super) fn branch_items(&self) -> (Vec<Item>, &'static str, Option<SharedString>) {
        let enabled = self.branch_blocker().is_none();
        match &self.state.repository.branches() {
            Load::Ready(branches) => {
                let now = now();
                let items = branches
                    .list
                    .iter()
                    .map(|branch| branch_item(branch, enabled, now))
                    .collect();
                let note = branches.more.then(|| {
                    format!("Showing the {BRANCH_LIMIT} most recently committed branches.").into()
                });
                (items, "No branches yet. Type a name to create one.", note)
            }
            Load::Error(error) => (
                Vec::new(),
                "Branches could not be listed.",
                Some(error.clone().into()),
            ),
            _ => (Vec::new(), "Reading branches…", None),
        }
    }

    /// After a Git job: fill an open branch palette once its list arrives.
    pub(super) fn deliver_branches(&mut self, cx: &mut Context<Self>) {
        let Some(palette) = self
            .overlay
            .as_ref()
            .filter(|overlay| overlay.branches)
            .map(|overlay| overlay.palette.clone())
        else {
            return;
        };
        if !matches!(
            self.state.repository.branches(),
            Load::Ready(_) | Load::Error(_)
        ) {
            return;
        }
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.branches = false;
        }
        let (items, _, note) = self.branch_items();
        let note = self.branch_note(note);
        palette.update(cx, |palette, cx| {
            palette.set_items(items, cx);
            palette.set_note(note, cx);
        });
    }

    /// Run a branch write chosen in the palette. Something may have started
    /// while the palette was open, so the reason is checked again here and
    /// reported where the write's result would be.
    pub(super) fn branch_write(
        &mut self,
        op: WriteOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Changes holds the result line, as for the menu's other Git writes.
        self.show_tab(Workspace::Changes, window, cx);
        if let Some(reason) = self.branch_blocker() {
            self.state.repository.write_mut().note = Some((reason.into(), true));
            return cx.notify();
        }
        self.git_write(op, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{branch_item, create_item};
    use plait_core::git::Branch;

    fn branch(name: &str, current: bool) -> Branch {
        Branch {
            name: name.into(),
            current,
            time: 0,
            elsewhere: None,
        }
    }

    fn items() -> Vec<super::Item> {
        [("main", true), ("feature/login", false), ("fix", false)]
            .iter()
            .map(|(name, current)| branch_item(&branch(name, *current), true, 0))
            .collect()
    }

    #[test]
    fn the_create_item_appears_only_for_a_name_that_is_not_listed() {
        let items = items();
        assert!(create_item("", &items, true, false).is_none());
        assert!(create_item("main", &items, true, false).is_none());
        assert!(create_item("feature/login", &items, true, false).is_none());
        let (item, _) = create_item("feature/signup", &items, true, false).unwrap();
        assert_eq!(item.title.as_ref(), "Create branch feature/signup");
        assert!(item.enabled);
        // Case matters to Git, so a name differing only in case is new.
        assert!(create_item("Main", &items, true, false).is_some());
        // Refused up front: listed, but it cannot be chosen.
        let (item, _) = create_item("other", &items, false, false).unwrap();
        assert!(!item.enabled);
    }

    #[test]
    fn typing_part_of_a_branch_keeps_the_match_selected_unless_making_a_new_one() {
        let items = items();
        // "logi" is part of feature/login: Enter should switch to it.
        assert_eq!(
            create_item("logi", &items, true, false).map(|(_, l)| l),
            Some(false)
        );
        // Part of only the current branch, which cannot be chosen: Enter creates.
        assert_eq!(
            create_item("mai", &items, true, false).map(|(_, l)| l),
            Some(true)
        );
        // A name nothing contains: Enter creates it.
        assert_eq!(
            create_item("release", &items, true, false).map(|(_, l)| l),
            Some(true)
        );
        // New Branch… always starts on Create.
        assert_eq!(
            create_item("logi", &items, true, true).map(|(_, l)| l),
            Some(true)
        );
    }

    #[test]
    fn the_current_branch_is_marked_and_cannot_be_chosen() {
        let items = items();
        assert_eq!(
            items[0].badge.as_ref().map(|(text, _)| text.as_ref()),
            Some("Current")
        );
        assert!(!items[0].enabled);
        assert!(items[1].enabled && items[1].badge.is_none());
        // Refused up front: every branch is listed but none can be chosen.
        assert!(!branch_item(&branch("fix", false), false, 0).enabled);
        let mut elsewhere = branch("plait/1a2b3c", false);
        elsewhere.elsewhere = Some("/tmp/plait.worktrees/1a2b3c".into());
        let item = branch_item(&elsewhere, true, 0);
        assert!(item.enabled, "Git decides, and says why");
        assert!(item.detail.starts_with("in worktree"));
    }
}
