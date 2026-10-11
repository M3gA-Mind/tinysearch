//! Provider-independent service tests.
use super::*;
use crate::SearchStatus;
use serde_json::json;
use tinysearch_bus::{PresentationConfig, ProviderRoute, SearchStatus as Status};

struct MockProvider;
impl SearchProvider for MockProvider {
    fn execute<'a>(
        &'a self,
        _config: &'a ProviderConfig,
        _backend: &'a BackendConfig,
        _request: &'a ExecuteToolRequest,
    ) -> ProviderFuture<'a> {
        Box::pin(async {
            Ok(ExecuteToolResponse {
                provider: "ignored".into(),
                results: vec![],
                citations: vec![],
                answer: None,
                status: Status::Empty,
                provider_data: None,
                role: None,
                fallback_from: vec![],
            })
        })
    }
}
fn service(mode: PresentationMode) -> SearchService {
    let mut config = SearchConfig {
        presentation: PresentationConfig {
            mode,
            ..PresentationConfig::default()
        },
        ..SearchConfig::default()
    };
    config.backend.credential = Some("test-key".into());
    config.providers.insert(
        "tinyfish".into(),
        ProviderConfig {
            credential: Some("tf-key".into()),
            ..ProviderConfig::default()
        },
    );
    config.providers.insert(
        "searxng".into(),
        ProviderConfig {
            base_url: Some("http://127.0.0.1:8080".into()),
            ..ProviderConfig::default()
        },
    );
    let providers: BTreeMap<String, Arc<dyn SearchProvider>> = [
        (
            "tinyfish".into(),
            Arc::new(MockProvider) as Arc<dyn SearchProvider>,
        ),
        (
            "searxng".into(),
            Arc::new(MockProvider) as Arc<dyn SearchProvider>,
        ),
    ]
    .into();
    SearchService::with_providers(config, providers)
}
#[test]
fn all_tools_lists_enabled_providers_in_stable_order() {
    let tools = service(PresentationMode::AllTools).list_tools().tools;
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [
            "searxng_search",
            "tinyfish_search",
            "tinyfish_fetch",
            "tinyfish_agent_run"
        ]
    );
}
#[tokio::test]
async fn router_selects_explicit_provider() -> crate::Result<()> {
    let response = service(PresentationMode::Router)
        .execute_tool(ExecuteToolRequest {
            name: "search".into(),
            arguments: json!({"query":"rust","provider":"searxng"}),
        })
        .await?;
    assert_eq!(response.provider, "searxng");
    assert_eq!(response.status, SearchStatus::Empty);
    Ok(())
}
#[tokio::test]
async fn router_defaults_to_the_first_search_role_provider() -> crate::Result<()> {
    use std::sync::Mutex;
    struct RecordingProvider(Arc<Mutex<Option<ExecuteToolRequest>>>);
    impl SearchProvider for RecordingProvider {
        fn execute<'a>(
            &'a self,
            _config: &'a ProviderConfig,
            _backend: &'a BackendConfig,
            request: &'a ExecuteToolRequest,
        ) -> ProviderFuture<'a> {
            if let Ok(mut captured) = self.0.lock() {
                *captured = Some(request.clone());
            }
            Box::pin(async {
                Ok(ExecuteToolResponse {
                    provider: String::new(),
                    results: vec![],
                    citations: vec![],
                    answer: None,
                    status: Status::Empty,
                    provider_data: None,
                    role: None,
                    fallback_from: vec![],
                })
            })
        }
    }
    let captured = Arc::new(Mutex::new(None));
    let mut config = SearchConfig::default();
    config.presentation.mode = PresentationMode::Router;
    config.backend.credential = Some("test-key".into());
    config.providers.insert(
        "exa".into(),
        ProviderConfig {
            route: ProviderRoute::Backend,
            ..ProviderConfig::default()
        },
    );
    config.providers.insert(
        "tavily".into(),
        ProviderConfig {
            credential: Some("other-key".into()),
            ..ProviderConfig::default()
        },
    );
    let providers: BTreeMap<String, Arc<dyn SearchProvider>> = [
        (
            "exa".into(),
            Arc::new(RecordingProvider(captured.clone())) as Arc<dyn SearchProvider>,
        ),
        (
            "tavily".into(),
            Arc::new(MockProvider) as Arc<dyn SearchProvider>,
        ),
    ]
    .into();
    let response = SearchService::with_providers(config, providers)
        .execute_tool(ExecuteToolRequest {
            name: "search".into(),
            arguments: json!({"query":"rust"}),
        })
        .await?;
    assert_eq!(response.provider, "exa");
    let captured = captured
        .lock()
        .map_err(|_| Error::Provider("test capture poisoned".into()))?;
    let request = captured
        .as_ref()
        .ok_or_else(|| Error::Provider("provider request was not captured".into()))?;
    assert_eq!(request.name, "exa_search");
    assert_eq!(request.arguments, json!({"query":"rust"}));
    Ok(())
}
#[tokio::test]
async fn router_never_dispatches_search_to_a_non_search_provider() {
    // Deep Research remains Answer-only. A direct credential must not
    // advertise it as a Search provider in Router mode.
    let mut config = SearchConfig::default();
    config.presentation.mode = PresentationMode::Router;
    config.providers.insert(
        "gemini_deep_research".into(),
        ProviderConfig {
            credential: Some("test-key".into()),
            ..ProviderConfig::default()
        },
    );
    let providers: BTreeMap<String, Arc<dyn SearchProvider>> = [(
        "gemini_deep_research".into(),
        Arc::new(MockProvider) as Arc<dyn SearchProvider>,
    )]
    .into();
    let service = SearchService::with_providers(config, providers);
    assert_eq!(service.list_tools().tools.len(), 0);
    let result = service
        .execute_tool(ExecuteToolRequest {
            name: "search".into(),
            arguments: json!({"query":"rust"}),
        })
        .await;
    assert_eq!(result.err(), Some(Error::UnavailableTool("search".into())));
}
#[tokio::test]
async fn rejects_unadvertised_tool_and_invalid_arguments() {
    let service = service(PresentationMode::AllTools);
    assert_eq!(
        service
            .execute_tool(ExecuteToolRequest {
                name: "missing".into(),
                arguments: json!({})
            })
            .await
            .err(),
        Some(Error::UnavailableTool("missing".into()))
    );
    assert_eq!(
        service
            .execute_tool(ExecuteToolRequest {
                name: "tinyfish_search".into(),
                arguments: json!(null)
            })
            .await
            .err(),
        Some(Error::InvalidArguments)
    );
}
#[test]
fn disabling_search_suppresses_tools() {
    let mut service = service(PresentationMode::AllTools);
    service.config.enabled = false;
    assert_eq!(service.list_tools().tools.len(), 0);
}

