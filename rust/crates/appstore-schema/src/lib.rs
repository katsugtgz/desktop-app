//! Appstore schema types.
//!
//! Port of the GraphQL data model consumed by `packages/appstore`:
//!
//! - the distant API schema is embedded verbatim at
//!   [`SCHEMA_GRAPHQL`](static@SCHEMA_GRAPHQL) (byte-for-byte copy of
//!   `packages/appstore/api-schema.graphqls`, also published next to this crate
//!   as `schema/api.graphqls`), and
//! - the serde structs below carry the fields the appstore UI actually reads
//!   (the `applications`, `boostedApplications` and `search` query shapes),
//!   replacing the Apollo `InMemoryCache` + client-schema local-state layer.
//!
//! Not ported in this slice: the HTTP transport and codegen of every unused
//! GraphQL type (workspaces, onboardees, ...). Only the projection the UI
//! queries is modeled; add fields when a query starts selecting them.

use serde::{Deserialize, Serialize};

/// Verbatim copy of `packages/appstore/api-schema.graphqls`.
pub static SCHEMA_GRAPHQL: &str = include_str!("../schema/api.graphqls");

/// `type ApplicationCategory` from the API schema, as selected by
/// `category { name }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationCategory {
    pub name: String,
}

/// `type Manifest` from the API schema (`scope`, `extended_scopes`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extended_scopes: Option<Vec<String>>,
}

/// `type Preconfigurations` from the API schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preconfigurations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onpremise: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
}

/// `type Application` from the API schema.
///
/// Field set matches the appstore queries (`applications`,
/// `boostedApplications`, `searchApplicationsByName`): every field the UI
/// selects is modeled; nullable fields the schema marks optional are
/// `Option`. `addedAt` / `manifest` / the recommendation metadata are not
/// selected by any current query and are omitted — add them when a query
/// starts asking for them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Application {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<ApplicationCategory>,
    #[serde(
        rename = "themeColor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub theme_color: Option<String>,
    #[serde(rename = "iconURL", default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(rename = "startURL", default, skip_serializing_if = "Option::is_none")]
    pub start_url: Option<String>,
    #[serde(
        rename = "isChromeExtension",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub is_chrome_extension: Option<bool>,
    #[serde(
        rename = "bxAppManifestURL",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bx_app_manifest_url: Option<String>,
    #[serde(
        rename = "previousServiceId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub previous_service_id: Option<String>,
    #[serde(rename = "isPrivate", default, skip_serializing_if = "Option::is_none")]
    pub is_private: Option<bool>,
    #[serde(
        rename = "isPreconfigurable",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub is_preconfigurable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preconfigurations: Option<Preconfigurations>,
}

/// UI-state types replacing the Apollo local cache
/// (`packages/appstore/src/graphql/initialClientState.ts` + resolvers, schema
/// in `packages/appstore/client-schema.graphqls`).
///
/// `camelCase` serde names keep the wire shape identical to what
/// `cache.writeData` produced, so serialized state round-trips.
pub mod ui_state {
    use serde::{Deserialize, Serialize};

