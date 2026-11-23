//! Pure lane assignment over a bounded topological history. Missing ancestry is unknown.
use crate::{git::log::GraphCommit, host::worktree::Checkout};
use std::collections::{HashMap, HashSet};
#[derive(Clone, Debug)]
pub struct Lane {
    pub head: Option<usize>,
    pub base: Option<usize>,
    pub parent: Option<usize>,
    pub merged: bool,
    pub no_new_commits: bool,
    pub detached: bool,
}
#[derive(Clone, Debug)]
pub struct Row {
    pub commit: GraphCommit,
    pub lane: usize,
    pub heads: Vec<usize>,
    /// Each visible line is precomputed; render performs no ancestry search.
    pub lines: Vec<usize>,
    /// Sorted columns whose edges cross the row's upper or lower boundary.
    pub incoming: Vec<usize>,
    pub outgoing: Vec<usize>,
    pub joins: Vec<(usize, usize)>,
}
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub lanes: Vec<Lane>,
    pub rows: Vec<Row>,
    pub more: bool,
    pub columns: usize,
    /// Each drawn edge as (child row, parent row, column of its vertical).
    #[cfg(test)]
    pub(crate) edges: Vec<(usize, usize, usize)>,
}
fn ancestors(
    tip: Option<&str>,
    index: &HashMap<&str, usize>,
    commits: &[GraphCommit],
) -> HashSet<usize> {
    let mut found = HashSet::new();
    let mut pending: Vec<_> = tip.into_iter().collect();
    while let Some(id) = pending.pop() {
        if let Some(&i) = index.get(id)
            && found.insert(i)
        {
            pending.extend(commits[i].parents.iter().map(String::as_str));
        }
    }
    found
}
pub fn layout(checkouts: &[Checkout], commits: Vec<GraphCommit>, more: bool) -> Graph {
    let index: HashMap<_, _> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.commit.id.as_str(), i))
        .collect();
    let reach: Vec<_> = checkouts
        .iter()
        .map(|c| ancestors(c.head.as_deref(), &index, &commits))
        .collect();
    let before: Vec<_> = checkouts
        .iter()
        .map(|c| ancestors(c.base.as_deref(), &index, &commits))
        .collect();
    let mut lanes = Vec::new();
    for (n, checkout) in checkouts.iter().enumerate() {
        let head = checkout.head.as_deref().and_then(|s| index.get(s).copied());
        let base = checkout.base.as_deref().and_then(|s| index.get(s).copied());
        let parent = base.and_then(|b| {
            reach
                .iter()
                .enumerate()
                .find(|(i, seen)| *i != n && seen.contains(&b))
                .map(|(i, _)| i)
        });
        let no_new_commits = n > 0 && head.is_some_and(|h| before[n].contains(&h));
        lanes.push(Lane {
            head,
            base,
            parent,
            merged: n > 0
                && !no_new_commits
                && base.is_some_and(|b| reach[n].contains(&b))
                && head.is_some_and(|h| reach[0].contains(&h)),
            no_new_commits,
            detached: n > 0
                && (base.is_none()
                    || parent.is_none()
                    || (!no_new_commits && !base.is_some_and(|b| reach[n].contains(&b)))),
        });
    }
    // Keep one trunk commit below the oldest known fork; a cap is labelled separately.
    let oldest_base = lanes.iter().filter_map(|lane| lane.base).max();
    let end = oldest_base
        .and_then(|base| {
            ((base + 1)..commits.len()).find(|i| reach.first().is_some_and(|r| r.contains(i)))
        })
        .map_or(commits.len(), |i| i + 1);
    let end = if lanes.iter().any(|l| l.detached) {
        commits.len()
    } else {
        end.max(
            lanes
                .iter()
                .filter_map(|l| l.head)
                .max()
                .map_or(0, |h| h + 1),
        )
    };
    // The trunk follows first parents; merged side commits retain their lane.
    let mut trunk = HashSet::new();
    let mut next = checkouts.first().and_then(|c| c.head.as_deref());
    while let Some(id) = next {
        let Some(&i) = index.get(id) else { break };
        if !trunk.insert(i) {
            break;
        }
        next = commits[i].parents.first().map(String::as_str);
    }
    // A lane holds only its head's first-parent chain down to its base. Commits
    // it merged in keep lanes of their own; otherwise its line would run
    // through them as if they were its parents.
    let segments: Vec<_> = checkouts
        .iter()
        .enumerate()
        .map(|(n, _)| {
            let mut chain = HashSet::new();
            let mut next = lanes[n].head;
            while let Some(i) = next.filter(|i| !before[n].contains(i)) {
                if !chain.insert(i) {
                    break;
                }
                next = commits[i]
                    .parents
                    .first()
                    .and_then(|p| index.get(p.as_str()).copied());
            }
            chain
        })
        .collect();
    let mut columns = checkouts.len().max(1);
    let mut node_lanes = vec![0; end];
    let mut inherited = HashMap::new();
    for i in 0..end {
        let lane = if trunk.contains(&i) {
            0
        } else if let Some(lane) = segments
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(n, r)| r.contains(&i).then_some(n))
        {
            lane
        } else {
            *inherited.entry(i).or_insert_with(|| {
                let lane = columns;
                columns += 1;
                lane
            })
        };
        node_lanes[i] = lane;
        for parent in commits[i].parents.iter().take(1) {
            if let Some(&p) = index.get(parent.as_str()) {
                inherited.entry(p).or_insert(lane);
            }
        }
    }
    let mut lines = vec![HashSet::new(); end];
    let mut incoming = vec![HashSet::new(); end];
    let mut outgoing = vec![HashSet::new(); end];
    let mut joins = vec![Vec::new(); end];
    #[cfg(test)]
    let mut edges = Vec::new();
    // Columns beyond every node's lane, each free again after its last row.
    let spare = columns;
    let mut spare_until: Vec<usize> = Vec::new();
    for (i, commit) in commits.iter().take(end).enumerate() {
        let from = node_lanes[i];
        for (p, parent) in commit.parents.iter().enumerate() {
            let Some(&to_row) = index.get(parent.as_str()).filter(|&&r| r < end && r > i) else {
                continue;
            };
            let to = node_lanes[to_row];
            if from != to && lanes.get(from).is_some_and(|l| l.detached) {
                continue;
            }
            let mut line = if p == 0 { from } else { to };
            // A line over another commit's node would read as that commit
            // being the parent. Route it through a column with no nodes.
            if (i + 1..to_row).any(|k| node_lanes[k] == line) {
                let slot = spare_until
                    .iter()
                    .position(|&last| last < i)
                    .unwrap_or_else(|| {
                        spare_until.push(0);
                        spare_until.len() - 1
                    });
                spare_until[slot] = to_row;
                line = spare + slot;
                columns = columns.max(line + 1);
                joins[i].push((from, line));
                joins[to_row].push((line, to));
            } else if from != to {
                joins[if p == 0 { to_row } else { i }].push((from, to));
            }
            for r in i..=to_row {
                lines[r].insert(line);
                if r > i {
                    incoming[r].insert(line);
                }
                if r < to_row {
                    outgoing[r].insert(line);
                }
            }
            #[cfg(test)]
            edges.push((i, to_row, line));
        }
    }
    // Two checkouts can point at the same commit. Each label keeps its own lane.
    for (n, lane) in lanes.iter().enumerate().skip(1) {
        if let Some(head) = lane.head.filter(|h| *h < end)
            && node_lanes[head] != n
        {
            lines[head].insert(n);
            if !lane.detached {
                joins[head].push((n, node_lanes[head]));
            }
        }
    }
    let rows = commits
        .into_iter()
        .take(end)
        .enumerate()
        .map(|(i, commit)| Row {
            commit,
            lane: node_lanes[i],
            heads: lanes
                .iter()
                .enumerate()
                .filter_map(|(n, l)| (l.head == Some(i)).then_some(n))
                .collect(),
            lines: ordered(&lines[i]),
            incoming: ordered(&incoming[i]),
            outgoing: ordered(&outgoing[i]),
            joins: std::mem::take(&mut joins[i]),
        })
        .collect();
    Graph {
        lanes,
        rows,
        more,
        columns,
        #[cfg(test)]
        edges,
    }
}

