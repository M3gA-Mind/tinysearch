//! Wire payloads and configuration for `TinySearch`.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

/// Module configuration supplied privately at initialization or refresh.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Whether search is available at all.
    pub enabled: bool,
    /// Managed backend settings.
    pub backend: BackendConfig,
    /// Provider settings keyed by stable provider name.
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Tool presentation settings.
    pub presentation: PresentationConfig,
}
impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: BackendConfig::default(),
            providers: BTreeMap::new(),
            presentation: PresentationConfig::default(),
        }
    }
}
impl fmt::Debug for SearchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchConfig")
            .field("enabled", &self.enabled)
            .field("backend", &self.backend)
            .field("providers", &self.providers)
            .field("presentation", &self.presentation)
            .finish()
    }
}

/// Settings for calls routed through the managed backend.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendConfig {
    /// Backend base URL.
    pub base_url: Option<String>,
    /// Private credential supplied by the host.
    pub credential: Option<String>,
    /// Product attribution header value.
    pub sdk_name: Option<String>,
    /// Authentication scheme for the backend credential.
    pub auth_mode: BackendAuthMode,
}
impl fmt::Debug for BackendConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackendConfig")
            .field("base_url", &self.base_url)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "[REDACTED]"),
            )
            .field("sdk_name", &self.sdk_name)
            .field("auth_mode", &self.auth_mode)
            .finish()
    }
}

/// Authentication scheme for a managed backend credential.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendAuthMode {
    /// Session token sent as an Authorization bearer.
    #[default]
    Session,
    /// API key sent in the x-api-key header.
    ApiKey,
}

/// Route for a provider request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProviderRoute {
    /// Call the provider directly.
    #[default]
    Direct,
    /// Call through the managed backend.
    #[serde(alias = "managed")]
    Backend,
}

impl ProviderRoute {
    /// The name hosts use in user-facing configuration (`managed` / `direct`).
    ///
    /// The wire name of [`ProviderRoute::Backend`] is `backend`; user config
    /// has always called the same route `managed`.
    #[must_use]
    pub const fn config_name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Backend => "managed",
        }
    }

    /// Parses a configuration name; accepts `managed`, `backend` and `direct`
    /// case-insensitively, ignoring surrounding whitespace.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "managed" | "backend" => Some(Self::Backend),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }
}

/// Serde adapter that reads and writes [`ProviderRoute`] in the user-config
/// spelling (`managed` / `direct`) instead of the wire spelling.
///
/// Use as `#[serde(with = "tinysearch_bus::managed_route")]`.
pub mod managed_route {
    use super::ProviderRoute;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes with [`ProviderRoute::config_name`].
    ///
    /// # Errors
    /// Propagates the serializer's error.
    pub fn serialize<S: Serializer>(route: &ProviderRoute, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(route.config_name())
    }

    /// Deserializes either spelling.
    ///
    /// # Errors
    /// Fails on an unknown route name.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ProviderRoute, D::Error> {
        let raw = String::deserialize(d)?;
        ProviderRoute::parse(&raw)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown search route `{raw}`")))
    }
}

/// Provider-specific private configuration.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Whether the provider is enabled.
    pub enabled: bool,
    /// Optional direct provider credential.
    pub credential: Option<String>,
    /// Optional provider base URL override.
    pub base_url: Option<String>,
    /// Chosen request route.
    pub route: ProviderRoute,
    /// Default result count when a tool call does not specify one.
    pub max_results: Option<u64>,
    /// Direct provider request timeout in seconds.
    pub timeout_secs: Option<u64>,
    /// Default `SearXNG` language when the search call omits one.
    pub default_language: Option<String>,
}
impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            credential: None,
            base_url: None,
            route: ProviderRoute::Direct,
            max_results: None,
            timeout_secs: None,
            default_language: None,
        }
    }
}
impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("enabled", &self.enabled)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "[REDACTED]"),
            )
            .field("base_url", &self.base_url)
            .field("route", &self.route)
            .field("max_results", &self.max_results)
            .field("timeout_secs", &self.timeout_secs)
            .field("default_language", &self.default_language)
            .finish()
    }
}

