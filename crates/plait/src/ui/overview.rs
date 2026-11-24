//! Every thread, projected from the current machine's cached sidebar data.

use super::{Plait, navigation::Row, panels::Panel, theme::*, thread_controls::TextTooltip};
use gpui::{prelude::*, *};
use plait_client::{
    navigation::{Card, Overview, OverviewCounts, OverviewDirection, OverviewGrid, ThreadRowState},
    state::project_name,
};
use std::{path::Path, rc::Rc};

const CARD_MIN: f32 = 290.;
const GAP: f32 = 10.;
const GRID_INSET: f32 = 16.;

pub(super) struct OverviewView {
    model: Overview,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
    sidebar: bool,
    scroll: ListState,
    sidebar_rows: Option<Rc<Vec<Row>>>,
    grid: Rc<OverviewGrid>,
    columns: usize,
    position: Option<(usize, usize)>,
}

impl OverviewView {
    #[cfg(all(test, debug_assertions))]
    pub(super) fn focused_thread(&self) -> Option<&str> {
        self.model.focus.as_deref()
    }

    fn reveal(&self, row: usize) {
        if self.scroll.bounds_for_item(row).is_some() {
            self.scroll.scroll_to_reveal_item(row);
        } else {
            self.scroll.scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: px(0.),
            });
        }
    }
}

