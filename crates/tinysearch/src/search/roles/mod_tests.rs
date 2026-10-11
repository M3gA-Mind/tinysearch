//! Role dispatch, fallback, and argument translation tests.
use super::*;
use crate::{BackendConfig, ProviderConfig, ProviderFuture, SearchProvider, SearchStatus};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tinysearch_bus::{PresentationMode, ProviderRoute, SearchConfig, names::tools};

type Calls = Arc<Mutex<Vec<ExecuteToolRequest>>>;

/// Records every request and answers with a scripted outcome.
struct Scripted {
    calls: Calls,
    failure: Option<Failure>,
}

/// Builds the error a scripted provider fails with.
type Failure = fn() -> Error;

impl SearchProvider for Scripted {
    fn execute<'a>(
        &'a self,
        _config: &'a ProviderConfig,
        _backend: &'a BackendConfig,
        request: &'a ExecuteToolRequest,
    ) -> ProviderFuture<'a> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(request.clone());
        }
        let outcome = self
            .failure
            .map_or_else(|| Ok(ok()), |failure| Err(failure()));
        Box::pin(async move { outcome })
    }
}

fn ok() -> ExecuteToolResponse {
    ExecuteToolResponse {
        provider: "ignored".into(),
        results: vec![],
        citations: vec![],
        answer: Some("answer".into()),
        status: SearchStatus::Ok,
        provider_data: None,
        role: None,
        fallback_from: vec![],
    }
}
fn broke() -> Error {
    Error::InsufficientBalance
}
fn throttled() -> Error {
    Error::RateLimited
}
fn down() -> Error {
    Error::ProviderUnavailable("provider returned HTTP 503".into())
}
fn rejected() -> Error {
    Error::RejectedArguments("provider returned HTTP 400".into())
}
fn signed_out() -> Error {
    Error::BackendUnauthorized
}
fn bad_key() -> Error {
    Error::ProviderUnauthorized {
        provider: "brave".into(),
        status: 401,
    }
}

struct Fixture {
    service: SearchService,
    calls: BTreeMap<&'static str, Calls>,
}

impl Fixture {
    fn calls(&self, provider: &str) -> Vec<ExecuteToolRequest> {
        self.calls
            .get(provider)
            .and_then(|calls| calls.lock().ok().map(|calls| calls.clone()))
            .unwrap_or_default()
    }
}

/// Exa, Gemini and `TinyFish` on the backend; Brave, Tavily and Deep Research
/// with direct keys. Parallel is registered but configured only by the tests
/// that opt in (see [`with_parallel`]). Each provider answers with its
/// scripted outcome.
fn fixture(
    outcomes: &[(&'static str, Failure)],
    configure: impl FnOnce(&mut SearchConfig),
) -> Fixture {
    let mut config = SearchConfig::default();
    config.backend.credential = Some("session".into());
    for name in ["exa", "gemini"] {
        config.providers.insert(
            name.into(),
            ProviderConfig {
                route: ProviderRoute::Backend,
                ..ProviderConfig::default()
            },
        );
    }
    for name in ["brave", "tavily", "gemini_deep_research", "tinyfish"] {
        config.providers.insert(
            name.into(),
            ProviderConfig {
                credential: Some("key".into()),
                ..ProviderConfig::default()
            },
        );
    }
    configure(&mut config);
    let mut providers: BTreeMap<String, Arc<dyn SearchProvider>> = BTreeMap::new();
    let mut calls = BTreeMap::new();
    for name in [
        "exa",
        "gemini",
        "tinyfish",
        "brave",
        "tavily",
        "gemini_deep_research",
        "parallel",
    ] {
        let failure = outcomes
            .iter()
            .find(|(provider, _)| *provider == name)
            .map(|(_, failure)| *failure);
        let recorded = Calls::default();
        calls.insert(name, recorded.clone());
        providers.insert(
            name.into(),
            Arc::new(Scripted {
                calls: recorded,
                failure,
            }),
        );
    }
    Fixture {
        service: SearchService::with_providers(config, providers),
        calls,
    }
}

fn call(name: &str, arguments: Value) -> ExecuteToolRequest {
    ExecuteToolRequest {
        name: name.into(),
        arguments,
    }
}

#[test]
fn roles_mode_lists_the_three_role_tools() {
    let fixture = fixture(&[], |_| {});
    let listed = fixture.service.list_tools().tools;
    assert_eq!(
        listed
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [tools::WEB_SEARCH, tools::WEB_ANSWER, tools::WEB_CONTENTS]
    );
}

#[tokio::test]
async fn search_uses_the_first_provider_and_translates_arguments() -> Result<()> {
    let fixture = fixture(&[], |_| {});
    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_SEARCH,
            json!({"query":"rust","max_results":3}),
        ))
        .await?;
    assert_eq!(response.provider, "exa");
    assert_eq!(response.role, Some(Role::Search));
    assert_eq!(response.fallback_from.len(), 0);
    let sent = fixture.calls("exa");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].name, "exa_search");
    // The backend Exa search accepts only the query.
    assert_eq!(sent[0].arguments, json!({"query":"rust"}));
    Ok(())
}

