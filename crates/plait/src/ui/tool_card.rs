//! Tool calls in the conversation: one compact line each, a presentation per
//! tool when opened, and one summary line for a long run of calls.
//!
//! What each call shows is prepared in `plait_client::tool_view`. A folded call
//! keeps its list row and draws nothing, so row identity and the scroll anchor
//! are the same whether a group is open or closed.

use super::{
    Plait,
    selection::{Owner, Site},
    theme::*,
};
use gpui::{prelude::*, *};
use plait_client::{
    host::{Tool, ToolStatus},
    markdown::Kind,
    tool_view::{Part, Run, Section, ToolView},
    transcript::{self, Slot},
};
use std::{collections::HashMap, sync::Arc};

fn tool_tint(status: ToolStatus) -> u32 {
    match status {
        ToolStatus::Running => colors().blue,
        ToolStatus::Done => colors().subtle,
        ToolStatus::Error => colors().red,
        ToolStatus::Interrupted => colors().amber,
    }
}

/// Line height of a call's row, so a wrapped subject and its chevron align.
const TOOL_LINE: f32 = 18.;
/// A collapsed call's subject wraps to at most this many lines.
const SUBJECT_LINES: usize = 4;

/// A chevron centered on the first line of a row that may wrap.
fn line_icon(name: &'static str) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .h(px(TOOL_LINE))
        .child(icon(name))
}

/// Rounded surface for a tool's content, matching fenced code.
fn frame() -> Div {
    div()
        .w_full()
        .min_w_0()
        .rounded_md()
        .bg(rgb(colors().rail))
        .border_1()
        .border_color(rgb(colors().border))
        .overflow_hidden()
}

