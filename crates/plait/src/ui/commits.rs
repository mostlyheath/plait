//! The History tab: commits reachable from HEAD, newest first, and the
//! selected commit's message and patch. Git runs in plait-client on the
//! background executor; pages load as the list asks for them.

use super::{Plait, theme::*};
use gpui::{prelude::*, *};
use plait_client::{git::Commit, state::Load};
use std::time::{SystemTime, UNIX_EPOCH};

/// How long ago, in the short form the thread list uses.
pub(super) fn age(time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    match seconds {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        86_400..2_592_000 => format!("{}d", seconds / 86_400),
        2_592_000..31_536_000 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

impl Plait {
    fn commit_row(
        &self,
        index: usize,
        commit: &Commit,
        now: i64,
        cx: &mut Context<Self>,
    ) -> Control {
        let selected = self.state.repository.selected_commit() == Some(&commit.id);
        let id = commit.id.clone();
        column()
            .id(("commit", index))
            .role(Role::ListItem)
            .aria_label(format!(
                "{}, {}, {}",
                commit.subject, commit.author, commit.short
            ))
            .aria_selected(selected)
            .audited()
            .w_full()
            .h(px(48.))
            .px_3()
            .justify_center()
            .cursor_pointer()
            .when(selected, |el| el.bg(rgb(colors().selected)))
            .when(!selected, |el| el.hover(|s| s.bg(rgba(colors().hover))))
            .child(
                div()
                    .truncate()
                    .text_color(rgb(colors().text))
                    .child(commit.subject.clone()),
            )
            .child(
                row()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(rgb(colors().subtle))
                    .child(
                        div()
                            .font_family(mono())
                            .text_size(px(code_size()))
                            .child(commit.short.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(commit.author.clone()),
                    )
                    .child(age(commit.time, now)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.workspace.restore_pending[2] = false;
                this.state.repository.select_commit(id.clone());
                this.workspace.commit_scroll = UniformListScrollHandle::new();
                this.pump(cx);
                cx.notify();
            }))
    }

    fn commit_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let history = match self.state.repository.commit_list() {
            Load::Empty | Load::Loading => {
                return empty("Reading history…", "Git is running in the background.")
                    .into_any_element();
            }
            Load::Error(error) => {
                return empty("Cannot read history", error.clone()).into_any_element();
            }
            Load::Ready(history) => history,
        };
        if history.commits.is_empty() {
            return empty("No commits yet", "This branch has no commits.").into_any_element();
        }
        let count = history.commits.len();
        let more = history.more;
        let loading = self.state.repository.commit_loading_more();
        column()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "commit-list",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        let now = now();
                        let Some(history) = this.state.repository.commit_list().ready() else {
                            return Vec::new();
                        };
                        // Borrowed, not cloned: only the visible rows are built.
                        history.commits[range.clone()]
                            .iter()
                            .zip(range)
                            .map(|(commit, index)| this.commit_row(index, commit, now, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.workspace.history_scroll)
                .flex_1()
                .min_h_0(),
            )
            // An explicit control: Git never starts from inside list layout.
            .when(more, |el| {
                el.child(
                    row().px_2().py_1().flex_shrink_0().child(
                        button(
                            "more-commits",
                            if loading {
                                "Loading…"
                            } else {
                                "Load more commits"
                            },
                        )
                        .disabled(loading)
                        .on_activate(cx, |this, _, cx| {
                            this.state.repository.request_more_history();
                            this.pump(cx);
                            cx.notify();
                        }),
                    ),
                )
            })
            .into_any_element()
    }

    fn commit_patch(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.state.repository.commit_patch() {
            Load::Empty => empty(
                "Select a commit",
                "Its message and changes appear here, compared with its first parent.",
            )
            .into_any_element(),
            Load::Loading => empty("Reading commit…", "The patch is loading in the background.")
                .into_any_element(),
            Load::Error(error) => empty("Cannot read commit", error.clone()).into_any_element(),
            Load::Ready(diff) => self.diff_lines(
                "commit-lines",
                diff.clone(),
                &self.workspace.commit_scroll,
                None,
                Default::default(),
                cx,
            ),
        }
    }

    pub(super) fn history_view(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let branch = self
            .state
            .repository
            .snapshot()
            .ready()
            .map(|snapshot| snapshot.branch.clone());
        let stacked = self.panel_is_open() || width < 540.;
        row()
            .when(stacked, |el| el.flex_col())
            .gap_0()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(
                column()
                    .map(|el| {
                        if stacked {
                            el.w_full().h(relative(0.45)).min_h_0().overflow_hidden()
                        } else {
                            el.w(px(if width > super::EXPANDED_CHROME_WIDTH {
                                320.
                            } else {
                                264.
                            }))
                        }
                    })
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(rgb(colors().border))
                    .bg(rgb(colors().rail))
                    .child(
                        row()
                            .h(px(36.))
                            .px_3()
                            .flex_shrink_0()
                            .child(icon("branch").text_color(rgb(colors().muted)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(branch.unwrap_or_default()),
                            ),
                    )
                    .child(self.commit_list(cx)),
            )
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.commit_patch(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::age;

    #[test]
    fn ages_use_the_largest_whole_unit_and_never_go_negative() {
        assert_eq!(age(100, 100), "now");
        assert_eq!(age(200, 100), "now");
        assert_eq!(age(0, 59), "now");
        assert_eq!(age(0, 60), "1m");
        assert_eq!(age(0, 7_200), "2h");
        assert_eq!(age(0, 3 * 86_400), "3d");
        assert_eq!(age(0, 2 * 2_592_000), "2mo");
        assert_eq!(age(0, 2 * 31_536_000), "2y");
    }
}