#[tokio::test]
async fn search_falls_back_in_order_past_provider_side_failures() -> Result<()> {
    let fixture = fixture(
        &[("exa", broke), ("brave", throttled), ("tavily", down)],
        |_| {},
    );
    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_SEARCH,
            json!({"query":"rust","max_results":4}),
        ))
        .await?;
    assert_eq!(response.provider, "tinyfish");
    assert_eq!(response.fallback_from, ["exa", "brave", "tavily"]);
    assert_eq!(
        fixture.calls("brave")[0].arguments,
        json!({"query":"rust","count":4})
    );
    assert_eq!(
        fixture.calls("tavily")[0].arguments,
        json!({"query":"rust","max_results":4})
    );
    assert_eq!(
        fixture.calls("tinyfish")[0].arguments,
        json!({"query":"rust"})
    );
    Ok(())
}

#[tokio::test]
async fn invalid_arguments_do_not_fall_back() {
    let fixture = fixture(&[("exa", rejected)], |_| {});
    let result = fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
        .await;
    assert_eq!(
        result.err(),
        Some(Error::RejectedArguments(
            "provider returned HTTP 400".into()
        ))
    );
    assert_eq!(fixture.calls("brave").len(), 0);
}

#[tokio::test]
async fn exhausted_fallbacks_return_the_last_error() {
    let fixture = fixture(
        &[
            ("exa", broke),
            ("brave", broke),
            ("tavily", broke),
            ("tinyfish", throttled),
            ("gemini", throttled),
        ],
        |_| {},
    );
    let result = fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
        .await;
    assert_eq!(result.err(), Some(Error::RateLimited));
}

#[tokio::test]
async fn explicit_provider_is_used_alone_without_fallback() -> Result<()> {
    let fixture = fixture(&[("brave", broke)], |_| {});
    let result = fixture
        .service
        .execute_tool(call(
            tools::WEB_SEARCH,
            json!({"query":"rust","provider":"brave"}),
        ))
        .await;
    assert_eq!(result.err(), Some(Error::InsufficientBalance));
    assert_eq!(fixture.calls("exa").len(), 0);
    assert_eq!(fixture.calls("tavily").len(), 0);

    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_SEARCH,
            json!({"query":"rust","provider":"tavily"}),
        ))
        .await?;
    assert_eq!(response.provider, "tavily");
    assert_eq!(fixture.calls("exa").len(), 0);
    Ok(())
}

#[tokio::test]
async fn role_arguments_are_validated_before_dispatch() {
    let fixture = fixture(&[], |_| {});
    for arguments in [
        json!({}),
        json!({"query":"rust","provider":"querit"}),
        json!({"query":"rust","max_results":50}),
    ] {
        assert_eq!(
            fixture
                .service
                .execute_tool(call(tools::WEB_SEARCH, arguments))
                .await
                .err(),
            Some(Error::InvalidArguments)
        );
    }
    assert_eq!(
        fixture
            .service
            .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust","extra":1})))
            .await
            .err(),
        Some(Error::UnsupportedArgument("extra".into()))
    );
    assert_eq!(
        fixture
            .service
            .execute_tool(call("exa_search", json!({"query":"rust"})))
            .await
            .err(),
        Some(Error::UnavailableTool("exa_search".into()))
    );
    assert_eq!(fixture.calls("exa").len(), 0);
}

#[tokio::test]
async fn quick_answers_skip_deep_research() -> Result<()> {
    let fixture = fixture(&[("gemini", down)], |_| {});
    let response = fixture
        .service
        .execute_tool(call(tools::WEB_ANSWER, json!({"query":"why"})))
        .await?;
    assert_eq!(response.provider, "exa");
    assert_eq!(response.role, Some(Role::Answer));
    assert_eq!(response.fallback_from, ["gemini"]);
    assert_eq!(fixture.calls("gemini")[0].name, "gemini_agentic_search");
    assert_eq!(fixture.calls("exa")[0].name, "exa_answer");
    assert_eq!(fixture.calls("exa")[0].arguments, json!({"query":"why"}));
    assert_eq!(fixture.calls("gemini_deep_research").len(), 0);
    Ok(())
}

