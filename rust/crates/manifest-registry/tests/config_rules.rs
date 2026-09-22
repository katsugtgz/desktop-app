//! Golden cases for the manifest config rules (port of
//! `packages/app/src/abstract-application/helpers.ts` and the
//! `manifest-provider/helpers.ts` predicates it builds on).
//!
//! Fixtures are the real bundled manifests — Gmail (google-account), Desk
//! (subdomain), Gitlab (on-premise only), Atlassian and Gitlab custom
//! (subdomain + on-premise) — so the goldens double as
//! bundled-definition regression checks.

use manifest_registry::{
    application_label, get_application_by_id, get_presets, is_configuration_required,
    is_multi_instance_configurator, ApplicationConfigState, ConfigRulesError, Preset,
};

fn gmail() -> manifest_registry::Manifest {
    get_application_by_id("14").unwrap()
}
fn desk() -> manifest_registry::Manifest {
    get_application_by_id("101").unwrap()
}
fn gitlab() -> manifest_registry::Manifest {
    get_application_by_id("123").unwrap()
}
fn atlassian() -> manifest_registry::Manifest {
    get_application_by_id("100").unwrap()
}
fn gitlab_custom() -> manifest_registry::Manifest {
    get_application_by_id("124").unwrap()
}

fn manifest_json(json: &str) -> manifest_registry::Manifest {
    manifest_registry::Manifest {
        inner: serde_json::from_str(json).unwrap(),
        id: "test".into(),
        icon: String::new(),
    }
}

// ---- getPresets ---------------------------------------------------------

#[test]
fn golden_get_presets_from_bundled_definitions() {
    assert_eq!(get_presets(&gmail().inner), vec![Preset::GoogleAccount]);
    assert_eq!(get_presets(&desk().inner), vec![Preset::Subdomain]);
    assert_eq!(get_presets(&gitlab().inner), vec![Preset::OnPremise]);
    assert_eq!(
        get_presets(&atlassian().inner),
        vec![Preset::Subdomain, Preset::OnPremise]
    );
    assert_eq!(
        get_presets(&gitlab_custom().inner),
        vec![Preset::OnPremise, Preset::Subdomain]
    );
}

