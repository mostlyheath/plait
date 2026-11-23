//! Line differences between the two sides of an edit, for display only.
//!
//! Inputs are bounded tool arguments, so a quadratic table is acceptable once
//! the shared prefix and suffix are set aside. Run it off the render thread.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Context,
    Removed,
    Added,
}

/// Above this many table cells the middle is shown as one replacement.
const CELL_LIMIT: usize = 400_000;

/// Every line of `old` and `new` in reading order: unchanged lines once,
/// and each changed stretch as its removals followed by its additions.
pub fn lines<'a>(old: &'a str, new: &'a str) -> Vec<(Change, &'a str)> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old_middle, new_middle) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );
    let mut out = Vec::with_capacity(old.len() + new_middle.len());
    out.extend(old[..prefix].iter().map(|line| (Change::Context, *line)));
    middle(old_middle, new_middle, &mut out);
    out.extend(
        old[old.len() - suffix..]
            .iter()
            .map(|line| (Change::Context, *line)),
    );
    out
}

/// How many lines an edit adds and removes.
pub fn counts(lines: &[(Change, &str)]) -> (usize, usize) {
    let count = |change| lines.iter().filter(|(kind, _)| *kind == change).count();
    (count(Change::Added), count(Change::Removed))
}

fn middle<'a>(old: &[&'a str], new: &[&'a str], out: &mut Vec<(Change, &'a str)>) {
    let width = new.len() + 1;
    if old.is_empty() || new.is_empty() || old.len().saturating_mul(new.len()) > CELL_LIMIT {
        out.extend(old.iter().map(|line| (Change::Removed, *line)));
        out.extend(new.iter().map(|line| (Change::Added, *line)));
        return;
    }
    // common[i][j] is the longest common subsequence of old[i..] and new[j..].
    let mut common = vec![0u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i * width + j] = if old[i] == new[j] {
                common[(i + 1) * width + j + 1] + 1
            } else {
                common[(i + 1) * width + j].max(common[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut added = Vec::new();
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            out.append(&mut added);
            out.push((Change::Context, old[i]));
            (i, j) = (i + 1, j + 1);
        } else if j == new.len()
            || (i < old.len() && common[(i + 1) * width + j] >= common[i * width + j + 1])
        {
            out.push((Change::Removed, old[i]));
            i += 1;
        } else {
            // Held back so a changed stretch reads as removals, then additions.
            added.push((Change::Added, new[j]));
            j += 1;
        }
    }
    out.append(&mut added);
}

#[cfg(test)]
mod tests {
    use super::{Change::*, *};

    #[test]
    fn unchanged_text_is_all_context() {
        assert_eq!(lines("a\nb", "a\nb"), [(Context, "a"), (Context, "b")]);
        assert!(lines("", "").is_empty());
        assert_eq!(counts(&lines("a\nb", "a\nb")), (0, 0));
    }

    #[test]
    fn a_replaced_line_keeps_its_surroundings() {
        let diff = lines(
            "fn main() {\n    old();\n}\n",
            "fn main() {\n    new();\n    more();\n}\n",
        );
        assert_eq!(
            diff,
            [
                (Context, "fn main() {"),
                (Removed, "    old();"),
                (Added, "    new();"),
                (Added, "    more();"),
                (Context, "}"),
            ]
        );
        assert_eq!(counts(&diff), (2, 1));
    }

    #[test]
    fn separate_changes_keep_the_lines_between_them() {
        let diff = lines("a\nb\nc\nd\ne", "a\nB\nc\nD\ne");
        assert_eq!(
            diff,
            [
                (Context, "a"),
                (Removed, "b"),
                (Added, "B"),
                (Context, "c"),
                (Removed, "d"),
                (Added, "D"),
                (Context, "e"),
            ]
        );
    }

    #[test]
    fn pure_insertions_and_deletions_have_one_side() {
        assert_eq!(lines("", "a\nb"), [(Added, "a"), (Added, "b")]);
        assert_eq!(lines("a\nb", ""), [(Removed, "a"), (Removed, "b")]);
        assert_eq!(
            lines("a\nc", "a\nb\nc"),
            [(Context, "a"), (Added, "b"), (Context, "c")]
        );
        assert_eq!(
            lines("x\nx\nx", "x\nx"),
            [(Context, "x"), (Context, "x"), (Removed, "x")]
        );
    }

    #[test]
    fn oversized_middles_fall_back_to_one_replacement() {
        let old: String = (0..700).map(|line| format!("old {line}\n")).collect();
        let new: String = (0..700).map(|line| format!("new {line}\n")).collect();
        let diff = lines(&old, &new);
        assert_eq!(counts(&diff), (700, 700));
        assert!(diff[..700].iter().all(|(change, _)| *change == Removed));
    }

    #[test]
    fn every_line_of_both_sides_appears_once() {
        let mut seed = 11u64;
        let mut random = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as usize
        };
        for _ in 0..500 {
            let mut text = || {
                (0..random() % 9)
                    .map(|_| ["a", "b", "c", "é"][random() % 4])
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let (old, new) = (text(), text());
            let diff = lines(&old, &new);
            let side = |skip: Change| {
                diff.iter()
                    .filter(|(change, _)| *change != skip)
                    .map(|(_, line)| *line)
                    .collect::<Vec<_>>()
            };
            assert_eq!(side(Added), old.lines().collect::<Vec<_>>());
            assert_eq!(side(Removed), new.lines().collect::<Vec<_>>());
        }
    }
}
