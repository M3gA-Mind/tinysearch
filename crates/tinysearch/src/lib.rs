//! `TinySearch` exposes provider-backed web search through `TinyBus`.
//!
//! The public [`SearchService`] handles discovery and dispatch. Provider
//! implementations supply tool schemas and normalized responses. Hosts pass
//! private configuration to the loadable module at initialization or refresh.
mod error;
mod provider;
mod search;
mod tinybus_module;
#[cfg(feature = "static-link")]
pub use tinybus_module::linked_module;
pub use error::{Error, Result};
pub use search::{ProviderFuture, SearchProvider, SearchService};
pub use tinysearch_bus;
pub use tinysearch_bus::{
    BACKEND_PROVIDERS, BackendAuthMode, BackendConfig, CONTRACT_VERSION, Citation,
    ExecuteToolRequest, ExecuteToolResponse, INTERFACE, ListToolsResponse, METHODS, OBJECT_PATH,
    PROVIDERS, PresentationConfig, PresentationMode, ProviderConfig, ProviderRoute, Role,
    SearchConfig, SearchResult, SearchStatus, ToolSpec, default_role_providers, errors,
    is_compatible, names, provider_roles, role_tool_name, version,
};
