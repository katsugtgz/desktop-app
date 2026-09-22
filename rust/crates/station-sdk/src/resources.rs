//! Resources consumer (port of `resources/`): lets an app override link
//! opening and metadata extraction for its resources. Note the TS quirk:
//! `ResourcesConsumer`'s constructor takes the manifest URL and uses it as
//! the consumer id — kept as-is.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::common::{ConsumerId, ProviderMissing, ProviderSlot};

/// Override for opening a resource URL. `default_open` runs the host's
/// normal open path (e.g. open in a tab).
pub type OpenHandler =
    Arc<dyn Fn(&str, DefaultOpen) -> futures_boxed::BoxFuture<'static, ()> + Send + Sync>;

/// Default open path passed to an `OpenHandler`.
pub type DefaultOpen = Arc<dyn Fn() -> futures_boxed::BoxFuture<'static, ()> + Send + Sync>;

/// Override for extracting metadata; receives the host's default metadata
/// and returns the (possibly amended) result.
pub type MetaDataHandler = Arc<
    dyn Fn(&str, ResourceMetaData) -> futures_boxed::BoxFuture<'static, ResourceMetaData>
        + Send
        + Sync,
>;

/// ponytail: hand-rolled boxed future aliases instead of pulling in the
/// `futures` crate; replace with `futures::future::BoxFuture` when another
/// slice adds that dependency.
pub mod futures_boxed {
    use std::future::Future;
    use std::pin::Pin;

    pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
}

/// OpenGraph-style resource metadata (`resources.ResourceMetaData`,
/// http://ogp.me/#metadata).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceMetaData {
    pub bx_resource_id: String,
    pub manifest_url: String,
    pub image: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Host side of the resources API (`resources.ResourcesProviderInterface`).
#[async_trait]
pub trait ResourcesProvider: Send + Sync {
    async fn set_open_handler(
        &self,
        manifest_url: &str,
        handler: Option<OpenHandler>,
    ) -> Result<(), ProviderMissing>;
    async fn set_meta_data_handler(
        &self,
        manifest_url: &str,
        handler: Option<MetaDataHandler>,
    ) -> Result<(), ProviderMissing>;
}

/// Resources consumer; `id` is the manifest URL.
pub struct ResourcesConsumer {
    pub id: ConsumerId,
    provider: ProviderSlot<dyn ResourcesProvider>,
}

impl ResourcesConsumer {
    /// `manifest_url` doubles as the consumer id (TS behavior).
    pub fn new(manifest_url: impl Into<String>) -> Self {
        ResourcesConsumer {
            id: ConsumerId::new(manifest_url),
            provider: ProviderSlot::new(),
        }
    }

    pub fn namespace(&self) -> &'static str {
        "resources"
    }

    pub async fn set_provider(&self, provider: Arc<dyn ResourcesProvider>) {
        self.provider.set(provider).await;
    }

    pub async fn clear_provider(&self) {
        self.provider.clear().await;
    }

    /// Install (or with `None`, clear) the open override.
    pub async fn set_open_handler(
        &self,
        handler: Option<OpenHandler>,
    ) -> Result<(), ProviderMissing> {
        self.provider
            .require()
            .await?
            .set_open_handler(self.id.as_str(), handler)
            .await
    }

    /// Install (or with `None`, clear) the metadata override.
    pub async fn set_meta_data_handler(
        &self,
        handler: Option<MetaDataHandler>,
    ) -> Result<(), ProviderMissing> {
        self.provider
            .require()
            .await?
            .set_meta_data_handler(self.id.as_str(), handler)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingProvider {
        opens: AtomicUsize,
        metas: AtomicUsize,
    }

    #[async_trait]
    impl ResourcesProvider for RecordingProvider {
        async fn set_open_handler(
            &self,
            _m: &str,
            handler: Option<OpenHandler>,
        ) -> Result<(), ProviderMissing> {
            if let Some(h) = handler {
                h("https://x", Arc::new(|| Box::pin(std::future::ready(())))).await;
            }
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn set_meta_data_handler(
            &self,
            _m: &str,
            handler: Option<MetaDataHandler>,
        ) -> Result<(), ProviderMissing> {
            if let Some(h) = handler {
                let md = ResourceMetaData {
                    bx_resource_id: "1".into(),
                    manifest_url: "m".into(),
                    image: "i".into(),
                    title: "t".into(),
                    description: None,
                    theme_color: None,
                    url: None,
                };
                h("https://x", md).await;
            }
            self.metas.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn handlers_route_to_provider() {
        let c = ResourcesConsumer::new("https://manifest/app.json");
        assert_eq!(c.namespace(), "resources");
        assert_eq!(c.id.as_str(), "https://manifest/app.json");
        assert!(matches!(
            c.set_open_handler(None).await,
            Err(ProviderMissing)
        ));

        let p = Arc::new(RecordingProvider {
            opens: AtomicUsize::new(0),
            metas: AtomicUsize::new(0),
        });
        c.set_provider(p.clone()).await;

        let called = Arc::new(AtomicUsize::new(0));
        let seen = called.clone();
        let open: OpenHandler = Arc::new(move |_url, _default| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.fetch_add(1, Ordering::SeqCst);
            })
        });
        c.set_open_handler(Some(open)).await.unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(p.opens.load(Ordering::SeqCst), 1);
    }
}
