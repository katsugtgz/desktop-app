//! Frecency engine (port of `@getstation/frecency`, the library behind
//! `packages/app/src/bang/search/score/frecency.ts`): pure scoring over
//! saved selections, keyed by query string and by result id.
//!
//! The storage provider is the in-memory `FrecencyData` itself (TS persists
//! it through `localStorage`; a host can serialize it back out with serde).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Per-(query, id) selection counts (`queries[query][i]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QuerySelection {
    pub id: String,
    /// Total number of times this result was selected for this query.
    pub times_selected: i64,
    /// Timestamps (ms) of the most recent selections, capped at
    /// `timestamps_limit`.
    pub selected_at: Vec<i64>,
}

/// Per-id selection counts (`selections[id]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IdSelection {
    pub times_selected: i64,
    pub selected_at: Vec<i64>,
    /// Queries this id was selected under (map to `true`), used by cleanup.
    pub queries: HashMap<String, bool>,
}

/// The frecency dataset (`FrecencyData`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrecencyData {
    pub queries: HashMap<String, Vec<QuerySelection>>,
    pub selections: HashMap<String, IdSelection>,
    /// Most-recently-selected ids first, capped at `recent_selections_limit`.
    pub recent_selections: Vec<String>,
}

/// Weight configuration (`FrecencyWeightOptions`); defaults match the
/// frecency library constructor.
#[derive(Debug, Clone, PartialEq)]
pub struct FrecencyWeights {
    pub exact_query_match: f64,
    pub sub_query_match: f64,
    pub recent_selections_match: f64,
}

impl Default for FrecencyWeights {
    fn default() -> Self {
        FrecencyWeights {
            exact_query_match: 1.0,
            sub_query_match: 0.7,
            recent_selections_match: 0.5,
        }
    }
}

/// Engine options: limits keep the library defaults.
#[derive(Debug, Clone)]
pub struct FrecencyOptions {
    pub timestamps_limit: usize,
    pub recent_selections_limit: usize,
    pub weights: FrecencyWeights,
}

impl Default for FrecencyOptions {
    fn default() -> Self {
        FrecencyOptions {
            timestamps_limit: 10,
            recent_selections_limit: 100,
            weights: FrecencyWeights::default(),
        }
    }
}

/// `save` parameters (`SaveParams`).
#[derive(Debug, Clone, Default)]
pub struct SaveParams<'a> {
    pub search_query: Option<&'a str>,
    pub selected_id: &'a str,
    /// Milliseconds since epoch (TS `Date`).
    pub date_selection: Option<i64>,
}

/// The frecency engine. Owned data; clone to snapshot.
#[derive(Debug, Clone)]
pub struct Frecency {
    options: FrecencyOptions,
    pub data: FrecencyData,
}

impl Frecency {
    pub fn new(options: FrecencyOptions) -> Self {
        Frecency {
            options,
            data: FrecencyData::default(),
        }
    }

    /// Record a selection (`save`).
    pub fn save(&mut self, params: SaveParams<'_>) {
        let now = now_ms();
        let date = params.date_selection.unwrap_or(now);
        let query = params.search_query.filter(|q| !q.is_empty());
        let id = params.selected_id;

        self.update_frecency_by_query(query, id, date);
        self.update_frecency_by_id(query, id, date);
        self.clean_up_old_ids(id);
    }

    /// Score one result (`computeScore`): exact query match, then
    /// sub-query matches, then the id's recent selections.
    pub fn compute_score(&self, search_query: &str, result_id: &str) -> f64 {
        self.compute_score_at(search_query, result_id, now_ms())
    }

