//! A host that links `TinySearch` as a library, instead of loading the
//! module, gets the same built-in providers through
//! `SearchService::with_builtins`.
use serde_json::{Value, json};
use tinysearch::{
    ExecuteToolRequest, ProviderConfig, ProviderRoute, Role, SearchConfig, SearchService,
    role_tool_name,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Exa on its direct route at `base_url`, the only provider configured.
fn exa_only(base_url: &str) -> SearchConfig {
    let mut config = SearchConfig::default();
    config.providers.insert(
        "exa".into(),
        ProviderConfig {
            credential: Some("exa-key".into()),
            base_url: Some(base_url.into()),
            route: ProviderRoute::Direct,
            ..ProviderConfig::default()
        },
    );
    config
}

#[tokio::test]
async fn a_host_searches_through_the_builtin_exa_provider() -> TestResult<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(header("x-api-key", "exa-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{ "url": "https://example.test/moye", "title": "Moye Moye" }]
        })))
        .mount(&server)
        .await;
    let service = SearchService::with_builtins(exa_only(&server.uri()));

    let search = role_tool_name(Role::Search);
    assert!(
        service
            .list_tools()
            .tools
            .iter()
            .any(|tool| tool.name == search),
        "the search role is offered once Exa has a key"
    );
    let response = service
        .execute_tool(ExecuteToolRequest {
            name: search.into(),
            arguments: json!({ "query": "moye moye meme", "max_results": 3 }),
        })
        .await?;

    assert_eq!(response.provider, "exa");
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].url, "https://example.test/moye");
    assert_eq!(response.results[0].title, "Moye Moye");
    let requests = server.received_requests().await.ok_or("recording is on")?;
    let body: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(body["query"], "moye moye meme");
    Ok(())
}

#[tokio::test]
async fn without_provider_keys_a_host_is_offered_nothing() {
    let service = SearchService::with_builtins(SearchConfig::default());
    assert_eq!(service.list_tools().tools.len(), 0);
    let unconfigured = format!("{service:?}");
    assert!(
        unconfigured.contains("exa"),
        "built-in providers are registered: {unconfigured}"
    );
}
