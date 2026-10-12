//! Exercises the published module through `TinyBus`'s dynamic loader.
//!
//! `TINYSEARCH_TEST_MODULE` can point at the native library extracted from the
//! archive under test. Provider requests go only to a local wiremock server.

use std::{error::Error, fs, io, path::PathBuf, time::Duration};

use tinybus::test_support::{call, start_bus, wait_until_idle, wait_until_serving};
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
    let artifact = if let Some(artifact) = std::env::var_os("TINYSEARCH_TEST_MODULE") {
        PathBuf::from(artifact)
    } else {
        let executable = std::env::current_exe()?;
        let target_dir = executable
            .parent()
            .ok_or_else(|| io::Error::other("test executable has no parent directory"))?;
        tinybus::test_support::artifact_path(target_dir, "tinysearch")
    };
    if !artifact.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "TinySearch cdylib artifact not found at {}",
                artifact.display()
            ),
        )
        .into());
    }
    // Cargo's `target/debug/deps` inherits runner ACLs on Windows and may be
    // writable by other users. TinyBus correctly refuses modules from there,
    // so stage the exact bytes and a matching pin in a private temp directory.
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| io::Error::other("current user's home directory is unavailable"))?;
    let staged = tempfile::Builder::new()
        .prefix("tinysearch-module-e2e-")
        .tempdir_in(home)?;
    let staged_artifact = staged.path().join(
        artifact
            .file_name()
            .ok_or_else(|| io::Error::other("module artifact has no filename"))?,
    );
    fs::copy(&artifact, &staged_artifact)?;
    let digest = tinybus::module::sha256_file(&staged_artifact)?;
    let filename = staged_artifact
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("module artifact filename is not UTF-8"))?;
    fs::write(
        staged.path().join("modules.toml"),
        format!("\"{filename}\" = \"{digest}\"\n"),
    )?;

    let server = MockServer::start().await;
    mount_provider_fixtures(&server).await;

    let (module_host, client, broker_task) = start_bus().await?;
    let config = serde_json::json!({
        "providers": {
            "exa": {
                "credential": "fixture-key",
                "base_url": server.uri()
            }
        },
        "presentation": { "roles": { "search": ["exa"] } }
    });
    let info = module_host.load_file_with_config(&staged_artifact, config)?;
    assert_eq!(info.name, "tinysearch");

    wait_until_serving(&client, names::INTERFACE, Duration::from_secs(5)).await?;

    let proxy = client.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;
    let listed: ListToolsResponse = call(&proxy, names::methods::LIST_TOOLS, ()).await?;
    assert!(
        listed
            .tools
            .iter()
            .any(|tool| tool.name == names::tools::WEB_SEARCH)
    );

    let succeeded: ExecuteToolResponse = call(
        &proxy,
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

    let failed = call::<ExecuteToolResponse, _>(
        &proxy,
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
    wait_until_idle(&module_host, "tinysearch", Duration::from_secs(5)).await?;

    broker_task.abort();
    Ok(())
}
