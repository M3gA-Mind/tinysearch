//! Exercises the published module through `TinyBus`'s dynamic loader.
//!
//! `TINYSEARCH_MODULE_ARTIFACT` must point at the native library extracted from
//! the archive under test. Provider requests go only to a local wiremock server.

use std::{error::Error, io, path::PathBuf, time::Duration};

use tinybus::{Connection, broker::Broker, module::ModuleHost, transport::memory::MemoryBus};
use tinysearch::{
    ExecuteToolRequest, ExecuteToolResponse, ListToolsResponse, SearchStatus, errors, names,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

async fn mount_provider_fixtures(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_partial_json(
            serde_json::json!({"query": "fixture success"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [{
                "url": "https://fixture.test/result",
                "title": "Fixture result",
                "text": "Local provider response"
            }]
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_partial_json(
            serde_json::json!({"query": "fixture failure"}),
        ))
        .respond_with(ResponseTemplate::new(503))
        .mount(server)
        .await;
}

#[tokio::test]
async fn loaded_artifact_lists_and_executes_search_tools() -> Result<(), Box<dyn Error>> {
    let artifact = PathBuf::from(std::env::var_os("TINYSEARCH_MODULE_ARTIFACT").ok_or_else(
        || {
            io::Error::new(
                io::ErrorKind::NotFound,
                "TINYSEARCH_MODULE_ARTIFACT is required",
            )
        },
    )?);
    let server = MockServer::start().await;
    mount_provider_fixtures(&server).await;

    let bus = MemoryBus::new();
    let broker = Broker::new();
    let broker_task = broker.spawn(bus.clone());
    let module_host = ModuleHost::new(broker);
    let config = serde_json::json!({
        "providers": {
            "exa": {
                "credential": "fixture-key",
                "base_url": server.uri()
            }
        },
        "presentation": { "roles": { "search": ["exa"] } }
    });
    let info = module_host.load_file_with_config(&artifact, config)?;
    assert_eq!(info.name, "tinysearch");

    let client = Connection::connect(bus.connect().await?).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if client
                .list_names()
                .await?
                .iter()
                .any(|name| name.as_str() == names::INTERFACE)
            {
                return tinybus::Result::Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;

    let proxy = client.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let listed: ListToolsResponse = proxy.call(names::methods::LIST_TOOLS, ()).await?;
    assert!(
        listed
            .tools
            .iter()
            .any(|tool| tool.name == names::tools::WEB_SEARCH)
    );

    let succeeded: ExecuteToolResponse = proxy
        .call(
            names::methods::EXECUTE_TOOL,
            (ExecuteToolRequest {
                name: names::tools::WEB_SEARCH.into(),
                arguments: serde_json::json!({"query": "fixture success"}),
            },),
        )
        .await?;
    assert_eq!(succeeded.status, SearchStatus::Ok);
    assert_eq!(succeeded.provider, "exa");
    assert_eq!(succeeded.results.len(), 1);
    assert_eq!(succeeded.results[0].url, "https://fixture.test/result");
    assert_eq!(
        succeeded.results[0].snippet.as_deref(),
        Some("Local provider response")
    );

    let failed = proxy
        .call::<ExecuteToolResponse>(
            names::methods::EXECUTE_TOOL,
            (ExecuteToolRequest {
                name: names::tools::WEB_SEARCH.into(),
                arguments: serde_json::json!({"query": "fixture failure"}),
            },),
        )
        .await;
    let failed = match failed {
        Ok(response) => return Err(format!("unexpected successful response: {response:?}").into()),
        Err(error) => error,
    };
    assert_eq!(
        errors::code_of(&failed.to_string()),
        Some(errors::UNAVAILABLE)
    );

    broker_task.abort();
    Ok(())
}