    /// [`Frecency::compute_score`] with an explicit `now` (ms).
    pub fn compute_score_at(&self, search_query: &str, result_id: &str, now: i64) -> f64 {
        let weights = &self.options.weights;

        if !search_query.is_empty() {
            if let Some(selections) = self.data.queries.get(search_query) {
                if let Some(selection) = selections.iter().find(|s| s.id == result_id) {
                    let score = weights.exact_query_match
                        * calculate_score(&selection.selected_at, selection.times_selected, now);
                    if score > 0.0 {
                        return score;
                    }
                }
            }
        }

        let empty = Vec::new();
        let sub_queries = self
            .data
            .queries
            .keys()
            .filter(|q| is_sub_query(search_query, q))
            .map(|q| (q, self.data.queries.get(q).unwrap_or(&empty)))
            .collect::<Vec<_>>();
        for (query, selections) in sub_queries {
            if let Some(selection) = selections.iter().find(|s| s.id == result_id) {
                let score = weights.sub_query_match
                    * calculate_score(&selection.selected_at, selection.times_selected, now);
                if score > 0.0 {
                    return score;
                }
            }
            let _ = query;
        }

        if let Some(selection) = self.data.selections.get(result_id) {
            return weights.recent_selections_match
                * calculate_score(&selection.selected_at, selection.times_selected, now);
        }
        0.0
    }

    /// Sort results by frecency, keeping the score (`sort` with
    /// `keepScores: true` — the way `createFrecencyAlgorithm` calls it).
    /// Scored results come first, descending; zero-score results keep
    /// their input order after them.
    pub fn sort<'a>(&self, search_query: &str, results: &[&'a str]) -> Vec<(&'a str, f64)> {
        self.sort_at(search_query, results, now_ms())
    }

    /// [`Frecency::sort`] with an explicit `now` (ms).
    pub fn sort_at<'a>(
        &self,
        search_query: &str,
        results: &[&'a str],
        now: i64,
    ) -> Vec<(&'a str, f64)> {
        let scored: Vec<(&str, f64)> = results
            .iter()
            .map(|id| (*id, self.compute_score_at(search_query, id, now)))
            .collect();

        let mut recent: Vec<(&str, f64)> =
            scored.iter().filter(|(_, s)| *s > 0.0).cloned().collect();
        let others: Vec<(&str, f64)> = scored.iter().filter(|(_, s)| *s == 0.0).cloned().collect();
        recent.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        recent.extend(others);
        recent
    }

    fn update_frecency_by_query(&mut self, query: Option<&str>, id: &str, date: i64) {
        let Some(query) = query else { return };
        let limit = self.options.timestamps_limit;
        let selections = self.data.queries.entry(query.to_owned()).or_default();
        match selections.iter_mut().find(|s| s.id == id) {
            None => selections.push(QuerySelection {
                id: id.to_owned(),
                times_selected: 1,
                selected_at: vec![date],
            }),
            Some(selection) => {
                selection.times_selected += 1;
                selection.selected_at.push(date);
                if selection.selected_at.len() > limit {
                    selection.selected_at.remove(0);
                }
            }
        }
    }

    fn update_frecency_by_id(&mut self, query: Option<&str>, id: &str, date: i64) {
        let limit = self.options.timestamps_limit;
        match self.data.selections.get_mut(id) {
            None => {
                let mut queries = HashMap::new();
                if let Some(q) = query {
                    queries.insert(q.to_owned(), true);
                }
                self.data.selections.insert(
                    id.to_owned(),
                    IdSelection {
                        times_selected: 1,
                        selected_at: vec![date],
                        queries,
                    },
                );
            }
            Some(selection) => {
                selection.times_selected += 1;
                selection.selected_at.push(date);
                if selection.selected_at.len() > limit {
                    selection.selected_at.remove(0);
                }
                if let Some(q) = query {
                    selection.queries.insert(q.to_owned(), true);
                }
            }
        }
    }

    fn clean_up_old_ids(&mut self, id: &str) {
        if self.data.recent_selections.contains(&id.to_owned()) {
            self.data.recent_selections.retain(|i| i != id);
            self.data.recent_selections.insert(0, id.to_owned());
            return;
        }

        if self.data.recent_selections.len() < self.options.recent_selections_limit {
            self.data.recent_selections.insert(0, id.to_owned());
            return;
        }

        let Some(id_to_remove) = self.data.recent_selections.pop() else {
            return;
        };
        self.data.recent_selections.insert(0, id.to_owned());

        let Some(selection_by_id) = self.data.selections.remove(&id_to_remove) else {
            return;
        };
        for query in selection_by_id.queries.keys() {
            if let Some(selections) = self.data.queries.get_mut(query) {
                selections.retain(|s| s.id != id_to_remove);
                if selections.is_empty() {
                    self.data.queries.remove(query);
                }
            }
        }
    }
}

