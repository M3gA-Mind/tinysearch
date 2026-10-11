//! Catalog, credential filtering, and role presentation tests.
use super::*;
use crate::{ProviderConfig, names::tools};

fn keyed(credential: &str) -> ProviderConfig {
    ProviderConfig {
        credential: Some(credential.into()),
        ..ProviderConfig::default()
    }
}

fn backend_routed() -> ProviderConfig {
    ProviderConfig {
        route: ProviderRoute::Backend,
        ..ProviderConfig::default()
    }
}

fn names(response: &ListToolsResponse) -> Vec<&str> {
    response
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect()
}

#[test]
fn catalog_and_selection_are_stable() {
    let specs = provider_tool_specs();
    assert_eq!(
        specs.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "brave",
            "exa",
            "gemini",
            "gemini_deep_research",
            "keenable",
            "parallel",
            "querit",
            "searxng",
            "seltz",
            "tavily",
            "tinyfish"
        ]
    );
    assert_eq!(specs["tinyfish"].len(), 3);
    assert_eq!(specs["keenable"].len(), 2);
    assert_eq!(specs["exa"].len(), 4);
    assert_eq!(specs["parallel"].len(), 9);
    let all = PresentationConfig {
        mode: PresentationMode::AllTools,
        ..PresentationConfig::default()
    };
    assert_eq!(select_tools(&specs, &all).tools.len(), 29);
}

#[test]
fn parallel_is_direct_only() {
    let specs = provider_tool_specs();
    assert!(PROVIDERS.contains(&"parallel"));
    assert!(!BACKEND_PROVIDERS.contains(&"parallel"));
    let mut config = SearchConfig::default();
    config.backend.credential = Some("secret".into());
    config.providers.insert("parallel".into(), backend_routed());
    assert!(
        configured_provider_tools(&config, &specs).is_empty(),
        "a backend route never makes Parallel usable"
    );
    config.providers.insert("parallel".into(), keyed(" "));
    assert!(configured_provider_tools(&config, &specs).is_empty());
    config
        .providers
        .insert("parallel".into(), keyed("parallel-key"));
    let available = configured_provider_tools(&config, &specs);
    assert_eq!(available["parallel"], specs["parallel"]);
    let names: Vec<&str> = available["parallel"]
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert!(names.contains(&"parallel_dataset_status"));
    let extract = &available["parallel"][1];
    assert_eq!(extract.name, "parallel_extract");
    assert!(extract.parameters["properties"].get("excerpts").is_none());
    for role in Role::ALL {
        assert_eq!(
            role_providers(&available, &config.presentation, role),
            ["parallel"]
        );
    }
}

#[test]
fn findall_match_conditions_require_name_and_description() -> Result<(), String> {
    let specs = provider_tool_specs();
    let dataset = specs["parallel"]
        .iter()
        .find(|tool| tool.name == "parallel_dataset")
        .ok_or("missing dataset tool")?;
    let condition = &dataset.parameters["properties"]["match_conditions"]["items"];
    assert_eq!(condition["required"], json!(["name", "description"]));
    assert_eq!(condition["properties"]["description"]["minLength"], 1);
    Ok(())
}

#[test]
fn every_provider_has_specs_roles_and_role_tools() {
    let specs = provider_tool_specs();
    for provider in PROVIDERS {
        assert!(specs.contains_key(*provider), "{provider} has no specs");
        let roles = provider_roles(provider);
        assert!(!roles.is_empty(), "{provider} serves no role");
        for role in roles {
            let tool = role_provider_tool(*role, provider).unwrap_or_default();
            assert!(
                specs[*provider].iter().any(|spec| spec.name == tool),
                "{provider} lacks {tool}"
            );
            assert!(default_role_providers(*role).contains(provider));
        }
    }
    assert_eq!(specs.len(), PROVIDERS.len());
}

