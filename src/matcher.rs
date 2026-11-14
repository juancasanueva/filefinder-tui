//! Name matching: what the filter (`/`) leaves standing and how Find
//! (`f`) ranks what it finds. No terminal types in here.

use std::cmp::Ordering;

use crate::format::natural_cmp;

/// A stretch of a name, counted in characters, that a match covers: what
/// a row draws bold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub len: usize,
}

/// A close match: how many letters it took to change, how well the
/// letters line up, and the stretches it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Close {
    pub edits: u8,
    pub score: i32,
    pub runs: Vec<Run>,
}

// Weights for letters that line up: in a run, at the start of a
// word, in the name rather than the folders above it; and a gap's cost.
const BONUS_CONSECUTIVE: i32 = 8;
const BONUS_BOUNDARY: i32 = 6;
const BONUS_BASENAME: i32 = 4;
const PENALTY_GAP: i32 = 1;
/// How many places a line-up is tried from.
const MAX_STARTS: usize = 16;
/// What an edit across a digit or punctuation costs: past every budget.
const NEVER: u16 = u16::MAX / 4;

/// What was typed, folded once.
#[derive(Clone, Debug)]
pub struct Query {
    needle: Vec<char>,
    wildcard: bool,
    budget: u8,
}

/// A character in lower case, one for one, so that positions stay the name's.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_separator(c: char) -> bool {
    matches!(c, '/' | '-' | '_' | '.' | ' ')
}

/// A name's characters folded, and where each one starts a word.
fn letters(name: &str) -> (Vec<char>, Vec<bool>) {
    let chars: Vec<char> = name.chars().collect();
    let words = (0..chars.len())
        .map(|at| {
            at == 0 || {
                let before = chars[at - 1];
                is_separator(before) || (chars[at].is_uppercase() && !before.is_uppercase())
            }
        })
        .collect();
    (chars.into_iter().map(fold).collect(), words)
}

impl Query {
    /// `None` for nothing typed, which is no query at all.
    pub fn new(text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let needle: Vec<char> = text.chars().map(fold).collect();
        let wildcard = needle.iter().any(|c| matches!(c, '*' | '?'));
        let letters = needle.iter().filter(|c| c.is_alphanumeric()).count();
        // The typo budget: none under four letters or digits, one up to
        // seven, two from eight; none for a pattern or punctuation in front.
        let budget = match letters {
            _ if wildcard || !needle[0].is_alphanumeric() => 0,
            0..=3 => 0,
            4..=7 => 1,
            _ => 2,
        };
        Some(Self {
            needle,
            wildcard,
            budget,
        })
    }

    /// How many letters a close match may change.
    #[cfg(test)]
    pub fn budget(&self) -> u8 {
        self.budget
    }

    /// The first stretch of the name that matches whatever the case: the
    /// text itself, or with `*` and `?` the shortest stretch the pattern
    /// takes, as if a `*` led it.
    pub fn exact(&self, name: &str) -> Option<Run> {
        let hay: Vec<char> = name.chars().map(fold).collect();
        if self.wildcard {
            return (0..=hay.len()).find_map(|start| {
                glob_prefix(&hay[start..], &self.needle).map(|len| Run { start, len })
            });
        }
        let len = self.needle.len();
        hay.windows(len)
            .position(|part| part == self.needle.as_slice())
            .map(|start| Run { start, len })
    }

    /// A close match: the letters in order with others between them, the
    /// best line-up winning; or else a typo within the budget. Never for a
    /// pattern.
    pub fn close(&self, name: &str) -> Option<Close> {
        if self.wildcard {
            return None;
        }
        let (hay, words) = letters(name);
        if let Some((score, positions)) = self.align(&hay, &words, 0) {
            return Some(Close {
                edits: 0,
                score,
                runs: runs_of(&positions),
            });
        }
        let (edits, run) = self.typo(&hay)?;
        Some(Close {
            edits,
            score: 0,
            runs: vec![run],
        })
    }

