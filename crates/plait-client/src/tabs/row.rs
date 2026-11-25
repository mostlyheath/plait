//! Cheap tab-row facts from the existing catalog; no resident view access.
use super::*;
use crate::{
    host::{ThreadStatus, ThreadSummary},
    navigation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabState {
    Approve,
    Interrupted,
    Failed,
    Working,
    Waiting,
    Held,
    Finished,
    Idle,
    Stopped,
    Offline,
    Unavailable,
}
impl TabState {
    pub fn word(self, active: bool) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Interrupted => "Interrupted",
            Self::Failed => "Failed",
            Self::Working => "Working",
            Self::Waiting => "Waiting",
            Self::Held => "Held",
            Self::Finished => "Done",
            Self::Idle if active => "Ready",
            Self::Idle => "",
            Self::Stopped => "Stopped",
            Self::Offline => "Offline",
            Self::Unavailable => "Unavailable",
        }
    }
    pub fn group(self) -> TabGroup {
        match self {
            Self::Approve | Self::Interrupted | Self::Held => TabGroup::NeedsMe,
            Self::Failed => TabGroup::Failed,
            Self::Working | Self::Waiting => TabGroup::Working,
            Self::Finished => TabGroup::Finished,
            Self::Idle | Self::Stopped | Self::Offline | Self::Unavailable => TabGroup::Idle,
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Approve => "Needs approval — open the conversation for the exact call",
            Self::Interrupted => "Interrupted before completion",
            Self::Failed => "Failed — inspect conversation",
            Self::Working => "Working — open the conversation for current activity",
            Self::Waiting => "Waiting for the checkout",
            Self::Held => "Checkout queue held — Resume to continue",
            Self::Finished => "Finished — unread result",
            Self::Idle => "Ready for input",
            Self::Stopped => "Stopped",
            Self::Offline => "Host disconnected — status is unavailable",
            Self::Unavailable => "Thread summary unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabGroup {
    NeedsMe,
    Failed,
    Working,
    Finished,
    Idle,
}
impl TabGroup {
    pub const ALL: [Self; 5] = [
        Self::NeedsMe,
        Self::Failed,
        Self::Working,
        Self::Finished,
        Self::Idle,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::NeedsMe => "Needs me",
            Self::Failed => "Failed",
            Self::Working => "Working",
            Self::Finished => "Finished",
            Self::Idle => "Idle",
        }
    }
}

pub struct RowContext<'a> {
    pub host: &'a HostKey,
    pub threads: &'a [ThreadSummary],
    pub navigation: &'a navigation::Preferences,
    pub machine: &'a str,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabMetadata {
    pub id: TabId,
    pub title: String,
    pub worktrees: bool,
    pub issues: bool,
    pub project: String,
    pub state: TabState,
    pub word: &'static str,
    pub hover: String,
    pub close_hover: &'static str,
    pub active: bool,
    pub unread: bool,
    pub archived: bool,
    pub shortcut: Option<u8>,
}

