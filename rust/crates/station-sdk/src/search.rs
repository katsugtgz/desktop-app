//! Search consumer (port of `search/`): a query `BehaviorSubject` the host
//! drives and a results `BehaviorSubject` the app drives. Pure state, no
//! provider interface (the TS `SearchConsumer` only holds the two subjects).

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

use crate::common::ConsumerId;

/// Callback fired when the user picks a result (`SearchResultItem.onSelect`;
/// TS also allows async callbacks, the host decides how to await them).
pub type OnSelect = Arc<dyn Fn() + Send + Sync>;

/// One search result (`search.SearchResultItem`).
#[derive(Clone, Serialize, Deserialize)]
pub struct SearchResultItem {
    pub resource_id: String,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_search_string: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub img_url: String,
    /// JS callback; never crosses the wire, so it is skipped by serde.
    #[serde(skip)]
    pub on_select: Option<OnSelect>,
}

impl fmt::Debug for SearchResultItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchResultItem")
            .field("resource_id", &self.resource_id)
            .field("category", &self.category)
            .field("additional_search_string", &self.additional_search_string)
            .field("manifest_url", &self.manifest_url)
            .field("label", &self.label)
            .field("context", &self.context)
            .field("url", &self.url)
            .field("img_url", &self.img_url)
            .field("on_select", &self.on_select.as_ref().map(|_| "<fn>"))
            .finish()
    }
}

/// Callbacks compare by pointer; data fields by value.
impl PartialEq for SearchResultItem {
    fn eq(&self, other: &Self) -> bool {
        self.resource_id == other.resource_id
            && self.category == other.category
            && self.additional_search_string == other.additional_search_string
            && self.manifest_url == other.manifest_url
            && self.label == other.label
            && self.context == other.context
            && self.url == other.url
            && self.img_url == other.img_url
            && match (&self.on_select, &other.on_select) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

/// Result batch pushed by the app (`search.SearchResultWrapper`); `loading`
/// names the category still being queried.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResultWrapper {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<SearchResultItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loading: Option<String>,
}

/// Query string update (`search.SearchQuery`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SearchQuery {
    pub value: String,
}

/// Search consumer: `query`/`results` as watch channels (latest value plus
/// change notifications, matching `BehaviorSubject` semantics).
pub struct SearchConsumer {
    pub id: ConsumerId,
    query_tx: watch::Sender<SearchQuery>,
    results_tx: watch::Sender<SearchResultWrapper>,
}

impl SearchConsumer {
    pub fn new(id: ConsumerId) -> Self {
        let (query_tx, _q) = watch::channel(SearchQuery::default());
        let (results_tx, _r) = watch::channel(SearchResultWrapper::default());
        SearchConsumer {
            id,
            query_tx,
            results_tx,
        }
    }

    pub fn namespace(&self) -> &'static str {
        "search"
    }

    /// Push a query update (`query.next(...)`; host side).
    pub fn publish_query(&self, query: SearchQuery) {
        // No subscribers is a normal state, not an error.
        let _ = self.query_tx.send(query);
    }

    /// Subscribe to query updates (starts at the current value).
    pub fn subscribe_query(&self) -> watch::Receiver<SearchQuery> {
        self.query_tx.subscribe()
    }

    /// Query updates as a stream (`query.subscribe(...)` over tokio-stream).
    pub fn query_stream(&self) -> WatchStream<SearchQuery> {
        WatchStream::new(self.query_tx.subscribe())
    }

    /// Current query (`query.getValue()`).
    pub fn current_query(&self) -> SearchQuery {
        self.query_tx.borrow().clone()
    }

    /// Push a result batch (`results.next(...)`; app side).
    pub fn publish_results(&self, results: SearchResultWrapper) {
        let _ = self.results_tx.send(results);
    }

    /// Subscribe to result batches (starts at the current value).
    pub fn subscribe_results(&self) -> watch::Receiver<SearchResultWrapper> {
        self.results_tx.subscribe()
    }

    /// Result batches as a stream over tokio-stream.
    pub fn results_stream(&self) -> WatchStream<SearchResultWrapper> {
        WatchStream::new(self.results_tx.subscribe())
    }

    /// Current results (`results.getValue()`).
    pub fn current_results(&self) -> SearchResultWrapper {
        self.results_tx.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_stream::StreamExt;

    fn item(label: &str) -> SearchResultItem {
        SearchResultItem {
            resource_id: format!("r-{label}"),
            category: "cat".into(),
            additional_search_string: None,
            manifest_url: None,
            label: label.into(),
            context: None,
            url: None,
            img_url: "https://img".into(),
            on_select: None,
        }
    }

    #[tokio::test]
    async fn starts_with_ts_defaults() {
        let c = SearchConsumer::new(ConsumerId::new("app1"));
        assert_eq!(c.namespace(), "search");
        assert_eq!(c.current_query().value, "");
        assert_eq!(c.current_results().results, None);
        assert_eq!(c.current_results().loading, None);
    }

    #[tokio::test]
    async fn query_updates_reach_subscribers() {
        let c = SearchConsumer::new(ConsumerId::new("app1"));
        let mut rx = c.subscribe_query();
        let mut stream = c.query_stream();

        c.publish_query(SearchQuery {
            value: "pizza".into(),
        });
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow_and_update().value, "pizza");
        assert_eq!(c.current_query().value, "pizza");

        // WatchStream yields the current value first, then changes
        // (BehaviorSubject semantics).
        assert_eq!(stream.next().await.unwrap().value, "pizza");
        c.publish_query(SearchQuery {
            value: "sushi".into(),
        });
        assert_eq!(stream.next().await.unwrap().value, "sushi");
    }

    #[tokio::test]
    async fn results_updates_reach_subscribers() {
        let c = SearchConsumer::new(ConsumerId::new("app1"));
        let mut rx = c.subscribe_results();

        c.publish_results(SearchResultWrapper {
            loading: Some("My Category".into()),
            results: None,
        });
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow_and_update().loading.as_deref(), Some("My Category"));

        let fired = Arc::new(AtomicBool::new(false));
        let mut with_callback = item("a");
        with_callback.on_select = Some({
            let fired = fired.clone();
            Arc::new(move || fired.store(true, Ordering::SeqCst))
        });
        c.publish_results(SearchResultWrapper {
            loading: None,
            results: Some(vec![item("b"), with_callback]),
        });
        rx.changed().await.unwrap();
        let got = rx.borrow_and_update();
        let results = got.results.as_ref().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].label, "b");
        (results[1].on_select.as_ref().unwrap())();
        assert!(fired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn item_roundtrips_serde_without_callback() {
        let wrapper = SearchResultWrapper {
            loading: None,
            results: Some(vec![item("a")]),
        };
        let json = serde_json::to_value(&wrapper).unwrap();
        assert_eq!(json["results"][0]["resource_id"], "r-a");
        let back: SearchResultWrapper = serde_json::from_value(json).unwrap();
        assert!(back.results.unwrap()[0].on_select.is_none());
    }
}
