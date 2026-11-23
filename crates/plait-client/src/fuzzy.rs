//! Case-insensitive subsequence matching for pickers: files, commands, models.
//!
//! Pure and synchronous. Rank large candidate lists on a background executor.

/// A successful match. Higher scores are better; scores are comparable only
/// between candidates matched against the same query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    /// Byte offsets of the matched characters in the candidate, ascending.
    pub positions: Vec<usize>,
}

const MATCHED: i32 = 16;
/// Extra for a character that directly follows the previous matched one.
const CONSECUTIVE: i32 = 16;
/// First character of the candidate or of a path component.
const SEGMENT_START: i32 = 12;
/// First character of a word inside a component: after `_`, `-`, `.`, a space, or a case hump.
const WORD_START: i32 = 10;
/// First character of the file name, on top of `SEGMENT_START`.
const NAME_START: i32 = 8;
/// Every matched character inside the file name rather than its directory.
const IN_NAME: i32 = 4;
/// Each candidate character skipped between two matched ones.
const GAP: i32 = 1;
/// Larger tables fall back to the first subsequence; pickers never need them.
const TABLE_LIMIT: usize = 64 * 1024;
const NONE: i32 = i32::MIN / 2;

struct Glyph {
    offset: usize,
    folded: char,
    bonus: i32,
}

fn fold(ch: char) -> char {
    if ch.is_ascii() {
        ch.to_ascii_lowercase()
    } else {
        ch.to_lowercase().next().unwrap_or(ch)
    }
}

fn is_separator(ch: char) -> bool {
    matches!(ch, '/' | '\\')
}

/// Per-character bonuses, independent of the query.
fn glyphs(candidate: &str) -> Vec<Glyph> {
    let name_start = candidate.rfind(is_separator).map_or(0, |index| index + 1);
    let mut previous: Option<char> = None;
    candidate
        .char_indices()
        .map(|(offset, ch)| {
            let mut bonus = match previous {
                None => SEGMENT_START,
                Some(before) if is_separator(before) => SEGMENT_START,
                Some(before) if !before.is_alphanumeric() && ch.is_alphanumeric() => WORD_START,
                Some(before) if before.is_lowercase() && ch.is_uppercase() => WORD_START,
                Some(_) => 0,
            };
            if offset >= name_start {
                bonus += IN_NAME;
            }
            if offset == name_start {
                bonus += NAME_START;
            }
            previous = Some(ch);
            Glyph {
                offset,
                folded: fold(ch),
                bonus,
            }
        })
        .collect()
}

/// The leftmost subsequence, or `None` when the query does not occur in order.
fn first_subsequence(query: &[char], glyphs: &[Glyph]) -> Option<Vec<usize>> {
    let mut found = Vec::with_capacity(query.len());
    let mut next = 0;
    for wanted in query {
        let index = next + glyphs[next..].iter().position(|g| g.folded == *wanted)?;
        found.push(index);
        next = index + 1;
    }
    Some(found)
}

fn score_of(indices: &[usize], glyphs: &[Glyph]) -> i32 {
    let mut score = 0;
    let mut previous: Option<usize> = None;
    for &index in indices {
        score += MATCHED;
        score += match previous {
            Some(before) if before + 1 == index => CONSECUTIVE.max(glyphs[index].bonus),
            Some(before) => glyphs[index].bonus - GAP * (index - before - 1) as i32,
            None => glyphs[index].bonus,
        };
        previous = Some(index);
    }
    score
}

/// Best-scoring alignment by dynamic programming over query × candidate.
///
/// `ending[i][j]` is the best score with query character `i` matched at glyph
/// `j`; `within[i][j]` is the best score for the first `i + 1` query characters
/// inside the first `j + 1` glyphs, after paying for the gap since the match.
fn best_alignment(query: &[char], glyphs: &[Glyph]) -> Vec<usize> {
    let (rows, columns) = (query.len(), glyphs.len());
    let mut ending = vec![NONE; rows * columns];
    let mut within = vec![NONE; rows * columns];
    for (i, wanted) in query.iter().enumerate() {
        let mut carried = NONE;
        for (j, glyph) in glyphs.iter().enumerate() {
            let cell = i * columns + j;
            if glyph.folded == *wanted {
                ending[cell] = if i == 0 {
                    MATCHED + glyph.bonus
                } else if j == 0 {
                    NONE
                } else {
                    let before = cell - columns - 1;
                    let after_gap = within[before].saturating_add(MATCHED + glyph.bonus);
                    let adjacent =
                        ending[before].saturating_add(MATCHED + CONSECUTIVE.max(glyph.bonus));
                    after_gap.max(adjacent)
                };
            }
            carried = ending[cell].max(carried.saturating_sub(GAP));
            within[cell] = carried;
        }
    }
    trace_back(query, glyphs, &ending, &within)
}

fn trace_back(query: &[char], glyphs: &[Glyph], ending: &[i32], within: &[i32]) -> Vec<usize> {
    let (rows, columns) = (query.len(), glyphs.len());
    let last = &ending[(rows - 1) * columns..];
    // The earliest best end keeps equal alignments stable.
    let mut j = (0..columns).rev().max_by_key(|j| last[*j]).unwrap_or(0);
    let mut indices = vec![0; rows];
    for i in (0..rows).rev() {
        indices[i] = j;
        if i == 0 {
            break;
        }
        let before = (i - 1) * columns + j - 1;
        let adjacent = ending[before].saturating_add(MATCHED + CONSECUTIVE.max(glyphs[j].bonus));
        if ending[before] > NONE / 2 && adjacent >= ending[i * columns + j] {
            j -= 1;
            continue;
        }
        // Otherwise the previous character ended where the carried score began.
        j -= 1;
        while j > 0 && within[(i - 1) * columns + j] != ending[(i - 1) * columns + j] {
            j -= 1;
        }
    }
    indices
}

