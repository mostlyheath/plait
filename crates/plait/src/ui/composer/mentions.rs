//! Ranks checkout files against a mention query. Pure and potentially slow on
//! a large checkout: the composer runs it on the background executor.

use plait_client::fuzzy::fuzzy_match;
use std::cmp::Reverse;

/// A file that matches the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RankedFile {
    /// Index into the path list that was ranked.
    pub index: usize,
    /// Byte offsets of the matched characters in the path.
    pub positions: Vec<usize>,
}

/// Best first: score, then the shorter path, then list order.
type Key = (i32, Reverse<usize>, Reverse<usize>);

/// The best `limit` matches for `query`, best first. An empty query lists the
/// shortest paths, which are the files nearest the checkout root.
pub(super) fn rank(paths: &[String], query: &str, limit: usize) -> Vec<RankedFile> {
    let mut best: Vec<(Key, Vec<usize>)> = Vec::new();
    let mut worst_kept: Option<Key> = None;
    for (index, path) in paths.iter().enumerate() {
        let Some(found) = fuzzy_match(query, path) else {
            continue;
        };
        let key = (found.score, Reverse(path.len()), Reverse(index));
        if worst_kept.is_some_and(|worst| key < worst) {
            continue;
        }
        best.push((key, found.positions));
        // Trim in batches so a broad query does not sort the whole checkout.
        if best.len() >= limit.max(1) * 2 {
            worst_kept = keep_best(&mut best, limit);
        }
    }
    keep_best(&mut best, limit);
    best.into_iter()
        .map(|((_, _, Reverse(index)), positions)| RankedFile { index, positions })
        .collect()
}

fn keep_best(best: &mut Vec<(Key, Vec<usize>)>, limit: usize) -> Option<Key> {
    best.sort_unstable_by_key(|entry| Reverse(entry.0));
    best.truncate(limit);
    best.last()
        .filter(|_| best.len() == limit)
        .map(|entry| entry.0)
}

/// Splits a path for display: the file name, then its directory.
pub(super) fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(slash) => (&path[slash + 1..], &path[..slash]),
        None => (path, ""),
    }
}

/// Divides matched path offsets between the file name and the directory, each
/// relative to its own text.
pub(super) fn split_positions(path: &str, positions: &[usize]) -> (Vec<usize>, Vec<usize>) {
    let name_start = path.rfind('/').map_or(0, |slash| slash + 1);
    let name = positions
        .iter()
        .filter(|offset| **offset >= name_start)
        .map(|offset| offset - name_start)
        .collect();
    let directory = positions
        .iter()
        .filter(|offset| **offset + 1 < name_start)
        .copied()
        .collect();
    (name, directory)
}

/// Guards an asynchronous result: only the newest request may apply.
#[derive(Default)]
pub(super) struct Latest(u64);

impl Latest {
    /// Start a request, superseding every earlier one.
    pub fn begin(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }

    /// Drop interest in any request still running.
    pub fn cancel(&mut self) {
        self.0 += 1;
    }

    pub fn accepts(&self, ticket: u64) -> bool {
        ticket == self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Vec<String> {
        [
            "Cargo.toml",
            "README.md",
            "crates/plait-client/src/git.rs",
            "crates/plait-client/src/git/tests.rs",
            "crates/plait/src/ui/composer/mod.rs",
            "crates/plait/src/ui/mod.rs",
            "docs/GIT.md",
        ]
        .map(String::from)
        .to_vec()
    }

    fn ranked(query: &str, limit: usize) -> Vec<String> {
        let paths = paths();
        rank(&paths, query, limit)
            .into_iter()
            .map(|file| paths[file.index].clone())
            .collect()
    }

    #[test]
    fn empty_query_lists_the_shortest_paths_in_list_order() {
        assert_eq!(ranked("", 3), ["README.md", "Cargo.toml", "docs/GIT.md"]);
        assert_eq!(ranked("", 99).len(), 7);
        assert!(ranked("", 0).is_empty());
    }

    #[test]
    fn file_name_matches_come_first_and_ties_prefer_the_shorter_path() {
        let found = ranked("git", 9);
        assert_eq!(found[0], "docs/GIT.md");
        assert_eq!(found[1], "crates/plait-client/src/git.rs");
        assert!(found.contains(&"crates/plait-client/src/git/tests.rs".to_owned()));
        assert!(!found.contains(&"README.md".to_owned()));
        assert_eq!(ranked("mod", 1), ["crates/plait/src/ui/mod.rs"]);
        assert!(ranked("zzz", 9).is_empty());
    }

    #[test]
    fn positions_point_at_the_matched_characters() {
        let paths = paths();
        let found = &rank(&paths, "uimod", 1)[0];
        assert_eq!(paths[found.index], "crates/plait/src/ui/mod.rs");
        let matched: String = found
            .positions
            .iter()
            .map(|offset| paths[found.index].as_bytes()[*offset] as char)
            .collect();
        assert_eq!(matched, "uimod");
    }

    #[test]
    fn batch_trimming_keeps_the_same_results_as_a_full_sort() {
        let paths: Vec<String> = (0..500)
            .map(|n| format!("dir{}/file_{:03}.rs", n % 7, (n * 37) % 500))
            .collect();
        for (query, limit) in [("", 5), ("f1", 5), ("d3file", 3), ("rs", 1)] {
            let mut all = rank(&paths, query, usize::MAX / 4);
            all.truncate(limit);
            assert_eq!(rank(&paths, query, limit), all, "{query}");
        }
    }

    #[test]
    fn paths_split_into_name_and_directory_with_their_own_offsets() {
        assert_eq!(split("a/b/c.rs"), ("c.rs", "a/b"));
        assert_eq!(split("top.rs"), ("top.rs", ""));
        assert_eq!(
            split_positions("a/b/c.rs", &[0, 2, 3, 4, 7]),
            (vec![0, 3], vec![0, 2])
        );
        assert_eq!(split_positions("top.rs", &[0, 5]), (vec![0, 5], vec![]));
    }

    #[test]
    fn only_the_newest_request_is_accepted() {
        let mut latest = Latest::default();
        let first = latest.begin();
        assert!(latest.accepts(first));
        let second = latest.begin();
        assert!(!latest.accepts(first));
        assert!(latest.accepts(second));
        latest.cancel();
        assert!(!latest.accepts(second));
    }
}