#[test]
fn provider_roles_and_defaults_match_the_contract() {
    for provider in ["exa", "parallel"] {
        assert_eq!(
            provider_roles(provider),
            [Role::Search, Role::Answer, Role::Contents]
        );
    }
    assert_eq!(
        role_provider_tool(Role::Search, "parallel"),
        Some("parallel_search")
    );
    assert_eq!(
        role_provider_tool(Role::Answer, "parallel"),
        Some("parallel_chat")
    );
    assert_eq!(
        role_provider_tool(Role::Contents, "parallel"),
        Some("parallel_extract")
    );
    assert_eq!(provider_roles("unknown").len(), 0);
    assert_eq!(provider_roles("gemini"), [Role::Search, Role::Answer]);
    assert_eq!(
        role_provider_tool(Role::Search, "gemini"),
        Some("gemini_agentic_search")
    );
    assert_eq!(provider_roles("gemini_deep_research"), [Role::Answer]);
    assert_eq!(provider_roles("tinyfish"), [Role::Search, Role::Contents]);
    assert_eq!(provider_roles("tavily"), [Role::Search, Role::Contents]);
    assert_eq!(provider_roles("keenable"), [Role::Search, Role::Contents]);
    assert_eq!(
        role_provider_tool(Role::Search, "keenable"),
        Some("keenable_search")
    );
    assert_eq!(
        role_provider_tool(Role::Contents, "keenable"),
        Some("keenable_fetch")
    );
    assert_eq!(role_provider_tool(Role::Answer, "keenable"), None);
    for provider in ["brave", "querit", "seltz", "searxng"] {
        assert_eq!(provider_roles(provider), [Role::Search]);
    }
    assert_eq!(
        default_role_providers(Role::Search),
        [
            "exa", "brave", "tavily", "parallel", "querit", "seltz", "searxng", "tinyfish",
            "keenable", "gemini"
        ]
    );
    assert_eq!(
        default_role_providers(Role::Answer),
        ["gemini", "gemini_deep_research", "exa", "parallel"]
    );
    assert_eq!(
        default_role_providers(Role::Contents),
        ["exa", "tavily", "parallel", "tinyfish", "keenable"]
    );
    assert_eq!(
        PROVIDERS,
        [
            "exa",
            "gemini",
            "gemini_deep_research",
            "tinyfish",
            "parallel",
            "brave",
            "querit",
            "tavily",
            "seltz",
            "searxng",
            "keenable",
            "gemini"
        ]
    );
    // The TinyHumans backend does not proxy TinyFish: it is own-key only.
    assert_eq!(BACKEND_PROVIDERS, ["exa", "gemini"]);
}

#[test]
fn searxng_categories_match_supported_execution_values() {
    let specs = provider_tool_specs();
    assert_eq!(
        specs["searxng"][0].parameters["properties"]["categories"]["items"]["enum"],
        json!(["web", "general", "news", "images"])
    );
}

#[test]
fn direct_tools_require_a_nonempty_private_credential() {
    let specs = provider_tool_specs();
    let mut config = SearchConfig::default();
    for name in [
        "exa", "parallel", "brave", "querit", "tavily", "seltz", "tinyfish",
    ] {
        config.providers.insert(name.into(), keyed("  "));
    }
    assert!(configured_provider_tools(&config, &specs).is_empty());
    for provider in config.providers.values_mut() {
        provider.credential = Some("secret".into());
    }
    let available = configured_provider_tools(&config, &specs);
    assert_eq!(available["exa"].len(), 4);
    assert_eq!(available["parallel"].len(), 9);
    assert_eq!(available["brave"].len(), 4);
    assert_eq!(available["querit"].len(), 1);
    assert_eq!(available["tavily"].len(), 2);
    assert_eq!(available["seltz"].len(), 1);
    assert_eq!(available["tinyfish"].len(), 3);
}

#[test]
fn backend_route_is_limited_to_backend_providers_with_a_credential() {
    let specs = provider_tool_specs();
    let mut config = SearchConfig::default();
    for provider in PROVIDERS {
        config
            .providers
            .insert((*provider).into(), backend_routed());
    }
    assert!(configured_provider_tools(&config, &specs).is_empty());
    config.backend.credential = Some(" ".into());
    assert!(configured_provider_tools(&config, &specs).is_empty());
    config.backend.credential = Some("session".into());
    let available = configured_provider_tools(&config, &specs);
    assert_eq!(
        available.keys().map(String::as_str).collect::<Vec<_>>(),
        ["exa", "gemini"]
    );
}