#[tokio::test]
async fn deep_answers_prefer_deep_research_then_fall_back() -> Result<()> {
    let fixture = fixture(&[], |_| {});
    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","depth":"deep"}),
        ))
        .await?;
    assert_eq!(response.provider, "gemini_deep_research");
    assert_eq!(
        fixture.calls("gemini_deep_research")[0].arguments,
        json!({"query":"why"})
    );

    let fixture = fixture_without_deep_research();
    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","depth":"deep"}),
        ))
        .await?;
    assert_eq!(response.provider, "gemini");
    Ok(())
}

fn fixture_without_deep_research() -> Fixture {
    fixture(&[], |config| {
        config.providers.remove("gemini_deep_research");
    })
}

#[tokio::test]
async fn deep_request_without_deep_research_is_answered_by_a_quick_provider() -> Result<()> {
    // Without a usable Deep Research provider the declaration still lists
    // `deep` (so host-side validation lets it through); the module must
    // answer it at quick depth, including through the quick-only provider.
    let fixture = fixture(&[], parallel_only);
    let spec = role_tool_specs(
        &fixture.service.available_tools(),
        &fixture.service.config.presentation,
        Role::Answer,
    )
    .ok_or(Error::UnavailableTool(tools::WEB_ANSWER.into()))?;
    validate_arguments(&spec, &json!({"query":"why","depth":"deep"}))?;

    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","depth":"deep"}),
        ))
        .await?;
    assert_eq!(response.provider, "parallel");
    assert_eq!(fixture.calls("parallel")[0].name, "parallel_chat");

    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","depth":"deep","provider":"parallel"}),
        ))
        .await?;
    assert_eq!(response.provider, "parallel");
    Ok(())
}

#[tokio::test]
async fn deep_request_without_deep_research_never_reaches_deep_research() -> Result<()> {
    // Deep Research is configured but unusable as a role provider (the role
    // order excludes it), so a `deep` request is answered quick in order.
    let fixture = fixture(&[("gemini", down)], |config| {
        config.presentation.roles.insert(
            Role::Answer,
            vec!["gemini".into(), "exa".into(), "parallel".into()],
        );
        with_parallel(config);
    });
    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","depth":"deep"}),
        ))
        .await?;
    assert_eq!(response.provider, "exa");
    assert_eq!(response.fallback_from, ["gemini"]);
    assert_eq!(fixture.calls("gemini_deep_research").len(), 0);
    Ok(())
}

#[tokio::test]
async fn explicit_quick_depth_is_rejected_when_only_deep_research_is_usable() {
    // When Deep Research is the sole usable Answer provider, `depth: "quick"`
    // is not servable. It must be rejected rather than silently dropped and
    // promoted to Deep Research (see `downgrade_unservable_depth`, which
    // narrows its removal to the stale-`deep` case only).
    let fixture = fixture(&[], |config| {
        config.providers.remove("gemini");
        config.providers.remove("exa");
    });
    let result = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","depth":"quick"}),
        ))
        .await;
    assert_eq!(result.err(), Some(Error::InvalidArguments));
    assert_eq!(fixture.calls("gemini_deep_research").len(), 0);
}

#[tokio::test]
async fn contents_translate_urls_per_provider() -> Result<()> {
    let fixture = fixture(&[("exa", down), ("tavily", down)], |_| {});
    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_CONTENTS,
            json!({"urls":["https://example.test"],"query":"pricing"}),
        ))
        .await?;
    assert_eq!(response.provider, "tinyfish");
    assert_eq!(response.role, Some(Role::Contents));
    assert_eq!(response.fallback_from, ["exa", "tavily"]);
    assert_eq!(
        fixture.calls("exa")[0].arguments,
        json!({"urls":["https://example.test"],"query":"pricing"})
    );
    assert_eq!(fixture.calls("tavily")[0].name, "tavily_extract");
    assert_eq!(
        fixture.calls("tavily")[0].arguments,
        json!({"urls":["https://example.test"]})
    );
    assert_eq!(
        fixture.calls("tinyfish")[0].arguments,
        json!({"urls":["https://example.test"],"format":"markdown"})
    );
    Ok(())
}

#[tokio::test]
async fn configured_role_order_drives_dispatch() -> Result<()> {
    let fixture = fixture(&[], |config| {
        config
            .presentation
            .roles
            .insert(Role::Search, vec!["tavily".into(), "exa".into()]);
    });
    let response = fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
        .await?;
    assert_eq!(response.provider, "tavily");
    Ok(())
}

