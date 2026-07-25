//! Fuzzy ranking for the command menu, and the match positions that colour it.
//!
//! A thin wrapper over `nucleo-matcher`, mirroring grok's `slash/matcher.rs`: `rank`
//! scores candidates and `indices` reports which characters of a string the last
//! pattern matched. Those indices are the entire basis of the highlighted letters —
//! writing our own matcher would mean writing our own (worse) version of both.
//!
//! The [`Matcher`] is kept alive between keystrokes rather than rebuilt: it owns a
//! ~100 KB scoring slab, which is cheap to reuse and wasteful to reallocate on every
//! character typed.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// A reusable fuzzy matcher plus the pattern most recently ranked with.
pub struct FuzzyMatcher {
    matcher: Matcher,
    /// Set by [`FuzzyMatcher::rank`]; read by [`FuzzyMatcher::indices`], so highlighting
    /// can never disagree with the ranking that produced the rows.
    pattern: Pattern,
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FuzzyMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Matcher` is an opaque scoring slab with no useful projection.
        f.debug_struct("FuzzyMatcher").finish_non_exhaustive()
    }
}

impl FuzzyMatcher {
    /// A matcher with nucleo's default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            pattern: Pattern::default(),
        }
    }

    /// Score `items` against `query`, best first.
    ///
    /// Returns `(index, score)` for the items that match at all, sorted by descending
    /// score and then by ascending key text — grok's ordering, and the reason a
    /// single-letter query produces a stable list rather than an arbitrary one (many
    /// candidates tie at the same score).
    ///
    /// Case handling is `Smart`: an all-lowercase query is case-insensitive, and any
    /// uppercase character makes the query case-sensitive.
    pub fn rank<T>(
        &mut self,
        items: &[T],
        query: &str,
        key: impl Fn(&T) -> &str,
    ) -> Vec<(usize, u32)> {
        self.pattern = Pattern::parse(query.trim(), CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut hits: Vec<(usize, u32, &str)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let text = key(item);
                if text.is_empty() {
                    return None;
                }
                let score = self
                    .pattern
                    .score(Utf32Str::new(text, &mut buf), &mut self.matcher)?;
                Some((i, score, text))
            })
            .collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(b.2)));
        hits.into_iter().map(|(i, score, _)| (i, score)).collect()
    }

    /// Character positions of `text` matched by the pattern [`FuzzyMatcher::rank`] last
    /// parsed, sorted and deduplicated.
    ///
    /// nucleo appends to the buffer in match order and may repeat a position, so both
    /// steps are required before the renderer can group them into runs.
    pub fn indices(&mut self, text: &str) -> Vec<u32> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        let mut indices = Vec::new();
        self.pattern.indices(
            Utf32Str::new(text, &mut buf),
            &mut self.matcher,
            &mut indices,
        );
        indices.sort_unstable();
        indices.dedup();
        indices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(hits: &[(usize, u32)], items: &[&str]) -> Vec<String> {
        hits.iter().map(|&(i, _)| items[i].to_string()).collect()
    }

    /// The point of a fuzzy matcher over the substring test it replaces: a query whose
    /// letters are scattered through the name still matches.
    #[test]
    fn subsequence_queries_match_and_rank_the_best_first() {
        let mut m = FuzzyMatcher::new();
        let items = ["model", "new", "quit", "compact"];
        assert_eq!(names(&m.rank(&items, "mdl", |s| *s), &items), vec!["model"]);
        assert_eq!(
            names(&m.rank(&items, "cmp", |s| *s), &items),
            vec!["compact"]
        );
        // A substring matcher would have found neither.
        assert!(!"model".contains("mdl"));
    }

    #[test]
    fn non_matches_are_dropped_entirely() {
        let mut m = FuzzyMatcher::new();
        let items = ["model", "new"];
        assert!(m.rank(&items, "zzz", |s| *s).is_empty());
    }

    /// Ties are broken by key text, so a one-letter query is stable rather than
    /// arbitrary (grok's `query_p_ties_…` case).
    #[test]
    fn equal_scores_break_on_the_key_text() {
        let mut m = FuzzyMatcher::new();
        let items = ["plan", "plugins", "personas"];
        let hits = m.rank(&items, "p", |s| *s);
        assert_eq!(
            names(&hits, &items),
            vec!["personas", "plan", "plugins"],
            "same score ⇒ alphabetical"
        );
        assert!(hits.iter().all(|&(_, s)| s == hits[0].1), "{hits:?}");
    }

    /// Smart case: lowercase matches anything, an uppercase letter demands one.
    #[test]
    fn case_matching_is_smart() {
        let mut m = FuzzyMatcher::new();
        let items = ["Model"];
        assert_eq!(m.rank(&items, "mod", |s| *s).len(), 1, "lowercase is loose");
        assert_eq!(m.rank(&items, "Mod", |s| *s).len(), 1);
        assert_eq!(
            m.rank(&items, "MOD", |s| *s).len(),
            0,
            "uppercase demands an exact case match"
        );
    }

    /// Indices come from the pattern the ranking used, and address the string handed
    /// to `indices` — which is the label (`/model`), not the key (`model`).
    #[test]
    fn indices_are_sorted_deduped_positions_of_the_string_asked_about() {
        let mut m = FuzzyMatcher::new();
        let items = ["model"];
        let _ = m.rank(&items, "mdl", |s| *s);
        let indices = m.indices("/model");
        assert_eq!(
            indices,
            vec![1, 3, 5],
            "the leading slash shifts each by one"
        );
        let label = "/model";
        let matched: String = indices
            .iter()
            .map(|&i| label.chars().nth(i as usize).unwrap())
            .collect();
        assert_eq!(matched, "mdl");
    }

    #[test]
    fn an_empty_string_has_no_indices() {
        let mut m = FuzzyMatcher::new();
        let _ = m.rank(&["x"], "x", |s| *s);
        assert!(m.indices("").is_empty());
    }
}
