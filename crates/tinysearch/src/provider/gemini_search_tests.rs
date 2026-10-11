//! Direct Gemini Search-role regression tests using real HTTP fixtures.
use super::super::builtins;
use super::*;
use crate::{Error, ExecuteToolRequest, ProviderConfig};
use crate::{Role, SearchConfig, SearchService, names::tools};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

const GEMINI_PATH: &str = "/v1beta/models/gemini-3.8-flash:generateContent";

fn grounding() -> Value {
    json!({"candidates":[{"content":{"parts":[{"text":"A grounded answer."}]},
    "groundingMetadata":{
        "groundingChunks":[
            {"web":{"uri":"https://first.test","title":"First"}},
            {"web":{"uri":"https://second.test","title":"Second"}},
            {"web":{"uri":"https://second.test","title":"Duplicate"}},
            {"web":{"uri":"https://third.test","title":"Third"}}
        ],
        "groundingSupports":[
            {"segment":{"text":"Second source snippet."},"groundingChunkIndices":[1,2]},
            {"segment":{"text":"First source snippet."},"groundingChunkIndices":[0]}
        ]
    }}]})
}

fn config(server: &MockServer, order: &[&str]) -> SearchConfig {
    let mut config = SearchConfig::default();
    config
        .presentation
        .roles
        .insert(Role::Search, order.iter().map(|s| (*s).into()).collect());
    for name in ["exa", "gemini"] {
        config.providers.insert(
            name.into(),
            ProviderConfig {
                credential: Some(format!("{name}-key")),
                base_url: Some(server.uri()),
                ..ProviderConfig::default()
            },
        );
    }
    // A configured backend must never receive either direct provider call.
    config.backend.base_url = Some(format!("{}/unrelated-backend", server.uri()));
    config.backend.credential = Some("backend-key".into());
    config
}

fn search(arguments: Value) -> ExecuteToolRequest {
    ExecuteToolRequest {
        name: tools::WEB_SEARCH.into(),
        arguments,
    }
}

async fn gemini_mock(server: &MockServer, value: Value, count: u64) {
    Mock::given(method("POST")).and(path(GEMINI_PATH))
        .and(header("x-goog-api-key", "gemini-key"))
        .and(body_json(json!({"contents":[{"parts":[{"text":"fixture query"}]}],"tools":[{"google_search":{}}]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(value)).expect(count).mount(server).await;
}

#[tokio::test]
async fn gemini_only_search_returns_ranked_links_and_supported_snippets() -> crate::Result<()> {
    let server = MockServer::start().await;
    gemini_mock(&server, grounding(), 1).await;
    let service = SearchService::with_providers(config(&server, &["gemini"]), builtins());
    let response = service
        .execute_tool(search(json!({"query":"fixture query"})))
        .await?;
    assert_eq!(response.provider, "gemini");
    assert_eq!(response.role, Some(Role::Search));
    assert_eq!(response.status, SearchStatus::Ok);
    assert_eq!(response.answer, None);
    assert_eq!(
        response
            .results
            .iter()
            .map(|r| r.url.as_str())
            .collect::<Vec<_>>(),
        [
            "https://second.test",
            "https://first.test",
            "https://third.test"
        ]
    );
    assert_eq!(
        response.results[0].snippet.as_deref(),
        Some("Second source snippet.")
    );
    assert_eq!(
        response.results[1].snippet.as_deref(),
        Some("First source snippet.")
    );
    assert_eq!(response.results[2].snippet, None);
    assert_eq!(response.citations.len(), 3);
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].headers.contains_key("authorization"));
    assert!(!requests[0].headers.contains_key("x-sdk-name"));
    Ok(())
}

#[tokio::test]
async fn direct_exa_failure_falls_back_to_gemini_in_configured_order() -> crate::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("x-api-key", "exa-key"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    gemini_mock(&server, grounding(), 1).await;
    let service = SearchService::with_providers(config(&server, &["exa", "gemini"]), builtins());
    let response = service
        .execute_tool(search(json!({"query":"fixture query","limit":1})))
        .await?;
    assert_eq!(response.provider, "gemini");
    assert_eq!(response.fallback_from, ["exa"]);
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.citations.len(), 1);
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.iter().map(|r| r.url.path()).collect::<Vec<_>>(),
        ["/search", GEMINI_PATH]
    );
    Ok(())
}