    /// What a name shows bold: its exact match, or else its close one.
    pub fn runs(&self, name: &str) -> Vec<Run> {
        match self.exact(name) {
            Some(run) => vec![run],
            None => self.close(name).map(|close| close.runs).unwrap_or_default(),
        }
    }

    /// A greedy line-up of the query's letters in order, tried from
    /// each of the first places the first letter is: its score and where
    /// each letter landed. Letters at or after `base` are in the name.
    fn align(&self, hay: &[char], words: &[bool], base: usize) -> Option<(i32, Vec<usize>)> {
        let mut best: Option<(i32, Vec<usize>)> = None;
        let starts = (0..hay.len()).filter(|at| hay[*at] == self.needle[0]);
        for start in starts.take(MAX_STARTS) {
            // A start that can't finish means no later one can.
            let Some((score, positions)) = self.align_from(hay, words, base, start) else {
                break;
            };
            if best.as_ref().is_none_or(|(top, _)| score > *top) {
                best = Some((score, positions));
            }
        }
        best
    }

    fn align_from(
        &self,
        hay: &[char],
        words: &[bool],
        base: usize,
        start: usize,
    ) -> Option<(i32, Vec<usize>)> {
        let (mut total, mut at, mut previous) = (0, start, None::<usize>);
        let mut positions = Vec::with_capacity(self.needle.len());
        for (k, want) in self.needle.iter().enumerate() {
            if k > 0 {
                at += 1 + hay.get(at + 1..)?.iter().position(|c| c == want)?;
            }
            total += match previous {
                Some(p) if at == p + 1 => BONUS_CONSECUTIVE,
                Some(p) => -PENALTY_GAP * (at - p - 1) as i32,
                None => 0,
            };
            if words[at] {
                total += BONUS_BOUNDARY;
            }
            if at >= base {
                total += BONUS_BASENAME;
            }
            positions.push(at);
            previous = Some(at);
        }
        Some((total, positions))
    }

    /// The fewest edits that make the query a stretch of the name, when
    /// they are within the budget, and that stretch. An edit puts in,
    /// leaves out, changes or swaps letters; digits and punctuation stay as
    /// they are.
    fn typo(&self, hay: &[char]) -> Option<(u8, Run)> {
        if self.budget == 0 {
            return None;
        }
        let (m, n) = (self.needle.len(), hay.len());
        let letter = |c: char| c.is_alphabetic();
        // Row i column j: the fewest edits for the query's first i letters
        // to end at the name's j-th, and where that stretch starts.
        let mut rows = vec![vec![(0u16, 0usize); n + 1]; m + 1];
        for (j, cell) in rows[0].iter_mut().enumerate() {
            *cell = (0, j);
        }
        for i in 1..=m {
            let q = self.needle[i - 1];
            let leave_out = if letter(q) { 1 } else { NEVER };
            rows[i][0] = (rows[i - 1][0].0.saturating_add(leave_out).min(NEVER), 0);
            for j in 1..=n {
                let h = hay[j - 1];
                let change = if q == h {
                    0
                } else if letter(q) && letter(h) {
                    1
                } else {
                    NEVER
                };
                let put_in = if letter(h) { 1 } else { NEVER };
                let mut best = (
                    rows[i - 1][j - 1].0.saturating_add(change),
                    rows[i - 1][j - 1].1,
                );
                for option in [
                    (rows[i - 1][j].0.saturating_add(leave_out), rows[i - 1][j].1),
                    (rows[i][j - 1].0.saturating_add(put_in), rows[i][j - 1].1),
                ] {
                    if option.0 < best.0 {
                        best = option;
                    }
                }
                let swapped = i > 1
                    && j > 1
                    && q != h
                    && q == hay[j - 2]
                    && self.needle[i - 2] == h
                    && letter(q)
                    && letter(h);
                if swapped && rows[i - 2][j - 2].0 + 1 < best.0 {
                    best = (rows[i - 2][j - 2].0 + 1, rows[i - 2][j - 2].1);
                }
                rows[i][j] = (best.0.min(NEVER), best.1);
            }
        }
        let (end, (edits, start)) = (1..=n)
            .map(|j| (j, rows[m][j]))
            .min_by_key(|(j, (edits, _))| (*edits, *j))?;
        (edits <= u16::from(self.budget)).then(|| {
            (
                edits as u8,
                Run {
                    start,
                    len: end - start,
                },
            )
        })
    }
}

