//! Prepared text ranges and one-point strokes; no ancestry work in a frame.
use super::{Graph, Item};
use std::{collections::HashMap, ops::Range};

pub(super) fn ancestry_label(lane: &plait_client::worktree_graph::Lane) -> Option<&'static str> {
    if lane.no_new_commits {
        Some("No new commits")
    } else if lane.merged {
        Some("Merged")
    } else if lane.detached {
        Some("Fork unknown")
    } else {
        None
    }
}

pub(super) struct LabelStyle {
    pub title: Range<usize>,
    pub state: Range<usize>,
    pub code: Range<usize>,
}

pub(super) fn head_label(
    title: &str,
    state: &str,
    branch: &str,
    facts: &[String],
) -> (String, LabelStyle) {
    let mut text = String::new();
    let title = part(&mut text, title, "");
    let state = part(&mut text, state, "  ");
    let code = part(&mut text, branch, " · ");
    for fact in facts {
        part(&mut text, fact, " · ");
    }
    (text, LabelStyle { title, state, code })
}

fn part(text: &mut String, value: &str, separator: &str) -> Range<usize> {
    if value.is_empty() {
        return text.len()..text.len();
    }
    if !text.is_empty() {
        text.push_str(separator);
    }
    let start = text.len();
    text.push_str(value);
    start..text.len()
}

pub(super) fn preservation_label(
    facts: &plait_client::git::worktree::inventory::Preservation,
) -> String {
    [
        facts.changed.map_or_else(
            || "Changed files unknown".into(),
            |n| format!("{n} changed"),
        ),
        facts.unpushed.map_or_else(
            || "Publication unknown".into(),
            |n| format!("{n} not pushed"),
        ),
        facts.exclusive.map_or_else(
            || "Commits kept only here: Unknown".into(),
            |n| format!("{n} kept only here"),
        ),
    ]
    .join(" · ")
}

pub(super) struct Stroke {
    pub lane: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub corner: Option<Corner>,
}

#[derive(Clone, Copy)]
pub(super) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

fn x(lane: usize) -> f32 {
    12. + lane as f32 * 14.
}

pub(super) fn prepare(items: &[Item], graph: &Graph) -> Vec<Vec<Stroke>> {
    let mut heads = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if let Item::Changed(row, lane) | Item::Head(row, lane) = item {
            heads.entry((*row, *lane)).or_insert(i);
        }
    }
    items
        .iter()
        .enumerate()
        .map(|(i, item)| strokes(i, item, graph, &heads))
        .collect()
}

