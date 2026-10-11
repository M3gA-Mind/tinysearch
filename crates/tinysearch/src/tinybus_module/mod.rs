//! `TinyBus` entrypoint and search interface.
use crate::{SearchConfig, SearchService};
use std::sync::Arc;
use tinybus::{Connection, Result as TinyBusResult};
use tinysearch_bus::{ExecuteToolRequest, ExecuteToolResponse, ListToolsResponse, names};

#[derive(Debug)]
struct SearchBusService(Arc<SearchService>);

#[tinybus::interface(name = "ai.tinyhumans.tinysearch.Search")]
impl SearchBusService {
    async fn list_tools(&self) -> TinyBusResult<ListToolsResponse> {
        std::future::ready(Ok(self.0.list_tools())).await
    }

    async fn execute_tool(
        &self,
        request: ExecuteToolRequest,
    ) -> TinyBusResult<ExecuteToolResponse> {
        self.0
            .execute_tool(request)
            .await
            .map_err(|error| tinybus::Error::failed(error.bus_message()))
    }
}

async fn setup(connection: Connection, config: SearchConfig) -> TinyBusResult<()> {
    let service = SearchService::with_providers(config, crate::provider::builtins());
    connection
        .serve_at(
            names::OBJECT_PATH.try_into()?,
            SearchBusService(Arc::new(service)),
        )
        .await?;
    connection.request_name(names::INTERFACE).await?;
    Ok(())
}

tinybus_module::module_export! {
    setup = setup,
    config = SearchConfig,
    worker_threads = 1,
    provides = ["ai.tinyhumans.tinysearch.Search"],
    methods = ["ListTools", "ExecuteTool"],
    signals = [],
    requires = [],
    optional = [],
    lazy = false,
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;

#[cfg(test)]
#[path = "mod_gemini_search_tests.rs"]
mod gemini_search_tests;
