//! Configuration-based discovery and routing for search providers.
use crate::{Error, Result};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};
use tinysearch_bus::{
    BackendConfig, ExecuteToolRequest, ExecuteToolResponse, ListToolsResponse, PresentationMode,
    ProviderConfig, Role, SearchConfig, ToolSpec, configured_provider_tools, provider_tool_specs,
    role_provider_tool, role_providers, select_tools,
};

mod roles;

/// Boxed provider future. Providers may perform asynchronous I/O.
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ExecuteToolResponse>> + Send + 'a>>;

/// An implementation of one provider's tool family.
pub trait SearchProvider: Send + Sync {
    /// Executes a catalog-declared tool using private configuration.
    fn execute<'a>(
        &'a self,
        config: &'a ProviderConfig,
        backend: &'a BackendConfig,
        request: &'a ExecuteToolRequest,
    ) -> ProviderFuture<'a>;

    /// Executes a provider tool for a capability role.
    ///
    /// Providers may shape the same tool's output differently for Search and
    /// Answer. The default preserves implementations that serve one response
    /// form for all roles.
    fn execute_role<'a>(
        &'a self,
        config: &'a ProviderConfig,
        backend: &'a BackendConfig,
        request: &'a ExecuteToolRequest,
        _role: Role,
    ) -> ProviderFuture<'a> {
        self.execute(config, backend, request)
    }
}

/// Provider-independent `TinySearch` service.
pub struct SearchService {
    config: SearchConfig,
    providers: BTreeMap<String, Arc<dyn SearchProvider>>,
}

impl std::fmt::Debug for SearchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchService")
            .field("config", &self.config)
            .field("provider_names", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SearchService {
    /// Constructs a service with injected provider implementations.
    #[must_use]
    pub fn with_providers(
        config: SearchConfig,
        providers: BTreeMap<String, Arc<dyn SearchProvider>>,
    ) -> Self {
        Self { config, providers }
    }

    /// Constructs a service with `TinySearch`'s built-in providers, the same
    /// ones the loadable module serves, for hosts that link this crate as a
    /// library instead of loading the module. Which providers are usable, and
    /// on which route, still comes from `config`.
    #[must_use]
    pub fn with_builtins(config: SearchConfig) -> Self {
        Self::with_providers(config, crate::provider::builtins())
    }

    /// Returns declarations for tools currently enabled by configuration.
    #[must_use]
    pub fn list_tools(&self) -> ListToolsResponse {
        if !self.config.enabled {
            return ListToolsResponse::default();
        }
        select_tools(&self.available_tools(), &self.config.presentation)
    }

    /// Executes a currently advertised tool.
    ///
    /// In `roles` mode the tool is a role tool, dispatched across the role's
    /// providers with fallback (see [`Error::is_fallback_eligible`]).
    ///
    /// # Errors
    /// Returns an error for disabled search, malformed arguments, or unavailable
    /// tools/providers, and propagates provider errors.
    pub async fn execute_tool(&self, request: ExecuteToolRequest) -> Result<ExecuteToolResponse> {
        if !self.config.enabled {
            return Err(Error::Disabled);
        }
        if !request.arguments.is_object() {
            return Err(Error::InvalidArguments);
        }
        let available = self.available_tools();
        match self.config.presentation.mode {
            PresentationMode::Roles => self.execute_role(&available, request).await,
            PresentationMode::Router => self.execute_router(&available, request).await,
            PresentationMode::AllTools | PresentationMode::OneProvider => {
                let advertised = select_tools(&available, &self.config.presentation);
                if !advertised
                    .tools
                    .iter()
                    .any(|tool| tool.name == request.name)
                {
                    return Err(Error::UnavailableTool(request.name));
                }
                let (provider, tool) = available
                    .iter()
                    .find_map(|(name, tools)| {
                        tools
                            .iter()
                            .find(|tool| tool.name == request.name)
                            .map(|tool| (name.clone(), tool.clone()))
                    })
                    .ok_or_else(|| Error::UnavailableTool(request.name.clone()))?;
                self.dispatch(&provider, &tool, request, None).await
            }
        }
    }

    /// Routes Search only to eligible providers, retaining their native options.
    ///
    /// Gemini's generic count options bound normalized results locally.
    async fn execute_router(
        &self,
        available: &BTreeMap<String, Vec<ToolSpec>>,
        request: ExecuteToolRequest,
    ) -> Result<ExecuteToolResponse> {
        if request.name != "search" {
            return Err(Error::UnavailableTool(request.name));
        }
        let router = select_tools(available, &self.config.presentation)
            .tools
            .into_iter()
            .next()
            .ok_or_else(|| Error::UnavailableTool(request.name.clone()))?;
        validate_arguments(&router, &request.arguments)?;
        let explicit = request
            .arguments
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let selected = explicit
            .or_else(|| self.config.presentation.provider.clone())
            .or_else(|| {
                role_providers(available, &self.config.presentation, Role::Search)
                    .into_iter()
                    .next()
            })
            .ok_or(Error::MissingProvider)?;
        // Explicit and configured Router choices override fallback order.
        // Eligibility still requires the selected provider's Search tool.
        let first_tool = role_provider_tool(Role::Search, &selected)
            .and_then(|name| {
                available
                    .get(&selected)?
                    .iter()
                    .find(|tool| tool.name == name)
            })
            .cloned()
            .ok_or_else(|| Error::UnavailableProvider(selected.clone()))?;
        let mut provider_request = request;
        let mut requested_count = None;
        if let Some(values) = provider_request.arguments.as_object_mut() {
            values.remove("provider");
            if selected == "gemini" {
                // These Search options bound normalized output, rather than
                // belonging to the native Gemini query/model request schema.
                for field in ["max_results", "limit"] {
                    if let Some(value) = values.get(field)
                        && !value
                            .as_u64()
                            .is_some_and(|count| (1..=20).contains(&count))
                    {
                        return Err(Error::InvalidArguments);
                    }
                }
                requested_count = values
                    .remove("max_results")
                    .or_else(|| values.get("limit").cloned())
                    .and_then(|value| value.as_u64());
                values.remove("limit");
            }
        }
        let mut response = self
            .dispatch(&selected, &first_tool, provider_request, Some(Role::Search))
            .await?;
        if selected == "gemini" {
            self.bound_gemini_search(requested_count, &mut response)?;
        }
        Ok(response)
    }

    /// Bounds aligned Gemini Search results and citations using the request,
    /// then configured maximum, then 20, with a common 1–20 ceiling.
    fn bound_gemini_search(
        &self,
        requested: Option<u64>,
        response: &mut ExecuteToolResponse,
    ) -> Result<()> {
        let count = requested
            .or_else(|| self.config.providers.get("gemini")?.max_results)
            .unwrap_or(20)
            .clamp(1, 20);
        let count = usize::try_from(count).map_err(|_| Error::InvalidArguments)?;
        response.results.truncate(count);
        response.citations.truncate(count);
        Ok(())
    }

    /// Validates `request` against a provider tool and runs it on `provider`.
    async fn dispatch(
        &self,
        provider_name: &str,
        tool: &ToolSpec,
        mut request: ExecuteToolRequest,
        role: Option<Role>,
    ) -> Result<ExecuteToolResponse> {
        let provider = self
            .providers
            .get(provider_name)
            .ok_or_else(|| Error::UnavailableProvider(provider_name.to_owned()))?;
        let config = self
            .config
            .providers
            .get(provider_name)
            .cloned()
            .unwrap_or_default();
        request.name.clone_from(&tool.name);
        validate_arguments(tool, &request.arguments)?;
        let mut response = match role {
            Some(role) => provider.execute_role(&config, &self.config.backend, &request, role),
            None => provider.execute(&config, &self.config.backend, &request),
        }
        .await?;
        provider_name.clone_into(&mut response.provider);
        Ok(response)
    }

    fn available_tools(&self) -> BTreeMap<String, Vec<ToolSpec>> {
        configured_provider_tools(&self.config, &provider_tool_specs())
            .into_iter()
            .filter(|(name, tools)| self.providers.contains_key(name) && !tools.is_empty())
            .collect()
    }
}

fn validate_arguments(tool: &ToolSpec, arguments: &serde_json::Value) -> Result<()> {
    let values = arguments.as_object().ok_or(Error::InvalidArguments)?;
    let schema = &tool.parameters;
    let properties = schema
        .get("properties")
        .and_then(serde_json::Value::as_object);
    if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
        for field in required {
            if !field.as_str().is_some_and(|name| values.contains_key(name)) {
                return Err(Error::InvalidArguments);
            }
        }
    }
    for (name, value) in values {
        if let Some(property) = properties.and_then(|properties| properties.get(name)) {
            if !matches_schema(property, value) {
                return Err(Error::InvalidArguments);
            }
        } else if schema.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
            return Err(Error::UnsupportedArgument(name.clone()));
        } else if let Some(extra_schema) =
            schema.get("additionalProperties").filter(|v| v.is_object())
            && !matches_schema(extra_schema, value)
        {
            return Err(Error::InvalidArguments);
        }
    }
    Ok(())
}

