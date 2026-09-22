//! Activity consumer (port of `activity/`): push entries and query them
//! through a host-registered provider. TS returns an rxjs `Observable` that
//! flattens the provider's `Promise<Observable<ActivityEntry[]>>`; the port
//! models the provider side as an mpsc sender (the host streams matching
//! batches) and the app side as a [`ReceiverStream`] of entry batches.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::common::{ConsumerId, ProviderMissing, ProviderSlot};

/// One activity log entry (`activity.ActivityEntry`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
    /// Type of activity; TS defaults to `''` when `push` gets no type.
    #[serde(default)]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_data: Option<Value>,
    /// Milliseconds since the UNIX epoch (`Date.now()`).
    pub created_at: i64,
}

/// Filter value of a query scope (`activity.ScopeFilter`).
pub type ScopeFilter = Option<Vec<String>>;

/// Inclusion/exclusion filters keyed by scope (`activity.QueryArgsScope`).
/// TS allows partial scopes; `None` fields match the TS `null` slots.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryArgsScope {
    #[serde(default)]
    pub resource_ids: ScopeFilter,
    #[serde(default)]
    pub manifest_urls: ScopeFilter,
    #[serde(default)]
    pub types: ScopeFilter,
}

/// Query parameters (`activity.QueryArgs`). `query` applies the TS defaults
/// for missing fields before calling the provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryArgs {
    /// Only `'createdAt'` exists in TS.
    pub order_by: String,
    pub ascending: bool,
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_by_date: Option<i64>,
    pub global: bool,
    pub where_: QueryArgsScope,
    pub where_not: QueryArgsScope,
}

impl Default for QueryArgs {
    /// The defaults `ActivityConsumer.query` spreads in TS.
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

/// Identifier of a stored entry (`{ activityEntryId }`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntryId {
    pub activity_entry_id: String,
}

/// Provider side of a query: the host streams batches of matching entries
/// and returns the receiving end (`ActivityProviderInterface.query`).
pub type QueryBatches = mpsc::Receiver<Vec<ActivityEntry>>;

/// Host side of the activity API (`activity.ActivityProviderInterface`).
#[async_trait]
pub trait ActivityProvider: Send + Sync {
    async fn push(
        &self,
        consumer_key: &str,
        entry: ActivityEntry,
    ) -> Result<ActivityEntryId, ProviderMissing>;
    async fn query(
        &self,
        consumer_key: &str,
        args: &QueryArgs,
    ) -> Result<QueryBatches, ProviderMissing>;
}

/// Activity consumer: `push` writes through the provider, `query` returns a
/// stream of entry batches (TS `Observable<ActivityEntry[]>`).
pub struct ActivityConsumer {
    pub id: ConsumerId,
    provider: ProviderSlot<dyn ActivityProvider>,
}

impl ActivityConsumer {
    pub fn new(id: ConsumerId) -> Self {
        ActivityConsumer {
            id,
            provider: ProviderSlot::new(),
        }
    }

    pub fn namespace(&self) -> &'static str {
        "activity"
    }

    /// Register the host provider (`setProviderInterface`).
    pub async fn set_provider(&self, provider: Arc<dyn ActivityProvider>) {
        self.provider.set(provider).await;
    }

    pub async fn clear_provider(&self) {
        self.provider.clear().await;
    }

    /// Push an activity entry. TS signature is
    /// `push(resourceId, extraData?, type?, manifestURL?)` with `createdAt`
    /// stamped at call time and `type` defaulting to `''`.
    pub async fn push(
        &self,
        resource_id: &str,
        extra_data: Option<Value>,
        type_: Option<&str>,
        manifest_url: Option<&str>,
    ) -> Result<ActivityEntryId, ProviderMissing> {
        let entry = ActivityEntry {
            resource_id: resource_id.to_owned(),
            manifest_url: manifest_url.map(str::to_owned),
            type_: type_.unwrap_or_default().to_owned(),
            extra_data,
            created_at: now_ms(),
        };
        self.provider
            .require()
            .await?
            .push(self.id.as_str(), entry)
            .await
    }

    /// Query activity entries; missing fields fall back to the TS defaults
    /// (`orderBy: 'createdAt'`, `ascending: false`, `limit: 1`, `global:
    /// false`, null filters). Returns a stream of entry batches.
    pub async fn query(
        &self,
        args: PartialQueryArgs,
    ) -> Result<ReceiverStream<Vec<ActivityEntry>>, ProviderMissing> {
        let full = args.into_full();
        let rx = self
            .provider
            .require()
            .await?
            .query(self.id.as_str(), &full)
            .await?;
        Ok(ReceiverStream::new(rx))
    }
}