#[test]
fn a_backend_credential_alone_enables_nothing() {
    let mut config = SearchConfig::default();
    config.backend.credential = Some("session".into());
    assert!(configured_provider_tools(&config, &provider_tool_specs()).is_empty());
}

#[test]
fn backend_route_narrows_exa_search_and_gemini_models() {
    let mut config = SearchConfig::default();
    config.backend.credential = Some("session".into());
    config.providers.insert("exa".into(), backend_routed());
    config.providers.insert("gemini".into(), backend_routed());
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let exa_search = &available["exa"][0];
    assert_eq!(exa_search.name, "exa_search");
    assert_eq!(
        exa_search.parameters["properties"],
        json!({"query":{"type":"string","minLength":1}})
    );
    assert_eq!(
        available["gemini"][0].parameters["properties"]["model"]["enum"][0],
        "gemini-3.8-flash"
    );
}

fn role_config() -> SearchConfig {
    let mut config = SearchConfig::default();
    config.backend.credential = Some("session".into());
    config.providers.insert("exa".into(), backend_routed());
    config.providers.insert("gemini".into(), backend_routed());
    config.providers.insert("tinyfish".into(), keyed("tf-key"));
    config.providers.insert("brave".into(), keyed("brave-key"));
    config
}

#[test]
fn roles_mode_lists_one_tool_per_served_role() {
    let config = role_config();
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let listed = select_tools(&available, &config.presentation);
    assert_eq!(
        names(&listed),
        [tools::WEB_SEARCH, tools::WEB_ANSWER, tools::WEB_CONTENTS]
    );
    assert_eq!(
        role_providers(&available, &config.presentation, Role::Search),
        ["exa", "brave", "tinyfish", "gemini"]
    );
    assert_eq!(
        role_providers(&available, &config.presentation, Role::Answer),
        ["gemini", "exa"]
    );
    assert_eq!(
        role_providers(&available, &config.presentation, Role::Contents),
        ["exa", "tinyfish"]
    );
}

#[test]
fn roles_mode_without_backend_credential_drops_backend_providers() {
    let mut config = role_config();
    config.backend.credential = None;
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let listed = select_tools(&available, &config.presentation);
    // Own-key providers survive: TinyFish runs on its key, not the backend.
    assert_eq!(names(&listed), [tools::WEB_SEARCH, tools::WEB_CONTENTS]);
    assert_eq!(
        listed.tools[0].parameters["properties"]["provider"]["enum"],
        json!(["brave", "tinyfish"])
    );
    assert_eq!(
        listed.tools[1].parameters["properties"]["provider"]["enum"],
        json!(["tinyfish"])
    );
    assert_eq!(
        select_tools(&BTreeMap::new(), &config.presentation)
            .tools
            .len(),
        0
    );
}

#[test]
fn configured_role_order_wins_and_skips_unusable_or_unfit_providers() {
    let mut config = role_config();
    config.presentation.roles.insert(
        Role::Search,
        vec![
            "brave".into(),
            "gemini".into(),
            "tavily".into(),
            "brave".into(),
            "exa".into(),
        ],
    );
    config.presentation.roles.insert(Role::Answer, Vec::new());
    let available = configured_provider_tools(&config, &provider_tool_specs());
    assert_eq!(
        role_providers(&available, &config.presentation, Role::Search),
        ["brave", "gemini", "exa"]
    );
    assert_eq!(
        role_providers(&available, &config.presentation, Role::Answer),
        ["gemini", "exa"]
    );
}