fn matches_schema(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    if let Some(alternatives) = schema.get("oneOf").and_then(serde_json::Value::as_array)
        && alternatives
            .iter()
            .filter(|choice| matches_schema(choice, value))
            .count()
            != 1
    {
        return false;
    }
    if let Some(kind) = schema.get("type").and_then(serde_json::Value::as_str) {
        let valid_type = match kind {
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            "null" => value.is_null(),
            _ => false,
        };
        if !valid_type {
            return false;
        }
    }
    if let Some(options) = schema.get("enum").and_then(serde_json::Value::as_array)
        && !options.contains(value)
    {
        return false;
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count();
        if !within_usize_bounds(schema, length, "minLength", "maxLength") {
            return false;
        }
    }
    if let Some(items) = value.as_array() {
        if !within_usize_bounds(schema, items.len(), "minItems", "maxItems") {
            return false;
        }
        if let Some(item_schema) = schema.get("items")
            && !items.iter().all(|item| matches_schema(item_schema, item))
        {
            return false;
        }
    }
    if let Some(number) = value.as_f64()
        && (schema
            .get("minimum")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|min| number < min)
            || schema
                .get("maximum")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|max| number > max)
            || schema
                .get("exclusiveMinimum")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|min| number <= min)
            || schema
                .get("exclusiveMaximum")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|max| number >= max))
    {
        return false;
    }
    if let Some(fields) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array)
            && required
                .iter()
                .any(|v| v.as_str().is_none_or(|name| !fields.contains_key(name)))
        {
            return false;
        }
        if let Some(properties) = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for (key, item) in fields {
                if let Some(property) = properties.get(key) {
                    if !matches_schema(property, item) {
                        return false;
                    }
                } else if schema.get("additionalProperties")
                    == Some(&serde_json::Value::Bool(false))
                {
                    return false;
                }
            }
        }
    }
    true
}

fn within_usize_bounds(
    schema: &serde_json::Value,
    size: usize,
    min_key: &str,
    max_key: &str,
) -> bool {
    let size = size as u128;
    schema
        .get(min_key)
        .and_then(serde_json::Value::as_u64)
        .is_none_or(|min| size >= u128::from(min))
        && schema
            .get(max_key)
            .and_then(serde_json::Value::as_u64)
            .is_none_or(|max| size <= u128::from(max))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