/// User-supplied query overrides (`Partial<QueryArgs>` in TS): `None`
/// fields take the [`QueryArgs::default`] value.
#[derive(Debug, Clone, Default)]
pub struct PartialQueryArgs {
    pub order_by: Option<String>,
    pub ascending: Option<bool>,
    pub limit: Option<u32>,
    pub limit_by_date: Option<i64>,
    pub global: Option<bool>,
    pub where_: Option<QueryArgsScope>,
    pub where_not: Option<QueryArgsScope>,
}

impl PartialQueryArgs {
    fn into_full(self) -> QueryArgs {
        let d = QueryArgs::default();
        QueryArgs {
            order_by: self.order_by.unwrap_or(d.order_by),
            ascending: self.ascending.unwrap_or(d.ascending),
            limit: self.limit.unwrap_or(d.limit),
            limit_by_date: self.limit_by_date.or(d.limit_by_date),
            global: self.global.unwrap_or(d.global),
            where_: self.where_.unwrap_or(d.where_),
            where_not: self.where_not.unwrap_or(d.where_not),
        }
    }
}

impl From<QueryArgs> for PartialQueryArgs {
    fn from(full: QueryArgs) -> Self {
        PartialQueryArgs {
            order_by: Some(full.order_by),
            ascending: Some(full.ascending),
            limit: Some(full.limit),
            limit_by_date: full.limit_by_date,
            global: Some(full.global),
            where_: Some(full.where_),
            where_not: Some(full.where_not),
        }
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
    use tokio_stream::StreamExt;

    /// In-memory provider: records pushes, answers queries from stored data.
    #[derive(Default)]
    struct MemProvider {
        entries: tokio::sync::RwLock<Vec<(String, ActivityEntry)>>,
    }

    #[async_trait]
    impl ActivityProvider for MemProvider {
        async fn push(
            &self,
            consumer_key: &str,
            entry: ActivityEntry,
        ) -> Result<ActivityEntryId, ProviderMissing> {
            let id = format!("act-{}", self.entries.read().await.len() + 1);
            self.entries
                .write()
                .await
                .push((consumer_key.into(), entry));
            Ok(ActivityEntryId {
                activity_entry_id: id,
            })
        }

        async fn query(
            &self,
            consumer_key: &str,
            args: &QueryArgs,
        ) -> Result<QueryBatches, ProviderMissing> {
            let mut matching: Vec<ActivityEntry> = self
                .entries
                .read()
                .await
                .iter()
                .filter(|(k, _)| args.global || k == consumer_key)
                .map(|(_, e)| e.clone())
                .filter(|e| scope_matches(&args.where_, e, true))
                .filter(|e| scope_matches(&args.where_not, e, false))
                .collect();
            if args.order_by == "createdAt" {
                matching.sort_by_key(|e| e.created_at);
                if !args.ascending {
                    matching.reverse();
                }
            }
            matching.truncate(args.limit as usize);
            let (tx, rx) = mpsc::channel(1);
            if !matching.is_empty() {
                tx.send(matching).await.ok();
            }
            Ok(rx)
        }
    }

    fn scope_matches(scope: &QueryArgsScope, e: &ActivityEntry, want_match: bool) -> bool {
        let check = |f: &ScopeFilter, v: &str| match f {
            None => true, // null filter: no opinion
            Some(list) => list.iter().any(|s| s == v) == want_match,
        };
        check(&scope.resource_ids, &e.resource_id)
            && check(
                &scope.manifest_urls,
                e.manifest_url.as_deref().unwrap_or_default(),
            )
            && check(&scope.types, &e.type_)
    }

    #[tokio::test]
    async fn push_requires_provider_then_stamps_entry() {
        let c = ActivityConsumer::new(ConsumerId::new("app1"));
        assert!(matches!(
            c.push("r1", None, None, None).await,
            Err(ProviderMissing)
        ));
        assert!(c.query(PartialQueryArgs::default()).await.is_err());

        c.set_provider(Arc::new(MemProvider::default())).await;
        let before = now_ms();
        let id = c
            .push(
                "r1",
                Some(serde_json::json!({"k": 1})),
                None,
                Some("https://m"),
            )
            .await
            .unwrap();
        assert!(!id.activity_entry_id.is_empty());
        let stream = c.query(PartialQueryArgs::default()).await.unwrap();
        let batches: Vec<Vec<ActivityEntry>> = stream.collect().await;
        assert_eq!(batches.len(), 1);
        let e = &batches[0][0];
        assert_eq!(e.resource_id, "r1");
        assert_eq!(e.type_, ""); // TS default
        assert_eq!(e.manifest_url.as_deref(), Some("https://m"));
        assert!(e.created_at >= before);
    }

    #[tokio::test]
    async fn query_defaults_match_ts() {
        let p = Arc::new(MemProvider::default());
        let c = ActivityConsumer::new(ConsumerId::new("app1"));
        c.set_provider(p.clone()).await;
        c.push("r1", None, Some("open"), None).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        c.push("r2", None, Some("open"), None).await.unwrap();

        // Defaults: limit 1, descending by createdAt -> newest only.
        let stream = c.query(PartialQueryArgs::default()).await.unwrap();
        let batches: Vec<_> = stream.collect().await;
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[0][0].resource_id, "r2");

        // Overrides: limit 2, ascending -> oldest first.
        let stream = c
            .query(PartialQueryArgs {
                limit: Some(2),
                ascending: Some(true),
                ..Default::default()
            })
            .await
            .unwrap();
        let batches: Vec<_> = stream.collect().await;
        assert_eq!(batches[0][0].resource_id, "r1");
        assert_eq!(batches[0].len(), 2);
    }

