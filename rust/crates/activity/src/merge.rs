//! Pure query-side merge of activity store results with the live global
//! activity feed (port of `createActivityObservable.ts`).
//!
//! TS pipes the DB query results and a `Subject<GlobalActivityEntry>`
//! through filter → `scan(orderAndLimit)` → `distinctUntilChanged`; the
//! port models the stream as successive live batches and exposes the same
//! reducer so each batch can be folded incrementally.

use crate::{ActivityEntry, QueryArgs};

/// Entries tagged with the producer's plugin id (`GlobalActivityEntry`).
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalActivityEntry {
    pub entry: ActivityEntry,
    pub plugin_id: String,
}

impl GlobalActivityEntry {
    pub fn new(entry: ActivityEntry, plugin_id: impl Into<String>) -> Self {
        GlobalActivityEntry { entry, plugin_id: plugin_id.into() }
    }
}

/// `createActivityFilter`: keep entries matching the query scopes.
/// `global: false` keeps only the querying consumer's entries.
pub fn activity_filter(
    entries: Vec<GlobalActivityEntry>,
    plugin_id: &str,
    args: &QueryArgs,
) -> Vec<GlobalActivityEntry> {
    let keep = |g: &GlobalActivityEntry| {
        let e = &g.entry;
        (args.global || g.plugin_id == plugin_id)
            && scope_keeps(&args.where_.resource_ids, Some(e.resource_id.as_str()), true)
            && scope_keeps(&args.where_.manifest_urls, e.manifest_url.as_deref(), true)
            && scope_keeps(&args.where_.types, Some(e.type_.as_str()), true)
            && scope_keeps(&args.where_not.resource_ids, Some(e.resource_id.as_str()), false)
            && scope_keeps(&args.where_not.manifest_urls, e.manifest_url.as_deref(), false)
            && scope_keeps(&args.where_not.types, Some(e.type_.as_str()), false)
    };
    entries.into_iter().filter(|g| keep(g)).collect()
}

/// One scope test (`filterWhere*` / `filterWhereNot*`): `None` passes,
/// `Some(list)` membership decides, `want` flips for whereNot. TS also
/// accepts a bare string filter; the SDK types only declare lists, so only
/// lists are modeled.
fn scope_keeps(filter: &Option<Vec<String>>, value: Option<&str>, want: bool) -> bool {
    match filter {
        None => true,
        Some(list) => list.iter().any(|s| Some(s.as_str()) == value) == want,
    }
}

/// `orderAndLimit` reducer state: entries kept ordered and truncated to
/// `limit` (TS inserts each new entry before the first entry with a
/// strictly bigger sort key; ties keep the new entry last).
#[derive(Debug, Clone, Default)]
pub struct OrderedActivity {
    entries: Vec<GlobalActivityEntry>,
    limit: usize,
    ascending: bool,
}

impl OrderedActivity {
    pub fn new(limit: i32) -> Self {
        OrderedActivity::with_order(limit, false)
    }

