//! Activity store backend (port of `packages/app/src/activity` +
//! `packages/app/src/sdk/activity`): sqlx/SQLite schema, `push`/`query`
//! persistence, plus the pure query logic ported from
//! `createActivityObservable.ts` (merge of DB history with the live global
//! activity feed) in [`merge`], and the frecency engine port of
//! `@getstation/frecency` (the quick-switcher scoring fed by activity
//! selections) in [`frecency`].
//!
//! Not ported in this slice: the GraphQL/sequelize model file (`model.ts`,
//! superseded by the SQL schema below), the redux saga that dispatches
//! pushes on tab switches (`sagas.ts` — host shell wiring), and the bang
//! search resolvers (`resolvers.ts` — later search slice).

pub mod frecency;
pub mod merge;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, FromRow, QueryBuilder, Sqlite, SqlitePool};

/// `activity` table + the three indexes of `model.ts`.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pluginId TEXT NOT NULL,
    resourceId TEXT NOT NULL,
    manifestURL TEXT NULL,
    type TEXT NOT NULL DEFAULT '',
    extraData TEXT NULL,
    createdAt INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS activity_pluginId ON activity (pluginId);
CREATE INDEX IF NOT EXISTS activity_resourceId ON activity (resourceId);
CREATE INDEX IF NOT EXISTS activity_type ON activity (type);
"#;

/// One activity log entry (`activity.ActivityEntry`; `createdAt` is
/// milliseconds since the UNIX epoch, like `Date.now()`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
    /// TS defaults to `''` when `push` gets no type.
    #[serde(default)]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_data: Option<Value>,
    pub created_at: i64,
}

/// Stored row projection (`SerializedActivityEntry` minus the columns only
/// used in WHERE clauses). `FromRow` maps the camelCase columns.
#[derive(Debug, Clone, FromRow)]
struct ActivityRow {
    #[sqlx(rename = "resourceId")]
    resource_id: String,
    #[sqlx(rename = "manifestURL")]
    manifest_url: Option<String>,
    #[sqlx(rename = "type")]
    type_: String,
    #[sqlx(rename = "extraData")]
    extra_data: Option<String>,
    #[sqlx(rename = "createdAt")]
    created_at: i64,
}

impl From<ActivityRow> for ActivityEntry {
    fn from(r: ActivityRow) -> Self {
        ActivityEntry {
            resource_id: r.resource_id,
            manifest_url: r.manifest_url,
            type_: r.type_,
            extra_data: r
                .extra_data
                .filter(|s| !s.is_empty())
                .and_then(|s| serde_json::from_str(&s).ok()),
            created_at: r.created_at,
        }
    }
}

/// Inclusion/exclusion filters keyed by scope (`activity.QueryArgsScope`).
/// `None` = TS `null` slot (no filter).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryArgsScope {
    #[serde(default)]
    pub resource_ids: Option<Vec<String>>,
    #[serde(default)]
    pub manifest_urls: Option<Vec<String>>,
    #[serde(default)]
    pub types: Option<Vec<String>>,
}

/// Query parameters (`activity.QueryArgs`). `Default` spreads the TS
/// defaults applied by `ActivityConsumer.query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryArgs {
    /// Only `'createdAt'` exists in TS.
    pub order_by: String,
    pub ascending: bool,
    pub limit: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_by_date: Option<i64>,
    pub global: bool,
    pub where_: QueryArgsScope,
    pub where_not: QueryArgsScope,
}

impl Default for QueryArgs {
    fn default() -> Self {
        QueryArgs {
            order_by: "createdAt".into(),
            ascending: false,
            limit: 1,
            limit_by_date: None,
            global: false,
            where_: QueryArgsScope::default(),
            where_not: QueryArgsScope::default(),
        }
    }
}

/// Result of a push (`{ activityEntryId }`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntryId {
    pub activity_entry_id: String,
}

/// SQLite-backed activity store (`ActivityProvider` + sequelize model).
#[derive(Debug, Clone)]
pub struct ActivityStore {
    pool: SqlitePool,
}

impl ActivityStore {
    /// Open (or create) a store at `url` and ensure the schema exists.
    /// Use `sqlite::memory:` for tests — each in-memory DB is private to
    /// its connection, so the pool is pinned to one connection to make all
    /// operations see the same database.
    pub async fn open(url: &str) -> sqlx::Result<Self> {
        let options: SqliteConnectOptions =
            url.parse::<SqliteConnectOptions>()?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Self::with_pool(pool).await
    }

    /// In-memory store (one shared connection).
    pub async fn in_memory() -> sqlx::Result<Self> {
        Self::open("sqlite::memory:").await
    }