#[test]
fn golden_get_presets_absent_config() {
    let m = manifest_json(r#"{"name":"Slack"}"#);
    assert!(get_presets(&m.inner).is_empty());
    // empty presets array is [] too, not an error
    let m = manifest_json(r#"{"bx_multi_instance_config":{"presets":[]}}"#);
    assert!(get_presets(&m.inner).is_empty());
}

// ---- isMultiInstanceConfigurator ---------------------------------------

#[test]
fn golden_is_multi_instance_configurator() {
    assert!(is_multi_instance_configurator(
        "station://multi-instance-configurator/whatever"
    ));
    assert!(is_multi_instance_configurator(
        "station://multi-instance-configurator"
    ));
    assert!(!is_multi_instance_configurator("station://something-else"));
    assert!(!is_multi_instance_configurator(""));
    assert!(!is_multi_instance_configurator(
        "https://multi-instance-configurator.example.com"
    ));
}

// ---- isConfigurationRequired -------------------------------------------

fn state(
    identity_id: Option<&str>,
    subdomain: Option<&str>,
    custom_url: Option<&str>,
) -> ApplicationConfigState {
    ApplicationConfigState {
        identity_id: identity_id.map(str::to_owned),
        subdomain: subdomain.map(str::to_owned),
        custom_url: custom_url.map(str::to_owned),
    }
}

#[test]
fn golden_configuration_not_required_without_presets() {
    let m = manifest_json(r#"{"name":"Slack"}"#);
    assert!(!is_configuration_required(&m.inner, &state(None, None, None), "").unwrap());
}

#[test]
fn golden_google_account_requires_identity() {
    let manifest = gmail().inner;
    // no identity picked yet -> configuration required
    assert!(is_configuration_required(&manifest, &state(None, None, None), "").unwrap());
    // identity set -> configured
    assert!(
        !is_configuration_required(&manifest, &state(Some("auth0|123"), None, None), "").unwrap()
    );
}

#[test]
fn golden_subdomain_only_requires_subdomain() {
    let manifest = desk().inner;
    assert!(is_configuration_required(&manifest, &state(None, None, None), "").unwrap());
    assert!(!is_configuration_required(&manifest, &state(None, Some("acme"), None), "").unwrap());
    // a custom URL does NOT satisfy a subdomain-only app
    assert!(
        is_configuration_required(&manifest, &state(None, None, Some("https://x")), "").unwrap()
    );
}

#[test]
fn golden_subdomain_onpremise_accepts_either() {
    let manifest = atlassian().inner;
    // neither -> required
    assert!(is_configuration_required(&manifest, &state(None, None, None), "").unwrap());
    // subdomain -> configured
    assert!(!is_configuration_required(&manifest, &state(None, Some("acme"), None), "").unwrap());
    // custom URL alone -> configured
    assert!(!is_configuration_required(
        &manifest,
        &state(None, None, Some("https://git.local")),
        ""
    )
    .unwrap());
    // both -> configured
    assert!(!is_configuration_required(
        &manifest,
        &state(None, Some("acme"), Some("https://git.local")),
        ""
    )
    .unwrap());
}

#[test]
fn golden_onpremise_only_uses_custom_url_or_configurator_home_tab() {
    let manifest = gitlab().inner;
    // no custom URL and home tab is a real URL (user already navigated
    // past the configurator) -> configured, not required
    assert!(
        !is_configuration_required(&manifest, &state(None, None, None), "https://gitlab.com")
            .unwrap()
    );
    // custom URL -> configured
    assert!(!is_configuration_required(
        &manifest,
        &state(None, None, Some("https://git.local")),
        ""
    )
    .unwrap());
    // no custom URL and home tab still on the multi-instance configurator
    // (onboarding in progress) -> configuration still required
    assert!(is_configuration_required(
        &manifest,
        &state(None, None, None),
        "station://multi-instance-configurator/123"
    )
    .unwrap());
}

#[test]
fn golden_onpremise_order_independent() {
    // 124.json ships ['on-premise', 'subdomain'] — same rules as 100.json.
    let manifest = gitlab_custom().inner;
    assert!(is_configuration_required(&manifest, &state(None, None, None), "").unwrap());
    assert!(!is_configuration_required(&manifest, &state(None, Some("team"), None), "").unwrap());
}

#[test]
fn golden_unknown_preset_combination_errors() {
    // TS declares `undefined` as a preset but no isConfigurationRequired
    // handler covers it, so a manifest carrying it throws. No bundled
    // definition uses it; the Rust `Preset` enum rejects unknown
    // variants at parse time — an even earlier failure than the TS
    // throw — so that is the golden here.
    assert!(serde_json::from_str::<manifest_registry::Preset>("\"undefined\"").is_err());
    // and a Preset-only slice no handler covers is unreachable through
    // typed manifests; the runtime guard is exercised via the error type
    // contract:
    assert_eq!(
        ConfigRulesError::MissingPresetHandler.to_string(),
        "Missing MultiInstanceConfigPreset handler for isConfigurationRequired"
    );
}

#[test]
fn golden_empty_subdomain_string_still_requires_configuration() {
    // TS immutable getters return the raw value; `!''` is true, so an
    // empty-string subdomain does not count as configured.
    let manifest = desk().inner;
    assert!(is_configuration_required(&manifest, &state(None, Some(""), None), "").unwrap());
}

// ---- label / applicationLabel ------------------------------------------

#[test]
fn golden_google_account_label_uses_identity_email() {
    // Gmail's instance_label_tpl is "{{email}}".
    let manifest = gmail().inner;
    let identity_email = Some("user@gmail.com".to_owned());
    assert_eq!(
        application_label(
            &manifest,
            &state(Some("auth0|1"), None, None),
            identity_email.as_deref()
        )
        .unwrap(),
        "Gmail - user@gmail.com"
    );
}

#[test]
fn golden_google_account_identity_found_with_empty_email_drops_label() {
    // Identity found but its email renders to "": the instance label is
    // falsy in TS, so applicationLabel keeps only the manifest name.
    let manifest = gmail().inner;
    assert_eq!(
        application_label(&manifest, &state(Some("auth0|1"), None, None), Some("")).unwrap(),
        "Gmail"
    );
}

#[test]
fn golden_google_account_identity_not_found_double_names() {
    // Identity lookup failed (TS `if (identity)` falsy): label() falls
    // through to the manifest name, which applicationLabel appends again.
    let manifest = gmail().inner;
    assert_eq!(
        application_label(&manifest, &state(Some("auth0|1"), None, None), None).unwrap(),
        "Gmail - Gmail"
    );
    // no identity id at all: same path
    assert_eq!(
        application_label(&manifest, &state(None, None, None), None).unwrap(),
        "Gmail - Gmail"
    );
}

#[test]
fn golden_subdomain_label() {
    // Desk's instance_label_tpl is "{{subdomain}}.desk.com".
    let manifest = desk().inner;
    assert_eq!(
        application_label(&manifest, &state(None, Some("acme"), None), None).unwrap(),
        "Desk - acme.desk.com"
    );
}

#[test]
fn golden_subdomain_without_subdomain_falls_back_to_name() {
    let manifest = desk().inner;
    assert_eq!(
        application_label(&manifest, &state(None, None, None), None).unwrap(),
        "Desk - Desk"
    );
}

#[test]
fn golden_onpremise_only_has_no_instance_label() {
    // Gitlab (on-premise only, no instance_label_tpl): plain name.
    let manifest = gitlab().inner;
    assert_eq!(
        application_label(
            &manifest,
            &state(None, None, Some("https://git.local")),
            None
        )
        .unwrap(),
        "Gitlab - Gitlab"
    );
}

#[test]
fn golden_subdomain_onpremise_label_prefers_subdomain() {
    // Atlassian tpl is "{{subdomain}}.atlassian.net"; custom URL alone
    // does not produce an instance label.
    let manifest = atlassian().inner;
    assert_eq!(
        application_label(
            &manifest,
            &state(None, Some("acme"), Some("https://atlassian.local")),
            None
        )
        .unwrap(),
        "Atlassian (Jira, Confluence..) - acme.atlassian.net"
    );
    assert_eq!(
        application_label(
            &manifest,
            &state(None, None, Some("https://atlassian.local")),
            None
        )
        .unwrap(),
        "Atlassian (Jira, Confluence..) - Atlassian (Jira, Confluence..)"
    );
}

#[test]
fn golden_no_presets_label_is_just_name_with_quirk() {
    // No presets: label() returns the name, which is truthy, so TS pushes
    // it again — "Slack - Slack".
    let m = manifest_json(r#"{"name":"Slack"}"#);
    assert_eq!(
        application_label(&m.inner, &state(None, None, None), None).unwrap(),
        "Slack - Slack"
    );
}

#[test]
fn golden_empty_rendered_label_is_dropped() {
    // A tpl that renders to exactly "" is falsy in TS: applicationLabel
    // keeps only the manifest name. ("{{email}}" with no email in scope
    // renders "" — verified against real handlebars.)
    let m = manifest_json(
        r#"{"name":"Weird","bx_multi_instance_config":{"presets":["subdomain"],"instance_label_tpl":"{{email}}"}}"#,
    );
    assert_eq!(
        application_label(&m.inner, &state(None, Some("acme"), None), None).unwrap(),
        "Weird"
    );
    // sanity: non-empty render joins normally
    let m2 = manifest_json(
        r#"{"name":"Weird2","bx_multi_instance_config":{"presets":["subdomain"],"instance_label_tpl":"{{subdomain}}-x"}}"#,
    );
    assert_eq!(
        application_label(&m2.inner, &state(None, Some("acme"), None), None).unwrap(),
        "Weird2 - acme-x"
    );
}
