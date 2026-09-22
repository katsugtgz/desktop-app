//! App-request state machine (port of `packages/appstore/src/app-request/duck.ts`
//! plus the `handleAppRequest` saga from `packages/appstore/src/app-request/sagas.ts`).
//!
//! The TS duck is a redux slice: three actions, a reducer that only two of
//! them mutate, and a saga that drives Public/Private submissions. The port
//! keeps the action set ([`AppRequestAction`]), the reducer
//! ([`AppRequest::reduce`]) and the saga ([`submit_app_request`]) as plain
//! Rust so s14 can wire them to napi-rs without redux.

use manifest_registry::NewPrivateApplication;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::write_commands::{ApplicationCreated, ApplicationService};

/// `Steps` from the duck: the wizard routes the AppRequest component walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Steps {
    #[serde(rename = "not-visible")]
    NotVisible,
    #[serde(rename = "basic-info-form")]
    AppName,
    #[serde(rename = "disambiguation-form")]
    Disambiguation,
    #[serde(rename = "logo-url-form")]
    AppData,
    #[serde(rename = "confirmation-message")]
    Confirmation,
    #[serde(rename = "exit")]
    Exit,
    #[serde(rename = "legacy-bx-api-app-modal")]
    LegacyBxApiApp,
}

/// `Visibility` from the duck (API enum values kept verbatim).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    #[serde(rename = "USER_EMAIL")]
    Private,
    #[serde(rename = "PUBLIC")]
    Public,
}

/// `AppRequestData` from the duck — the wizard's collected form values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppRequestData {
    pub name: String,
    #[serde(rename = "themeColor")]
    pub theme_color: String,
    #[serde(rename = "logoURL")]
    pub logo_url: String,
    #[serde(rename = "signinURL")]
    pub signin_url: String,
    pub scope: String,
    pub visibility: Visibility,
}

/// `ApiResponse` from the duck. TS serializes the enum as its numeric value
/// (`0 | 1 | 2`) through redux-persist; serde does the same by hand below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiResponse {
    Error,
    Pending,
    Done,
}

impl Serialize for ApiResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(match self {
            ApiResponse::Error => 0,
            ApiResponse::Pending => 1,
            ApiResponse::Done => 2,
        })
    }
}

impl<'de> Deserialize<'de> for ApiResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match u8::deserialize(deserializer)? {
            0 => Ok(ApiResponse::Error),
            1 => Ok(ApiResponse::Pending),
            2 => Ok(ApiResponse::Done),
            n => Err(serde::de::Error::custom(format!(
                "unknown ApiResponse value: {n}"
            ))),
        }
    }
}

/// State slice `AppRequest` from the duck (`defaultState` = both `None`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppRequest {
    #[serde(default, rename = "apiResponse", skip_serializing_if = "Option::is_none")]
    pub api_response: Option<ApiResponse>,
    #[serde(
        default,
        rename = "applicationCreated",
        skip_serializing_if = "Option::is_none"
    )]
    pub application_created: Option<ApplicationCreated>,
}

/// The duck's action union (`SUBMIT_APP_REQUEST` / `API_RESPONSE` /
/// `APPLICATION_CREATED`).
#[derive(Debug, Clone, PartialEq)]
pub enum AppRequestAction {
    /// `submitAppRequest` — the saga input; the reducer returns state
    /// unchanged for it.
    SubmitAppRequest { data: AppRequestData },
    /// `setApiResponse`.
    ApiResponse { response: ApiResponse },
    /// `setApplicationCreated`.
    ApplicationCreated { application_created: ApplicationCreated },
}

impl AppRequest {
    /// Port of the duck's `reducer` default export.
    pub fn reduce(&mut self, action: AppRequestAction) {
        match action {
            // TS: `case SUBMIT_APP_REQUEST: return state;`
            AppRequestAction::SubmitAppRequest { .. } => {}
            AppRequestAction::ApiResponse { response } => self.api_response = Some(response),
            AppRequestAction::ApplicationCreated { application_created } => {
                self.application_created = Some(application_created)
            }
        }
    }

    /// `submit_app_request` wired to the real private-manifest store
    /// (`window.bxApi.applications.requestPrivate`, which cannot fail in the
    /// local port — `saveNewApplication` either persists or panics).
    pub fn submit_via(&mut self, request: &AppRequestData, apps: &mut ApplicationService) {
        submit_app_request(self, request, |recipe| {
            Ok::<_, std::convert::Infallible>(apps.request_private_application(recipe))
        });
    }
}