/// Match `query` against `candidate` as a case-insensitive subsequence.
///
/// An empty query matches everything with score 0. Consecutive runs, word
/// starts, path-component starts, and characters in the file name (the part
/// after the last `/`) score higher than scattered or directory matches.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<Match> {
    let query: Vec<char> = query.chars().map(fold).collect();
    if query.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    if query.len() > candidate.len() {
        return None;
    }
    let glyphs = glyphs(candidate);
    // Cheap rejection keeps the table off the common non-matching path.
    let first = first_subsequence(&query, &glyphs)?;
    let indices = if query.len().saturating_mul(glyphs.len()) <= TABLE_LIMIT {
        best_alignment(&query, &glyphs)
    } else {
        first
    };
    Some(Match {
        score: score_of(&indices, &glyphs),
        positions: indices.iter().map(|index| glyphs[*index].offset).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, candidate: &str) -> i32 {
        fuzzy_match(query, candidate)
            .unwrap_or_else(|| panic!("{query:?} should match {candidate:?}"))
            .score
    }

    fn matched(query: &str, candidate: &str) -> String {
        let found = fuzzy_match(query, candidate).unwrap();
        found
            .positions
            .iter()
            .map(|offset| candidate[*offset..].chars().next().unwrap())
            .collect()
    }

    #[test]
    fn empty_query_matches_everything_with_score_zero() {
        for candidate in ["", "src/main.rs", "Ünï"] {
            assert_eq!(
                fuzzy_match("", candidate),
                Some(Match {
                    score: 0,
                    positions: Vec::new()
                })
            );
        }
    }

    #[test]
    fn requires_every_query_character_in_order() {
        assert!(fuzzy_match("abc", "a-b-c").is_some());
        assert!(fuzzy_match("abc", "acb").is_none());
        assert!(fuzzy_match("abcd", "abc").is_none());
        assert!(fuzzy_match("a", "").is_none());
        assert!(fuzzy_match("aa", "a").is_none());
        assert!(fuzzy_match("x y", "x_y").is_none());
    }

    #[test]
    fn matching_ignores_case_in_both_directions() {
        assert_eq!(
            fuzzy_match("readme", "README.md"),
            fuzzy_match("README", "README.md")
        );
        assert_eq!(matched("RM", "readme.md"), "rm");
        assert_eq!(matched("é", "CAFÉ"), "É");
    }

    #[test]
    fn positions_are_ascending_byte_offsets_of_the_matched_characters() {
        let candidate = "docs/日本語/ünï.md";
        let found = fuzzy_match("ünmd", candidate).unwrap();
        assert!(found.positions.windows(2).all(|pair| pair[0] < pair[1]));
        for offset in &found.positions {
            assert!(candidate.is_char_boundary(*offset));
        }
        assert_eq!(matched("ünmd", candidate), "ünmd");
        assert_eq!(
            fuzzy_match("main", "main.rs").unwrap().positions,
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn consecutive_runs_beat_scattered_characters() {
        assert!(score("comp", "composer.rs") > score("comp", "c_o_m_p.rs"));
        assert!(score("stat", "state.rs") > score("stat", "set_attributes.rs"));
        // The run is found even when an earlier scattered subsequence exists.
        assert_eq!(
            fuzzy_match("mod", "m_o_d/mod.rs").unwrap().positions,
            [6, 7, 8]
        );
    }

    #[test]
    fn word_starts_beat_matches_inside_a_word() {
        assert!(score("hs", "host_state.rs") > score("hs", "ghosts.rs"));
        assert!(score("lc", "live_conversation.rs") > score("lc", "balcony.rs"));
        assert!(score("fn", "FileName.rs") > score("fn", "often.rs"));
        assert_eq!(
            fuzzy_match("hs", "host_state.rs").unwrap().positions,
            [0, 5]
        );
    }

    #[test]
    fn path_component_starts_beat_word_starts_and_inner_characters() {
        assert!(score("u", "src/ui") > score("u", "src/build"));
        assert!(score("cu", "crates/ui/x") > score("cu", "crates_ui/x"));
        assert_eq!(
            fuzzy_match("su", "crates/src/ui/mod.rs").unwrap().positions,
            [7, 11]
        );
    }

    #[test]
    fn file_name_matches_beat_directory_matches() {
        assert!(score("git", "crates/core/src/git.rs") > score("git", "git/tests/helpers.rs"));
        assert!(score("comp", "ui/composer.rs") > score("comp", "composer/ui.rs"));
        // The alignment itself prefers the name over an equal directory run.
        assert_eq!(
            fuzzy_match("mod", "models/ui/mod.rs").unwrap().positions,
            [10, 11, 12]
        );
    }

    #[test]
    fn closer_matches_beat_wider_gaps() {
        assert!(score("ab", "a_b") > score("ab", "a____b"));
        assert!(score("mr", "main.rs") > score("mr", "main_window_controller.rs"));
    }

    #[test]
    fn oversized_tables_still_match_with_valid_positions() {
        let candidate = format!("{}/needle.rs", "a/".repeat(TABLE_LIMIT));
        let found = fuzzy_match("needle", &candidate).unwrap();
        assert_eq!(found.positions.len(), 6);
        assert!(found.positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(fuzzy_match("needlez", &candidate).is_none());
    }
}
