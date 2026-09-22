//! SDK facade (port of `index.ts` `sdk()`): builds the leaf consumers for
//! one app and registers them with the host provider. TS replaces
//! `sdk[consumer.namespace]` dynamically on register/unregister; the Rust
//! port keeps the fixed set typed and models register/unregister through
//! the [`Provider`] trait.

use std::sync::Arc;

use crate::activity::ActivityConsumer;
use crate::common::ConsumerId;
use crate::config::ConfigConsumer;
use crate::history::HistoryConsumer;
use crate::ipc::IpcConsumer;
use crate::resources::ResourcesConsumer;
use crate::search::SearchConsumer;
use crate::session::SessionConsumer;
use crate::storage::StorageConsumer;

/// Facade construction options (`SDKOptions`).
#[derive(Debug, Clone)]
pub struct SdkOptions {
    pub id: String,
    pub name: String,
}

/// Host-side registration hook (`Provider`). The host receives every
/// consumer the facade owns; `close` unregisters them all.
pub trait Provider: Send + Sync {
    fn register(&self, sdk: &Sdk);
    fn unregister(&self, sdk: &Sdk);
}

/// The SDK facade: typed consumers for one app (`SDK`).
pub struct Sdk {
    pub search: SearchConsumer,
    pub storage: StorageConsumer,
    pub config: ConfigConsumer,
    pub history: HistoryConsumer,
    pub ipc: IpcConsumer,
    pub resources: ResourcesConsumer,
    pub session: SessionConsumer,
    pub activity: ActivityConsumer,
}

impl Sdk {
    /// Build the facade and register its consumers with `provider`.
    pub fn new(options: &SdkOptions, provider: Arc<dyn Provider>) -> Arc<Sdk> {
        let id = ConsumerId::new(options.id.clone());
        let sdk = Arc::new(Sdk {
            search: SearchConsumer::new(id.clone()),
            storage: StorageConsumer::new(id.clone()),
            config: ConfigConsumer::new(id.clone()),
            history: HistoryConsumer::new(id.clone()),
            ipc: IpcConsumer::new(id.clone()),
            // TS quirk kept: resources takes the manifest URL as id, but the
            // facade constructor only receives `options.id`, so that is what
            // TS passes (`new ResourcesConsumer(options.id)`).
            resources: ResourcesConsumer::new(options.id.clone()),
            session: SessionConsumer::new(id.clone()),
            activity: ActivityConsumer::new(id),
        });
        provider.register(&sdk);
        sdk
    }

    /// Unregister every consumer from the provider (`close`).
    pub fn close(&self, provider: &dyn Provider) {
        provider.unregister(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingProvider {
        registered: AtomicUsize,
        unregistered: AtomicUsize,
    }

    impl Provider for CountingProvider {
        fn register(&self, _sdk: &Sdk) {
            self.registered.fetch_add(1, Ordering::SeqCst);
        }
        fn unregister(&self, _sdk: &Sdk) {
            self.unregistered.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn facade_registers_and_closes() {
        let provider = Arc::new(CountingProvider {
            registered: AtomicUsize::new(0),
            unregistered: AtomicUsize::new(0),
        });
        let sdk = Sdk::new(
            &SdkOptions {
                id: "app1".into(),
                name: "App One".into(),
            },
            provider.clone(),
        );
        assert_eq!(provider.registered.load(Ordering::SeqCst), 1);
        assert_eq!(sdk.search.namespace(), "search");
        assert_eq!(sdk.storage.namespace(), "storage");
        assert_eq!(sdk.config.namespace(), "config");
        assert_eq!(sdk.history.namespace(), "history");
        assert_eq!(sdk.ipc.namespace(), "ipc");
        assert_eq!(sdk.resources.namespace(), "resources");
        assert_eq!(sdk.session.namespace(), "session");
        assert_eq!(sdk.activity.namespace(), "activity");
        assert_eq!(sdk.storage.id.as_str(), "app1");

        sdk.close(provider.as_ref());
        assert_eq!(provider.unregistered.load(Ordering::SeqCst), 1);
    }
}