/// Port of the `handleAppRequest` saga: Public requests complete
/// immediately; Private ones go through `requestPrivate` and record the
/// created application, or land on `Error` when the call fails (the TS
/// try/catch around the bxApi IPC call).
pub fn submit_app_request<E>(
    state: &mut AppRequest,
    request: &AppRequestData,
    request_private: impl FnOnce(NewPrivateApplication) -> Result<ApplicationCreated, E>,
) {
    if request.visibility == Visibility::Public {
        state.reduce(AppRequestAction::ApiResponse {
            response: ApiResponse::Done,
        });
        return;
    }

    // Field mapping from `handleAppRequest`'s `applicationRecipe`.
    let recipe = NewPrivateApplication {
        name: request.name.clone(),
        theme_color: request.theme_color.clone(),
        icon_url: request.logo_url.clone(),
        start_url: request.signin_url.clone(),
        scope: request.scope.clone(),
    };

    match request_private(recipe) {
        Ok(created) => {
            state.reduce(AppRequestAction::ApiResponse {
                response: ApiResponse::Done,
            });
            state.reduce(AppRequestAction::ApplicationCreated {
                application_created: created,
            });
        }
        Err(_) => state.reduce(AppRequestAction::ApiResponse {
            response: ApiResponse::Error,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_request() -> AppRequestData {
        AppRequestData {
            name: "Acme Intranet".into(),
            theme_color: "#112233".into(),
            logo_url: "https://acme.test/logo.png".into(),
            signin_url: "https://acme.test/login".into(),
            scope: "https://acme.test".into(),
            visibility: Visibility::Private,
        }
    }

    #[test]
    fn golden_default_state_matches_duck_defaults() {
        let state = AppRequest::default();
        assert_eq!(state.api_response, None);
        assert_eq!(state.application_created, None);
    }

    #[test]
    fn golden_submit_action_is_a_reducer_noop() {
        let mut state = AppRequest::default();
        state.reduce(AppRequestAction::SubmitAppRequest {
            data: private_request(),
        });
        assert_eq!(state, AppRequest::default());
    }

    #[test]
    fn golden_public_flow_sets_done_without_creating_an_application() {
        // The component sets Pending before submitting (onSubmitAppData).
        let mut state = AppRequest::default();
        state.reduce(AppRequestAction::ApiResponse {
            response: ApiResponse::Pending,
        });
        let request = AppRequestData {
            visibility: Visibility::Public,
            ..private_request()
        };
        state.submit_via(&request, &mut dummy_service());
        assert_eq!(state.api_response, Some(ApiResponse::Done));
        assert_eq!(state.application_created, None);
    }

    #[test]
    fn golden_private_flow_success_sets_done_and_application_created() {
        let mut state = AppRequest::default();
        let mut apps = service("private-success");
        state.submit_via(&private_request(), &mut apps);
        assert_eq!(state.api_response, Some(ApiResponse::Done));
        let created = state.application_created.clone().unwrap();
        // First private id above the TS base of 1000000.
        assert_eq!(created.id, "1000001");
        assert_eq!(created.bx_app_manifest_url, "station-manifest://1000001");

        // Second request: id increments, store keeps both manifests.
        state.submit_via(&private_request(), &mut apps);
        assert_eq!(state.application_created.unwrap().id, "1000002");
        let manifests = apps.private_manifests();
        assert_eq!(manifests.len(), 2);
        assert_eq!(manifests[0].inner.name.as_deref(), Some("Acme Intranet"));
        assert_eq!(
            manifests[0].inner.category.as_deref(),
            Some("Miscellaneous")
        );
    }

    #[test]
    fn golden_private_flow_failure_sets_error() {
        let mut state = AppRequest::default();
        submit_app_request(&mut state, &private_request(), |_| {
            Err("simulated bxApi IPC failure")
        });
        assert_eq!(state.api_response, Some(ApiResponse::Error));
        assert_eq!(state.application_created, None);
    }

    #[test]
    fn api_response_serializes_as_the_ts_enum_number() {
        assert_eq!(serde_json::to_value(ApiResponse::Error).unwrap(), serde_json::json!(0));
        assert_eq!(serde_json::to_value(ApiResponse::Pending).unwrap(), serde_json::json!(1));
        assert_eq!(serde_json::to_value(ApiResponse::Done).unwrap(), serde_json::json!(2));
        assert_eq!(
            serde_json::from_value::<ApiResponse>(serde_json::json!(1)).unwrap(),
            ApiResponse::Pending
        );
        assert!(serde_json::from_value::<ApiResponse>(serde_json::json!(7)).is_err());
    }

    #[test]
    fn steps_and_visibility_serialize_to_their_ts_strings() {
        assert_eq!(
            serde_json::to_value(Steps::AppName).unwrap(),
            "basic-info-form"
        );
        assert_eq!(
            serde_json::from_value::<Steps>(serde_json::json!("legacy-bx-api-app-modal")).unwrap(),
            Steps::LegacyBxApiApp
        );
        assert_eq!(
            serde_json::to_value(Visibility::Private).unwrap(),
            serde_json::json!("USER_EMAIL")
        );
    }

    #[test]
    fn app_request_data_and_state_roundtrip_camel_case() {
        let data = private_request();
        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(json["themeColor"], serde_json::json!("#112233"));
        assert_eq!(json["logoURL"], serde_json::json!("https://acme.test/logo.png"));
        assert_eq!(json["signinURL"], serde_json::json!("https://acme.test/login"));
        assert_eq!(json["visibility"], serde_json::json!("USER_EMAIL"));
        assert_eq!(serde_json::from_value::<AppRequestData>(json).unwrap(), data);

        let mut state = AppRequest::default();
        state.reduce(AppRequestAction::ApiResponse {
            response: ApiResponse::Done,
        });
        state.reduce(AppRequestAction::ApplicationCreated {
            application_created: ApplicationCreated {
                id: "1000001".into(),
                bx_app_manifest_url: "station-manifest://1000001".into(),
            },
        });
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["apiResponse"], serde_json::json!(2));
        assert_eq!(
            json["applicationCreated"]["bxAppManifestURL"],
            "station-manifest://1000001"
        );
        assert_eq!(
            serde_json::from_value::<AppRequest>(json).unwrap(),
            state
        );

        // Default state serializes both optional fields away.
        let json = serde_json::to_value(AppRequest::default()).unwrap();
        assert!(json.get("apiResponse").is_none());
        assert!(json.get("applicationCreated").is_none());
    }

    fn service(name: &str) -> ApplicationService {
        ApplicationService::for_tests(&format!(
            "s11b-app-request-{name}-{}",
            std::process::id()
        ))
    }

    fn dummy_service() -> ApplicationService {
        service("dummy")
    }
}
