//! Wire-format and credential-redaction tests.
use super::*;

#[test]
fn config_defaults_and_redacts_credentials() -> serde_json::Result<()> {
    let mut config = SearchConfig::default();
    config.backend.credential = Some("backend-secret".into());
    config.providers.insert(
        "exa".into(),
        ProviderConfig {
            credential: Some("provider-secret".into()),
            ..ProviderConfig::default()
        },
    );
    let debug = format!("{config:?}");
    assert!(!debug.contains("backend-secret"));
    assert!(!debug.contains("provider-secret"));
    let decoded: SearchConfig = serde_json::from_value(serde_json::json!({}))?;
    assert!(decoded.enabled);
    assert_eq!(decoded.presentation.mode, PresentationMode::Roles);
    assert!(decoded.presentation.roles.is_empty());
    Ok(())
}

#[test]
fn execution_wire_names_are_stable() -> serde_json::Result<()> {
    let request = ExecuteToolRequest {
        name: "web_search_tool".into(),
        arguments: serde_json::json!({"query":"rust"}),
    };
    assert_eq!(
        serde_json::to_value(request)?,
        serde_json::json!({"name":"web_search_tool","arguments":{"query":"rust"}})
    );
    Ok(())
}

#[test]
fn search_result_publication_is_optional_on_the_wire() -> serde_json::Result<()> {
    let old: SearchResult = serde_json::from_value(serde_json::json!({
        "title":"Old", "url":"https://example.test", "snippet":null
    }))?;
    assert_eq!(old.published, None);
    assert!(serde_json::to_value(&old)?.get("published").is_none());

    let dated: SearchResult = serde_json::from_value(serde_json::json!({
        "title":"Dated", "url":"https://example.test", "snippet":null,
        "published":"2026-01-02"
    }))?;
    assert_eq!(dated.published.as_deref(), Some("2026-01-02"));
    assert_eq!(serde_json::to_value(dated)?["published"], "2026-01-02");
    Ok(())
}

#[test]
fn provider_limits_round_trip_without_exposing_credential_in_debug() -> serde_json::Result<()> {
    let config: ProviderConfig = serde_json::from_value(serde_json::json!({
        "credential":"private-key", "max_results":12, "timeout_secs":8,
        "default_language":"fr"
    }))?;
    assert_eq!(config.max_results, Some(12));
    assert_eq!(config.timeout_secs, Some(8));
    assert_eq!(config.default_language.as_deref(), Some("fr"));
    let serialized = serde_json::to_value(&config)?;
    assert_eq!(serialized["max_results"], 12);
    assert_eq!(serialized["timeout_secs"], 8);
    assert_eq!(serialized["default_language"], "fr");
    assert!(!format!("{config:?}").contains("private-key"));
    let legacy: ProviderConfig =
        serde_json::from_value(serde_json::json!({"credential":"private-key"}))?;
    assert_eq!(legacy.max_results, None);
    assert_eq!(legacy.timeout_secs, None);
    assert_eq!(legacy.default_language, None);
    Ok(())
}

#[test]
fn roles_and_role_lists_round_trip_as_snake_case() -> serde_json::Result<()> {
    let presentation: PresentationConfig = serde_json::from_value(serde_json::json!({
        "mode":"roles",
        "roles":{"search":["brave","exa"],"answer":["gemini"],"contents":[]}
    }))?;
    assert_eq!(presentation.mode, PresentationMode::Roles);
    assert_eq!(presentation.roles[&Role::Search], ["brave", "exa"]);
    assert_eq!(presentation.roles[&Role::Answer], ["gemini"]);
    assert_eq!(presentation.roles[&Role::Contents].len(), 0);
    assert_eq!(
        serde_json::to_value(&presentation)?["roles"]["search"],
        serde_json::json!(["brave", "exa"])
    );
    assert_eq!(serde_json::to_value(Role::Contents)?, "contents");
    assert_eq!(Role::Answer.to_string(), "answer");
    for mode in ["roles", "all_tools", "router", "one_provider"] {
        let decoded: PresentationMode = serde_json::from_value(serde_json::json!(mode))?;
        assert_eq!(serde_json::to_value(decoded)?, mode);
    }
    Ok(())
}