impl Plait {
    fn group_open(&self, key: &str) -> bool {
        let thread = self
            .host
            .thread
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.id);
        thread == Some(&self.workspace.reading.groups_thread)
            && self.workspace.reading.open_groups.contains(key)
    }

    /// Whether a tool's details are on screen: opened, and not folded into a closed group.
    pub(super) fn tool_details_shown(&self, id: &str) -> bool {
        let hidden = self
            .workspace
            .formatted
            .runs
            .place(id)
            .is_some_and(|place| place.folded() && !self.group_open(place.run.key()));
        self.workspace.expanded_live_tools.contains(id) && !hidden
    }

    pub(super) fn toggle_live_tool(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(snapshot) = self.conversation_snapshot() else {
            return;
        };
        let Some(offset) = snapshot.tools.iter().position(|tool| tool.id == id) else {
            return;
        };
        let Some(index) = self
            .workspace
            .live_layout
            .iter()
            .position(|slot| *slot == Slot::Tool(offset))
        else {
            return;
        };
        if !self.workspace.expanded_live_tools.remove(id) {
            self.workspace.expanded_live_tools.insert(id.to_owned());
        }
        self.workspace.live_scroll.remeasure_items(index..index + 1);
        cx.notify();
    }

    /// Open or close the group that starts with the tool `key`.
    pub(super) fn toggle_tool_group(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(thread) = self
            .host
            .thread
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.id)
        else {
            return;
        };
        if self.workspace.reading.groups_thread != *thread {
            self.workspace.reading.groups_thread = thread.clone();
            self.workspace.reading.open_groups.clear();
        }
        if !self.workspace.reading.open_groups.remove(key) {
            self.workspace.reading.open_groups.insert(key.to_owned());
        }
        // The next frame remeasures the rows this shows or hides.
        self.workspace.reading.planned.set(0);
        cx.notify();
    }

    /// Remeasure tool rows whose folding changed since they were last drawn.
    /// A row that is on screen is laid out afresh anyway; this keeps the
    /// heights of rows outside the viewport honest, so scrolling stays even.
    pub(super) fn reconcile_tool_rows(&self) {
        let stamp = Arc::as_ptr(&self.workspace.formatted) as usize;
        if self.workspace.reading.planned.replace(stamp) == stamp {
            return;
        }
        let mut previous = self.workspace.reading.folds.borrow_mut();
        let mut folds = HashMap::with_capacity(previous.len());
        let rows = self.workspace.live_scroll.item_count();
        for run in self.workspace.formatted.runs.iter() {
            let open = self.group_open(run.key());
            for (position, id) in run.tools.iter().enumerate() {
                let state = (
                    position < run.grouped && !open,
                    position == 0 && run.grouped > 0,
                );
                let row = run.first_row + position;
                if previous.get(id).is_some_and(|drawn| *drawn != state) && row < rows {
                    self.workspace.live_scroll.remeasure_items(row..row + 1);
                }
                folds.insert(id.clone(), state);
            }
        }
        *previous = folds;
    }

    /// One conversation row for a tool, or nothing while it is folded away.
    pub(super) fn tool_row(
        &self,
        tool: &Tool,
        row: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let place = self.workspace.formatted.runs.place(&tool.id);
        let open = place.is_some_and(|place| self.group_open(place.run.key()));
        let folded = place.is_some_and(|place| place.folded());
        let summary = place
            .filter(|place| place.leads())
            .map(|place| self.group_summary(place.run, open, cx));
        let card = (!folded || open).then(|| {
            column()
                .when(
                    self.workspace.expanded_live_tools.contains(&tool.id),
                    |el| {
                        el.children(self.history_parts(
                            plait_client::history::Key::Tool(tool.id.clone()),
                            None,
                            cx,
                        ))
                    },
                )
                .child(self.tool_card(tool, row, folded, cx))
        });
        (summary.is_some() || card.is_some()).then(|| {
            column()
                .gap(px(2.))
                .children(summary)
                .children(card)
                .into_any_element()
        })
    }

    fn group_summary(&self, run: &Run, open: bool, cx: &mut Context<Self>) -> AnyElement {
        let key = run.key().to_owned();
        let failed = (run.failed > 0).then(|| format!("{} failed", run.failed));
        let name = match &failed {
            Some(failed) => format!("{}, {failed}", run.summary),
            None => run.summary.clone(),
        };
        row()
            .min_w_0()
            .child(
                button(
                    (
                        ElementId::from("tool-group"),
                        SharedString::from(key.clone()),
                    ),
                    "",
                )
                .px_1()
                .ml(px(-TOOL_INSET))
                .min_w_0()
                .aria_label(name)
                .aria_expanded(open)
                .child(icon(if open { "down" } else { "right" }))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .child(run.summary.clone()),
                )
                .when_some(failed, |el, failed| {
                    el.child(tag("tool-group-failed", failed, colors().red).flex_shrink_0())
                })
                .on_activate(cx, move |this, _, cx| this.toggle_tool_group(&key, cx)),
            )
            .into_any_element()
    }

    fn tool_card(
        &self,
        tool: &Tool,
        index: usize,
        nested: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.workspace.expanded_live_tools.contains(&tool.id);
        let id = tool.id.clone();
        let subject = transcript::tool_subject(tool);
        let view = self.workspace.formatted.tools.get(&tool.id);
        let facts = view.and_then(|view| view.facts.clone());
        column()
            .gap_1()
            // Calls inside an open group step in under its summary line.
            .when(nested, |el| el.pl(px(16.)))
            .child(
                row().min_w_0().child(
                    button(
                        (
                            ElementId::from("live-tool"),
                            SharedString::from(tool.id.clone()),
                        ),
                        "",
                    )
                    .px_1()
                    .ml(px(-TOOL_INSET))
                    .min_w_0()
                    // A long subject wraps under itself rather than running
                    // past the window, so the row grows and may shrink.
                    .flex_shrink_1()
                    .h_auto()
                    .py(px(4.))
                    .items_start()
                    .line_height(px(TOOL_LINE))
                    .aria_label(format!(
                        "{} tool{} · {}",
                        tool.name,
                        subject
                            .as_ref()
                            .map(|subject| format!(" {subject}"))
                            .unwrap_or_default(),
                        tool.status.label()
                    ))
                    .aria_expanded(expanded)
                    .child(line_icon(if expanded { "down" } else { "right" }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(13.))
                            .child(tool.name.clone()),
                    )
                    .when_some(subject, |el, subject| {
                        el.child(
                            div()
                                .min_w_0()
                                // The opened call shows the whole command.
                                .line_clamp(SUBJECT_LINES)
                                .font_family(mono())
                                .text_size(px(12.))
                                .text_color(rgb(colors().muted))
                                .child(subject),
                        )
                    })
                    .when_some(facts, |el, facts| {
                        el.child(
                            tag("tool-facts", facts, colors().subtle)
                                .flex_shrink_0()
                                .whitespace_nowrap(),
                        )
                    })
                    // Completion is the normal case; only call out the others.
                    .when(tool.status != ToolStatus::Done, |el| {
                        el.child(
                            tag("tool-status", tool.status.label(), tool_tint(tool.status))
                                .flex_shrink_0(),
                        )
                    })
                    .on_activate(cx, move |this, _, cx| this.toggle_live_tool(&id, cx)),
                ),
            )
            .when(expanded, |el| {
                el.child(self.tool_details(tool, view.cloned(), index, cx))
            })
            .into_any_element()
    }

    fn tool_details(
        &self,
        tool: &Tool,
        view: Option<Arc<ToolView>>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = SharedString::from(format!("tool-details-{}", tool.id));
        let sections = view
            .as_ref()
            .map(|view| view.sections.as_slice())
            .unwrap_or_default();
        let has_copy = sections.len() > 1
            || sections
                .iter()
                .any(|section| !section.block.text.is_empty());
        let body = sections.iter().enumerate().map(|(section_index, section)| {
            let site = Site {
                owner: Owner::Tool(tool.id.clone().into()),
                row: index,
                block: section_index,
            };
            self.tool_section(tool, section, &site)
        });
        code_block()
            // Captions and notes are prose; each code body sets its own font.
            .font_family(".SystemUIFont")
            .group(group.clone())
            .relative()
            .gap_2()
            .children(body.collect::<Vec<_>>())
            .when(sections.is_empty(), |el| {
                el.child(tag(
                    SharedString::from(format!("tool-empty-{}", tool.id)),
                    if tool.status == ToolStatus::Running {
                        "No result yet."
                    } else {
                        "Nothing to show."
                    },
                    colors().subtle,
                ))
            })
            .when(has_copy, |el| {
                el.child(
                    div().absolute().top(px(4.)).right(px(4.)).child(
                        quiet_button(
                            SharedString::from(format!("copy-tool-{}", tool.id)),
                            "Copy",
                            group,
                        )
                        .bg(rgb(colors().panel))
                        .aria_label("Copy tool details")
                        .on_activate(cx, move |_, _, cx| {
                            if let Some(view) = &view {
                                cx.write_to_clipboard(ClipboardItem::new_string(view.text()));
                            }
                        }),
                    ),
                )
            })
            .into_any_element()
    }

    fn tool_section(&self, tool: &Tool, section: &Section, site: &Site) -> AnyElement {
        let block = &section.block;
        let key = format!("tool-{}-{}", tool.id, site.block);
        let code = |color: u32| {
            self.code_text(site, block, SharedString::from(key.clone()).into())
                .text_color(rgb(color))
        };
        // A failed call says so in words on its own line, which stays in view
        // above this; its output is outlined, and the word is not repeated.
        let (caption, edge) = match (section.part, tool.status) {
            (Part::Output, ToolStatus::Error) => (None, Some(colors().failed_edge)),
            (Part::Output, ToolStatus::Interrupted) => (
                Some(("Interrupted before it finished", colors().amber)),
                None,
            ),
            (Part::Input, _) => (Some(("Arguments as received", colors().subtle)), None),
            _ => (None, None),
        };
        let body = match (section.part, &block.kind) {
            (Part::Command, _) => div()
                .flex()
                .items_start()
                .w_full()
                .min_w_0()
                .font_family(mono())
                .text_size(px(12.))
                .line_height(px(19.))
                .child(
                    div()
                        .w(px(16.))
                        .flex_shrink_0()
                        .text_color(rgb(colors().subtle))
                        .child("$"),
                )
                // A command is read as a line, not scanned like code; wrap it.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(code(colors().text).whitespace_normal()),
                )
                .into_any_element(),
            (Part::Diff, Kind::Diff(lines)) => frame()
                .child(self.diff(site, block, lines, &key))
                .into_any_element(),
            (Part::Input, _) => frame()
                .px_3()
                .py_2()
                .child(code(colors().muted))
                .into_any_element(),
            _ => frame()
                .px_3()
                .py_2()
                .when_some(edge, |el, edge| el.border_color(rgba(edge)))
                .child(code(colors().text))
                .into_any_element(),
        };
        column()
            .w_full()
            .min_w_0()
            .gap_1()
            .when_some(caption, |el, (caption, tint)| {
                el.child(tag(
                    SharedString::from(format!("{key}-caption")),
                    caption,
                    tint,
                ))
            })
            .child(body)
            .when_some(section.note.clone(), |el, note| {
                el.child(tag(
                    SharedString::from(format!("{key}-note")),
                    note,
                    colors().subtle,
                ))
            })
            .into_any_element()
    }
}