    /// `screenNames` from `packages/appstore/src/shared/constants/constants.ts`.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ScreenName {
        #[serde(rename = "MOST_POPULAR")]
        MostPopular,
        #[serde(rename = "ALL_APPS")]
        AllApps,
        #[serde(rename = "ALL_EXTENSIONS")]
        AllExtensions,
        #[serde(rename = "BOOSTED_APPS")]
        BoostedApps,
        #[serde(rename = "ONBOARD_EMPLOYEES")]
        OnboardEmployees,
        #[serde(rename = "MY_CUSTOM_APPS")]
        MyCustomApps,
        #[default]
        #[serde(rename = "COMPANY_APPS")]
        CompanyApps,
    }

    /// `WrappedStringValue` — `activeScreenName` / `isBurgerOpen`.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct WrappedStringValue {
        pub value: String,
    }

    /// `WrappedBooleanValue` — `isBurgerOpen`.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct WrappedBooleanValue {
        pub value: bool,
    }

    /// `ModalStatus` — `appModalStatus`.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct ModalStatus {
        #[serde(rename = "isAppModalStatus")]
        pub is_app_modal_status: bool,
    }

    /// `SearchStringVariables` — `search` (initial state from
    /// `initialClientState.ts`, all fields defaulted).
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct SearchStringVariables {
        #[serde(default, rename = "searchString")]
        pub search_string: String,
        #[serde(
            default,
            rename = "searchStringAfterEnterPress",
            skip_serializing_if = "Option::is_none"
        )]
        pub search_string_after_enter_press: Option<String>,
        #[serde(default, rename = "isEnterPressed")]
        pub is_enter_pressed: bool,
    }

    /// `CustomAppRequestStatus` — `appRequestMode`.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct CustomAppRequestStatus {
        #[serde(rename = "appRequestIsOpen")]
        pub app_request_is_open: bool,
        #[serde(default, rename = "currentMode")]
        pub current_mode: String,
    }

    /// `CustomApp` — a locally-defined application (client schema).
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct CustomApp {
        pub id: String,
        pub name: String,
        #[serde(rename = "bxAppManifestURL")]
        pub bx_app_manifest_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub category: Option<super::ApplicationCategory>,
        #[serde(rename = "iconURL", default, skip_serializing_if = "Option::is_none")]
        pub icon_url: Option<String>,
        #[serde(rename = "startURL", default, skip_serializing_if = "Option::is_none")]
        pub start_url: Option<String>,
        #[serde(
            rename = "themeColor",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        pub theme_color: Option<String>,
        #[serde(
            rename = "isChromeExtension",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        pub is_chrome_extension: Option<bool>,
    }

    /// `SelectedCustomApp` — `selectedCustomApp`.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct SelectedCustomApp {
        pub app: CustomApp,
    }

    /// The whole local cache root, i.e. `initialClientState` defaults and the
    /// six `set*` mutation targets.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct UiState {
        /// `isBurgerOpen`
        #[serde(rename = "isBurgerOpen")]
        pub is_burger_open: bool,
        /// `search`
        pub search: SearchStringVariables,
        /// `activeScreenName`
        #[serde(rename = "activeScreenName")]
        pub active_screen_name: ScreenName,
        /// `appRequestMode`
        #[serde(rename = "appRequestMode")]
        pub app_request_mode: CustomAppRequestStatus,
        /// `selectedCustomApp`
        #[serde(rename = "selectedCustomApp")]
        pub selected_custom_app: SelectedCustomApp,
        /// `appModalStatus`
        #[serde(rename = "appModalStatus")]
        pub app_modal_status: ModalStatus,
    }

    impl Default for UiState {
        /// Mirrors `initialClientState.ts` exactly.
        fn default() -> Self {
            Self {
                is_burger_open: false,
                search: SearchStringVariables::default(),
                active_screen_name: ScreenName::CompanyApps,
                app_request_mode: CustomAppRequestStatus::default(),
                selected_custom_app: SelectedCustomApp::default(),
                app_modal_status: ModalStatus::default(),
            }
        }
    }

    impl UiState {
        /// `setActiveScreenName` resolver.
        pub fn set_active_screen_name(&mut self, active_screen_name: ScreenName) {
            self.active_screen_name = active_screen_name;
        }

        /// `setAppModalStatus` resolver.
        pub fn set_app_modal_status(&mut self, is_app_modal_open: bool) {
            self.app_modal_status.is_app_modal_status = is_app_modal_open;
        }

        /// `setBurgerStatus` resolver.
        pub fn set_burger_status(&mut self, is_burger_open: bool) {
            self.is_burger_open = is_burger_open;
        }

        /// `setCustomAppRequestMode` resolver.
        pub fn set_custom_app_request_mode(
            &mut self,
            app_request_is_open: bool,
            current_mode: impl Into<String>,
        ) {
            self.app_request_mode.app_request_is_open = app_request_is_open;
            self.app_request_mode.current_mode = current_mode.into();
        }

        /// `setSearchString` resolver.
        pub fn set_search_string(
            &mut self,
            search_string: impl Into<String>,
            search_string_after_enter_press: Option<String>,
            is_enter_pressed: bool,
        ) {
            self.search.search_string = search_string.into();
            self.search.search_string_after_enter_press = search_string_after_enter_press;
            self.search.is_enter_pressed = is_enter_pressed;
        }

        /// `setSelectedCustomApp` resolver.
        pub fn set_selected_custom_app(&mut self, app: CustomApp) {
            self.selected_custom_app.app = app;
        }
    }
}