#[tokio::test]
async fn legacy_modes_ignore_role_tools() {
    let fixture = fixture(&[], |config| {
        config.presentation.mode = PresentationMode::AllTools;
    });
    assert_eq!(
        fixture
            .service
            .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
            .await
            .err(),
        Some(Error::UnavailableTool(tools::WEB_SEARCH.into()))
    );
}

#[test]
fn answer_order_honours_depth() {
    let usable = vec![
        "gemini".to_owned(),
        DEEP_RESEARCH.to_owned(),
        "exa".to_owned(),
    ];
    let quick = Map::new();
    assert_eq!(answer_order(usable.clone(), &quick), ["gemini", "exa"]);
    let mut deep = Map::new();
    deep.insert("depth".into(), json!("deep"));
    assert_eq!(
        answer_order(usable, &deep),
        [DEEP_RESEARCH, "gemini", "exa"]
    );
    assert_eq!(
        answer_order(vec![DEEP_RESEARCH.to_owned()], &quick),
        [DEEP_RESEARCH]
    );
}

/// Configures Parallel with its own direct key.
fn with_parallel(config: &mut SearchConfig) {
    config.providers.insert(
        "parallel".into(),
        ProviderConfig {
            credential: Some("parallel-key".into()),
            ..ProviderConfig::default()
        },
    );
}

/// Only Parallel, with its own key: it must serve every role alone.
fn parallel_only(config: &mut SearchConfig) {
    config.providers.clear();
    config.backend.credential = None;
    with_parallel(config);
}

#[tokio::test]
async fn parallel_serves_every_role_with_its_own_key() -> Result<()> {
    let fixture = fixture(&[], parallel_only);
    let listed = fixture.service.list_tools().tools;
    assert_eq!(
        listed
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [tools::WEB_SEARCH, tools::WEB_ANSWER, tools::WEB_CONTENTS]
    );
    assert_eq!(
        listed[1].parameters["properties"]["depth"]["enum"],
        json!(["quick", "deep"])
    );

    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_SEARCH,
            json!({"query":"rust","max_results":3}),
        ))
        .await?;
    assert_eq!(response.provider, "parallel");
    assert_eq!(response.role, Some(Role::Search));
    let response = fixture
        .service
        .execute_tool(call(tools::WEB_ANSWER, json!({"query":"why"})))
        .await?;
    assert_eq!(response.role, Some(Role::Answer));
    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_CONTENTS,
            json!({"urls":["https://example.test"],"query":"pricing"}),
        ))
        .await?;
    assert_eq!(response.role, Some(Role::Contents));

    let sent = fixture.calls("parallel");
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].name, "parallel_search");
    assert_eq!(
        sent[0].arguments,
        json!({"objective":"rust","search_queries":["rust"],"num_results":3})
    );
    assert_eq!(sent[1].name, "parallel_chat");
    assert_eq!(
        sent[1].arguments,
        json!({"model":"speed","messages":[{"role":"user","content":"why"}]})
    );
    assert_eq!(sent[2].name, "parallel_extract");
    assert_eq!(
        sent[2].arguments,
        json!({"urls":["https://example.test"],"objective":"pricing","full_content":true})
    );
    Ok(())
}

#[tokio::test]
async fn parallel_role_arguments_omit_absent_optionals() -> Result<()> {
    let fixture = fixture(&[], parallel_only);
    fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
        .await?;
    fixture
        .service
        .execute_tool(call(
            tools::WEB_CONTENTS,
            json!({"urls":["https://example.test"]}),
        ))
        .await?;
    let sent = fixture.calls("parallel");
    assert_eq!(
        sent[0].arguments,
        json!({"objective":"rust","search_queries":["rust"]})
    );
    assert_eq!(
        sent[1].arguments,
        json!({"urls":["https://example.test"],"full_content":true})
    );
    Ok(())
}

#[tokio::test]
async fn parallel_is_never_usable_on_the_backend_route() {
    let fixture = fixture(&[], |config| {
        parallel_only(config);
        config.backend.credential = Some("session".into());
        if let Some(parallel) = config.providers.get_mut("parallel") {
            parallel.route = ProviderRoute::Backend;
        }
    });
    assert_eq!(fixture.service.list_tools().tools.len(), 0);
    for (tool, arguments) in [
        (tools::WEB_SEARCH, json!({"query":"rust"})),
        (tools::WEB_ANSWER, json!({"query":"why"})),
        (
            tools::WEB_CONTENTS,
            json!({"urls":["https://example.test"]}),
        ),
    ] {
        assert_eq!(
            fixture
                .service
                .execute_tool(call(tool, arguments))
                .await
                .err(),
            Some(Error::UnavailableTool(tool.into()))
        );
    }
    assert_eq!(fixture.calls("parallel").len(), 0);
}

