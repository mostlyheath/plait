//! The turn record beside the conversation. File rows are built on demand.
use super::{Plait, Workspace, commits, theme::*};
use gpui::{prelude::*, *};
use plait_client::{
    history::{
        records::{File, Line},
        this_turn,
    },
    review::recorded::Target,
};

impl Plait {
    pub(super) fn show_earlier_turn(
        &mut self,
        run: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace.turn_records.selection.earlier(run);
        if self.draws_work() {
            self.choose_work_scope(true, window, cx);
            return;
        }
        if !Self::wide(window) {
            self.workspace.inspector_note = true;
        } else if self.preferences.value.hide_inspector {
            self.toggle_inspector(window, cx);
        }
        cx.notify();
    }

    pub(super) fn this_turn(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = self.host.thread.snapshot.as_ref()?;
        if self.host.thread.selected.as_deref() != Some(&snapshot.id)
            || self.host.protocol() < 5
            || self.workspace.turn_review.older
            || self
                .state
                .repository
                .snapshot()
                .ready()
                .is_some_and(|s| s.plain)
        {
            return None;
        }
        let records = &self.workspace.turn_records;
        let panel = this_turn::panel(
            &records.lines,
            &records.selection,
            snapshot.status,
            self.undo_context(),
        )?;
        let mut body = self.turn_header(panel.heading, panel.state, panel.line, cx);
        if let Some(line) = panel.line {
            // The heading stands only over paths the record has.
            if !panel.files.is_empty() || panel.more > 0 {
                body = body.child(section_label("turn-changed", "Changed"));
            }
            if !panel.files.is_empty() {
                body = body.child(self.turn_paths(&snapshot.id, line, panel.files.len(), cx));
            }
            if panel.more > 0 {
                body = body.child(tag(
                    "turn-more",
                    format!("{} more", panel.more),
                    colors().subtle,
                ));
            }
            if let Some(action) = panel.action {
                body = body
                    .child(self.record_undo_button(line, action, None, cx))
                    .children(self.undo_panel(line, cx));
            }
            body = body.child(
                button("open-in-work", "Open in Work view")
                    .child(self.shortcut_label("view.toggle_work"))
                    .aria_keyshortcuts(self.shortcut_aria("view.toggle_work"))
                    .on_activate(cx, |this, window, cx| {
                        this.toggle_work(window, cx);
                        if this
                            .tabs
                            .model
                            .work()
                            .is_some_and(|w| w.open && !w.recorded)
                        {
                            this.choose_work_scope(true, window, cx);
                        }
                    }),
            );
        }
        Some(
            column()
                .flex_1()
                .min_h_0()
                .child(body)
                .when_some(self.state.repository.snapshot().ready(), |el, s| {
                    el.child(self.turn_footer(s, cx))
                })
                .into_any_element(),
        )
    }
    fn turn_header(
        &self,
        heading: &'static str,
        state: String,
        line: Option<&Line>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        column()
            .id("this-turn-panel")
            .flex_1()
            .min_h_0()
            .gap_2()
            .p_3()
            .child(
                row()
                    .child(section_label("turn-heading", heading))
                    .child(div().flex_1())
                    .when_some(line, |el, line| {
                        el.child(tag(
                            "turn-age",
                            commits::age((line.created_at / 1000) as i64, commits::now()),
                            colors().subtle,
                        ))
                    }),
            )
            .child(
                row()
                    .child(tag("turn-state", state, colors().muted))
                    .child(div().flex_1())
                    .when(heading == "Earlier turn", |el| {
                        el.child(
                            button("turn-latest", "Latest").on_activate(cx, |this, _, cx| {
                                this.workspace.turn_records.selection.latest();
                                cx.notify();
                            }),
                        )
                    }),
            )
    }

    fn turn_paths(
        &self,
        thread: &str,
        line: &Line,
        count: usize,
        cx: &Context<Self>,
    ) -> UniformList {
        let lines = self.workspace.turn_records.lines.clone();
        let target = Target {
            thread: thread.to_owned(),
            checkout: self.host.creation.checkout.clone(),
            record: line.displayed.clone(),
            original: line.run.clone(),
            path: None,
        };
        uniform_list(
            "turn-paths",
            count,
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                let files = &lines.runs[&target.original].files;
                range
                    .map(|index| turn_file(index, &files[index], &target, cx))
                    .collect::<Vec<_>>()
            }),
        )
        // As tall as its rows, so Undo follows them; it shrinks and scrolls
        // when the record lists more than the panel can show.
        .h(px(count as f32 * ROW_HEIGHT))
        .flex_shrink(1.)
        .min_h_0()
    }

    fn turn_footer(&self, snapshot: &plait_client::model::Snapshot, cx: &mut Context<Self>) -> Div {
        row()
            .h(px(40.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(colors().border))
            .px_2()
            .child(self.branch_control(true, &snapshot.branch, cx))
            .child(div().flex_1())
            .child(
                button(
                    "turn-uncommitted",
                    format!("{} uncommitted", snapshot.changes.len()),
                )
                .on_activate(cx, |this, window, cx| {
                    this.clear_turn_review();
                    this.show_tab(Workspace::Changes, window, cx);
                }),
            )
    }
}

fn turn_file(index: usize, file: &File, target: &Target, cx: &mut Context<Plait>) -> Control {
    // A mode change is a modification to the eye; its name says which.
    let (letter, tint, word) = match file.change {
        "Added" => ("A", colors().mint, "Added"),
        "Deleted" => ("D", colors().red, "Deleted"),
        "Mode" => ("M", colors().blue, "Mode changed"),
        _ => ("M", colors().blue, "Modified"),
    };
    let mut target = target.clone();
    target.path = Some(file.path.clone());
    nav_row(
        ("turn-file", index),
        format!("{word} · {}", file.review_detail),
        false,
    )
    .gap(px(6.))
    .child(
        div()
            .w(px(10.))
            .flex_shrink_0()
            .font_family(mono())
            .text_size(px(12.))
            .text_color(rgb(tint))
            .child(letter),
    )
    .child(div().min_w_0().truncate().child(file.name.clone()))
    .child(
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(rgb(colors().subtle))
            .child(file.folder.clone()),
    )
    .when(!file.counts.is_empty(), |el| {
        el.child(
            div()
                .flex_shrink_0()
                .font_family(mono())
                .text_size(px(12.))
                .text_color(rgb(colors().muted))
                .child(file.counts.clone()),
        )
    })
    .on_activate(cx, move |this, window, cx| {
        this.open_turn_review(target.clone(), window, cx)
    })
}