#[test]
fn role_tool_schemas_are_generic() -> Result<(), String> {
    let config = role_config();
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let search =
        role_tool_specs(&available, &config.presentation, Role::Search).ok_or("missing search")?;
    assert_eq!(search.name, "web_search_tool");
    assert_eq!(search.parameters["required"], json!(["query"]));
    assert_eq!(search.parameters["additionalProperties"], false);
    assert_eq!(
        search.parameters["properties"]["max_results"],
        json!({"type":"integer","minimum":1,"maximum":20})
    );
    assert_eq!(
        search.parameters["properties"]["provider"]["enum"],
        json!(["exa", "brave", "tinyfish", "gemini"])
    );
    assert!(search.description.contains("exa, brave, tinyfish"));

    let answer =
        role_tool_specs(&available, &config.presentation, Role::Answer).ok_or("missing answer")?;
    assert_eq!(answer.name, "web_answer_tool");
    assert_eq!(answer.parameters["required"], json!(["query"]));
    assert_eq!(
        answer.parameters["properties"]["depth"],
        json!({"type":"string","enum":["quick","deep"],"default":"quick"}),
        "deep is still accepted (answered quick) without a deep-research provider"
    );

    let contents = role_tool_specs(&available, &config.presentation, Role::Contents)
        .ok_or("missing contents")?;
    assert_eq!(contents.name, "web_contents_tool");
    assert_eq!(contents.parameters["required"], json!(["urls"]));
    assert_eq!(contents.parameters["properties"]["urls"]["minItems"], 1);
    assert!(contents.parameters["properties"].get("query").is_some());
    Ok(())
}

#[test]
fn quick_only_answer_still_accepts_deep_depth() -> Result<(), String> {
    // Models send `depth: "deep"` for research questions whether or not Deep
    // Research is usable, and hosts validate against this declaration before
    // calling the module. Declaring only `quick` made those calls fail
    // (production traces, Oct 2026), so `deep` stays declared and the module
    // answers it at quick depth.
    let mut config = SearchConfig::default();
    config
        .providers
        .insert("gemini".into(), keyed("google-key"));
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let answer =
        role_tool_specs(&available, &config.presentation, Role::Answer).ok_or("missing answer")?;
    assert_eq!(
        answer.parameters["properties"]["depth"],
        json!({"type":"string","enum":["quick","deep"],"default":"quick"})
    );
    assert!(
        answer
            .description
            .contains("depth=\"deep\" is answered at quick depth"),
        "{}",
        answer.description
    );
    assert!(!answer.description.contains("multi-step research report"));
    Ok(())
}

#[test]
fn deep_research_answer_keeps_deep_note() -> Result<(), String> {
    let mut config = SearchConfig::default();
    for name in ["gemini", "gemini_deep_research"] {
        config.providers.insert(name.into(), keyed("google-key"));
    }
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let answer =
        role_tool_specs(&available, &config.presentation, Role::Answer).ok_or("missing answer")?;
    assert_eq!(
        answer.parameters["properties"]["depth"],
        json!({"type":"string","enum":["quick","deep"],"default":"quick"})
    );
    assert!(answer.description.contains("multi-step research report"));
    assert!(!answer.description.contains("answered at quick depth"));
    Ok(())
}

#[test]
fn deep_research_alone_serves_only_deep_answers() -> Result<(), String> {
    let mut config = SearchConfig::default();
    config
        .providers
        .insert("gemini_deep_research".into(), keyed("google-key"));
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let answer =
        role_tool_specs(&available, &config.presentation, Role::Answer).ok_or("missing answer")?;
    assert_eq!(
        answer.parameters["properties"]["depth"],
        json!({"type":"string","enum":["deep"],"default":"deep"})
    );
    assert!(answer.description.contains("depth=\"deep\""));
    Ok(())
}

#[test]
fn legacy_modes_still_select_provider_tools() {
    let config = role_config();
    let available = configured_provider_tools(&config, &provider_tool_specs());
    let one = PresentationConfig {
        mode: PresentationMode::OneProvider,
        provider: Some("brave".into()),
        ..PresentationConfig::default()
    };
    assert_eq!(select_tools(&available, &one).tools.len(), 4);
    let router = PresentationConfig {
        mode: PresentationMode::Router,
        ..PresentationConfig::default()
    };
    assert_eq!(names(&select_tools(&available, &router)), ["search"]);
}