/// Activation does not mark a conversation read: Files is not an approval view.
/// The UI updates navigation.unread only once it actually presents the content.
pub fn metadata(window: &WindowTabs, context: &RowContext<'_>) -> Vec<TabMetadata> {
    let summaries = tab_summaries(window, context);
    window
        .tabs
        .iter()
        .zip(summaries)
        .enumerate()
        .map(|(index, (tab, summary))| {
            let target = &tab.session().target;
            let checkout = target.checkout();
            let same_host = &checkout.host == context.host;
            let active = window.active == Some(tab.id);
            let unread = summary.is_some_and(|t| context.navigation.unread.contains(&t.id));
            let state = if !same_host || !tab.session().connected() {
                TabState::Offline
            } else if target.is_draft() || target.is_project_view() {
                TabState::Idle
            } else {
                summary.map_or(TabState::Unavailable, |t| summary_state(t, unread))
            };
            let title = if target.is_worktrees() {
                "Worktrees"
            } else if target.is_issues() {
                "Issues"
            } else if target.is_draft() {
                "New thread"
            } else {
                summary
                    .and_then(|t| t.title.as_deref())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("Untitled thread")
            };
            let title = crate::display_text::escape(title);
            let project_path = context.navigation.project_of(&checkout.root);
            let project = crate::display_text::escape(&crate::state::project_name(project_path));
            let archived = summary.is_some_and(|t| t.archived == Some(true));
            let mut hover = format!(
                "{title} · {project} / {}\n{}{}",
                crate::display_text::escape(context.machine),
                if target.is_worktrees() && state != TabState::Offline {
                    "Project branches, worktrees and storage"
                } else if target.is_issues() && state != TabState::Offline {
                    "The project's Linear issues"
                } else {
                    state.description()
                },
                if archived { "\nArchived" } else { "" }
            );
            if let Some(queue) = summary.and_then(|t| t.checkout_queue.as_ref()) {
                hover.push_str(&format!("\n{}", queue.detail()));
            }
            TabMetadata {
                id: tab.id,
                worktrees: target.is_worktrees(),
                issues: target.is_issues(),
                title,
                project,
                state,
                word: if target.is_project_view() {
                    ""
                } else {
                    state.word(active)
                },
                hover,
                active,
                unread,
                archived,
                close_hover: if matches!(state, TabState::Working | TabState::Approve) {
                    "Close tab — thread keeps running in the background"
                } else {
                    "Close tab (⌘W)"
                },
                shortcut: if index < 8 {
                    Some(index as u8 + 1)
                } else if index + 1 == window.tabs.len() {
                    Some(9)
                } else {
                    None
                },
            }
        })
        .collect()
}

/// One pass over the catalog, which may hold thousands of threads, rather than
/// one scan per tab. The first summary with the tab's ID and checkout wins.
fn tab_summaries<'a>(
    window: &WindowTabs,
    context: &RowContext<'a>,
) -> Vec<Option<&'a ThreadSummary>> {
    let mut found = vec![None; window.tabs.len()];
    let wanted = window
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(i, tab)| match &tab.session().target {
            TabTarget::Existing { id, checkout } if &checkout.host == context.host => {
                Some((id.as_str(), (i, checkout.root.as_path())))
            }
            _ => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut remaining = wanted.len();
    for thread in context.threads {
        if remaining == 0 {
            break;
        }
        if let Some(&(i, root)) = wanted.get(thread.id.as_str())
            && found[i].is_none()
            && thread.checkout == root
        {
            found[i] = Some(thread);
            remaining -= 1;
        }
    }
    found
}

pub fn summary_state(thread: &ThreadSummary, unread: bool) -> TabState {
    if !thread.status.busy()
        && let Some(queue) = &thread.checkout_queue
    {
        return if queue.state == crate::host::checkout_queue::State::Held {
            TabState::Held
        } else {
            TabState::Waiting
        };
    }
    if thread.pending_approval == Some(true) {
        return TabState::Approve;
    }
    match thread.status {
        ThreadStatus::AwaitingApproval => TabState::Approve,
        ThreadStatus::Interrupted => TabState::Interrupted,
        ThreadStatus::Error => TabState::Failed,
        ThreadStatus::Running => TabState::Working,
        ThreadStatus::Idle if unread => TabState::Finished,
        ThreadStatus::Idle => TabState::Idle,
        ThreadStatus::Stopped => TabState::Stopped,
    }
}

pub struct GroupedTabs {
    pub group: TabGroup,
    pub tabs: Vec<TabId>,
}
pub struct TabLayout {
    pub visible: Vec<TabId>,
    pub hidden: Vec<TabId>,
    pub tab_width: f32,
    pub first_visible: usize,
    /// The popover contains ALL tabs, preserving strip order within each group.
    pub groups: Vec<GroupedTabs>,
    pub overflow_label: String,
    pub overflow_hover: String,
}

pub struct RowWidths {
    /// Excludes lights, sidebar toggle, new-tab button and window drag area.
    pub available: f32,
    pub minimum_tab: f32,
    pub maximum_tab: f32,
    /// Measured by the UI; reserved only when tabs overflow.
    pub overflow: f32,
}

