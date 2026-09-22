//! History consumer (port of `history/`): a `BehaviorSubject` of history
//! entries the app pushes after querying activity. Pure state, no provider.
//! TS `HistoryEntry = SearchResultItem & { date: Date }` — the search item
//! shape arrives with slice s03; here it is mirrored as a local struct so
//! this slice stays self-contained.

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::common::ConsumerId;

/// One pushed history entry (`history.HistoryEntry`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub resource_id: String,
    pub category: String,
    pub label: String,
    pub img_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
    /// Seconds since the UNIX epoch (TS used a `Date`).
    pub date: i64,
}

/// History consumer: `entries` as a watch channel (latest list + change
/// notifications, matching `BehaviorSubject` semantics for consumers).
pub struct HistoryConsumer {
    pub id: ConsumerId,
    entries_tx: watch::Sender<Vec<HistoryEntry>>,
}

impl HistoryConsumer {
    pub fn new(id: ConsumerId) -> Self {
        let (entries_tx, _rx) = watch::channel(Vec::new());
        HistoryConsumer { id, entries_tx }
    }

    pub fn namespace(&self) -> &'static str {
        "history"
    }

    /// Subscribe to entries (starts with the latest list).
    pub fn subscribe(&self) -> watch::Receiver<Vec<HistoryEntry>> {
        self.entries_tx.subscribe()
    }

    /// Replace the entry list (`entries.next(...)` in TS).
    pub fn publish(&self, entries: Vec<HistoryEntry>) {
        // No subscribers is a normal state, not an error.
        let _ = self.entries_tx.send(entries);
    }

    /// Snapshot of the current entries (`entries.getValue()` in TS).
    pub fn current(&self) -> Vec<HistoryEntry> {
        self.entries_tx.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str) -> HistoryEntry {
        HistoryEntry {
            resource_id: format!("r-{label}"),
            category: "cat".into(),
            label: label.into(),
            img_url: "https://img".into(),
            url: None,
            context: None,
            manifest_url: None,
            date: 1_758_000_000,
        }
    }

    #[tokio::test]
    async fn starts_empty_then_updates() {
        let c = HistoryConsumer::new(ConsumerId::new("app1"));
        assert!(c.current().is_empty());

        let mut rx = c.subscribe();
        c.publish(vec![entry("a")]);
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow_and_update().len(), 1);
        assert_eq!(c.current()[0].label, "a");

        c.publish(vec![entry("a"), entry("b")]);
        rx.changed().await.unwrap();
        assert_eq!(c.current().len(), 2);
    }
}
