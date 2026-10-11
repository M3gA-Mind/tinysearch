//! Grounded Search-role fallback through the real in-memory bus contract.
use super::*;
use serde_json::json;
use tinybus::{broker::Broker, transport::memory::MemoryBus};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

#[tokio::test]
async fn direct_exa_to_gemini_search_fallback_crosses_the_bus() -> tinybus::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1beta/models/gemini-3.8-flash:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"candidates":[{
            "groundingMetadata":{
                "groundingChunks":[{"web":{"uri":"https://source.test","title":"Source"}}],
                "groundingSupports":[{"segment":{"text":"Grounded snippet"},"groundingChunkIndices":[0]}]
            }
        }]}))).expect(1).mount(&server).await;
    let mut config = SearchConfig::default();
    for provider in ["exa", "gemini"] {
        config.providers.insert(
            provider.into(),
            crate::ProviderConfig {
                base_url: Some(server.uri()),
                credential: Some(format!("{provider}-key")),
                ..crate::ProviderConfig::default()
            },
        );
    }
    config
        .presentation
        .roles
        .insert(crate::Role::Search, vec!["exa".into(), "gemini".into()]);
    let bus = MemoryBus::new();
    Broker::new().spawn(bus.clone());
    let connection = Connection::connect(bus.connect().await?).await?;
    setup(connection.clone(), config).await?;
    let client = Connection::connect(bus.connect().await?).await?;
    let proxy = client.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let listed: ListToolsResponse = proxy.call(names::methods::LIST_TOOLS, ()).await?;
    let search = listed
        .tools
        .iter()
        .find(|tool| tool.name == names::tools::WEB_SEARCH);
    assert_eq!(
        search.map(|tool| &tool.parameters["properties"]["provider"]["enum"]),
        Some(&json!(["exa", "gemini"]))
    );
    let response: ExecuteToolResponse = proxy
        .call(
            names::methods::EXECUTE_TOOL,
            (ExecuteToolRequest {
                name: names::tools::WEB_SEARCH.into(),
                arguments: json!({"query":"fixture query","limit":1}),
            },),
        )
        .await?;
    assert_eq!(response.provider, "gemini");
    assert_eq!(response.role, Some(crate::Role::Search));
    assert_eq!(response.fallback_from, ["exa"]);
    assert_eq!(response.results[0].url, "https://source.test");
    assert_eq!(
        response.results[0].snippet.as_deref(),
        Some("Grounded snippet")
    );
    assert_eq!(response.answer, None);
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        2
    );
    Ok(())
}