/// A contiguous viewport preserves order and shifts only enough to reveal the
/// active tab. At less than one minimum width, one active tab is clipped to fit.
pub fn layout(
    tabs: &[TabMetadata],
    widths: RowWidths,
    first_visible: usize,
    urgent_limit: usize,
) -> TabLayout {
    let available = finite_width(widths.available);
    let minimum = finite_width(widths.minimum_tab).max(1.0);
    let maximum = finite_width(widths.maximum_tab).max(minimum);
    let overflowing = tabs.len() as f32 * minimum > available;
    let space = (available
        - if overflowing {
            finite_width(widths.overflow)
        } else {
            0.0
        })
    .max(0.0);
    // Dividing again could round an exact fit below the tab count.
    let count = if !overflowing {
        tabs.len()
    } else if space == 0.0 {
        0
    } else {
        ((space / minimum).floor() as usize).max(1).min(tabs.len())
    };
    let mut start = first_visible.min(tabs.len().saturating_sub(count));
    if count > 0
        && let Some(active) = tabs.iter().position(|t| t.active)
    {
        if active < start {
            start = active;
        }
        if active >= start + count {
            start = active + 1 - count;
        }
    }
    let end = start + count;
    let visible = tabs[start..end].iter().map(|t| t.id).collect();
    let hidden = tabs
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < start || *i >= end)
        .map(|(_, t)| t)
        .collect::<Vec<_>>();
    let groups = TabGroup::ALL
        .into_iter()
        .filter_map(|group| {
            let ids = tabs
                .iter()
                .filter(|t| t.state.group() == group)
                .map(|t| t.id)
                .collect::<Vec<_>>();
            (!ids.is_empty()).then_some(GroupedTabs { group, tabs: ids })
        })
        .collect();
    let (overflow_label, overflow_hover) = overflow_text(&hidden, urgent_limit.min(2));
    TabLayout {
        visible,
        hidden: hidden.iter().map(|t| t.id).collect(),
        tab_width: if count == 0 {
            0.0
        } else {
            (space / count as f32).min(maximum)
        },
        first_visible: start,
        groups,
        overflow_label,
        overflow_hover,
    }
}
fn finite_width(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}
fn overflow_text(hidden: &[&TabMetadata], urgent_limit: usize) -> (String, String) {
    if hidden.is_empty() {
        return (String::new(), String::new());
    }
    let mut label = format!("+{}", hidden.len());
    let mut urgent = 0;
    let mut counts = Vec::new();
    for (state, word, description, important) in [
        (TabState::Approve, "Approve", "need approval", true),
        (TabState::Interrupted, "Interrupted", "interrupted", true),
        (TabState::Failed, "Failed", "failed", true),
        (TabState::Working, "Working", "working", true),
        (TabState::Finished, "Done", "done", false),
        (TabState::Idle, "Ready", "ready", false),
        (TabState::Stopped, "Stopped", "stopped", false),
        (TabState::Offline, "Offline", "offline", false),
        (TabState::Unavailable, "Unavailable", "unavailable", false),
    ] {
        let count = hidden.iter().filter(|t| t.state == state).count();
        if count == 0 {
            continue;
        }
        counts.push(format!("{count} {description}"));
        if important && urgent < urgent_limit {
            label.push_str(&format!(" · {count} {word}"));
            urgent += 1;
        }
    }
    (
        label,
        format!("{} hidden tabs: {}", hidden.len(), counts.join(", ")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::tests::{checkout, host, open, summary, target, window};

    #[test]
    fn all_states_come_from_summary_and_approval_has_priority() {
        for (status, unread, expected) in [
            (ThreadStatus::Running, false, TabState::Working),
            (ThreadStatus::AwaitingApproval, false, TabState::Approve),
            (ThreadStatus::Interrupted, true, TabState::Interrupted),
            (ThreadStatus::Error, true, TabState::Failed),
            (ThreadStatus::Idle, true, TabState::Finished),
            (ThreadStatus::Idle, false, TabState::Idle),
            (ThreadStatus::Stopped, true, TabState::Stopped),
        ] {
            let mut t = summary("a", status);
            assert_eq!(summary_state(&t, unread), expected);
            t.pending_approval = Some(true);
            assert_eq!(summary_state(&t, unread), TabState::Approve);
        }
        assert_eq!(TabState::Idle.word(false), "");
        assert_eq!(TabState::Idle.word(true), "Ready");
    }

    #[test]
    fn rename_and_archive_update_metadata_without_changing_tab_or_bookmarks() {
        let mut w = window();
        let a = open(&mut w, "a");
        let stamp = w.read_stamp(a).unwrap();
        let mut threads = vec![summary("a", ThreadStatus::Running)];
        let navigation = navigation::Preferences::default();
        let host = host();
        let rows = metadata(
            &w,
            &RowContext {
                host: &host,
                threads: &threads,
                navigation: &navigation,
                machine: "Mac",
            },
        );
        assert_eq!(rows[0].title, "Thread a");
        assert_eq!(rows[0].state, TabState::Working);
        assert!(rows[0].close_hover.contains("keeps running"));
        assert!(!rows[0].hover.contains("waiting for model"));
        threads[0].title = Some("Renamed".into());
        threads[0].archived = Some(true);
        let rows = metadata(
            &w,
            &RowContext {
                host: &host,
                threads: &threads,
                navigation: &navigation,
                machine: "Mac",
            },
        );
        assert_eq!(rows[0].title, "Renamed");
        assert!(rows[0].archived);
        assert!(rows[0].hover.contains("Archived"));
        assert!(w.accepts_read(stamp));
        assert_eq!(w.tabs().len(), 1);
        w.close(a, false).unwrap();
        assert!(w.reopen(|_| Some(Some(1))).unwrap().is_some());
        assert_eq!(threads[0].archived, Some(true));
    }

    #[test]
    fn opening_files_does_not_clear_unread_and_offline_never_implies_live_status() {
        let mut w = window();
        let a = open(&mut w, "a");
        w.save_bookmark(
            a,
            BookmarkRecord::capture(&bookmark::ThreadBookmark {
                view: ThreadView::Files,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let threads = vec![summary("a", ThreadStatus::Idle)];
        let host = host();
        let mut navigation = navigation::Preferences::default();
        navigation.unread.insert("a".into());
        let context = RowContext {
            host: &host,
            threads: &threads,
            navigation: &navigation,
            machine: "Mac",
        };
        assert_eq!(metadata(&w, &context)[0].state, TabState::Finished);
        assert!(navigation.unread.contains("a"));
        w.disconnected(&host);
        let rows = metadata(&w, &context);
        assert_eq!(rows[0].state, TabState::Offline);
        assert!(rows[0].hover.contains("disconnected"));
    }

    #[test]
    fn missing_or_wrong_host_summary_is_never_shown_as_ready() {
        let mut w = window();
        open(&mut w, "a");
        let host = host();
        let nav = navigation::Preferences::default();
        let context = RowContext {
            host: &host,
            threads: &[],
            navigation: &nav,
            machine: "Mac",
        };
        let rows = metadata(&w, &context);
        assert_eq!(rows[0].state, TabState::Unavailable);
        let other_host = HostKey {
            id: "other".into(),
            ..host.clone()
        };
        assert_eq!(
            metadata(
                &w,
                &RowContext {
                    host: &other_host,
                    ..context
                }
            )[0]
            .state,
            TabState::Offline
        );
    }

    #[test]
    fn titles_are_escaped_and_context_uses_real_project_grouping() {
        let mut w = window();
        open(&mut w, "a");
        let mut threads = vec![summary("a", ThreadStatus::Idle)];
        threads[0].title = Some("Title\u{202e}spoof".into());
        let host = host();
        let mut navigation = navigation::Preferences::default();
        navigation.remember_worktree("/repo".into(), "/main-project".into(), "branch".into());
        let rows = metadata(
            &w,
            &RowContext {
                host: &host,
                threads: &threads,
                navigation: &navigation,
                machine: "Mac",
            },
        );
        assert!(!rows[0].title.contains('\u{202e}'));
        assert_eq!(rows[0].project, "main-project");
        assert!(rows[0].hover.contains("main-project / Mac"));
    }

    fn fixture() -> Vec<TabMetadata> {
        let mut w = window();
        let statuses = [
            ThreadStatus::Idle,
            ThreadStatus::Running,
            ThreadStatus::AwaitingApproval,
            ThreadStatus::Error,
            ThreadStatus::Interrupted,
            ThreadStatus::Stopped,
            ThreadStatus::Idle,
            ThreadStatus::Idle,
            ThreadStatus::Running,
            ThreadStatus::Idle,
        ];
        let threads = statuses
            .into_iter()
            .enumerate()
            .map(|(i, status)| summary(&format!("t{i}"), status))
            .collect::<Vec<_>>();
        for t in &threads {
            w.open(target(&t.id), Some(1)).unwrap();
        }
        let mut navigation = navigation::Preferences::default();
        navigation.unread.insert("t6".into());
        metadata(
            &w,
            &RowContext {
                host: &host(),
                threads: &threads,
                navigation: &navigation,
                machine: "Mac",
            },
        )
    }
    fn widths(available: f32) -> RowWidths {
        RowWidths {
            available,
            minimum_tab: 128.0,
            maximum_tab: 240.0,
            overflow: 156.0,
        }
    }

    #[test]
    fn widths_compress_equally_then_overflow_reserving_measured_control_width() {
        let tabs = fixture();
        let fitted = layout(&tabs[..3], widths(1184.0), 0, 2);
        assert_eq!(fitted.visible.len(), 3);
        assert_eq!(fitted.tab_width, 240.0);
        assert!(fitted.hidden.is_empty());
        let fitted = layout(&tabs[..9], widths(1184.0), 0, 2);
        assert_eq!(fitted.visible.len(), 9);
        assert!((fitted.tab_width - 1184.0 / 9.0).abs() < 0.01);
        let overflow = layout(&tabs, widths(1184.0), 0, 2);
        assert_eq!(overflow.visible.len(), 8);
        assert_eq!(overflow.first_visible, 2);
        assert_eq!(overflow.hidden.len(), 2);
        assert_eq!(overflow.visible.last(), Some(&tabs[9].id));
        assert_eq!(overflow.tab_width, 128.5);
    }

    #[test]
    fn exactly_fitting_fractional_widths_show_every_tab_without_overflow() {
        // Measured widths are fractional; `n * min` fitting must not round to n - 1.
        let tabs = fixture();
        for n in 1..=tabs.len() {
            for step in 0..500 {
                let minimum_tab = 100.0 + step as f32 * 0.037;
                let available = n as f32 * minimum_tab;
                let layout = layout(
                    &tabs[..n],
                    RowWidths {
                        available,
                        minimum_tab,
                        maximum_tab: 240.0,
                        overflow: 100.0,
                    },
                    0,
                    2,
                );
                assert_eq!(layout.visible.len(), n, "{n} tabs at {minimum_tab}");
                assert!(layout.hidden.is_empty() && layout.overflow_label.is_empty());
            }
        }
    }

    #[test]
    fn viewport_moves_just_enough_and_popover_groups_all_tabs_in_strip_order() {
        let mut tabs = fixture();
        tabs[9].active = false;
        tabs[0].active = true;
        let layout = layout(&tabs, widths(704.0), 8, 2);
        assert_eq!(layout.first_visible, 0);
        assert_eq!(layout.visible.len(), 4);
        assert_eq!(
            layout.groups.iter().map(|g| g.group).collect::<Vec<_>>(),
            TabGroup::ALL
        );
        assert_eq!(layout.groups[0].tabs, [tabs[2].id, tabs[4].id]);
        assert_eq!(layout.groups[2].tabs, [tabs[1].id, tabs[8].id]);
        assert_eq!(
            layout.groups.iter().map(|g| g.tabs.len()).sum::<usize>(),
            tabs.len()
        );
        assert_eq!(layout.overflow_label, "+6 · 1 Interrupted · 1 Working");
        assert!(layout.overflow_hover.contains("1 done"));
    }

    #[test]
    fn hidden_urgent_counts_use_priority_and_limit_only_hidden_tabs() {
        let tabs = fixture();
        let layout = layout(&tabs, widths(300.0), 0, 2);
        assert_eq!(layout.visible, [tabs[9].id]);
        assert_eq!(layout.overflow_label, "+9 · 1 Approve · 1 Interrupted");
        assert!(layout.overflow_hover.contains("1 failed"));
        assert!(layout.overflow_hover.contains("2 working"));
        let narrow = super::layout(&tabs, widths(300.0), 0, 1);
        assert_eq!(narrow.overflow_label, "+9 · 1 Approve");
    }

    #[test]
    fn impossible_and_nonfinite_widths_are_total_without_negative_or_nan_geometry() {
        let tabs = fixture();
        for available in [0.0, -1.0, f32::NAN, f32::INFINITY, 100.0, 157.0] {
            let layout = layout(&tabs, widths(available), usize::MAX, usize::MAX);
            assert!(layout.tab_width.is_finite() && layout.tab_width >= 0.0);
            assert_eq!(layout.visible.len() + layout.hidden.len(), tabs.len());
            if available == 157.0 {
                assert_eq!(layout.visible, [tabs[9].id]);
                assert_eq!(layout.tab_width, 1.0);
            }
        }
        assert!(layout(&[], widths(100.0), usize::MAX, 2).visible.is_empty());
    }

    #[test]
    fn metadata_matches_each_tab_with_one_catalog_pass() {
        let navigation = navigation::Preferences::default();
        let host = host();
        // The first summary with the tab's ID and checkout wins, as before.
        let mut w = window();
        open(&mut w, "a");
        let mut other_checkout = summary("a", ThreadStatus::Running);
        other_checkout.checkout = "/elsewhere".into();
        let mut second = summary("a", ThreadStatus::Error);
        second.title = Some("Second".into());
        let threads = [other_checkout, summary("a", ThreadStatus::Idle), second];
        let context = RowContext {
            host: &host,
            threads: &threads,
            navigation: &navigation,
            machine: "Mac",
        };
        let rows = metadata(&w, &context);
        assert_eq!(
            (rows[0].title.as_str(), rows[0].state),
            ("Thread a", TabState::Idle)
        );
        // Cost must not scale with tabs × catalog: compare 300 tabs with 1 tab
        // whose summaries sit at the catalog's end, so each lookup scans it all.
        let mut threads = (0..50_000)
            .map(|i| summary(&format!("x{i}"), ThreadStatus::Idle))
            .collect::<Vec<_>>();
        threads.extend((0..300).map(|i| summary(&format!("t{i}"), ThreadStatus::Idle)));
        let mut one = window();
        open(&mut one, "t0");
        let mut full = window();
        for i in 0..300 {
            open(&mut full, &format!("t{i}"));
        }
        let fastest = |w: &WindowTabs| {
            (0..5)
                .map(|_| {
                    let start = std::time::Instant::now();
                    let rows = metadata(
                        w,
                        &RowContext {
                            threads: &threads,
                            ..context
                        },
                    );
                    assert!(rows.iter().all(|r| r.state == TabState::Idle));
                    start.elapsed()
                })
                .min()
                .unwrap()
        };
        let (one, full) = (fastest(&one), fastest(&full));
        assert!(
            full < one * 10,
            "300 tabs took {full:?}; 1 tab took {one:?}"
        );
    }

    #[test]
    fn last_tab_has_nine_hint_beyond_eight_and_drafts_use_no_invented_thread() {
        let rows = fixture();
        assert_eq!(rows[0].shortcut, Some(1));
        assert_eq!(rows[7].shortcut, Some(8));
        assert_eq!(rows[8].shortcut, None);
        assert_eq!(rows[9].shortcut, Some(9));
        let mut w = window();
        w.open(
            TabTarget::Draft {
                id: "draft".into(),
                checkout: checkout("/repo"),
            },
            Some(1),
        )
        .unwrap();
        let rows = metadata(
            &w,
            &RowContext {
                host: &host(),
                threads: &[],
                navigation: &navigation::Preferences::default(),
                machine: "Mac",
            },
        );
        assert_eq!(rows[0].title, "New thread");
        assert_eq!(rows[0].state, TabState::Idle);
    }
}