impl Plait {
    pub(super) fn toggle_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || self.workspace.images.enlarged.is_some() {
            return;
        }
        if self.overview.is_some() {
            self.close_overview(window, cx);
            return;
        }
        self.close_overlay(true, window, cx);
        let focus = cx.focus_handle().tab_stop(true);
        let restore = window.focused(cx);
        window.focus(&focus, cx);
        self.overview = Some(OverviewView {
            model: Overview {
                focus: self.host.thread.selected.clone(),
            },
            focus,
            restore,
            sidebar: self.sidebar,
            scroll: ListState::new(0, ListAlignment::Top, px(90.)),
            sidebar_rows: None,
            grid: Rc::new(OverviewGrid {
                counts: OverviewCounts::default(),
                rows: Vec::new(),
            }),
            columns: 0,
            position: None,
        });
        cx.notify();
    }

    pub(super) fn close_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.overview.take() {
            let restore = view.restore.filter(|_| view.sidebar == self.sidebar);
            window.focus(restore.as_ref().unwrap_or(&self.focus), cx);
            cx.notify();
        }
    }

    pub(super) fn overview_focus(&self) -> Option<&FocusHandle> {
        self.overview.as_ref().map(|view| &view.focus)
    }

    fn open_overview_thread(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overview(window, cx);
        // Use the normal tab-opening path, including checkout identity and bookmarks.
        self.select_host_thread(id, window, cx);
    }

    fn overview_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers != Modifiers::default() {
            return;
        }
        self.refresh_overview(cx);
        use OverviewDirection::*;
        let Some(view) = &mut self.overview else {
            return;
        };
        let threads = self.host.connection.threads();
        let direction = match event.keystroke.key.as_str() {
            "left" => Left,
            "right" => Right,
            "up" => Up,
            "down" => Down,
            "home" => First,
            "end" => Last,
            "enter" => {
                if !event.is_held
                    && let Some((row, col)) = view.position
                {
                    let id = threads[view.grid.rows[row].threads[col]].id.clone();
                    self.open_overview_thread(id, window, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if let Some(position) = view.model.move_focus(direction, &view.grid, threads) {
            view.position = Some(position);
            view.reveal(position.0);
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn refresh_overview(&mut self, cx: &Context<Self>) {
        // Match the sheet's margin, border and inner padding when the sidebar is shown.
        let sheet_width = self.panels.viewport
            - if self.sidebar {
                self.panel_width(Panel::Sidebar) + 18.
            } else {
                0.
            };
        let columns = (((sheet_width - 2. * GRID_INSET + GAP) / (CARD_MIN + GAP)) as usize).max(1);
        let sidebar_rows = self.navigation_rows(cx);
        let refresh = self.overview.as_ref().is_some_and(|view| {
            view.columns != columns
                || view
                    .sidebar_rows
                    .as_ref()
                    .is_none_or(|old| !Rc::ptr_eq(old, &sidebar_rows))
        });
        if refresh {
            let threads = self.host.connection.threads();
            let grid = OverviewGrid::new(threads, &self.state.projects, self.lists(), columns);
            let view = self.overview.as_mut().expect("overview is open");
            let position = view.model.focused(&grid, threads);
            view.model.focus =
                position.map(|(row, col)| threads[grid.rows[row].threads[col]].id.clone());
            let layout_changed = view.columns != columns
                || !view
                    .grid
                    .rows
                    .iter()
                    .map(row_shape)
                    .eq(grid.rows.iter().map(row_shape));
            if layout_changed {
                let top = view.scroll.logical_scroll_top();
                view.scroll
                    .splice(0..view.scroll.item_count(), grid.rows.len());
                view.scroll.scroll_to(top);
            }
            if let Some((row, _)) = position
                && (layout_changed || view.position.is_none_or(|(old, _)| old != row))
            {
                view.reveal(row);
            }
            view.grid = Rc::new(grid);
            view.sidebar_rows = Some(sidebar_rows);
            view.position = position;
            view.columns = columns;
        }
    }

    pub(super) fn overview_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_overview(cx);
        let view = self.overview.as_ref().expect("overview is open");
        let columns = view.columns;
        let grid = view.grid.clone();
        let position = view.position;
        column()
            .flex_1()
            .min_h_0()
            .child(self.overview_bar(&grid.counts, cx))
            .child(
                column()
                    .id("overview-grid")
                    .role(Role::Group)
                    .aria_label("Every thread cards")
                    .audited()
                    .flex_1()
                    .min_h_0()
                    .track_focus(&view.focus)
                    .tab_stop(true)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if this
                            .overview_focus()
                            .is_some_and(|focus| focus.is_focused(window))
                        {
                            this.overview_key(event, window, cx);
                        }
                    }))
                    .when(grid.rows.is_empty(), |el| {
                        el.child(
                            column()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .child(a11y_text("overview-empty", "No threads yet"))
                                .child(
                                    toned_button("overview-empty-new", "New thread", Tone::Filled)
                                        .on_activate(cx, |this, window, cx| {
                                            this.close_overview(window, cx);
                                            this.open_draft(window, cx);
                                        }),
                                ),
                        )
                    })
                    .when(!grid.rows.is_empty(), |el| {
                        el.child(
                            list(
                                view.scroll.clone(),
                                cx.processor(move |this, index: usize, _, cx| {
                                    let item = &grid.rows[index];
                                    column()
                                        .w_full()
                                        .pb(px(GAP))
                                        .when_some(item.heading.as_deref(), |el, path| {
                                            el.when(index > 0, |el| el.pt(px(4.)))
                                                .child(this.overview_project(path, index, cx))
                                        })
                                        .when(!item.threads.is_empty(), |el| {
                                            el.child(
                                                row()
                                                    .items_stretch()
                                                    .gap(px(GAP))
                                                    .children(item.threads.iter().enumerate().map(
                                                        |(col, &thread)| {
                                                            this.overview_card(
                                                                thread,
                                                                position == Some((index, col)),
                                                                cx,
                                                            )
                                                        },
                                                    ))
                                                    .children(
                                                        (item.threads.len()..columns)
                                                            .map(|_| div().flex_1().min_w_0()),
                                                    ),
                                            )
                                        })
                                        .into_any_element()
                                }),
                            )
                            .size_full()
                            .px(px(GRID_INSET))
                            .pt_2()
                            .pb_4(),
                        )
                    }),
            )
    }

    fn overview_bar(&self, counts: &OverviewCounts, cx: &mut Context<Self>) -> AnyElement {
        let find = self.shortcut_label("palette.toggle");
        self.window_drag(
            row()
                .h(px(46.))
                .flex_shrink_0()
                .px_3()
                .gap(px(6.))
                .border_b_1()
                .border_color(rgb(colors().border))
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(rgb(colors().strong))
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .child(a11y_text("overview-title", "Every thread")),
                )
                .children(
                    [
                        (
                            "overview-need",
                            counts.need_you,
                            // "1 needs you", "2 need you".
                            if counts.need_you == 1 {
                                "needs you"
                            } else {
                                "need you"
                            },
                            colors().amber,
                        ),
                        ("overview-working", counts.working, "working", colors().blue),
                        (
                            "overview-review",
                            counts.to_review,
                            "to review",
                            colors().mint,
                        ),
                    ]
                    .into_iter()
                    .filter(|(_, count, _, _)| *count != 0)
                    .map(|(id, count, suffix, color)| {
                        tag(id, format!("{count} {suffix}"), color)
                            .rounded_full()
                            .bg(rgba((color << 8) | 0x24))
                            .px(px(7.))
                            .py(px(2.))
                            .whitespace_nowrap()
                    }),
                )
                .child(div().flex_1().min_w_0())
                .child(
                    button("overview-find", "Find")
                        .gap_1()
                        .aria_keyshortcuts(self.shortcut_aria("palette.toggle"))
                        .when(!find.is_empty(), |el| {
                            el.child(key_hint("overview-find-key", find))
                        })
                        .on_activate(cx, |this, window, cx| {
                            this.toggle_command_palette(window, cx)
                        }),
                )
                .child(
                    toned_button("overview-new", "New thread", Tone::Primary)
                        .aria_keyshortcuts(self.shortcut_aria("thread.new"))
                        .on_activate(cx, |this, window, cx| {
                            this.close_overview(window, cx);
                            this.open_draft(window, cx);
                        }),
                )
                .child(
                    button("overview-back", "Back")
                        .gap_1()
                        .aria_keyshortcuts("Escape")
                        .child(key_hint("overview-back-key", "Esc"))
                        .on_activate(cx, Self::close_overview),
                ),
            cx,
        )
        .into_any_element()
    }

    fn overview_project(&self, path: &Path, index: usize, cx: &mut Context<Self>) -> Div {
        let name = project_name(path);
        let full = path.display().to_string();
        let checkout = path.to_owned();
        row()
            .h(px(26.))
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(12.))
                    .text_color(rgb(colors().muted))
                    .child(a11y_text(("overview-project", index), name.clone())),
            )
            .child(
                div()
                    .id(("overview-project-path", index))
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .font_family(mono())
                    .text_size(px(11.))
                    .text_color(rgb(colors().subtle))
                    .child(a11y_text("path", home_relative(path)))
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(full.clone())).into()),
            )
            .child(
                icon_button(
                    ("overview-project-new", index),
                    "plus",
                    format!("New thread in {name}"),
                )
                .size(px(22.))
                .on_activate(cx, move |this, window, cx| {
                    this.close_overview(window, cx);
                    this.open_draft_in(checkout.clone(), window, cx);
                }),
            )
    }

    fn overview_card(&self, index: usize, focused: bool, cx: &mut Context<Self>) -> Control {
        use ThreadRowState::*;
        let thread = &self.host.connection.threads()[index];
        let card = Card::new(
            thread,
            self.lists(),
            self.summary_title(thread),
            self.card_model_name(thread),
            super::navigation::now_ms(),
        );
        let state = match card.state {
            FinishedUnread => "Finished",
            Other if card.spoken_state == "Done" => "Finished",
            Other if card.spoken_state == "Ready" => "Idle",
            _ => card.spoken_state,
        };
        let color = thread_row_colors(card.state, false).0;
        let palette = colors();
        let border = if focused {
            rgb(palette.subtle)
        } else if card.state == Approval {
            rgba((color << 8) | 0x73)
        } else {
            rgb(palette.border)
        };
        let id = thread.id.clone();
        let full_title = card.title.clone();
        let label = format!(
            "{}. {state}{}",
            card.title,
            if card.unread { ". Unread activity" } else { "" },
        );
        nav_row(
            SharedString::from(format!("overview-card-{id}")),
            label,
            false,
        )
        .tab_stop(false)
        .flex_1()
        .min_w_0()
        .h(px(111.))
        .px_3()
        .py_2()
        .gap_1()
        .flex_col()
        .items_stretch()
        .rounded(px(SURFACE_CORNER))
        .border_color(border)
        .when(
            matches!(
                card.state,
                Working | Approval | Failed | Interrupted | FinishedUnread
            ),
            |el| el.bg(rgb(palette.panel)),
        )
        .when(focused, |el| {
            el.bg(rgb(palette.raised))
                .shadow(floating_shadow())
                .aria_active_descendant()
        })
        .tooltip(move |_, cx| cx.new(|_| TextTooltip(full_title.clone())).into())
        .child(
            row()
                .gap_1()
                .text_size(px(11.))
                .h(px(16.))
                .child(super::navigation::card_mark(
                    card.state,
                    color,
                    px(9.),
                    self.host.connection.connected,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(if card.state == Other {
                            palette.subtle
                        } else {
                            color
                        }))
                        .child(state),
                )
                .when_some(
                    card.details.as_ref().and_then(|d| d.age.clone()),
                    |el, age| {
                        el.child(
                            div()
                                .font_family(mono())
                                .text_color(rgb(palette.subtle))
                                .child(age),
                        )
                    },
                ),
        )
        .child(
            div()
                .line_height(px(19.))
                .line_clamp(2)
                .text_size(px(13.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(palette.strong))
                .child(card.title),
        )
        .child(div().flex_1())
        .when(
            card.branch.is_some()
                || card
                    .details
                    .as_ref()
                    .is_some_and(|d| d.harness.is_some() || d.model.is_some()),
            |el| {
                el.child(
                    row()
                        .h(px(22.))
                        .gap(px(5.))
                        .border_t_1()
                        .border_color(rgb(palette.border))
                        .text_size(px(11.))
                        .text_color(rgb(palette.subtle))
                        .when_some(card.details, |el, details| {
                            el.when_some(details.harness, |el, harness| {
                                if let Some(logo) = harness_logo(harness) {
                                    el.child(logo.size(px(11.)).text_color(rgb(palette.subtle)))
                                } else {
                                    el.child(harness.label())
                                }
                            })
                            .when_some(details.model, |el, model| {
                                el.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(model.into_owned()),
                                )
                            })
                        })
                        .when_some(card.branch, |el, branch| {
                            el.child(icon("branch").size(px(11.)).text_color(rgb(palette.subtle)))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(mono())
                                        .child(branch.to_owned()),
                                )
                        }),
                )
            },
        )
        .on_activate(cx, move |this, window, cx| {
            this.open_overview_thread(id.clone(), window, cx)
        })
    }
}

fn row_shape(row: &plait_client::navigation::OverviewRow) -> (bool, bool) {
    (row.heading.is_some(), row.threads.is_empty())
}
