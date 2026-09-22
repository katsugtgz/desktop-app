//! Tabs consumer surface (port of `tabs/`): every operation touches the
//! webview layer (mount/attach state, in-page JS, navigation), so — unlike
//! the stateful consumers — this slice exposes only the host-side trait and
//! its data types. No consumer implementation yet; the webview host (a later
//! slice) implements [`TabsProvider`].

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use crate::common::ProviderMissing;

/// One tab (`tabs.Tab`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub application_id: String,
    pub badge: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub favicons: Vec<String>,
    pub is_application_home: bool,
    pub is_loading: bool,
    pub tab_id: String,
    pub title: String,
    pub url: String,
}

/// Partial tab update (`tabs.TabUpdate`); only `url` is settable and
/// modifying it triggers a navigation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Navigation event (`tabs.Nav`): focus moved between tabs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nav {
    pub tab_id: String,
    pub previous_tab_id: String,
}

/// Tab creation options (`tabs.CreateOptions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOptions {
    pub application_id: String,
    pub url: String,
}

/// Options for `nav_to_tab` (`tabs.NavToTabOptions`); `silent` tells the
/// activity API to skip history recording for automatic navigations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavToTabOptions {
    /// TS defaults to `{ silent: false }` when `navToTab` gets no options.
    #[serde(default)]
    pub silent: bool,
}

/// WebContents lifecycle of a tab (`tabs.TabWebContentsState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabWebContentsState {
    NotMounted,
    WaitingToAttach,
    Detaching,
    Mounted,
    Crashed,
}

/// Host side of the tabs API (`tabs.TabsProviderInterface`): webview-coupled
/// operations the shell implements. `get_tab`/`nav` return watch receivers
/// (TS `Observable`s carrying the latest value).
#[async_trait::async_trait]
pub trait TabsProvider: Send + Sync {
    /// List all tabs for the consumer (`getTabs(id)`).
    async fn get_tabs(&self, consumer_id: &str) -> Result<Vec<Tab>, ProviderMissing>;
    /// Observe one tab (`getTab(tabId)`); receiver starts at the current tab.
    async fn get_tab(&self, tab_id: &str) -> Result<watch::Receiver<Tab>, ProviderMissing>;
    /// Observe navigation events (`nav()`).
    async fn nav(&self) -> Result<watch::Receiver<Nav>, ProviderMissing>;
    /// Create a tab for an application and navigate to `url` (`create`).
    async fn create(
        &self,
        consumer_id: &str,
        options: CreateOptions,
    ) -> Result<(), ProviderMissing>;
    /// Modify tab properties (`updateTab`); unspecified fields are unchanged.
    async fn update_tab(&self, tab_id: &str, update: TabUpdate) -> Result<(), ProviderMissing>;
    /// Navigate to a tab (`navToTab`).
    async fn nav_to_tab(
        &self,
        tab_id: &str,
        options: NavToTabOptions,
    ) -> Result<(), ProviderMissing>;
    /// Execute JavaScript in the tab's webview (`executeJavaScript`).
    async fn execute_javascript(&self, tab_id: &str, code: &str) -> Result<Value, ProviderMissing>;
    /// Get the tab's webContents state (`getTabWebContentsState`).
    async fn get_tab_web_contents_state(
        &self,
        tab_id: &str,
    ) -> Result<TabWebContentsState, ProviderMissing>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tab(tab_id: &str) -> Tab {
        Tab {
            application_id: "slack".into(),
            badge: String::new(),
            can_go_back: false,
            can_go_forward: true,
            favicons: vec!["https://favicon".into()],
            is_application_home: false,
            is_loading: false,
            tab_id: tab_id.into(),
            title: "Slack".into(),
            url: "https://slack.com".into(),
        }
    }

    /// Host stand-in proving the trait surface is implementable and usable
    /// as a trait object.
    struct StubProvider {
        tabs: Vec<Tab>,
    }