    /// `OrderedActivity::new` with an explicit sort direction.
    pub fn with_order(limit: i32, ascending: bool) -> Self {
        OrderedActivity { entries: Vec::new(), limit: limit.max(0) as usize, ascending }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Fold one batch of already-filtered entries (`scan(activityReducer)`).
    /// Ascending inserts before the first strictly-bigger entry; descending
    /// inserts after the last one (TS `findLastIndex` + `inc`, where the
    /// no-match case yields `list.length` via ramda `insert`'s clamp).
    pub fn fold(&mut self, batch: Vec<GlobalActivityEntry>) -> &Self {
        for new_entry in batch {
            let key = |g: &GlobalActivityEntry| g.entry.created_at;
            let idx = if self.ascending {
                self.entries
                    .iter()
                    .position(|e| key(e) > key(&new_entry))
                    .unwrap_or(self.entries.len())
            } else {
                // findLastIndex + inc: no bigger entry -> 0 (front).
                self.entries
                    .iter()
                    .rposition(|e| key(e) > key(&new_entry))
                    .map(|i| i + 1)
                    .unwrap_or(0)
            };
            self.entries.insert(idx, new_entry);
            if self.entries.len() > self.limit {
                self.entries.truncate(self.limit);
            }
        }
        self
    }

    /// `convertGlobalActivity`: drop the plugin id tag.
    pub fn into_entries(self) -> Vec<ActivityEntry> {
        self.entries.into_iter().map(|g| g.entry).collect()
    }
}

/// Full merge for one query (`createActivityObservable` minus the rxjs
/// plumbing): filter the DB batch, seed the ordered window, then fold each
/// live batch as it arrives. Returns the seeded reducer and the seed
/// batch so a caller can drive live batches through [`OrderedActivity::fold`].
pub fn create_activity_merge(
    plugin_id: &str,
    args: &QueryArgs,
    db_results: Vec<GlobalActivityEntry>,
) -> OrderedActivity {
    let mut ordered = OrderedActivity::with_order(args.limit, args.ascending);
    let filtered = activity_filter(db_results, plugin_id, args);
    // One fold of singleton batches matches TS `newEntries.map(e => [e])`.
    ordered.fold(filtered);
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryArgsScope;

    fn global(resource_id: &str, created_at: i64, plugin_id: &str) -> GlobalActivityEntry {
        GlobalActivityEntry::new(
            ActivityEntry {
                resource_id: resource_id.into(),
                manifest_url: None,
                type_: String::new(),
                extra_data: None,
                created_at,
            },
            plugin_id,
        )
    }

    fn args(limit: i32, ascending: bool) -> QueryArgs {
        QueryArgs { limit, ascending, ..Default::default() }
    }

    #[test]
    fn filter_respects_scopes_and_plugin() {
        let mut e = global("r1", 1, "app1");
        e.entry.type_ = "open".into();
        let mut other = global("r2", 2, "app2");
        other.entry.type_ = "close".into();

        // Plugin isolation: only the querying consumer's entries.
        let a = QueryArgs { limit: 10, ..Default::default() };
        let got = activity_filter(vec![e.clone(), other.clone()], "app1", &a);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].entry.resource_id, "r1");

        // global: true keeps both.
        let a = QueryArgs { limit: 10, global: true, ..Default::default() };
        assert_eq!(activity_filter(vec![e.clone(), other.clone()], "app1", &a).len(), 2);

        // where types
        let a = QueryArgs {
            limit: 10,
            global: true,
            where_: QueryArgsScope { types: Some(vec!["close".into()]), ..Default::default() },
            ..Default::default()
        };
        let got = activity_filter(vec![e.clone(), other.clone()], "app1", &a);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].entry.resource_id, "r2");

        // whereNot manifestURLs: None manifest never matches, so kept.
        let a = QueryArgs {
            limit: 10,
            where_not: QueryArgsScope {
                manifest_urls: Some(vec!["https://m".into()]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(activity_filter(vec![e.clone(), other], "app1", &a).len(), 1);
    }

    #[test]
    fn order_and_limit_sorts_descending_and_truncates() {
        let mut ordered = OrderedActivity::new(2);
        ordered.fold(vec![global("r1", 100, "a"), global("r2", 200, "a")]);
        assert_eq!(
            ordered.clone().into_entries().iter().map(|e| e.resource_id.as_str()).collect::<Vec<_>>(),
            vec!["r2", "r1"]
        );

        // New oldest entry falls out at limit 2.
        ordered.fold(vec![global("r3", 50, "a")]);
        assert_eq!(
            ordered.clone().into_entries().iter().map(|e| e.resource_id.as_str()).collect::<Vec<_>>(),
            vec!["r2", "r1"]
        );

        // New newest entry takes the head.
        ordered.fold(vec![global("r4", 300, "a")]);
        assert_eq!(
            ordered.into_entries().iter().map(|e| e.resource_id.as_str()).collect::<Vec<_>>(),
            vec!["r4", "r2"]
        );
    }

    #[test]
    fn merge_seeds_from_db_then_folds_live_batches() {
        let db = vec![global("r1", 100, "app1"), global("r2", 200, "app1")];
        let mut ordered = create_activity_merge("app1", &args(3, false), db);
        assert_eq!(ordered.len(), 2);

        // Live batch from another plugin filtered out.
        ordered.fold(activity_filter(vec![global("r9", 900, "app2")], "app1", &args(3, false)));
        assert_eq!(ordered.len(), 2);

        ordered.fold(activity_filter(vec![global("r3", 300, "app1")], "app1", &args(3, false)));
        let got = ordered.into_entries();
        assert_eq!(
            got.iter().map(|e| e.resource_id.as_str()).collect::<Vec<_>>(),
            vec!["r3", "r2", "r1"]
        );
    }

    #[test]
    fn empty_limit_yields_nothing() {
        let mut ordered = OrderedActivity::new(0);
        ordered.fold(vec![global("r1", 100, "a")]);
        assert!(ordered.is_empty());
    }
}