#[tokio::test]
async fn direct_exa_success_does_not_call_gemini() -> crate::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"results":[{"url":"https://exa.test","title":"Exa"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    gemini_mock(&server, grounding(), 0).await;
    let service = SearchService::with_providers(config(&server, &["exa", "gemini"]), builtins());
    let response = service
        .execute_tool(search(json!({"query":"fixture query"})))
        .await?;
    assert_eq!(response.provider, "exa");
    assert_eq!(response.fallback_from.len(), 0);
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn forced_search_provider_never_falls_back() -> crate::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    gemini_mock(&server, grounding(), 1).await;
    let service = SearchService::with_providers(config(&server, &["exa", "gemini"]), builtins());
    assert!(matches!(
        service
            .execute_tool(search(json!({"query":"fixture query","provider":"exa"})))
            .await,
        Err(Error::ProviderUnavailable(_))
    ));
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );
    let response = service
        .execute_tool(search(json!({"query":"fixture query","provider":"gemini"})))
        .await?;
    assert_eq!(response.provider, "gemini");
    assert_eq!(response.fallback_from.len(), 0);
    Ok(())
}

#[tokio::test]
async fn missing_keys_are_removed_from_search_candidates() -> crate::Result<()> {
    let server = MockServer::start().await;
    gemini_mock(&server, grounding(), 1).await;
    let mut config = config(&server, &["exa", "gemini"]);
    config
        .providers
        .get_mut("exa")
        .ok_or(Error::InvalidArguments)?
        .credential = None;
    let service = SearchService::with_providers(config.clone(), builtins());
    let response = service
        .execute_tool(search(json!({"query":"fixture query"})))
        .await?;
    assert_eq!(response.provider, "gemini");
    assert_eq!(response.fallback_from.len(), 0);
    config
        .providers
        .get_mut("gemini")
        .ok_or(Error::InvalidArguments)?
        .credential = Some(" ".into());
    let service = SearchService::with_providers(config, builtins());
    assert!(
        !service
            .list_tools()
            .tools
            .iter()
            .any(|t| t.name == tools::WEB_SEARCH)
    );
    assert!(matches!(
        service
            .execute_tool(search(json!({"query":"fixture query"})))
            .await,
        Err(Error::UnavailableTool(_))
    ));
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn search_without_usable_grounding_returns_empty_even_with_answer_text() -> crate::Result<()>
{
    for chunks in [
        json!([]),
        json!([{}, {"web":{"uri":" "}}, {"web":{"uri":"javascript:alert(1)"}}, {"web":{"uri":"https://"}}, {"web":{"uri":"ftp://files.test"}}]),
    ] {
        let server = MockServer::start().await;
        gemini_mock(&server, json!({"candidates":[{"content":{"parts":[{"text":"Unsupported answer"}]},"groundingMetadata":{"groundingChunks":chunks}}]}), 1).await;
        let service = SearchService::with_providers(config(&server, &["gemini"]), builtins());
        let response = service
            .execute_tool(search(json!({"query":"fixture query"})))
            .await?;
        assert_eq!(response.status, SearchStatus::Empty);
        assert_eq!(response.results.len(), 0);
        assert_eq!(response.citations.len(), 0);
        assert_eq!(response.answer, None);
    }
    Ok(())
}

#[tokio::test]
async fn gemini_search_limits_results_and_preserves_answer_behavior() -> crate::Result<()> {
    let server = MockServer::start().await;
    gemini_mock(&server, grounding(), 4).await;
    let mut config = config(&server, &["gemini"]);
    config
        .providers
        .get_mut("gemini")
        .ok_or(Error::InvalidArguments)?
        .max_results = Some(2);
    let service = SearchService::with_providers(config, builtins());
    for (arguments, count) in [
        (json!({"query":"fixture query"}), 2),
        (
            json!({"query":"fixture query","max_results":1,"limit":3}),
            1,
        ),
        (json!({"query":"fixture query","limit":3}), 3),
    ] {
        let response = service.execute_tool(search(arguments)).await?;
        assert_eq!(response.results.len(), count);
        assert_eq!(response.citations.len(), count);
    }
    let response = service
        .execute_tool(ExecuteToolRequest {
            name: tools::WEB_ANSWER.into(),
            arguments: json!({"query":"fixture query","provider":"gemini"}),
        })
        .await?;
    assert_eq!(response.answer.as_deref(), Some("A grounded answer."));
    assert_eq!(response.results.len(), 0);
    assert_eq!(response.citations.len(), 3);
    assert_eq!(response.role, Some(Role::Answer));
    Ok(())
}

#[tokio::test]
async fn gemini_search_failure_keeps_provider_error_classification() {
    for status in [401, 429, 503] {
        let server = MockServer::start().await;
        Mock::given(path(GEMINI_PATH))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        let service = SearchService::with_providers(config(&server, &["gemini"]), builtins());
        let error = service
            .execute_tool(search(json!({"query":"fixture query"})))
            .await;
        assert!(match status {
            401 => matches!(error, Err(Error::ProviderUnauthorized { .. })),
            429 => matches!(error, Err(Error::RateLimited)),
            _ => matches!(error, Err(Error::ProviderUnavailable(_))),
        });
    }
}

#[test]
fn search_grounding_ignores_invalid_chunks_and_bounds_output_and_input() {
    let mut chunks = vec![
        json!({}),
        json!({"web":{"uri":"https://user:password@host.test"}}),
        json!({"web":{"uri":format!("https://long.test/{}", "x".repeat(2048))}}),
    ];
    chunks.extend((0..25).map(
        |i| json!({"web":{"uri":format!("https://source{i}.test"),"title":"界".repeat(301)}}),
    ));
    let response = normalize(&json!({"candidates":[{"groundingMetadata":{
        "groundingChunks":chunks,"groundingSupports":[
            {"groundingChunkIndices":[null,-1,"not an index",100_000,0,1,2]},
            {"segment":{"text":" 字 ".repeat(1000)},"groundingChunkIndices":[3,3]}
        ]
    }}]}));
    assert_eq!(response.results.len(), 20);
    assert_eq!(response.citations.len(), 20);
    assert_eq!(response.results[0].url, "https://source0.test");
    assert_eq!(response.results[0].title.chars().count(), 300);
    assert_eq!(
        response.results[0]
            .snippet
            .as_deref()
            .map(|s| s.chars().count()),
        Some(1200)
    );
    // Sources beyond the bounded input scan never become results.
    let mut chunks = vec![json!({}); MAX_GROUNDING_CHUNKS];
    chunks.push(json!({"web":{"uri":"https://outside.test"}}));
    let response = normalize(&json!({"candidates":[{"groundingMetadata":{
        "groundingChunks":chunks,"groundingSupports":[{"groundingChunkIndices":[MAX_GROUNDING_CHUNKS]}]
    }}]}));
    assert_eq!(response.status, SearchStatus::Empty);
}

#[test]
fn search_grounding_without_metadata_or_support_text_is_well_formed() {
    for value in [json!({}), json!({"candidates":[{"groundingMetadata":{}}]})] {
        let response = normalize(&value);
        assert_eq!(response.status, SearchStatus::Empty);
        assert_eq!(response.answer, None);
    }
    let response = normalize(&json!({"candidates":[{"groundingMetadata":{
        "groundingChunks":[{"web":{"uri":"https://bare.test"}}],
        "groundingSupports":[{"segment":{"text":" "},"groundingChunkIndices":[0]}]
    }}]}));
    assert_eq!(response.results[0].title, "");
    assert_eq!(response.results[0].snippet, None);
}

#[tokio::test]
async fn configured_gemini_first_search_falls_back_to_exa_after_failure() -> crate::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path(GEMINI_PATH))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"results":[{"url":"https://exa.test"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let service = SearchService::with_providers(config(&server, &["gemini", "exa"]), builtins());
    let response = service
        .execute_tool(search(json!({"query":"fixture query"})))
        .await?;
    assert_eq!(response.provider, "exa");
    assert_eq!(response.fallback_from, ["gemini"]);
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.iter().map(|r| r.url.path()).collect::<Vec<_>>(),
        [GEMINI_PATH, "/search"]
    );
    Ok(())
}