    #[tokio::test]
    async fn query_filters_scopes() {
        let p = Arc::new(MemProvider::default());
        let c = ActivityConsumer::new(ConsumerId::new("app1"));
        c.set_provider(p.clone()).await;
        c.push("r1", None, Some("open"), None).await.unwrap();
        c.push("r2", None, Some("close"), None).await.unwrap();

        let stream = c
            .query(PartialQueryArgs {
                limit: Some(10),
                where_: Some(QueryArgsScope {
                    types: Some(vec!["open".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap();
        let batches: Vec<_> = stream.collect().await;
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[0][0].type_, "open");

        // whereNot excludes.
        let stream = c
            .query(PartialQueryArgs {
                limit: Some(10),
                where_not: Some(QueryArgsScope {
                    types: Some(vec!["open".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap();
        let batches: Vec<_> = stream.collect().await;
        assert_eq!(batches[0][0].type_, "close");
    }

    #[tokio::test]
    async fn consumer_namespaces_are_isolated() {
        let p = Arc::new(MemProvider::default());
        let a = ActivityConsumer::new(ConsumerId::new("a"));
        let b = ActivityConsumer::new(ConsumerId::new("b"));
        a.set_provider(p.clone()).await;
        b.set_provider(p).await;
        a.push("r1", None, None, None).await.unwrap();
        let stream = b
            .query(PartialQueryArgs {
                limit: Some(10),
                ..Default::default()
            })
            .await
            .unwrap();
        let batches: Vec<_> = stream.collect().await;
        assert!(batches.is_empty()); // plugin activity, not global
        let stream = a
            .query(PartialQueryArgs {
                limit: Some(10),
                ..Default::default()
            })
            .await
            .unwrap();
        let batches: Vec<_> = stream.collect().await;
        assert_eq!(batches[0].len(), 1);
    }
}
