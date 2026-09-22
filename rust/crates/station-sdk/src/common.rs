//! Common base shared by all consumers (port of `common.ts`).
//!
//! TS keeps a `Consumer` base class with an `id` plus a `DefaultWeakMap`
//! that silently no-ops when no provider interface was registered. The Rust
//! port models the provider slot as an explicit `ProviderSlot<T>` behind a
//! tokio `RwLock`, and surfaces the "not implemented" case as an
//! [`ProviderMissing`] error instead of a silent proxy.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use tokio::sync::RwLock;

/// Identifies one plugin/app instance wired into an SDK facade
/// (`Consumer.id` in TS; `resources` uses the manifest URL as id).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConsumerId(pub Arc<str>);

impl ConsumerId {
    pub fn new(id: impl Into<String>) -> Self {
        ConsumerId(Arc::from(id.into().as_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for ConsumerId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConsumerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Error returned when a consumer method is called before a provider
/// interface has been registered. TS logs "<prop> provider not implemented"
/// and returns an inert proxy; Rust makes the failure explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMissing;

impl fmt::Display for ProviderMissing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("provider not implemented")
    }
}

impl std::error::Error for ProviderMissing {}

/// A settable-once-ish provider slot: `None` until the host registers a
/// provider interface, shared read access afterwards
/// (the `DefaultWeakMap` role from `common.ts`).
pub struct ProviderSlot<T: ?Sized> {
    provider: RwLock<Option<Arc<T>>>,
}

impl<T: ?Sized> Default for ProviderSlot<T> {
    fn default() -> Self {
        ProviderSlot {
            provider: RwLock::new(None),
        }
    }
}

impl<T: ?Sized> ProviderSlot<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set(&self, provider: Arc<T>) {
        *self.provider.write().await = Some(provider);
    }

    pub async fn clear(&self) {
        *self.provider.write().await = None;
    }

    pub async fn get(&self) -> Option<Arc<T>> {
        self.provider.read().await.clone()
    }

    pub async fn require(&self) -> Result<Arc<T>, ProviderMissing> {
        self.get().await.ok_or(ProviderMissing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slot_is_missing_then_set() {
        let slot: ProviderSlot<u8> = ProviderSlot::new();
        assert_eq!(slot.get().await, None);
        assert!(slot.require().await.is_err());
        slot.set(Arc::new(7)).await;
        assert_eq!(*slot.require().await.unwrap(), 7);
        slot.clear().await;
        assert!(slot.require().await.is_err());
    }
}
