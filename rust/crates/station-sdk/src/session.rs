//! Session consumer (port of `session/`): Station user agent and the
//! cookies for the current service.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::common::{ConsumerId, ProviderMissing, ProviderSlot};

/// A cookie (Electron Cookie structure, http://electron.atom.io/docs/api/structures/cookie).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Seconds since the UNIX epoch; absent for session cookies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration_date: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<bool>,
}

/// Host side of the session API (`session.SessionProviderInterface`).
#[async_trait]
pub trait SessionProvider: Send + Sync {
    async fn get_user_agent(&self) -> Result<String, ProviderMissing>;
    async fn get_cookies(&self, consumer_id: &str) -> Result<Vec<Cookie>, ProviderMissing>;
}

/// Session consumer: `getUserAgent` and `getCookies` routed to the provider;
/// cookies are scoped to this consumer's id by the host.
pub struct SessionConsumer {
    pub id: ConsumerId,
    provider: ProviderSlot<dyn SessionProvider>,
}

impl SessionConsumer {
    pub fn new(id: ConsumerId) -> Self {
        SessionConsumer {
            id,
            provider: ProviderSlot::new(),
        }
    }

    pub fn namespace(&self) -> &'static str {
        "session"
    }

    pub async fn set_provider(&self, provider: Arc<dyn SessionProvider>) {
        self.provider.set(provider).await;
    }

    pub async fn clear_provider(&self) {
        self.provider.clear().await;
    }

    pub async fn get_user_agent(&self) -> Result<String, ProviderMissing> {
        self.provider.require().await?.get_user_agent().await
    }

    pub async fn get_cookies(&self) -> Result<Vec<Cookie>, ProviderMissing> {
        self.provider
            .require()
            .await?
            .get_cookies(self.id.as_str())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSession;

    #[async_trait]
    impl SessionProvider for FakeSession {
        async fn get_user_agent(&self) -> Result<String, ProviderMissing> {
            Ok("Station/1.0".into())
        }
        async fn get_cookies(&self, consumer_id: &str) -> Result<Vec<Cookie>, ProviderMissing> {
            Ok(vec![Cookie {
                name: format!("{consumer_id}-session"),
                value: "v".into(),
                domain: Some("example.com".into()),
                expiration_date: None,
                host_only: None,
                http_only: None,
                path: None,
                secure: None,
                session: Some(true),
            }])
        }
    }

    #[tokio::test]
    async fn routes_and_scopes_to_consumer_id() {
        let c = SessionConsumer::new(ConsumerId::new("app1"));
        assert!(matches!(c.get_user_agent().await, Err(ProviderMissing)));

        c.set_provider(Arc::new(FakeSession)).await;
        assert_eq!(c.get_user_agent().await.unwrap(), "Station/1.0");
        assert_eq!(c.get_cookies().await.unwrap()[0].name, "app1-session");
    }
}
