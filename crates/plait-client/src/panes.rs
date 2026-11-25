//! Focus regions and displayed diff search for the review panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneKind {
    Thread,
    Changes,
    History,
    Files,
}
/// Matching displayed patch rows, in scroll order. Each match navigates to its row.
pub fn matching_diff_rows(diff: &crate::model::Diff, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    diff.rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            !crate::conversation_find::find(&diff.lines[row.line()].text, query, 1).is_empty()
        })
        .map(|(index, _)| index..index + 1)
        .take(crate::file_view::FIND_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_only_displayed_patch_rows() {
        let mut patch = crate::model::Diff::message("line to find", Default::default());
        assert_eq!(matching_diff_rows(&patch, "FIND"), vec![0..1]);
        patch.rows.clear();
        assert!(matching_diff_rows(&patch, "find").is_empty());
    }
}
