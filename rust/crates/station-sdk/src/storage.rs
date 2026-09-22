//! Storage consumer (port of `storage/`): namespaced get/set over a
//! host-provided storage, plus an `onChanged` event. TS uses a listener
//! array; the port uses a tokio `broadcast` channel, which gives the same
//! fan-out with async `recv`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};

use crate::common::{ConsumerId, ProviderMissing, ProviderSlot};

/// One item change (`storage.StorageChange`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct StorageChange {
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
}

/// Changes fired on `onChanged` keyed by item key (`storage.StorageChanges`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct StorageChanges(pub HashMap<String, StorageChange>);

/// Host side of the storage API (`storage.StorageProviderInterface`).
#[async_trait]
pub trait StorageProvider: Send + Sync {
    async fn get_item(&self, consumer_key: &str, key: &str) -> Result<Option<Value>, ProviderMissing>;
    async fn set_item(
        &self,
        consumer_key: &str,
        key: &str,
        value: Value,
    ) -> Result<(), ProviderMissing>;
}

/// Storage consumer: `getItem`/`setItem` routed to the registered provider,
/// `onChanged` as a broadcast stream of [`StorageChanges`].
pub struct StorageConsumer {
    /// Consumer id; also the storage namespace key.
    pub id: ConsumerId,
    provider: ProviderSlot<dyn StorageProvider>,
    on_changed: broadcast::Sender<StorageChanges>,
}

impl StorageConsumer {
    pub fn new(id: ConsumerId) -> Self {
        let (tx, _rx) = broadcast::channel(64);
        StorageConsumer {
            id,
            provider: ProviderSlot::new(),
            on_changed: tx,
        }
    }

    pub fn namespace(&self) -> &'static str {
        "storage"
    }

    pub async fn set_provider(&self, provider: Arc<dyn StorageProvider>) {
        self.provider.set(provider).await;
    }

    pub async fn clear_provider(&self) {
        self.provider.clear().await;
    }

    /// Subscribe to item changes for this consumer's namespace.
    pub fn subscribe(&self) -> broadcast::Receiver<StorageChanges> {
        self.on_changed.subscribe()
    }

    /// Fire `onChanged` (host side after a `set_item`).
    pub fn emit_changes(&self, changes: StorageChanges) {
        // No subscribers is a normal state, not an error.
        let _ = self.on_changed.send(changes);
    }

    pub async fn get_item(&self, key: &str) -> Result<Option<Value>, ProviderMissing> {
        self.provider
            .require()
            .await?
            .get_item(self.id.as_str(), key)
            .await
    }

    pub async fn set_item(&self, key: &str, value: Value) -> Result<(), ProviderMissing> {
        self.provider
            .require()
            .await?
            .set_item(self.id.as_str(), key, value)
            .await
    }
}

/// In-memory provider used by tests and as a reference implementation.
#[derive(Default)]
pub struct MemoryStorageProvider {
    areas: RwLock<HashMap<String, HashMap<String, Value>>>,
}

impl MemoryStorageProvider {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl StorageProvider for MemoryStorageProvider {
    async fn get_item(&self, consumer_key: &str, key: &str) -> Result<Option<Value>, ProviderMissing> {
        Ok(self
            .areas
            .read()
            .await
            .get(consumer_key)
            .and_then(|area| area.get(key).cloned()))
    }

    async fn set_item(
        &self,
        consumer_key: &str,
        key: &str,
        value: Value,
    ) -> Result<(), ProviderMissing> {
        let old = self
            .areas
            .write()
            .await
            .entry(consumer_key.to_owned())
            .or_default()
            .insert(key.to_owned(), value.clone());
        let _ = old; // change notification is the host's job; see `emit_changes`
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn get_set_roundtrip_and_missing_provider() {
        let c = StorageConsumer::new(ConsumerId::new("app1"));
        assert!(matches!(c.get_item("k").await, Err(ProviderMissing)));

        c.set_provider(Arc::new(MemoryStorageProvider::new())).await;
        c.set_item("k", json!({"n": 1})).await.unwrap();
        assert_eq!(c.get_item("k").await.unwrap(), Some(json!({"n": 1})));
        assert_eq!(c.get_item("other").await.unwrap(), None);
    }

    #[tokio::test]
    async fn on_changed_broadcasts() {
        let c = StorageConsumer::new(ConsumerId::new("app1"));
        let mut rx = c.subscribe();
        let mut changes = StorageChanges::default();
        changes.0.insert(
            "k".into(),
            StorageChange {
                old_value: None,
                new_value: Some(json!(1)),
            },
        );
        c.emit_changes(changes);
        let got = rx.recv().await.unwrap();
        assert!(got.0.contains_key("k"));
    }

    #[tokio::test]
    async fn namespaces_are_isolated() {
        let shared = Arc::new(MemoryStorageProvider::new());
        let a = StorageConsumer::new(ConsumerId::new("a"));
        let b = StorageConsumer::new(ConsumerId::new("b"));
        a.set_provider(shared.clone()).await;
        b.set_provider(shared).await;
        a.set_item("k", json!(1)).await.unwrap();
        assert_eq!(b.get_item("k").await.unwrap(), None);
    }
}