/// The shortest stretch from the front of `hay` that a pattern takes: `*`
/// any run of characters, `?` one.
fn glob_prefix(hay: &[char], pattern: &[char]) -> Option<usize> {
    let (mut h, mut p, mut star) = (0, 0, None::<(usize, usize)>);
    loop {
        if p == pattern.len() {
            return Some(h);
        }
        if h < hay.len() && (pattern[p] == '?' || pattern[p] == hay[h]) {
            h += 1;
            p += 1;
        } else if pattern[p] == '*' {
            star = Some((p, h));
            p += 1;
        } else if let Some((at, from)) = star.filter(|(_, from)| *from < hay.len()) {
            star = Some((at, from + 1));
            p = at + 1;
            h = from + 1;
        } else {
            return None;
        }
    }
}

/// Positions next to each other gathered into stretches.
fn runs_of(positions: &[usize]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for &at in positions {
        match runs.last_mut() {
            Some(run) if run.start + run.len == at => run.len += 1,
            _ => runs.push(Run { start: at, len: 1 }),
        }
    }
    runs
}

/// What a filter leaves standing, by place in the list it was given: the
/// exact matches in their order or, when there are none, the close ones
/// with the fewest edits and the best line-up first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Narrowed {
    pub exact: Vec<usize>,
    pub close: Vec<usize>,
}

/// `None` for nothing typed, which leaves everything standing.
pub fn narrow<'a>(text: &str, names: impl IntoIterator<Item = &'a str>) -> Option<Narrowed> {
    let query = Query::new(text)?;
    let names: Vec<&str> = names.into_iter().collect();
    let exact: Vec<usize> = (0..names.len())
        .filter(|index| query.exact(names[*index]).is_some())
        .collect();
    if !exact.is_empty() {
        return Some(Narrowed {
            exact,
            close: Vec::new(),
        });
    }
    let mut ranked: Vec<(u8, i32, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            query
                .close(name)
                .map(|close| (close.edits, close.score, index))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    Some(Narrowed {
        exact,
        close: ranked.into_iter().map(|(_, _, index)| index).collect(),
    })
}

/// How well something Find found matches, worst first: its tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Within the typo budget.
    Typo,
    /// The letters in order in the name.
    Subsequence,
    /// A query with a `/`, its letters in order along the path.
    Aligned,
    /// At the start of a word in the name.
    WordStart,
    /// At the start of the name.
    Prefix,
    /// The whole name.
    Exact,
}

/// A tier, then how well within it: the line-up's score, or fewer edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Score {
    pub tier: Tier,
    pub bonus: i32,
}

/// How a name matches for Find, `path` being where it is below the folder
/// looked in; `None` when it doesn't.
pub fn score(query: &Query, name: &str, path: &str) -> Option<Score> {
    let (hay, words) = letters(name);
    let needle = &query.needle;
    let at = |start: usize| hay.get(start..start + needle.len()) == Some(needle.as_slice());
    let tier = |tier| Some(Score { tier, bonus: 0 });
    if at(0) {
        return tier(if hay.len() == needle.len() {
            Tier::Exact
        } else {
            Tier::Prefix
        });
    }
    if (1..hay.len()).any(|start| words[start] && at(start)) {
        return tier(Tier::WordStart);
    }
    if needle.contains(&'/') {
        let (along, words) = letters(path);
        let base = along.len() - hay.len().min(along.len());
        if let Some((bonus, _)) = query.align(&along, &words, base) {
            return Some(Score {
                tier: Tier::Aligned,
                bonus,
            });
        }
    }
    if let Some((bonus, _)) = query.align(&hay, &words, 0) {
        return Some(Score {
            tier: Tier::Subsequence,
            bonus,
        });
    }
    let (edits, _) = query.typo(&hay)?;
    Some(Score {
        tier: Tier::Typo,
        bonus: -1000 * i32::from(edits),
    })
}