fn ordered(values: &HashSet<usize>) -> Vec<usize> {
    let mut values: Vec<_> = values.iter().copied().collect();
    values.sort_unstable();
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{log::Commit, worktree::inventory::Preservation};
    fn commit(n: usize, parents: &[usize]) -> GraphCommit {
        GraphCommit {
            commit: Commit {
                id: n.to_string(),
                short: n.to_string(),
                author: "".into(),
                time: 0,
                subject: "".into(),
            },
            parents: parents.iter().map(usize::to_string).collect(),
        }
    }
    fn checkout(head: usize, base: Option<usize>) -> Checkout {
        Checkout {
            build_directories: Vec::new(),
            build_output: None,
            path: format!("/repo/{head}").into(),
            main: base.is_none(),
            branch: None,
            head: Some(head.to_string()),
            base: base.map(|n| n.to_string()),
            thread_id: None,
            identity: None,
            missing: false,
            removed: false,
            preservation: Preservation::default(),
            creation_id: None,
            last_used: None,
        }
    }
    /// Every parent link within the drawn rows is an edge, and no edge's line
    /// passes over another commit's node in its column.
    fn assert_edges_are_parents(graph: &Graph, case: &str) {
        let index: HashMap<_, _> = graph
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.commit.commit.id.as_str(), i))
            .collect();
        for &(i, j, line) in &graph.edges {
            let parent = graph.rows[j].commit.commit.id.as_str();
            assert!(
                graph.rows[i].commit.parents.iter().any(|p| p == parent),
                "{case}: {i}->{j}"
            );
            for k in i + 1..j {
                assert_ne!(
                    graph.rows[k].lane, line,
                    "{case}: edge {i}->{j} passes over {k}"
                );
            }
            assert!(
                (i..=j).all(|r| graph.rows[r].lines.contains(&line)),
                "{case}"
            );
        }
        for (i, row) in graph.rows.iter().enumerate() {
            for &lane in &row.lines {
                assert_eq!(
                    row.incoming.contains(&lane),
                    graph
                        .edges
                        .iter()
                        .any(|&(a, b, l)| l == lane && a < i && b >= i),
                    "{case}: upper endpoint at {i}"
                );
                assert_eq!(
                    row.outgoing.contains(&lane),
                    graph
                        .edges
                        .iter()
                        .any(|&(a, b, l)| l == lane && a <= i && b > i),
                    "{case}: lower endpoint at {i}"
                );
            }
            for parent in &row.commit.parents {
                let Some(&j) = index.get(parent.as_str()) else {
                    continue;
                };
                let crosses = graph.rows[j].lane != row.lane;
                let detached = graph.lanes.get(row.lane).is_some_and(|l| l.detached);
                if !(crosses && detached) {
                    assert!(
                        graph.edges.iter().any(|e| (e.0, e.1) == (i, j)),
                        "{case}: {i}->{j} missing"
                    );
                }
            }
        }
    }
    #[test]
    fn merged_side_branches_and_crossing_merges_never_run_through_other_commits() {
        // A worktree that merged its own side branch, and an octopus merge.
        for side in [vec![5], vec![5, 4]] {
            let mut parents = vec![6];
            parents.extend(&side);
            let mut commits = vec![commit(9, &[1]), commit(7, &parents), commit(6, &[1])];
            commits.extend(side.iter().map(|&n| commit(n, &[1])));
            commits.push(commit(1, &[]));
            let graph = layout(&[checkout(9, None), checkout(7, Some(1))], commits, false);
            assert_edges_are_parents(&graph, "side merge");
            assert_eq!(graph.rows[1].lane, 1);
            assert_eq!(graph.rows[2].lane, 1);
            assert_ne!(graph.rows[3].lane, 1);
        }
        // The trunk merged an older commit of a worktree that has moved on.
        let graph = layout(
            &[checkout(9, None), checkout(8, Some(1))],
            vec![
                commit(9, &[2, 7]),
                commit(8, &[7]),
                commit(7, &[1]),
                commit(2, &[1]),
                commit(1, &[]),
            ],
            false,
        );
        assert_edges_are_parents(&graph, "crossing merge");
        assert!(graph.columns > 2);
    }
    #[test]
    fn random_histories_never_panic_or_draw_an_edge_to_a_non_parent() {
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        };
        for round in 0..2000 {
            let n = 2 + next(40);
            // Newest first: every parent is an older, later row.
            let commits = (0..n)
                .rev()
                .map(|k| {
                    let mut parents = Vec::new();
                    if k > 0 {
                        parents.push(next(k));
                        let other = next(k);
                        if next(4) == 0 && !parents.contains(&other) {
                            parents.push(other);
                        }
                    }
                    commit(k, &parents)
                })
                .collect();
            let mut rows = vec![checkout(n - 1, None)];
            for _ in 0..next(7) {
                let mut row = checkout(next(n), Some(next(n)));
                row.path = format!("/repo/{round}-{}", rows.len()).into();
                rows.push(row);
            }
            let graph = layout(&rows, commits, next(4) == 0);
            assert_edges_are_parents(&graph, &format!("round {round}"));
        }
    }
    #[test]
    fn merge_parents_and_reordered_nested_lanes_never_draw_a_false_trunk() {
        let graph = layout(
            &[checkout(5, None), checkout(3, Some(1))],
            vec![
                commit(5, &[4, 3]),
                commit(4, &[1]),
                commit(3, &[2]),
                commit(2, &[1]),
                commit(1, &[]),
            ],
            false,
        );
        assert!(graph.lanes[1].merged);
        assert_eq!(graph.rows[2].lane, 1);
        assert_eq!(graph.rows[3].lane, 1);
        assert!(graph.rows[0].joins.contains(&(0, 1)));
        let nested = layout(
            &[
                checkout(5, None),
                checkout(4, Some(3)),
                checkout(3, Some(1)),
            ],
            vec![
                commit(5, &[1]),
                commit(4, &[3]),
                commit(3, &[2]),
                commit(2, &[1]),
                commit(1, &[]),
            ],
            false,
        );
        assert_eq!(nested.lanes[1].parent, Some(2));
        assert_eq!(nested.rows[2].lane, 2);
        assert_eq!(nested.rows[3].lane, 2);
        assert!(nested.rows[2].joins.contains(&(1, 2)));
        let historic = layout(
            &[checkout(5, None)],
            vec![
                commit(5, &[4, 3]),
                commit(4, &[1]),
                commit(3, &[2]),
                commit(2, &[1]),
                commit(1, &[]),
            ],
            false,
        );
        assert_ne!(historic.rows[2].lane, 0);
        assert_eq!(historic.rows[2].lane, historic.rows[3].lane);
    }
    #[test]
    fn linear_trunk_and_one_three_twenty_lanes_keep_real_parentage() {
        for count in [0, 1, 3, 20] {
            let mut rows = vec![checkout(3, None)];
            let mut commits = Vec::new();
            for n in 0..count {
                rows.push(checkout(10 + n, Some(2)));
                commits.push(commit(10 + n, &[2]));
            }
            commits.extend([commit(3, &[2]), commit(2, &[1]), commit(1, &[])]);
            let graph = layout(&rows, commits, false);
            assert_eq!(graph.lanes.len(), count + 1);
            for lane in &graph.lanes[1..] {
                assert_eq!(lane.parent, Some(0));
                assert!(!lane.detached);
                assert!(!lane.merged);
            }
            assert_eq!(
                graph.rows.iter().filter(|r| !r.joins.is_empty()).count(),
                usize::from(count > 0)
            );
        }
    }
    #[test]
    fn fork_from_worktree_empty_lane_merged_and_lost_base_are_distinct() {
        let rows = [
            checkout(5, None),
            checkout(3, Some(2)),
            checkout(4, Some(3)),
            checkout(2, Some(2)),
            checkout(6, Some(99)),
        ];
        let graph = layout(
            &rows,
            vec![
                commit(6, &[1]),
                commit(5, &[2]),
                commit(4, &[3]),
                commit(3, &[2]),
                commit(2, &[1]),
                commit(1, &[]),
            ],
            false,
        );
        assert_eq!(graph.lanes[2].parent, Some(1));
        assert!(!graph.lanes[2].detached);
        assert!(!graph.lanes[3].merged);
        assert_eq!(graph.lanes[3].head, graph.lanes[3].base);
        assert!(graph.lanes[4].detached);
        assert_eq!(graph.lanes[4].base, None);
        let rebased = layout(
            &[checkout(5, None), checkout(6, Some(3))],
            vec![
                commit(6, &[2]),
                commit(5, &[3]),
                commit(3, &[2]),
                commit(2, &[]),
            ],
            false,
        );
        assert!(rebased.lanes[1].detached);
    }
    #[test]
    fn head_on_trunk_is_merged_and_capped_ancestry_stays_unknown() {
        let merged = layout(
            &[checkout(5, None), checkout(3, Some(2))],
            vec![
                commit(5, &[4]),
                commit(4, &[3]),
                commit(3, &[2]),
                commit(2, &[]),
            ],
            false,
        );
        assert!(merged.lanes[1].merged);
        let limited = layout(
            &[checkout(5, None), checkout(4, Some(1))],
            vec![commit(5, &[3]), commit(4, &[2])],
            true,
        );
        assert!(limited.more);
        assert!(limited.lanes[1].detached);
        assert!(!limited.lanes[1].merged);
    }

    #[test]
    fn a_head_at_or_before_its_base_has_no_new_commits() {
        for head in [2, 3] {
            let graph = layout(
                &[checkout(5, None), checkout(head, Some(3))],
                vec![
                    commit(5, &[4]),
                    commit(4, &[3]),
                    commit(3, &[2]),
                    commit(2, &[]),
                ],
                false,
            );
            assert!(graph.lanes[1].no_new_commits);
            assert!(!graph.lanes[1].merged);
            assert!(!graph.lanes[1].detached);
        }
        let unknown = layout(
            &[checkout(5, None), checkout(3, Some(99))],
            vec![commit(5, &[3]), commit(3, &[])],
            true,
        );
        assert!(!unknown.lanes[1].no_new_commits);
        assert!(!unknown.lanes[1].merged);
        assert!(unknown.lanes[1].detached);
    }
}