/// Time-bucketed score (`_calculateScore`): each timestamp contributes by
/// recency bucket, then the average is scaled by total selections.
fn calculate_score(timestamps: &[i64], times_selected: i64, now: i64) -> f64 {
    if timestamps.is_empty() {
        return 0.0;
    }
    const HOUR: i64 = 1000 * 60 * 60;
    const DAY: i64 = 24 * HOUR;

    let total: i64 = timestamps
        .iter()
        .map(|t| {
            if *t >= now - 3 * HOUR {
                100
            } else if *t >= now - DAY {
                80
            } else if *t >= now - 3 * DAY {
                60
            } else if *t >= now - 7 * DAY {
                30
            } else if *t >= now - 14 * DAY {
                10
            } else {
                0
            }
        })
        .sum();
    times_selected as f64 * (total as f64 / timestamps.len() as f64)
}

/// By-word prefix sub-query test (`isSubQuery` in the frecency lib):
/// `'de tea'` is a sub-query of `'design team'`, order-insensitive, each
/// search word must prefix-match a distinct query word.
pub fn is_sub_query(search_query: &str, candidate: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let mut query_words: Vec<String> = candidate
        .to_lowercase()
        .split(' ')
        .map(str::to_owned)
        .collect();
    let search_words: Vec<String> = search_query
        .to_lowercase()
        .split(' ')
        .map(str::to_owned)
        .collect();

    for search in search_words {
        if search.is_empty() {
            continue;
        }
        let Some(idx) = query_words.iter().position(|q| q.starts_with(&search)) else {
            return false;
        };
        query_words.remove(idx);
    }
    true
}