#[tokio::test]
async fn router_rejects_unsupported_provider_option() {
    let result = service(PresentationMode::Router)
        .execute_tool(ExecuteToolRequest {
            name: "search".into(),
            arguments: json!({"query":"rust","provider":"tinyfish","unexpected":true}),
        })
        .await;
    assert_eq!(
        result.err(),
        Some(Error::UnsupportedArgument("unexpected".into()))
    );
}

#[tokio::test]
async fn catalog_rejects_missing_and_wrong_type_before_dispatch() {
    let service = service(PresentationMode::AllTools);
    for arguments in [json!({}), json!({"query": 42}), json!({"query": null})] {
        assert_eq!(
            service
                .execute_tool(ExecuteToolRequest {
                    name: "tinyfish_search".into(),
                    arguments,
                })
                .await
                .err(),
            Some(Error::InvalidArguments)
        );
    }
    assert_eq!(
        service
            .execute_tool(ExecuteToolRequest {
                name: "tinyfish_search".into(),
                arguments: json!({"query":"rust", "extra": true}),
            })
            .await
            .err(),
        Some(Error::UnsupportedArgument("extra".into()))
    );
}

#[tokio::test]
async fn router_rejects_invalid_provider_type_and_missing_query() {
    let service = service(PresentationMode::Router);
    for arguments in [
        json!({"provider": 7, "query":"rust"}),
        json!({"provider":"tinyfish"}),
    ] {
        assert_eq!(
            service
                .execute_tool(ExecuteToolRequest {
                    name: "search".into(),
                    arguments,
                })
                .await
                .err(),
            Some(Error::InvalidArguments)
        );
    }
}

