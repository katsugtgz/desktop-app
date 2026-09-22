//! React consumer surface (port of `react/`): portal rendering into the
//! shell's React tree. Rust has no React `ComponentClass`, so portal
//! content is an opaque handle the host (a later slice) interprets — e.g.
//! a key the host resolves to a mounted component. Trait surface only, no
//! consumer implementation yet.

use std::any::Any;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::common::ProviderMissing;

/// Opaque stand-in for the TS `React.ComponentClass` passed to
/// `createPortal`; the host owns the mapping from handle to component.
pub type PortalComponent = Arc<dyn Any + Send + Sync>;

/// Valid portal destinations (`react.ValidPortalIds`; only `'quickswitch'`
/// exists today).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortalId {
    Quickswitch,
}

/// Host side of the react API (`react.ReactProviderInterface`).
#[async_trait::async_trait]
pub trait ReactProvider: Send + Sync {
    /// Render `children` into portal `id` at `position`
    /// (`createPortal(children, id, position?)`).
    async fn create_portal(
        &self,
        consumer_id: &str,
        children: PortalComponent,
        id: PortalId,
        position: Option<u32>,
    ) -> Result<(), ProviderMissing>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubProvider;

    #[async_trait::async_trait]
    impl ReactProvider for StubProvider {
        async fn create_portal(
            &self,
            _consumer_id: &str,
            _children: PortalComponent,
            _id: PortalId,
            _position: Option<u32>,
        ) -> Result<(), ProviderMissing> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn portal_surface_roundtrips() {
        let provider: Arc<dyn ReactProvider> = Arc::new(StubProvider);
        let component: PortalComponent = Arc::new("QuickSwitchList" as &'static str);
        provider
            .create_portal("app1", component, PortalId::Quickswitch, Some(0))
            .await
            .unwrap();
        provider
            .create_portal("app1", Arc::new(42u8), PortalId::Quickswitch, None)
            .await
            .unwrap();
    }

    #[test]
    fn portal_id_serde() {
        assert_eq!(
            serde_json::to_value(PortalId::Quickswitch).unwrap(),
            serde_json::json!("quickswitch")
        );
    }
}