#[tokio::test]
async fn gemini_router_search_returns_grounded_results_instead_of_answer() -> crate::Result<()> {
    let server = MockServer::start().await;
    gemini_mock(&server, grounding(), 1).await;
    let mut config = config(&server, &["gemini"]);
    config.providers.remove("exa");
    config.presentation.mode = crate::PresentationMode::Router;
    let service = SearchService::with_providers(config, builtins());
    assert_eq!(service.list_tools().tools[0].name, "search");
    let response = service
        .execute_tool(ExecuteToolRequest {
            name: "search".into(),
            arguments: json!({"query":"fixture query"}),
        })
        .await?;
    assert_eq!(response.provider, "gemini");
    assert_eq!(response.status, SearchStatus::Ok);
    assert_eq!(response.answer, None);
    assert_eq!(
        response
            .results
            .iter()
            .map(|result| result.url.as_str())
            .collect::<Vec<_>>(),
        [
            "https://second.test",
            "https://first.test",
            "https://third.test"
        ]
    );
    assert_eq!(
        response.results[0].snippet.as_deref(),
        Some("Second source snippet.")
    );
    assert_eq!(
        response
            .citations
            .iter()
            .map(|citation| citation.url.as_str())
            .collect::<Vec<_>>(),
        response
            .results
            .iter()
            .map(|result| result.url.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn gemini_router_search_without_grounding_returns_empty() -> crate::Result<()> {
    let server = MockServer::start().await;
    gemini_mock(
        &server,
        json!({"candidates":[{"content":{"parts":[{"text":"Unsupported answer"}]}}]}),
        1,
    )
    .await;
    let mut config = config(&server, &["gemini"]);
    config.providers.remove("exa");
    config.presentation.mode = crate::PresentationMode::Router;
    let service = SearchService::with_providers(config, builtins());
    let response = service
        .execute_tool(ExecuteToolRequest {
            name: "search".into(),
            arguments: json!({"query":"fixture query","provider":"gemini"}),
        })
        .await?;
    assert_eq!(response.status, SearchStatus::Empty);
    assert_eq!(response.results.len(), 0);
    assert_eq!(response.citations.len(), 0);
    assert_eq!(response.answer, None);
    Ok(())
}

#[tokio::test]
async fn native_gemini_tool_keeps_answer_and_citations() -> crate::Result<()> {
    let server = MockServer::start().await;
    gemini_mock(&server, grounding(), 1).await;
    let mut config = config(&server, &["gemini"]);
    config.presentation.mode = crate::PresentationMode::AllTools;
    let service = SearchService::with_providers(config, builtins());
    let response = service
        .execute_tool(ExecuteToolRequest {
            name: "gemini_agentic_search".into(),
            arguments: json!({"query":"fixture query"}),
        })
        .await?;
    assert_eq!(response.answer.as_deref(), Some("A grounded answer."));
    assert_eq!(response.results.len(), 0);
    assert_eq!(response.citations.len(), 3);
    assert_eq!(response.role, None);
    Ok(())
}

#[tokio::test]
async fn gemini_router_respects_configured_result_limit() -> crate::Result<()> {
    let server = MockServer::start().await;
    gemini_mock(&server, grounding(), 1).await;
    let mut config = config(&server, &["gemini"]);
    config.providers.remove("exa");
    config.presentation.mode = crate::PresentationMode::Router;
    config.providers.get_mut("gemini").unwrap().max_results = Some(2);
    let response = SearchService::with_providers(config, builtins())
        .execute_tool(ExecuteToolRequest {
            name: "search".into(),
            arguments: json!({"query":"fixture query"}),
        })
        .await?;
    assert_eq!(response.results.len(), 2);
    assert_eq!(response.citations.len(), 2);
    assert_eq!(response.results[0].url, "https://second.test");
    assert_eq!(response.results[1].url, "https://first.test");
    Ok(())
}