/// Find's order: the better match first, then the shorter path, then the
/// paths in natural order.
pub fn rank(a: &Score, a_path: &str, b: &Score, b_path: &str) -> Ordering {
    b.cmp(a)
        .then_with(|| a_path.chars().count().cmp(&b_path.chars().count()))
        .then_with(|| natural_cmp(a_path, b_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(query: &str, name: &str) -> Option<(usize, usize)> {
        Query::new(query)?
            .exact(name)
            .map(|run| (run.start, run.len))
    }

    fn shown(query: &str, names: &[&str]) -> (Vec<usize>, Vec<usize>) {
        let narrowed = narrow(query, names.iter().copied()).unwrap();
        (narrowed.exact, narrowed.close)
    }

    #[test]
    fn a_query_matches_any_part_of_a_name_whatever_the_case() {
        assert_eq!(exact("jpg", "Harbour.JPG"), Some((8, 3)));
        assert_eq!(exact("É M", "café menu"), Some((3, 3)));
        assert_eq!(exact("x", "notes"), None);
        // Nothing typed is no filter at all.
        assert!(Query::new("").is_none());
        assert!(narrow("", ["a"]).is_none());
    }

    #[test]
    fn star_and_question_mark_are_a_pattern_behind_an_implied_star() {
        assert_eq!(exact("*.txt", "a b.txt"), Some((0, 7)));
        assert_eq!(exact("?.txt", "a b.txt"), Some((2, 5)));
        assert_eq!(exact("a*b", "xa yb"), Some((1, 4)));
        assert_eq!(exact("a?b", "a b.txt"), Some((0, 3)));
        assert_eq!(exact("a*z", "a b.txt"), None);
        assert_eq!(exact("t*s", "notes.txt"), Some((2, 3)));
        // A pattern never has close matches.
        assert_eq!(shown("q*", &["notes"]), (vec![], vec![]));
    }

    #[test]
    fn the_typo_budget_is_none_under_four_letters_one_up_to_seven_two_from_eight() {
        let budget = |query: &str| Query::new(query).unwrap().budget();
        assert_eq!(budget("abc"), 0);
        assert_eq!(budget("abcd"), 1);
        assert_eq!(budget("abcdefg"), 1);
        assert_eq!(budget("abcdefgh"), 2);
        // Only letters and digits count; a pattern or punctuation in front gets none.
        assert_eq!(budget("a.b-c"), 0);
        assert_eq!(budget(".abcdefgh"), 0);
        assert_eq!(budget("abcd*"), 0);

        // One edit at three letters is too many, at four it is close.
        assert_eq!(shown("nto", &["note"]), (vec![], vec![]));
        assert_eq!(shown("ntoe", &["notes"]), (vec![], vec![0]));
        // Seven letters take one edit, not two; eight take two.
        assert_eq!(shown("prjoect", &["project"]), (vec![], vec![0]));
        assert_eq!(shown("prjoecx", &["project"]), (vec![], vec![]));
        assert_eq!(shown("prjoectz", &["projects"]), (vec![], vec![0]));
        assert_eq!(shown("prjoeczz", &["projects"]), (vec![], vec![]));
    }

    #[test]
    fn close_matches_only_when_nothing_matches_exactly() {
        let names = ["zeta.md", "notes.txt", "nuts.txt", "notes-old.txt"];
        assert_eq!(shown("notes", &names), (vec![1, 3], vec![]));
        // A swap of two letters is one edit.
        let (exact, close) = shown("ntoes", &names);
        assert!(exact.is_empty());
        assert!(close.contains(&1) && close.contains(&3) && !close.contains(&0));
        assert_eq!(shown("qqqqqq", &names), (vec![], vec![]));
        // A subsequence is close too, the best aligned first.
        assert_eq!(
            shown("nsx", &["notes.txt", "n_s_x", "zz"]),
            (vec![], vec![1, 0])
        );
        // Fewer edits come first, then the earlier name.
        assert_eq!(
            shown("projects", &["projcets", "prjcts", "proejcts"]),
            (vec![], vec![0, 2, 1])
        );
    }

    #[test]
    fn what_a_name_shows_bold_is_the_exact_match_or_else_the_close_one() {
        let runs = |query: &str, name: &str| -> Vec<(usize, usize)> {
            let query = Query::new(query).unwrap();
            query
                .runs(name)
                .iter()
                .map(|run| (run.start, run.len))
                .collect()
        };
        assert_eq!(runs("trip", "day-trip.jpg"), [(4, 4)]);
        assert_eq!(runs("dtj", "day-trip.jpg"), [(0, 1), (4, 1), (9, 1)]);
        assert_eq!(runs("ntoes", "notes.txt"), [(0, 5)]);
        assert_eq!(runs("zzz", "notes.txt"), []);
    }

    fn tier(query: &str, name: &str) -> Option<Tier> {
        score(&Query::new(query)?, name, name).map(|score| score.tier)
    }

    #[test]
    fn find_ranks_exact_then_prefix_then_word_start_then_aligned_then_subsequence_then_typo() {
        assert_eq!(tier("trip", "Trip"), Some(Tier::Exact));
        assert_eq!(tier("trip", "trip-plan.md"), Some(Tier::Prefix));
        assert_eq!(tier("trip", "day-trip.jpg"), Some(Tier::WordStart));
        assert_eq!(tier("trip", "DayTrip.jpg"), Some(Tier::WordStart));
        assert_eq!(tier("trip", "daytrip.jpg"), Some(Tier::Subsequence));
        assert_eq!(tier("tp", "trip"), Some(Tier::Subsequence));
        assert_eq!(tier("tirp", "trip"), Some(Tier::Typo));
        assert_eq!(tier("zzzz", "trip"), None);
        // A query with a slash lines up with the path.
        let query = Query::new("pic/trip").unwrap();
        let aligned = score(&query, "day-trip.jpg", "Pictures/Trip/day-trip.jpg");
        assert_eq!(aligned.map(|score| score.tier), Some(Tier::Aligned));
        assert!(Tier::Exact > Tier::Prefix && Tier::Prefix > Tier::WordStart);
        assert!(Tier::WordStart > Tier::Aligned && Tier::Aligned > Tier::Subsequence);
        assert!(Tier::Subsequence > Tier::Typo);
    }

    #[test]
    fn within_a_tier_a_tighter_match_and_fewer_edits_rank_higher() {
        let query = Query::new("abc").unwrap();
        let tight = score(&query, "xabcx", "xabcx").unwrap();
        let loose = score(&query, "xaxbxcx", "xaxbxcx").unwrap();
        assert_eq!(
            (tight.tier, loose.tier),
            (Tier::Subsequence, Tier::Subsequence)
        );
        assert!(tight > loose);
        let query = Query::new("abcdefgh").unwrap();
        let one = score(&query, "abcdefgx", "abcdefgx").unwrap();
        let two = score(&query, "abcdefxx", "abcdefxx").unwrap();
        assert_eq!((one.tier, two.tier), (Tier::Typo, Tier::Typo));
        assert!(one > two);
    }

    #[test]
    fn ties_go_to_the_shorter_path_then_natural_order() {
        let query = Query::new("report").unwrap();
        let mut hits = [
            "deep/er/report 2.txt",
            "b/report 9.txt",
            "a/report 10.txt",
            "a/report 9.txt",
            "report.txt",
        ]
        .map(|path| {
            let name = path.rsplit('/').next().unwrap();
            (score(&query, name, path).unwrap(), path.to_string())
        })
        .to_vec();
        hits.sort_by(|a, b| rank(&a.0, &a.1, &b.0, &b.1));
        let order: Vec<&str> = hits.iter().map(|(_, path)| path.as_str()).collect();
        assert_eq!(
            order,
            [
                "report.txt",
                "a/report 9.txt",
                "b/report 9.txt",
                "a/report 10.txt",
                "deep/er/report 2.txt"
            ]
        );
    }
}