pub use ui_state::{
    CustomApp, CustomAppRequestStatus, ModalStatus, ScreenName, SearchStringVariables,
    SelectedCustomApp, UiState, WrappedBooleanValue, WrappedStringValue,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn schema_graphql_matches_api_schema_verbatim() {
        // Same check the slice verification runs outside: `diff` must be
        // empty. Done in Rust so `cargo test` alone proves it.
        let source = fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../packages/appstore/api-schema.graphqls"
        ))
        .expect("packages/appstore/api-schema.graphqls readable");
        assert_eq!(SCHEMA_GRAPHQL, source);
    }

    #[test]
    fn application_roundtrips_camel_case() {
        let app = Application {
            id: "application/1".into(),
            name: "Google Drive".into(),
            category: Some(ApplicationCategory {
                name: "File Management".into(),
            }),
            theme_color: Some("#4285F4".into()),
            icon_url: Some("https://example.com/icon.png".into()),
            start_url: Some("https://drive.google.com".into()),
            is_chrome_extension: Some(false),
            bx_app_manifest_url: Some("https://api.getstation.com/manifests/google-drive/".into()),
            previous_service_id: Some("gdrive".into()),
            is_private: Some(false),
            is_preconfigurable: Some(true),
            preconfigurations: Some(Preconfigurations {
                onpremise: Some(false),
                subdomain: Some("drive".into()),
            }),
        };
        let json = serde_json::to_value(&app).unwrap();
        assert_eq!(
            json["bxAppManifestURL"],
            "https://api.getstation.com/manifests/google-drive/"
        );
        assert_eq!(json["themeColor"], "#4285F4");
        assert_eq!(json["startURL"], "https://drive.google.com");
        assert_eq!(json["iconURL"], "https://example.com/icon.png");
        assert_eq!(json["isChromeExtension"], false);
        assert_eq!(json["previousServiceId"], "gdrive");
        assert_eq!(json["isPrivate"], false);
        assert_eq!(json["isPreconfigurable"], true);
        assert_eq!(json["preconfigurations"]["onpremise"], false);
        assert_eq!(json["preconfigurations"]["subdomain"], "drive");
        let back: Application = serde_json::from_value(json).unwrap();
        assert_eq!(back, app);
    }

    #[test]
    fn application_tolerates_missing_optional_fields() {
        let app: Application = serde_json::from_str(r#"{"id":"a","name":"A"}"#).unwrap();
        assert_eq!(app.category, None);
        assert_eq!(app.preconfigurations, None);
    }

    #[test]
    fn ui_state_defaults_match_initial_client_state() {
        let state = UiState::default();
        assert!(!state.is_burger_open);
        assert_eq!(state.search.search_string, "");
        assert_eq!(state.search.search_string_after_enter_press, None);
        assert!(!state.search.is_enter_pressed);
        assert_eq!(state.active_screen_name, ScreenName::CompanyApps);
        assert!(!state.app_request_mode.app_request_is_open);
        assert_eq!(state.app_request_mode.current_mode, "");
        assert_eq!(state.selected_custom_app.app.id, "");
        assert!(!state.app_modal_status.is_app_modal_status);
    }

    #[test]
    fn ui_state_setters_apply_resolvers() {
        let mut state = UiState::default();
        state.set_burger_status(true);
        assert!(state.is_burger_open);
        state.set_active_screen_name(ScreenName::MyCustomApps);
        assert_eq!(state.active_screen_name, ScreenName::MyCustomApps);
        state.set_app_modal_status(true);
        assert!(state.app_modal_status.is_app_modal_status);
        state.set_custom_app_request_mode(true, "CREATE_MODE");
        assert!(state.app_request_mode.app_request_is_open);
        assert_eq!(state.app_request_mode.current_mode, "CREATE_MODE");
        state.set_search_string("drive", Some("drive".to_string()), true);
        assert_eq!(state.search.search_string, "drive");
        assert_eq!(
            state.search.search_string_after_enter_press.as_deref(),
            Some("drive")
        );
        assert!(state.search.is_enter_pressed);
        let app = CustomApp {
            id: "custom/1".into(),
            name: "Custom".into(),
            bx_app_manifest_url: "https://example.com/manifest.json".into(),
            ..Default::default()
        };
        state.set_selected_custom_app(app.clone());
        assert_eq!(state.selected_custom_app.app, app);
    }

    #[test]
    fn ui_state_roundtrips_camel_case() {
        let state = UiState::default();
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(
            json["appRequestMode"]["appRequestIsOpen"],
            serde_json::json!(false)
        );
        assert_eq!(
            json["search"]["searchStringAfterEnterPress"],
            serde_json::Value::Null
        );
        assert_eq!(
            json["appModalStatus"]["isAppModalStatus"],
            serde_json::json!(false)
        );
        let back: UiState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn screen_name_serializes_to_api_string() {
        assert_eq!(
            serde_json::to_value(ScreenName::OnboardEmployees).unwrap(),
            serde_json::json!("ONBOARD_EMPLOYEES")
        );
        let name: ScreenName = serde_json::from_value(serde_json::json!("ALL_EXTENSIONS")).unwrap();
        assert_eq!(name, ScreenName::AllExtensions);
    }
}
