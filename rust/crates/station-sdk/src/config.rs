//! Config consumer (port of `config/`): app config stream plus Dock icon
//! update. TS exposes `configData: Observable<ConfigData[]>`; the port uses
//! a tokio `watch` channel (latest-value semantics match `BehaviorSubject`
//! for this read-mostly use).

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::common::{ConsumerId, ProviderMissing, ProviderSlot};

/// One app config row (`config.ConfigData`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigData {
    pub application_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
}

/// Host side of the config API (`config.ConfigProviderInterface`).
#[async_trait]
pub trait ConfigProvider: Send + Sync {
    /// Current config snapshot; further updates go through `config_tx`.
    async fn config_data(&self) -> Result<Vec<ConfigData>, ProviderMissing>;
    /// Update Dock icon for `application_id` with `url`.
    async fn set_icon(&self, application_id: &str, url: &str) -> Result<(), ProviderMissing>;
}

/// Config consumer: `configData` as a watch channel, `setIcon` routed to the
/// provider.
pub struct ConfigConsumer {
    pub id: ConsumerId,
    provider: ProviderSlot<dyn ConfigProvider>,
    config_tx: watch::Sender<Vec<ConfigData>>,
}

impl ConfigConsumer {
    pub fn new(id: ConsumerId) -> Self {
        let (config_tx, _rx) = watch::channel(Vec::new());
        ConfigConsumer {
            id,
            provider: ProviderSlot::new(),
            config_tx,
        }
    }

    pub fn namespace(&self) -> &'static str {
        "config"
    }

    pub async fn set_provider(&self, provider: Arc<dyn ConfigProvider>) {
        self.provider.set(provider).await;
    }

    pub async fn clear_provider(&self) {
        self.provider.clear().await;
    }

    /// Subscribe to config data (starts with the latest value).
    pub fn subscribe(&self) -> watch::Receiver<Vec<ConfigData>> {
        self.config_tx.subscribe()
    }

    /// Push a config snapshot into the stream (host side).
    pub fn publish(&self, data: Vec<ConfigData>) {
        // No subscribers is a normal state, not an error.
        let _ = self.config_tx.send(data);
    }

    pub async fn config_data(&self) -> Result<Vec<ConfigData>, ProviderMissing> {
        self.provider.require().await?.config_data().await
    }

    pub async fn set_icon(&self, application_id: &str, url: &str) -> Result<(), ProviderMissing> {
        self.provider
            .require()
            .await?
            .set_icon(application_id, url)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedProvider(Vec<ConfigData>);

    #[async_trait]
    impl ConfigProvider for FixedProvider {
        async fn config_data(&self) -> Result<Vec<ConfigData>, ProviderMissing> {
            Ok(self.0.clone())
        }
        async fn set_icon(&self, _a: &str, _u: &str) -> Result<(), ProviderMissing> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn watch_stream_and_provider_calls() {
        let c = ConfigConsumer::new(ConsumerId::new("app1"));
        assert!(matches!(c.config_data().await, Err(ProviderMissing)));

        let data = vec![ConfigData {
            application_id: "app1".into(),
            subdomain: Some("acme".into()),
        }];
        c.set_provider(Arc::new(FixedProvider(data.clone()))).await;
        assert_eq!(c.config_data().await.unwrap(), data);

        let mut rx = c.subscribe();
        let next = vec![ConfigData {
            application_id: "app1".into(),
            subdomain: None,
        }];
        c.publish(next.clone());
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), next);
    }
}