/// A capability a provider can serve, presented to the model as one tool.
///
/// Each role is backed by an ordered list of providers: the first usable one
/// answers and the rest are fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Ranked web results (links with snippets) for a query.
    Search,
    /// A synthesized answer to a question, grounded in cited web sources.
    Answer,
    /// The readable contents of specific URLs.
    Contents,
}

impl Role {
    /// Every role, in presentation order.
    pub const ALL: [Self; 3] = [Self::Search, Self::Answer, Self::Contents];

    /// The stable wire name of the role, as it appears in configuration.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Answer => "answer",
            Self::Contents => "contents",
        }
    }
}

/// Every role, in display order.
pub const ROLES: [Role; 3] = Role::ALL;

/// The `[search.roles]` key for a role, which is its serde name.
#[must_use]
pub const fn role_key(role: Role) -> &'static str {
    role.as_str()
}

/// Parses a `[search.roles]` key; surrounding whitespace is ignored.
#[must_use]
pub fn parse_role(key: &str) -> Option<Role> {
    let key = key.trim();
    ROLES.into_iter().find(|role| role_key(*role) == key)
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How provider tools appear to the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PresentationMode {
    /// One generic tool per [`Role`] that has at least one usable provider,
    /// dispatched across the role's ordered provider list with fallback.
    #[default]
    Roles,
    /// One tool for each available provider.
    AllTools,
    /// One router tool with an optional provider argument.
    Router,
    /// One tool bound to the configured provider.
    OneProvider,
}

impl PresentationMode {
    /// The stable wire and configuration name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Roles => "roles",
            Self::AllTools => "all_tools",
            Self::Router => "router",
            Self::OneProvider => "one_provider",
        }
    }

    /// Parses a name, case-insensitively, ignoring surrounding whitespace.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "roles" => Some(Self::Roles),
            "all_tools" => Some(Self::AllTools),
            "router" => Some(Self::Router),
            "one_provider" => Some(Self::OneProvider),
            _ => None,
        }
    }
}

/// Presentation configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresentationConfig {
    /// Selected presentation mode.
    pub mode: PresentationMode,
    /// Provider selected for `one_provider`, or router default.
    pub provider: Option<String>,
    /// Ordered provider list per role for `roles` mode: the first usable
    /// provider answers and the rest are fallbacks. An absent or empty role
    /// uses [`default_role_providers`](crate::default_role_providers).
    #[serde(default)]
    pub roles: BTreeMap<Role, Vec<String>>,
}

/// Tool declaration returned by `ListTools`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Stable tool name.
    pub name: String,
    /// Description for the model.
    pub description: String,
    /// Provider-defined JSON Schema for arguments.
    pub parameters: Value,
}

/// Response from `ListTools`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListToolsResponse {
    /// Currently available tools.
    pub tools: Vec<ToolSpec>,
}

/// Request to execute a declared tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecuteToolRequest {
    /// Name from `ListTools`.
    pub name: String,
    /// Provider-defined arguments.
    pub arguments: Value,
}

/// Normalized search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Result title.
    pub title: String,
    /// Canonical URL.
    pub url: String,
    /// Optional excerpt.
    pub snippet: Option<String>,
    /// Optional publication date reported by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

/// Normalized citation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    /// Citation URL.
    pub url: String,
    /// Optional citation title.
    pub title: Option<String>,
}

/// Outcome classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStatus {
    /// Provider returned a successful response.
    Ok,
    /// Provider returned no results.
    Empty,
    /// An asynchronous research operation is still running.
    InProgress,
}

/// Provider-independent tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecuteToolResponse {
    /// Provider that handled the request.
    pub provider: String,
    /// Normalized results.
    pub results: Vec<SearchResult>,
    /// Normalized citations.
    pub citations: Vec<Citation>,
    /// Optional synthesized answer.
    pub answer: Option<String>,
    /// Outcome status.
    pub status: SearchStatus,
    /// Optional provider-specific data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_data: Option<Value>,
    /// Role served, when the call was a role tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Providers tried before the one that answered, in order. Each failed
    /// with a fallback-eligible error (see [`errors`](crate::errors)).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_from: Vec<String>,
}