#[test]
fn router_hides_unknown_providers() {
    let mut config = SearchConfig::default();
    config.presentation.mode = PresentationMode::Router;
    config.providers.insert(
        "unknown".into(),
        ProviderConfig {
            credential: Some("test-key".into()),
            ..ProviderConfig::default()
        },
    );
    let providers: BTreeMap<String, Arc<dyn SearchProvider>> = [(
        "unknown".into(),
        Arc::new(MockProvider) as Arc<dyn SearchProvider>,
    )]
    .into();
    assert_eq!(
        SearchService::with_providers(config, providers)
            .list_tools()
            .tools
            .len(),
        0
    );
}

#[test]
fn schema_validation_checks_arrays_enums_and_bounds() {
    let tool = ToolSpec {
        name: "fixture".into(),
        description: String::new(),
        parameters: json!({
            "type":"object",
            "properties":{
                "mode":{"type":"string","enum":["web","news"]},
                "limit":{"type":"integer","minimum":1,"maximum":10},
                "tags":{"type":"array","minItems":1,"maxItems":2,"items":{"type":"string","minLength":2}}
            },
            "required":["mode","limit","tags"],
            "additionalProperties":false
        }),
    };
    assert_eq!(
        validate_arguments(&tool, &json!({"mode":"web","limit":2,"tags":["rs"]})),
        Ok(())
    );
    for value in [
        json!({"mode":"images","limit":2,"tags":["rs"]}),
        json!({"mode":"web","limit":0,"tags":["rs"]}),
        json!({"mode":"web","limit":11,"tags":["rs"]}),
        json!({"mode":"web","limit":2.5,"tags":["rs"]}),
        json!({"mode":"web","limit":2,"tags":[]}),
        json!({"mode":"web","limit":2,"tags":["rs","go","js"]}),
        json!({"mode":"web","limit":2,"tags":["x"]}),
        json!({"mode":"web","limit":2,"tags":[8]}),
    ] {
        assert_eq!(
            validate_arguments(&tool, &value),
            Err(Error::InvalidArguments),
            "{value}"
        );
    }
}

#[test]
fn direct_provider_without_credential_is_hidden() {
    struct Keyed;
    impl SearchProvider for Keyed {
        fn execute<'a>(
            &'a self,
            _config: &'a ProviderConfig,
            _backend: &'a BackendConfig,
            _request: &'a ExecuteToolRequest,
        ) -> ProviderFuture<'a> {
            Box::pin(async { Err(Error::Provider("unreachable".into())) })
        }
    }
    let mut config = SearchConfig::default();
    config
        .providers
        .insert("tinyfish".into(), ProviderConfig::default());
    let mut providers: BTreeMap<String, Arc<dyn SearchProvider>> = BTreeMap::new();
    providers.insert("tinyfish".into(), Arc::new(Keyed));
    assert_eq!(
        SearchService::with_providers(config.clone(), providers.clone())
            .list_tools()
            .tools
            .len(),
        0
    );
    config.providers.insert(
        "tinyfish".into(),
        ProviderConfig {
            credential: Some("secret".into()),
            ..ProviderConfig::default()
        },
    );
    config.presentation.mode = PresentationMode::AllTools;
    assert_eq!(
        SearchService::with_providers(config, providers)
            .list_tools()
            .tools
            .len(),
        3
    );
}