fn strokes(
    item_index: usize,
    item: &Item,
    graph: &Graph,
    heads: &HashMap<(usize, usize), usize>,
) -> Vec<Stroke> {
    let index = match item {
        Item::Head(i, _) | Item::Changed(i, _) | Item::Commit(i) => *i,
        Item::Detached(_) => return Vec::new(),
    };
    let row = &graph.rows[index];
    let joins = if matches!(item, Item::Commit(_)) {
        row.joins.as_slice()
    } else {
        &[]
    };
    let mut strokes = Vec::new();
    let mut lanes = row.lines.clone();
    for &lane in &row.heads {
        if !lanes.contains(&lane) {
            lanes.push(lane);
        }
    }
    let commit = matches!(item, Item::Commit(_));
    for lane in lanes {
        let incoming = row.incoming.binary_search(&lane).is_ok();
        let outgoing = row.outgoing.binary_search(&lane).is_ok();
        let head = heads.get(&(index, lane)).copied();
        if !commit && !incoming && head.is_none_or(|h| item_index < h) {
            continue;
        }
        let mut top: f32 = if incoming || head.is_some_and(|h| h < item_index) {
            0.
        } else {
            14.
        };
        let mut bottom: f32 = if !commit || outgoing { 28. } else { 14. };
        for &(from, to) in joins {
            if row.lane == from && lane == to && !incoming && head.is_none() {
                top = 20.;
            } else if row.lane != from && lane == from && !outgoing {
                bottom = 8.;
            }
        }
        if bottom > top {
            strokes.push(Stroke {
                lane,
                x: x(lane),
                y: top,
                width: 1.,
                height: bottom - top,
                corner: None,
            });
        }
    }
    for &(from, to) in joins {
        let merge = row.lane == from;
        let lane = if merge { to } else { from };
        let right = to > from;
        let left = x(from.min(to));
        let width = x(from.max(to)) - left;
        let corner = match (merge, right) {
            (true, true) => Corner::TopRight,
            (true, false) => Corner::TopLeft,
            (false, true) => Corner::BottomLeft,
            (false, false) => Corner::BottomRight,
        };
        let corner_left = matches!(corner, Corner::TopLeft | Corner::BottomLeft);
        strokes.push(Stroke {
            lane,
            x: left + if corner_left { 6. } else { 0. },
            y: 14.,
            width: width - 6.,
            height: 1.,
            corner: None,
        });
        strokes.push(Stroke {
            lane,
            x: if corner_left { left } else { left + width - 6. },
            y: if merge { 14. } else { 8. },
            width: 7.,
            height: 7.,
            corner: Some(corner),
        });
    }
    strokes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_row(
        lane: usize,
        heads: &[usize],
        lines: &[usize],
        incoming: &[usize],
        outgoing: &[usize],
        joins: &[(usize, usize)],
    ) -> plait_client::worktree_graph::Row {
        plait_client::worktree_graph::Row {
            commit: plait_client::git::log::GraphCommit {
                commit: plait_client::git::log::Commit {
                    id: "a".into(),
                    short: "a".into(),
                    author: String::new(),
                    subject: String::new(),
                    time: 0,
                },
                parents: vec![],
            },
            lane,
            heads: heads.to_vec(),
            lines: lines.to_vec(),
            incoming: incoming.to_vec(),
            outgoing: outgoing.to_vec(),
            joins: joins.to_vec(),
        }
    }
    fn vertical(strokes: &[Stroke], lane: usize) -> Option<(f32, f32)> {
        strokes
            .iter()
            .find(|s| s.lane == lane && s.corner.is_none() && s.width == 1.)
            .map(|s| (s.y, s.height))
    }
    #[test]
    fn lines_begin_at_the_first_head_and_end_at_the_last_node() {
        let graph = Graph {
            rows: vec![
                graph_row(0, &[0], &[0], &[], &[0], &[]),
                graph_row(0, &[], &[0], &[0], &[], &[]),
            ],
            ..Default::default()
        };
        let items = [
            Item::Changed(0, 0),
            Item::Head(0, 0),
            Item::Commit(0),
            Item::Commit(1),
        ];
        let drawn = prepare(&items, &graph);
        assert_eq!(vertical(&drawn[0], 0), Some((14., 14.)));
        assert_eq!(vertical(&drawn[1], 0), Some((0., 28.)));
        assert_eq!(vertical(&drawn[2], 0), Some((0., 28.)));
        assert_eq!(vertical(&drawn[3], 0), Some((0., 14.)));
    }
    #[test]
    fn shared_head_stubs_start_at_their_labels_and_stop_at_the_fork() {
        let graph = Graph {
            rows: vec![graph_row(0, &[1, 0], &[1], &[], &[], &[(1, 0)])],
            ..Default::default()
        };
        let drawn = prepare(
            &[Item::Head(0, 1), Item::Head(0, 0), Item::Commit(0)],
            &graph,
        );
        assert_eq!(vertical(&drawn[0], 1), Some((14., 14.)));
        assert_eq!(vertical(&drawn[0], 0), None);
        assert_eq!(vertical(&drawn[1], 0), Some((14., 14.)));
        assert_eq!(vertical(&drawn[2], 0), Some((0., 14.)));
        assert_eq!(vertical(&drawn[2], 1), Some((0., 8.)));
    }
    #[test]
    fn routed_edges_start_at_the_merge_and_end_at_the_join() {
        let graph = Graph {
            rows: vec![
                graph_row(0, &[0], &[2], &[], &[2], &[(0, 2)]),
                graph_row(1, &[], &[2], &[2], &[], &[(2, 1)]),
            ],
            ..Default::default()
        };
        let drawn = prepare(
            &[Item::Head(0, 0), Item::Commit(0), Item::Commit(1)],
            &graph,
        );
        assert_eq!(vertical(&drawn[0], 2), None);
        assert_eq!(vertical(&drawn[1], 2), Some((20., 8.)));
        assert_eq!(vertical(&drawn[2], 2), Some((0., 8.)));
    }
    #[test]
    fn unknown_publication_is_never_presented_as_a_commit_count() {
        let mut facts = plait_client::git::worktree::inventory::Preservation {
            changed: Some(0),
            exclusive: Some(0),
            ..Default::default()
        };
        assert_eq!(
            preservation_label(&facts),
            "0 changed · Publication unknown · 0 kept only here"
        );
        facts.unpushed = Some(2);
        assert_eq!(
            preservation_label(&facts),
            "0 changed · 2 not pushed · 0 kept only here"
        );
    }

    #[test]
    fn a_reset_or_fresh_lane_says_no_new_commits_instead_of_merged() {
        let mut lane = plait_client::worktree_graph::Lane {
            head: Some(2),
            base: Some(1),
            parent: Some(0),
            merged: false,
            detached: false,
            no_new_commits: true,
        };
        for head in [Some(1), Some(2)] {
            lane.head = head;
            let label = ancestry_label(&lane).unwrap();
            assert_eq!(label, "No new commits");
            assert_eq!(
                head_label("Thread", "Ready", "topic", &[label.into()]).0,
                "Thread  Ready · topic · No new commits"
            );
        }
        lane.no_new_commits = false;
        lane.merged = true;
        assert_eq!(ancestry_label(&lane), Some("Merged"));
        lane.merged = false;
        lane.detached = true;
        assert_eq!(ancestry_label(&lane), Some("Fork unknown"));
    }

    #[test]
    fn head_labels_omit_missing_parts_without_empty_slots() {
        for (title, state, branch, facts, expected) in [
            (
                "Main checkout",
                "",
                "main",
                vec!["Clean"],
                "Main checkout · main · Clean",
            ),
            (
                "External worktree",
                "",
                "topic",
                vec!["3 changed"],
                "External worktree · topic · 3 changed",
            ),
            (
                "Thread unavailable",
                "",
                "topic",
                vec![],
                "Thread unavailable · topic",
            ),
            (
                "Review files",
                "Ready",
                "Detached",
                vec![],
                "Review files  Ready · Detached",
            ),
            (
                "External worktree",
                "",
                "Detached",
                vec![],
                "External worktree · Detached",
            ),
            (
                "Réparer",
                "Working",
                "sujet-é",
                vec!["", "2 not pushed"],
                "Réparer  Working · sujet-é · 2 not pushed",
            ),
            ("Thread unavailable", "", "", vec![], "Thread unavailable"),
        ] {
            let facts: Vec<_> = facts.into_iter().map(str::to_owned).collect();
            let (label, style) = head_label(title, state, branch, &facts);
            assert_eq!(label, expected);
            assert_eq!(&label[style.title], title);
            assert_eq!(&label[style.state], state);
            assert_eq!(&label[style.code], branch);
        }
    }
}