#[test]
fn role_response_fields_are_optional_on_the_wire() -> serde_json::Result<()> {
    let legacy: ExecuteToolResponse = serde_json::from_value(serde_json::json!({
        "provider":"exa","results":[],"citations":[],"answer":null,"status":"empty"
    }))?;
    assert_eq!(legacy.role, None);
    assert_eq!(legacy.fallback_from.len(), 0);
    let encoded = serde_json::to_value(&legacy)?;
    assert!(encoded.get("role").is_none());
    assert!(encoded.get("fallback_from").is_none());

    let routed = ExecuteToolResponse {
        role: Some(Role::Search),
        fallback_from: vec!["exa".into()],
        provider: "brave".into(),
        ..legacy
    };
    let encoded = serde_json::to_value(&routed)?;
    assert_eq!(encoded["role"], "search");
    assert_eq!(encoded["fallback_from"], serde_json::json!(["exa"]));
    assert_eq!(
        serde_json::from_value::<ExecuteToolResponse>(encoded)?,
        routed
    );
    Ok(())
}

#[test]
fn role_helpers_round_trip_config_keys() {
    assert_eq!(ROLES, [Role::Search, Role::Answer, Role::Contents]);
    for role in ROLES {
        assert_eq!(parse_role(role_key(role)), Some(role));
        assert_eq!(parse_role(&format!("  {} ", role_key(role))), Some(role));
    }
    assert_eq!(role_key(Role::Contents), "contents");
    assert_eq!(parse_role("Search"), None);
    assert_eq!(parse_role("nope"), None);
}

#[test]
fn presentation_mode_names_parse_and_match_serde() -> serde_json::Result<()> {
    for mode in [
        PresentationMode::Roles,
        PresentationMode::AllTools,
        PresentationMode::Router,
        PresentationMode::OneProvider,
    ] {
        assert_eq!(PresentationMode::parse(mode.as_str()), Some(mode));
        assert_eq!(
            serde_json::to_value(mode)?,
            serde_json::json!(mode.as_str())
        );
    }
    assert_eq!(
        PresentationMode::parse(" Router "),
        Some(PresentationMode::Router)
    );
    assert_eq!(PresentationMode::parse("x"), None);
    Ok(())
}

#[test]
fn provider_route_keeps_wire_and_config_spellings() -> serde_json::Result<()> {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Holder {
        #[serde(with = "managed_route")]
        route: ProviderRoute,
    }

    assert_eq!(
        serde_json::to_value(ProviderRoute::Backend)?,
        serde_json::json!("backend")
    );
    let aliased: ProviderRoute = serde_json::from_value(serde_json::json!("managed"))?;
    assert_eq!(aliased, ProviderRoute::Backend);
    assert_eq!(ProviderRoute::Backend.config_name(), "managed");
    assert_eq!(
        ProviderRoute::parse("Managed"),
        Some(ProviderRoute::Backend)
    );
    assert_eq!(
        ProviderRoute::parse("backend"),
        Some(ProviderRoute::Backend)
    );
    assert_eq!(ProviderRoute::parse("direct"), Some(ProviderRoute::Direct));
    assert_eq!(ProviderRoute::parse("x"), None);

    let holder = Holder {
        route: ProviderRoute::Backend,
    };
    assert_eq!(
        serde_json::to_value(&holder)?,
        serde_json::json!({"route": "managed"})
    );
    for spelling in ["managed", "backend"] {
        let decoded: Holder = serde_json::from_value(serde_json::json!({"route": spelling}))?;
        assert_eq!(decoded, holder);
    }
    assert!(serde_json::from_value::<Holder>(serde_json::json!({"route": "x"})).is_err());
    Ok(())
}