#[tokio::test]
async fn role_calls_fall_back_into_parallel() -> Result<()> {
    let fixture = fixture(
        &[
            ("exa", down),
            ("brave", broke),
            ("tavily", throttled),
            ("gemini", down),
        ],
        with_parallel,
    );
    let response = fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
        .await?;
    assert_eq!(response.provider, "parallel");
    assert_eq!(response.fallback_from, ["exa", "brave", "tavily"]);
    assert_eq!(fixture.calls("tinyfish").len(), 0);

    let response = fixture
        .service
        .execute_tool(call(tools::WEB_ANSWER, json!({"query":"why"})))
        .await?;
    assert_eq!(response.provider, "parallel");
    assert_eq!(response.fallback_from, ["gemini", "exa"]);

    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_CONTENTS,
            json!({"urls":["https://example.test"]}),
        ))
        .await?;
    assert_eq!(response.provider, "parallel");
    assert_eq!(response.fallback_from, ["exa", "tavily"]);
    assert_eq!(
        fixture
            .calls("parallel")
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["parallel_search", "parallel_chat", "parallel_extract"]
    );
    Ok(())
}

#[tokio::test]
async fn deep_answers_skip_parallel() -> Result<()> {
    let fixture = fixture(
        &[
            ("gemini_deep_research", down),
            ("gemini", down),
            ("exa", down),
        ],
        with_parallel,
    );
    let result = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","depth":"deep"}),
        ))
        .await;
    assert!(matches!(result, Err(Error::ProviderUnavailable(_))));
    assert_eq!(fixture.calls("gemini_deep_research").len(), 1);
    assert_eq!(fixture.calls("parallel").len(), 0);

    assert_eq!(
        fixture
            .service
            .execute_tool(call(
                tools::WEB_ANSWER,
                json!({"query":"why","depth":"deep","provider":"parallel"}),
            ))
            .await
            .err(),
        Some(Error::InvalidArguments)
    );
    assert_eq!(fixture.calls("parallel").len(), 0);

    let response = fixture
        .service
        .execute_tool(call(
            tools::WEB_ANSWER,
            json!({"query":"why","provider":"parallel"}),
        ))
        .await?;
    assert_eq!(response.provider, "parallel");
    Ok(())
}

#[test]
fn answer_order_drops_quick_only_providers_for_deep() {
    let usable = vec![
        "gemini".to_owned(),
        DEEP_RESEARCH.to_owned(),
        "exa".to_owned(),
        "parallel".to_owned(),
    ];
    assert_eq!(
        answer_order(usable.clone(), &Map::new()),
        ["gemini", "exa", "parallel"]
    );
    let mut deep = Map::new();
    deep.insert("depth".into(), json!("deep"));
    assert_eq!(
        answer_order(usable, &deep),
        [DEEP_RESEARCH, "gemini", "exa"]
    );
}

#[tokio::test]
async fn a_rejected_credential_falls_back_to_the_next_provider() -> Result<()> {
    let fixture = fixture(&[("exa", signed_out), ("brave", bad_key)], |_| {});
    let response = fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
        .await?;
    assert_eq!(response.fallback_from, ["exa", "brave"]);
    Ok(())
}

#[tokio::test]
async fn a_rejected_credential_outranks_a_later_unavailable_provider() {
    let fixture = fixture(
        &[
            ("exa", signed_out),
            ("brave", down),
            ("tavily", throttled),
            ("tinyfish", down),
            ("gemini", down),
        ],
        |_| {},
    );
    let result = fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust"})))
        .await;
    assert_eq!(result.err(), Some(Error::BackendUnauthorized));
}

#[tokio::test]
async fn limit_is_accepted_as_an_alias_of_max_results() -> Result<()> {
    let fixture = fixture(&[("exa", down)], |_| {});
    fixture
        .service
        .execute_tool(call(tools::WEB_SEARCH, json!({"query":"rust","limit":7})))
        .await?;
    assert_eq!(
        fixture.calls("brave")[0].arguments,
        json!({"query":"rust","count":7})
    );
    Ok(())
}

#[tokio::test]
async fn max_results_wins_over_limit_when_both_are_sent() -> Result<()> {
    let fixture = fixture(&[("exa", down), ("brave", down)], |_| {});
    fixture
        .service
        .execute_tool(call(
            tools::WEB_SEARCH,
            json!({"query":"rust","max_results":3,"limit":9}),
        ))
        .await?;
    assert_eq!(
        fixture.calls("tavily")[0].arguments,
        json!({"query":"rust","max_results":3})
    );
    Ok(())
}