    /// Wrap an existing pool after ensuring the schema.
    pub async fn with_pool(pool: SqlitePool) -> sqlx::Result<Self> {
        pool.execute(SCHEMA).await?;
        Ok(ActivityStore { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert one entry (`pushDb`): stamps `created_at` when zero, stores
    /// `extraData` as JSON text, returns the row id.
    pub async fn push(
        &self,
        plugin_id: &str,
        entry: ActivityEntry,
    ) -> sqlx::Result<ActivityEntryId> {
        let created_at = if entry.created_at == 0 {
            now_ms()
        } else {
            entry.created_at
        };
        let extra_data = entry
            .extra_data
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default();
        let row = sqlx::query_scalar::<_, i64>(
            "INSERT INTO activity (pluginId, resourceId, manifestURL, type, extraData, createdAt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             RETURNING id",
        )
        .bind(plugin_id)
        .bind(&entry.resource_id)
        .bind(&entry.manifest_url)
        .bind(&entry.type_)
        .bind(&extra_data)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(ActivityEntryId {
            activity_entry_id: row.to_string(),
        })
    }

    /// Query stored entries (`queryDb` + `deserializeActivityEntry`).
    pub async fn query(
        &self,
        consumer_id: &str,
        args: &QueryArgs,
    ) -> sqlx::Result<Vec<ActivityEntry>> {
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT resourceId, manifestURL, type, extraData, createdAt FROM activity");
        build_where(&mut qb, consumer_id, args);
        // Only 'createdAt' is orderable in TS; anything else falls back to it.
        qb.push(" ORDER BY createdAt");
        qb.push(if args.ascending { " ASC" } else { " DESC" });
        let limit = args.limit.max(0);
        qb.push(" LIMIT ").push_bind(limit);
        let rows: Vec<ActivityRow> = qb.build_query_as().fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Delete entries older than `time_to_keep_data` milliseconds
    /// (`cleanOldActivity` in `sagas.ts`; kept = `now - timeToKeepData`).
    pub async fn clean_old_activity(&self, time_to_keep_data_ms: i64) -> sqlx::Result<u64> {
        let timestamp_limit = now_ms() - time_to_keep_data_ms;
        let res = sqlx::query("DELETE FROM activity WHERE createdAt < ?1")
            .bind(timestamp_limit)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

/// Append the WHERE clause mirroring `getSerializedQueryParams`:
/// inclusion scopes AND-ed, then `NOT (any whereNot scope OR-ed)`.
fn build_where(qb: &mut QueryBuilder<Sqlite>, consumer_id: &str, args: &QueryArgs) {
    let mut clauses: Vec<String> = Vec::new();

    if let Some(ids) = filter_list(&args.where_.resource_ids) {
        clauses.push(in_clause("resourceId", &ids));
    }
    if let Some(urls) = filter_list(&args.where_.manifest_urls) {
        clauses.push(in_clause("manifestURL", &urls));
    }
    if let Some(types) = filter_list(&args.where_.types) {
        clauses.push(in_clause("type", &types));
    }

    // whereNot: TS builds Op.not over the OR of the three sub-conditions —
    // an entry is excluded when it matches ANY whereNot scope.
    let not_parts: Vec<String> = [
        filter_list(&args.where_not.resource_ids).map(|v| in_clause("resourceId", &v)),
        filter_list(&args.where_not.manifest_urls).map(|v| in_clause("manifestURL", &v)),
        filter_list(&args.where_not.types).map(|v| in_clause("type", &v)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !not_parts.is_empty() {
        clauses.push(format!("NOT ({})", not_parts.join(" OR ")));
    }

    if !args.global {
        clauses.push("pluginId = ".to_owned() + &quote(consumer_id));
    }
    if let Some(date) = args.limit_by_date {
        clauses.push(format!("createdAt >= {date}"));
    }

    if !clauses.is_empty() {
        qb.push(" WHERE ").push(clauses.join(" AND "));
    }
}

/// `['a','b']` -> `(a,b)` literal list — `sqlite` has no array bind.
fn in_clause(col: &str, values: &[String]) -> String {
    let list = values
        .iter()
        .map(|v| quote(v))
        .collect::<Vec<_>>()
        .join(",");
    format!("{col} IN ({list})")
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn filter_list(f: &Option<Vec<String>>) -> Option<Vec<String>> {
    match f {
        None => None,
        Some(list) if list.is_empty() => None, // TS [] matches nothing; ponytail: treated as absent
        Some(list) => Some(list.clone()),
    }
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

    fn entry(resource_id: &str, type_: &str, created_at: i64) -> ActivityEntry {
        ActivityEntry {
            resource_id: resource_id.into(),
            manifest_url: None,
            type_: type_.into(),
            extra_data: None,
            created_at,
        }
    }

    #[tokio::test]
    async fn push_stamps_created_at_and_serializes_extra_data() {
        let store = ActivityStore::in_memory().await.unwrap();
        let id = store
            .push(
                "app1",
                ActivityEntry {
                    resource_id: "r1".into(),
                    manifest_url: Some("https://m".into()),
                    type_: "nav-to-tab".into(),
                    extra_data: Some(serde_json::json!({"tabId": "t1"})),
                    created_at: 0, // Date.now() stamp
                },
            )
            .await
            .unwrap();
        assert!(!id.activity_entry_id.is_empty());

        let got = store
            .query("app1", &QueryArgs { limit: 10, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].resource_id, "r1");
        assert_eq!(got[0].type_, "nav-to-tab");
        assert_eq!(got[0].manifest_url.as_deref(), Some("https://m"));
        assert_eq!(got[0].extra_data, Some(serde_json::json!({"tabId": "t1"})));
        assert!(got[0].created_at > 0);
    }

    #[tokio::test]
    async fn query_defaults_match_ts() {
        let store = ActivityStore::in_memory().await.unwrap();
        store.push("app1", entry("r1", "", 100)).await.unwrap();
        store.push("app1", entry("r2", "", 200)).await.unwrap();
        store.push("app1", entry("r3", "", 300)).await.unwrap();

        // Defaults: limit 1, descending -> newest only.
        let got = store.query("app1", &QueryArgs::default()).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].resource_id, "r3");

        // limit 2 ascending -> oldest first.
        let got = store
            .query(
                "app1",
                &QueryArgs { limit: 2, ascending: true, ..Default::default() },
            )
            .await
            .unwrap();
        assert_eq!(
            got.iter().map(|e| e.resource_id.as_str()).collect::<Vec<_>>(),
            vec!["r1", "r2"]
        );
    }

    #[tokio::test]
    async fn query_filters_and_scopes() {
        let store = ActivityStore::in_memory().await.unwrap();
        store.push("app1", entry("r1", "open", 100)).await.unwrap();
        store.push("app1", entry("r2", "close", 200)).await.unwrap();
        store.push("app2", entry("r3", "open", 300)).await.unwrap();

        let where_types = QueryArgs {
            limit: 10,
            where_: QueryArgsScope { types: Some(vec!["open".into()]), ..Default::default() },
            ..Default::default()
        };
        let got = store.query("app1", &where_types).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].resource_id, "r1");

        let where_not = QueryArgs {
            limit: 10,
            where_not: QueryArgsScope { types: Some(vec!["open".into()]), ..Default::default() },
            ..Default::default()
        };
        let got = store.query("app1", &where_not).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].resource_id, "r2");

        // global: true sees app2's rows too.
        let global = QueryArgs { limit: 10, global: true, ..Default::default() };
        let got = store.query("app1", &global).await.unwrap();
        assert_eq!(got.len(), 3);

        // limitByDate keeps only entries at/after the timestamp.
        let by_date = QueryArgs { limit: 10, limit_by_date: Some(150), ..Default::default() };
        let got = store.query("app1", &by_date).await.unwrap();
        assert_eq!(got.iter().map(|e| e.resource_id.as_str()).collect::<Vec<_>>(), vec!["r2"]);
    }

    #[tokio::test]
    async fn consumer_namespaces_are_isolated() {
        let store = ActivityStore::in_memory().await.unwrap();
        store.push("a", entry("r1", "", 100)).await.unwrap();

        let got = store
            .query("b", &QueryArgs { limit: 10, ..Default::default() })
            .await
            .unwrap();
        assert!(got.is_empty());
        let got = store
            .query("a", &QueryArgs { limit: 10, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
    }

    #[tokio::test]
    async fn clean_old_activity_deletes_older_than_cutoff() {
        let store = ActivityStore::in_memory().await.unwrap();
        store.push("app1", entry("r1", "", now_ms() - 10_000)).await.unwrap();
        store.push("app1", entry("r2", "", now_ms())).await.unwrap();

        let deleted = store.clean_old_activity(5_000).await.unwrap();
        assert_eq!(deleted, 1);
        let got = store
            .query("app1", &QueryArgs { limit: 10, ..Default::default() })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].resource_id, "r2");
    }

    #[tokio::test]
    async fn store_reopens_with_schema_already_present() {
        // with_pool runs CREATE ... IF NOT EXISTS twice -> no error.
        let store = ActivityStore::in_memory().await.unwrap();
        let again = ActivityStore::with_pool(store.pool().clone()).await;
        assert!(again.is_ok());
    }
}
