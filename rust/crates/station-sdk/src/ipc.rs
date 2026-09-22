//! IPC consumer (port of `ipc/`): pub/sub between the processes of one
//! plugin. TS models it as `pluginToBxChannel: Subject` (publish) and
//! `bxToPluginChannel: Observable` (subscribe); the port uses a tokio
//! `broadcast` channel for both directions. Messages are `serde_json::Value`
//! (TS `any`).

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{broadcast, RwLock};

use crate::common::{ConsumerId, ProviderMissing};

/// Host side of the IPC bridge (`ipc.IpcProviderInterface`): moves messages
/// between the per-plugin channel and other processes.
#[async_trait::async_trait]
pub trait IpcProvider: Send + Sync {
    /// Called for every `publish`; the host relays it back out through
    /// `IpcConsumer::deliver` to every process of the plugin.
    async fn relay(&self, consumer_id: &str, message: Value) -> Result<(), ProviderMissing>;
}

/// IPC consumer: `publish` goes to the provider, `subscribe` is a broadcast
/// receiver fed by [`IpcConsumer::deliver`].
pub struct IpcConsumer {
    pub id: ConsumerId,
    provider: RwLock<Option<Arc<dyn IpcProvider>>>,
    incoming: broadcast::Sender<Value>,
}

impl IpcConsumer {
    pub fn new(id: ConsumerId) -> Self {
        let (incoming, _rx) = broadcast::channel(128);
        IpcConsumer {
            id,
            provider: RwLock::new(None),
            incoming,
        }
    }

    pub fn namespace(&self) -> &'static str {
        "ipc"
    }

    pub async fn set_provider(&self, provider: Arc<dyn IpcProvider>) {
        *self.provider.write().await = Some(provider);
    }

    pub async fn clear_provider(&self) {
        *self.provider.write().await = None;
    }

    /// Send a message to all other processes of the plugin (`publish`).
    pub async fn publish(&self, message: Value) -> Result<(), ProviderMissing> {
        self.provider
            .read()
            .await
            .clone()
            .ok_or(ProviderMissing)?
            .relay(self.id.as_str(), message)
            .await
    }

    /// Subscribe to messages from other processes.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.incoming.subscribe()
    }

    /// Host side: deliver a relayed message to this process.
    pub fn deliver(&self, message: Value) {
        // No subscribers is a normal state, not an error.
        let _ = self.incoming.send(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LoopbackProvider;

    #[async_trait::async_trait]
    impl IpcProvider for LoopbackProvider {
        async fn relay(&self, _consumer_id: &str, _message: Value) -> Result<(), ProviderMissing> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn publish_requires_provider_deliver_broadcasts() {
        let c = IpcConsumer::new(ConsumerId::new("app1"));
        assert!(matches!(c.publish(Value::Null).await, Err(ProviderMissing)));

        c.set_provider(Arc::new(LoopbackProvider)).await;
        c.publish(serde_json::json!({"message": "hello world"}))
            .await
            .unwrap();

        let mut rx = c.subscribe();
        c.deliver(serde_json::json!({"message": "hello world"}));
        assert_eq!(rx.recv().await.unwrap()["message"], "hello world");
    }
}