/// Milliseconds since the UNIX epoch (`Date.now()`).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 1000 * 60 * 60;

    #[test]
    fn save_records_query_and_id_selections() {
        let mut f = Frecency::new(FrecencyOptions::default());
        f.save(SaveParams {
            search_query: Some("gmail"),
            selected_id: "tab-1",
            date_selection: Some(1000),
        });
        f.save(SaveParams {
            search_query: Some("gmail"),
            selected_id: "tab-1",
            date_selection: Some(2000),
        });

        let qs = &f.data.queries["gmail"];
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].times_selected, 2);
        assert_eq!(qs[0].selected_at, vec![1000, 2000]);

        let sel = &f.data.selections["tab-1"];
        assert_eq!(sel.times_selected, 2);
        assert_eq!(sel.queries.keys().collect::<Vec<_>>(), vec!["gmail"]);
        assert_eq!(f.data.recent_selections, vec!["tab-1"]);
    }

    #[test]
    fn save_without_query_skips_query_map() {
        let mut f = Frecency::new(FrecencyOptions::default());
        f.save(SaveParams {
            search_query: None,
            selected_id: "tab-1",
            date_selection: Some(1000),
        });
        assert!(f.data.queries.is_empty());
        assert!(f.data.selections.contains_key("tab-1"));
    }

    #[test]
    fn timestamps_are_capped() {
        let mut f = Frecency::new(FrecencyOptions {
            timestamps_limit: 2,
            ..Default::default()
        });
        for t in [1, 2, 3, 4] {
            f.save(SaveParams {
                search_query: Some("q"),
                selected_id: "a",
                date_selection: Some(t),
            });
        }
        assert_eq!(f.data.selections["a"].selected_at, vec![3, 4]);
        assert_eq!(f.data.selections["a"].times_selected, 4);
    }

    #[test]
    fn score_uses_recency_buckets() {
        let mut f = Frecency::new(FrecencyOptions::default());
        let now = 100 * HOUR;
        f.save(SaveParams {
            search_query: Some("q"),
            selected_id: "a",
            date_selection: Some(now - HOUR),
        });
        // 1 selection, 1h old -> bucket 100 -> 1 * 100/1 = 100, weight 1.0.
        assert_eq!(f.compute_score_at("q", "a", now), 100.0);

        f.save(SaveParams {
            search_query: Some("q"),
            selected_id: "b",
            date_selection: Some(now - 4 * HOUR),
        });
        // 4h old: past the 3h bucket, inside the 24h bucket.
        assert_eq!(f.compute_score_at("q", "b", now), 80.0);

        // Two saves at 1h and 4h: timesSelected 2 * avg(100, 80).
        f.save(SaveParams {
            search_query: Some("q"),
            selected_id: "c",
            date_selection: Some(now - HOUR),
        });
        f.save(SaveParams {
            search_query: Some("q"),
            selected_id: "c",
            date_selection: Some(now - 4 * HOUR),
        });
        assert_eq!(f.compute_score_at("q", "c", now), 2.0 * 90.0);

        // No query match: recent-selections weight 0.5 over the id data.
        assert_eq!(f.compute_score_at("other", "a", now), 0.5 * 100.0);

        // Unknown id: 0.
        assert_eq!(f.compute_score_at("q", "zz", now), 0.0);
    }

    #[test]
    fn sub_query_match_reduces_score() {
        let mut f = Frecency::new(FrecencyOptions::default());
        let now = 100 * HOUR;
        // Selection made under the longer query 'design team'.
        f.save(SaveParams {
            search_query: Some("design team"),
            selected_id: "a",
            date_selection: Some(now - HOUR),
        });
        // Searching 'de tea' hits via sub-query with weight 0.7.
        assert!((f.compute_score_at("de tea", "a", now) - 0.7 * 100.0).abs() < 1e-9);
    }

    #[test]
    fn is_sub_query_matches_word_prefixes() {
        assert!(is_sub_query("de tea", "design team"));
        assert!(is_sub_query("team desi", "design team"));
        assert!(!is_sub_query("de zz", "design team"));
        assert!(!is_sub_query("design", ""));
    }

    #[test]
    fn sort_keeps_scored_first_descending() {
        let mut f = Frecency::new(FrecencyOptions::default());
        let now = 100 * HOUR;
        // 4h old scores 80, 1h old scores 100, so the sort is unambiguous.
        f.save(SaveParams {
            search_query: Some("q"),
            selected_id: "old",
            date_selection: Some(now - 4 * HOUR),
        });
        f.save(SaveParams {
            search_query: Some("q"),
            selected_id: "new",
            date_selection: Some(now - HOUR),
        });

        let got = f.sort_at("q", &["unscored", "old", "new"], now);
        let ids: Vec<_> = got.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["new", "old", "unscored"]);
        assert!(got[0].1 > got[1].1);
        assert_eq!(got[2].1, 0.0);
    }

    #[test]
    fn cleanup_evicts_least_recently_used_id() {
        let mut f = Frecency::new(FrecencyOptions {
            recent_selections_limit: 2,
            ..Default::default()
        });
        f.save(SaveParams {
            search_query: Some("q1"),
            selected_id: "a",
            date_selection: Some(1),
        });
        f.save(SaveParams {
            search_query: Some("q2"),
            selected_id: "b",
            date_selection: Some(2),
        });
        // Re-selecting 'a' moves it to the front, making 'b' the LRU id.
        f.save(SaveParams {
            search_query: None,
            selected_id: "a",
            date_selection: Some(3),
        });
        f.save(SaveParams {
            search_query: None,
            selected_id: "c",
            date_selection: Some(4),
        });

        assert_eq!(f.data.recent_selections, vec!["c", "a"]);
        assert!(!f.data.selections.contains_key("b"));
        // 'b' removed from the query map too (empty query list dropped).
        assert!(!f.data.queries.contains_key("q2"));
        assert!(f.data.selections.contains_key("a"));
    }
}
