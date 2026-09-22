//! Manifest multi-instance configuration rules: port of
//! `packages/app/src/abstract-application/helpers.ts`
//! (`isConfigurationRequired`, `label`, `applicationLabel`,
//! `getChromeExtensionId` — the latter lives in
//! [`crate::get_chrome_extension_id`]) plus the
//! `manifest-provider/helpers.ts` predicates they build on
//! (`getPresets`, `isMultiInstanceConfigurator`).
//!
//! The TS functions read application state through Immutable getters
//! (`identityId`, `subdomain`, `customURL`) and resolve the identity's
//! email from the store; the port takes a plain [`ApplicationConfigState`]
//! snapshot and the email string, so the rules stay pure and testable
//! without a store.

use handlebars::Handlebars;
use serde_json::json;

use crate::{BxAppManifest, Preset};

/// `BX_PROTOCOL` from `packages/app/src/webui/const.ts`.
pub const BX_PROTOCOL: &str = "station";

/// Application fields the config rules look at (TS Immutable getters
/// `getApplicationIdentityId` / `getApplicationSubdomain` /
/// `getApplicationCustomURL`).
///
/// `None` and `Some("")` are kept distinct because the TS getters return
/// the raw value and JS truthiness treats `''` as unconfigured.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplicationConfigState {
    pub identity_id: Option<String>,
    pub subdomain: Option<String>,
    pub custom_url: Option<String>,
}

impl ApplicationConfigState {
    fn identity_id_truthy(&self) -> bool {
        is_truthy(self.identity_id.as_deref())
    }
    fn subdomain_truthy(&self) -> bool {
        is_truthy(self.subdomain.as_deref())
    }
    fn custom_url_truthy(&self) -> bool {
        is_truthy(self.custom_url.as_deref())
    }
}

/// JS truthiness for `Option<String>`: `None` is `undefined`, `Some("")`
/// is `''` — both falsy.
fn is_truthy(v: Option<&str>) -> bool {
    v.is_some_and(|s| !s.is_empty())
}

/// `isConfigurationRequired` failure modes (TS throws plain `Error`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigRulesError {
    /// `"Missing MultiInstanceConfigPreset handler for isConfigurationRequired"`.
    #[error("Missing MultiInstanceConfigPreset handler for isConfigurationRequired")]
    MissingPresetHandler,
    /// A bad `instance_label_tpl` that handlebars cannot compile.
    #[error("invalid instance label template: {0}")]
    Template(String),
}

/// Port of `getPresets` (TS `oc(manifest).bx_multi_instance_config.presets([])`).
pub fn get_presets(manifest: &BxAppManifest) -> Vec<Preset> {
    manifest
        .bx_multi_instance_config
        .as_ref()
        .map(|c| c.presets.clone())
        .unwrap_or_default()
}

/// Port of `isMultiInstanceConfigurator`: does this URL point at the
/// in-app configurator (`station://multi-instance-configurator...`).
pub fn is_multi_instance_configurator(url: &str) -> bool {
    url.starts_with(&format!("{BX_PROTOCOL}://multi-instance-configurator"))
}

fn is_configuration_required_for_google_account(
    application: &ApplicationConfigState,
    presets: &[Preset],
) -> Option<bool> {
    presets.contains(&Preset::GoogleAccount).then(|| !application.identity_id_truthy())
}

fn is_configuration_required_for_subdomain(
    application: &ApplicationConfigState,
    presets: &[Preset],
) -> Option<bool> {
    let has_subdomain = presets.contains(&Preset::Subdomain);
    let has_on_premise = presets.contains(&Preset::OnPremise);
    if has_subdomain && has_on_premise {
        // on-premise + subdomain
        Some(!application.subdomain_truthy() && !application.custom_url_truthy())
    } else if has_subdomain && !has_on_premise {
        // subdomain without on-premise
        Some(!application.subdomain_truthy())
    } else {
        None
    }
}

fn is_configuration_required_for_on_premise(
    application: &ApplicationConfigState,
    presets: &[Preset],
    home_tab_url: &str,
) -> Option<bool> {
    if !presets.contains(&Preset::Subdomain) && presets.contains(&Preset::OnPremise) {
        let configured =
            application.custom_url_truthy() || !is_multi_instance_configurator(home_tab_url);
        Some(!configured)
    } else {
        None
    }
}

/// Port of `isConfigurationRequired`: does this application instance still
/// need its multi-instance configuration filled in?
///
/// Presets with no handler at all are an error, not a silent `false`.
pub fn is_configuration_required(
    manifest: &BxAppManifest,
    application: &ApplicationConfigState,
    home_tab_url: &str,
) -> Result<bool, ConfigRulesError> {
    let presets = get_presets(manifest);

    if presets.is_empty() {
        return Ok(false);
    }

    let need_configurations = [
        is_configuration_required_for_google_account(application, &presets),
        is_configuration_required_for_subdomain(application, &presets),
        is_configuration_required_for_on_premise(application, &presets, home_tab_url),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    if need_configurations.is_empty() {
        return Err(ConfigRulesError::MissingPresetHandler);
    }

    Ok(need_configurations.contains(&true))
}

/// `utils/handlebars.ts` `format` — handlebars render of `tpl` with `data`.
fn format(tpl: &str, data: serde_json::Value) -> Result<String, ConfigRulesError> {
    Handlebars::new()
        .render_template(tpl, &data)
        .map_err(|e| ConfigRulesError::Template(e.to_string()))
}

/// Port of `label`: the per-instance label for an application.
///
/// `identity_email` is the identity TS looks up via `getIdentityById` +
/// `getEmail`: `None` when the identity was not found (TS `if (identity)`
/// is falsy), `Some("")` when found with an empty email. Only
/// google-account presets consult it.
pub fn label(
    manifest: &BxAppManifest,
    application: &ApplicationConfigState,
    identity_email: Option<&str>,
) -> Result<String, ConfigRulesError> {
    let presets = get_presets(manifest);

    if presets.is_empty() {
        return Ok(manifest.name.clone().unwrap_or_default());
    }

    // TS looks the identity up in the store and only renders when found:
    // `None` here means "identity not found" (falls through to the name),
    // `Some("")` means found with an empty email (renders to an empty
    // label, which `application_label` then drops).
    if presets.contains(&Preset::GoogleAccount) && identity_email.is_some() {
        let tpl = instance_label_tpl(manifest);
        return format(tpl, json!({ "email": identity_email }));
    }

    if presets.contains(&Preset::Subdomain) && application.subdomain_truthy() {
        let tpl = instance_label_tpl(manifest);
        return format(tpl, json!({ "subdomain": application.subdomain }));
    }

    // on-premise + normal flow
    Ok(manifest.name.clone().unwrap_or_default())
}

fn instance_label_tpl(manifest: &BxAppManifest) -> &str {
    manifest
        .bx_multi_instance_config
        .as_ref()
        .and_then(|c| c.instance_label_tpl.as_deref())
        .unwrap_or_default()
}

/// Port of `applicationLabel`: `"<name> - <instance label>"`, dropping a
/// falsy (empty) instance label — including the TS quirk that the plain
/// `name` fallback is truthy and therefore appended again
/// (`"Gmail - Gmail"`).
pub fn application_label(
    manifest: &BxAppManifest,
    application: &ApplicationConfigState,
    identity_email: Option<&str>,
) -> Result<String, ConfigRulesError> {
    let name = manifest.name.clone().unwrap_or_default();
    let instance_label = label(manifest, application, identity_email)?;
    if instance_label.is_empty() {
        return Ok(name);
    }
    Ok(format!("{name} - {instance_label}"))
}