    #[async_trait::async_trait]
    impl TabsProvider for StubProvider {
        async fn get_tabs(&self, _consumer_id: &str) -> Result<Vec<Tab>, ProviderMissing> {
            Ok(self.tabs.clone())
        }

        async fn get_tab(&self, tab_id: &str) -> Result<watch::Receiver<Tab>, ProviderMissing> {
            let tab = self
                .tabs
                .iter()
                .find(|t| t.tab_id == tab_id)
                .cloned()
                .ok_or(ProviderMissing)?;
            let (tx, rx) = watch::channel(tab);
            tokio::spawn(async move {
                // Simulate a title update pushed by the host.
                let mut tab = tx.borrow().clone();
                tab.title = "Updated".into();
                let _ = tx.send(tab);
            });
            Ok(rx)
        }

        async fn nav(&self) -> Result<watch::Receiver<Nav>, ProviderMissing> {
            Ok(watch::channel(Nav {
                tab_id: "t1".into(),
                previous_tab_id: "t0".into(),
            })
            .1)
        }

        async fn create(
            &self,
            _consumer_id: &str,
            _options: CreateOptions,
        ) -> Result<(), ProviderMissing> {
            Ok(())
        }

        async fn update_tab(
            &self,
            _tab_id: &str,
            _update: TabUpdate,
        ) -> Result<(), ProviderMissing> {
            Ok(())
        }

        async fn nav_to_tab(
            &self,
            _tab_id: &str,
            _options: NavToTabOptions,
        ) -> Result<(), ProviderMissing> {
            Ok(())
        }

        async fn execute_javascript(
            &self,
            _tab_id: &str,
            _code: &str,
        ) -> Result<Value, ProviderMissing> {
            Ok(Value::Null)
        }

        async fn get_tab_web_contents_state(
            &self,
            _tab_id: &str,
        ) -> Result<TabWebContentsState, ProviderMissing> {
            Ok(TabWebContentsState::Mounted)
        }
    }

    #[tokio::test]
    async fn provider_surface_roundtrips() {
        let provider: Arc<dyn TabsProvider> = Arc::new(StubProvider {
            tabs: vec![tab("t1"), tab("t2")],
        });

        let tabs = provider.get_tabs("app1").await.unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].tab_id, "t1");

        let mut rx = provider.get_tab("t2").await.unwrap();
        assert_eq!(rx.borrow_and_update().tab_id, "t2");
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow_and_update().title, "Updated");

        let nav = provider.nav().await.unwrap();
        assert_eq!(nav.borrow().previous_tab_id, "t0");

        provider
            .create(
                "app1",
                CreateOptions {
                    application_id: "slack".into(),
                    url: "https://google.fr".into(),
                },
            )
            .await
            .unwrap();
        provider
            .update_tab(
                "t1",
                TabUpdate {
                    url: Some("https://google.com".into()),
                },
            )
            .await
            .unwrap();
        provider
            .nav_to_tab("t1", NavToTabOptions { silent: true })
            .await
            .unwrap();
        assert_eq!(
            provider.execute_javascript("t1", "1+1").await.unwrap(),
            Value::Null
        );
        assert_eq!(
            provider.get_tab_web_contents_state("t1").await.unwrap(),
            TabWebContentsState::Mounted
        );
    }

    #[test]
    fn types_serde_roundtrip() {
        let json = serde_json::to_value(tab("t1")).unwrap();
        let back: Tab = serde_json::from_value(json).unwrap();
        assert_eq!(back, tab("t1"));
        assert_eq!(back.application_id, "slack");

        let state: TabWebContentsState =
            serde_json::from_value(serde_json::json!("crashed")).unwrap();
        assert_eq!(state, TabWebContentsState::Crashed);

        let opts: NavToTabOptions = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!opts.silent);

        let update: TabUpdate =
            serde_json::from_value(serde_json::json!({ "url": "https://x" })).unwrap();
        assert_eq!(update.url.as_deref(), Some("https://x"));
    }
}