#[test]
fn schema_validation_checks_nested_objects_oneof_and_exclusive_bounds() {
    let tool = ToolSpec {
        name: "fixture".into(),
        description: String::new(),
        parameters: json!({
            "type":"object",
            "properties":{
                "choice":{"oneOf":[{"type":"string","enum":["fast"]},{"type":"integer","minimum":1}]},
                "score":{"type":"number","exclusiveMinimum":0,"exclusiveMaximum":1},
                "details":{"type":"object","required":["active"],"properties":{"active":{"type":"boolean"}},"additionalProperties":false},
                "empty":{"type":"null"}
            },
            "required":["choice","score","details","empty"],
            "additionalProperties":{"type":"string","maxLength":3}
        }),
    };
    assert_eq!(
        validate_arguments(
            &tool,
            &json!({"choice":"fast","score":0.5,"details":{"active":true},"empty":null,"tag":"ok"})
        ),
        Ok(())
    );
    for arguments in [
        json!({"choice":"slow","score":0.5,"details":{"active":true},"empty":null}),
        json!({"choice":0,"score":0.5,"details":{"active":true},"empty":null}),
        json!({"choice":"fast","score":0,"details":{"active":true},"empty":null}),
        json!({"choice":"fast","score":1,"details":{"active":true},"empty":null}),
        json!({"choice":"fast","score":0.5,"details":{},"empty":null}),
        json!({"choice":"fast","score":0.5,"details":{"active":true,"extra":1},"empty":null}),
        json!({"choice":"fast","score":0.5,"details":{"active":"yes"},"empty":null}),
        json!({"choice":"fast","score":0.5,"details":{"active":true},"empty":false}),
        json!({"choice":"fast","score":0.5,"details":{"active":true},"empty":null,"tag":"long"}),
    ] {
        assert_eq!(
            validate_arguments(&tool, &arguments),
            Err(Error::InvalidArguments),
            "{arguments}"
        );
    }
}

#[tokio::test]
async fn service_rejects_disabled_and_unknown_router_target() {
    let mut disabled = service(PresentationMode::AllTools);
    disabled.config.enabled = false;
    assert_eq!(
        disabled
            .execute_tool(ExecuteToolRequest {
                name: "tinyfish_search".into(),
                arguments: json!({"query":"test"})
            })
            .await
            .err(),
        Some(Error::Disabled)
    );
    let router = service(PresentationMode::Router);
    assert_eq!(
        router
            .execute_tool(ExecuteToolRequest {
                name: "other".into(),
                arguments: json!({"query":"test"})
            })
            .await
            .err(),
        Some(Error::UnavailableTool("other".into()))
    );
    assert_eq!(
        router
            .execute_tool(ExecuteToolRequest {
                name: "search".into(),
                arguments: json!({"query":"test","provider":"missing"})
            })
            .await
            .err(),
        Some(Error::UnavailableProvider("missing".into()))
    );
}

#[tokio::test]
async fn router_rejects_answer_only_provider_in_mixed_configuration() {
    for explicit in [false, true] {
        let mut config = SearchConfig::default();
        config.presentation.mode = PresentationMode::Router;
        if !explicit {
            config.presentation.provider = Some("gemini_deep_research".into());
        }
        for name in ["gemini", "gemini_deep_research"] {
            config.providers.insert(
                name.into(),
                ProviderConfig {
                    credential: Some("test-key".into()),
                    ..ProviderConfig::default()
                },
            );
        }
        let providers: BTreeMap<String, Arc<dyn SearchProvider>> =
            ["gemini", "gemini_deep_research"]
                .into_iter()
                .map(|name| {
                    (
                        name.into(),
                        Arc::new(MockProvider) as Arc<dyn SearchProvider>,
                    )
                })
                .collect();
        let service = SearchService::with_providers(config, providers);
        assert_eq!(service.list_tools().tools.len(), 1);
        let arguments = if explicit {
            json!({"query":"fixture query","provider":"gemini_deep_research"})
        } else {
            json!({"query":"fixture query"})
        };
        assert_eq!(
            service
                .execute_tool(ExecuteToolRequest {
                    name: "search".into(),
                    arguments,
                })
                .await
                .err(),
            Some(Error::UnavailableProvider("gemini_deep_research".into()))
        );
    }
}
