//! Built-in HTTP providers and bounded result normalization.
use crate::{
    BackendAuthMode, BackendConfig, Error, ExecuteToolRequest, ExecuteToolResponse, ProviderConfig,
    ProviderFuture, ProviderRoute, Result, Role, SearchProvider, SearchStatus,
};
use reqwest::{Client, Method};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

const MAX_RESULTS: usize = 20;
const MAX_CITATIONS: usize = 40;
const MAX_ANSWER_CHARS: usize = 12_000;
const MAX_BODY_BYTES: u64 = 2_000_000;
/// Upper bound on how many Gemini grounding chunks/support entries are ever
/// examined when selecting citations, independent of `MAX_CITATIONS`. Keeps
/// traversal and the `seen` tracking allocation bounded even against an
/// oversized `groundingChunks`/`groundingSupports` provider payload, while
/// comfortably exceeding any grounding payload a real response produces (at
/// most a few dozen chunks for `MAX_CITATIONS = 40` citations).
const MAX_GROUNDING_CHUNKS: usize = 2_000;

#[derive(Debug)]
struct BuiltinProvider {
    name: &'static str,
    client: Client,
}

/// Returns the production provider registry. Providers without suitable
/// private configuration remain hidden by the service catalog.
pub(crate) fn builtins() -> BTreeMap<String, Arc<dyn SearchProvider>> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();
    tinysearch_bus::PROVIDERS
        .iter()
        .copied()
        .map(|name| {
            (
                name.into(),
                Arc::new(BuiltinProvider {
                    name,
                    client: client.clone(),
                }) as Arc<dyn SearchProvider>,
            )
        })
        .collect()
}

impl SearchProvider for BuiltinProvider {
    fn execute<'a>(
        &'a self,
        config: &'a ProviderConfig,
        backend: &'a BackendConfig,
        request: &'a ExecuteToolRequest,
    ) -> ProviderFuture<'a> {
        Box::pin(async move { self.run(config, backend, request).await })
    }

    fn execute_role<'a>(
        &'a self,
        config: &'a ProviderConfig,
        backend: &'a BackendConfig,
        request: &'a ExecuteToolRequest,
        role: Role,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.run_for_role(config, backend, request, Some(role))
                .await
        })
    }
}

impl BuiltinProvider {
    async fn run(
        &self,
        config: &ProviderConfig,
        backend: &BackendConfig,
        request: &ExecuteToolRequest,
    ) -> Result<ExecuteToolResponse> {
        self.run_for_role(config, backend, request, None).await
    }

    async fn run_for_role(
        &self,
        config: &ProviderConfig,
        backend: &BackendConfig,
        request: &ExecuteToolRequest,
        role: Option<Role>,
    ) -> Result<ExecuteToolResponse> {
        // A key counts only if this provider's request carries one. SearXNG
        // never sends `credential`, so a stray value in its config must not
        // turn an ACL or proxy refusal into "your key was rejected".
        let keyed = !KEYLESS_PROVIDERS.contains(&self.name)
            && config
                .credential
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty());
        self.run_unattributed(config, backend, request, role)
            .await
            .map_err(|error| error.attributed_to(self.name, keyed))
    }

    /// [`Self::run`] before a rejected credential is attributed: a backend
    /// rejection is already marked by `send_json`, and anything left
    /// unattributed belongs to this provider's own key.
    async fn run_unattributed(
        &self,
        config: &ProviderConfig,
        backend: &BackendConfig,
        request: &ExecuteToolRequest,
        role: Option<Role>,
    ) -> Result<ExecuteToolResponse> {
        let (path, body) = match self.name {
            "exa" if config.route == ProviderRoute::Backend => exa_request(request)?,
            "exa" | "parallel" | "brave" | "querit" | "tavily" | "seltz" | "searxng"
            | "tinyfish" | "keenable" => {
                return direct::run(&self.client, self.name, config, request).await;
            }
            "gemini" => return self.gemini(config, backend, request, role).await,
            "gemini_deep_research" => return self.deep_research(config, request).await,
            _ => return Err(Error::UnavailableProvider(self.name.into())),
        };
        if config.route != ProviderRoute::Backend {
            return Err(Error::Provider(
                "this provider requires the backend route".into(),
            ));
        }
        let value = send_json(
            &self.client,
            Method::POST,
            backend_url(backend, &path)?,
            Some(body),
            Auth::Backend(backend),
            direct::configured_timeout(config, Duration::from_secs(35)),
        )
        .await?;
        let value = unwrap_backend(value)?;
        if matches!(status_state(&value), Some("failed" | "cancelled" | "error")) {
            return Err(Error::Provider("provider task failed".into()));
        }
        let mut response = normalize(self.name, &request.name, &value);
        if matches!(
            status_state(&value),
            Some("pending" | "queued" | "running" | "in_progress")
        ) {
            response.status = SearchStatus::InProgress;
        }
        Ok(response)
    }

    async fn gemini(
        &self,
        config: &ProviderConfig,
        backend: &BackendConfig,
        request: &ExecuteToolRequest,
        role: Option<Role>,
    ) -> Result<ExecuteToolResponse> {
        if request.name != "gemini_agentic_search" {
            return Err(Error::UnavailableTool(request.name.clone()));
        }
        let query = required_string(&request.arguments, "query")?;
        let model = request
            .arguments
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("gemini-3.8-flash");
        if !model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
        {
            return Err(Error::InvalidArguments);
        }
        let tools = if config.route == ProviderRoute::Backend {
            json!([{"googleSearch":{}}])
        } else {
            json!([{"google_search":{}}])
        };
        let body = json!({"contents":[{"parts":[{"text":query}]}],"tools":tools});
        let (url, auth) = match config.route {
            ProviderRoute::Backend => (
                backend_url(
                    backend,
                    &format!("/agent-integrations/gemini/models/{model}/generate-content"),
                )?,
                Auth::Backend(backend),
            ),
            ProviderRoute::Direct => (
                direct_url(
                    config.base_url.as_deref(),
                    "https://generativelanguage.googleapis.com",
                    &format!("/v1beta/models/{model}:generateContent"),
                )?,
                Auth::Google(
                    config
                        .credential
                        .as_deref()
                        .ok_or_else(|| Error::Provider("Gemini credential unavailable".into()))?,
                ),
            ),
        };
        let is_backend = matches!(config.route, ProviderRoute::Backend);
        let value = send_json(
            &self.client,
            Method::POST,
            url,
            Some(body),
            auth,
            Duration::from_secs(35),
        )
        .await?;
        let value = if is_backend {
            unwrap_backend(value)?
        } else {
            value
        };
        Ok(if role == Some(Role::Search) {
            gemini_search::normalize(&value)
        } else {
            normalize("gemini", &request.name, &value)
        })
    }

    async fn deep_research(
        &self,
        config: &ProviderConfig,
        request: &ExecuteToolRequest,
    ) -> Result<ExecuteToolResponse> {
        if request.name != "gemini_deep_research" {
            return Err(Error::UnavailableTool(request.name.clone()));
        }
        if config.route != ProviderRoute::Direct {
            return Err(Error::Provider(
                "Deep Research requires a direct Gemini route".into(),
            ));
        }
        let key = config
            .credential
            .as_deref()
            .ok_or_else(|| Error::Provider("Gemini credential unavailable".into()))?;
        let base = config
            .base_url
            .as_deref()
            .unwrap_or("https://generativelanguage.googleapis.com");
        let resume_id = request
            .arguments
            .get("interaction_id")
            .and_then(Value::as_str);
        let mut value = if let Some(id) = resume_id {
            validate_interaction_id(id)?;
            let url = direct_url(Some(base), base, &format!("/v1beta/interactions/{id}"))?;
            send_json(
                &self.client,
                Method::GET,
                url,
                None,
                Auth::Google(key),
                Duration::from_secs(35),
            )
            .await?
        } else {
            let query = required_string(&request.arguments, "query")?;
            let url = direct_url(Some(base), base, "/v1beta/interactions")?;
            let body =
                json!({"input":query,"agent":"deep-research-preview-04-2026","background":true});
            send_json(
                &self.client,
                Method::POST,
                url,
                Some(body),
                Auth::Google(key),
                Duration::from_secs(35),
            )
            .await?
        };
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .or(resume_id)
            .ok_or_else(|| Error::Provider("Deep Research response omitted interaction id".into()))?
            .to_owned();
        validate_interaction_id(&id)?;
        let max_attempts = request
            .arguments
            .get("max_poll_attempts")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .min(30);
        for attempt in 0..=max_attempts {
            match value.get("status").and_then(Value::as_str).unwrap_or("") {
                "completed" => return Ok(normalize("gemini_deep_research", &request.name, &value)),
                "failed" | "cancelled" => {
                    return Err(Error::Provider("Deep Research task failed".into()));
                }
                _ => {}
            }
            if attempt == max_attempts {
                break;
            }
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            let url = direct_url(Some(base), base, &format!("/v1beta/interactions/{id}"))?;
            value = send_json(
                &self.client,
                Method::GET,
                url,
                None,
                Auth::Google(key),
                Duration::from_secs(35),
            )
            .await?;
        }
        let mut response = normalize("gemini_deep_research", &request.name, &value);
        response.status = SearchStatus::InProgress;
        response.answer = None;
        response.provider_data = Some(json!({"interaction_id":id,"status":"in_progress"}));
        Ok(response)
    }
}

fn status_state(value: &Value) -> Option<&str> {
    value.get("status").and_then(|status| {
        status
            .as_str()
            .or_else(|| status.get("state").and_then(Value::as_str))
    })
}

fn validate_interaction_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::Provider(
            "invalid Deep Research interaction id".into(),
        ));
    }
    Ok(())
}

/// Providers whose requests never carry `ProviderConfig::credential`.
const KEYLESS_PROVIDERS: &[&str] = &["searxng"];

enum Auth<'a> {
    Backend(&'a BackendConfig),
    Google(&'a str),
}
fn backend_url(config: &BackendConfig, path: &str) -> Result<String> {
    let base = config
        .base_url
        .as_deref()
        .ok_or_else(|| Error::Provider("backend URL unavailable".into()))?;
    direct_url(Some(base), base, path)
}
fn direct_url(override_base: Option<&str>, default_base: &str, path: &str) -> Result<String> {
    let base = override_base.unwrap_or(default_base);
    let url =
        reqwest::Url::parse(base).map_err(|_| Error::Provider("invalid provider URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Provider("invalid provider URL".into()));
    }
    Ok(format!("{}{}", base.trim_end_matches('/'), path))
}
async fn send_json(
    client: &Client,
    method: Method,
    url: String,
    body: Option<Value>,
    auth: Auth<'_>,
    timeout: Duration,
) -> Result<Value> {
    let mut request = client
        .request(method, url)
        .timeout(timeout)
        .header(reqwest::header::ACCEPT, "application/json");
    let auth_kind = match &auth {
        Auth::Backend(_) => AuthKind::Backend,
        Auth::Google(_) => AuthKind::Provider,
    };
    request = match auth {
        Auth::Backend(config) => {
            let credential = config
                .credential
                .as_deref()
                .ok_or_else(|| Error::Provider("backend credential unavailable".into()))?;
            let mut req = match config.auth_mode {
                BackendAuthMode::Session => request.bearer_auth(credential),
                BackendAuthMode::ApiKey => request.header("x-api-key", credential),
            };
            if let Some(name) = config.sdk_name.as_deref() {
                req = req.header("x-sdk-name", name);
            }
            req
        }
        Auth::Google(key) => request.header("x-goog-api-key", key),
    };
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(http::transport_error)?;
    let result = http::read_json(response).await;
    match auth_kind {
        AuthKind::Backend => result.map_err(Error::from_backend),
        AuthKind::Provider => result,
    }
}

/// Whose credential a request carried, kept after [`Auth`] is consumed.
#[derive(Clone, Copy)]
enum AuthKind {
    Backend,
    Provider,
}

fn unwrap_backend(mut value: Value) -> Result<Value> {
    if value.get("success") == Some(&Value::Bool(false)) {
        return Err(http::classify_envelope(&value));
    }
    if value.get("success") == Some(&Value::Bool(true))
        && let Some(data) = value.as_object_mut().and_then(|o| o.remove("data"))
    {
        return Ok(data);
    }
    Ok(value)
}

fn required_string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or(Error::InvalidArguments)
}
fn mapped(args: &Value, pairs: &[(&str, &str)]) -> Value {
    let mut body = Map::new();
    for (source, target) in pairs {
        if let Some(value) = args.get(*source) {
            body.insert((*target).into(), value.clone());
        }
    }
    Value::Object(body)
}
/// Maps an Exa tool onto the managed backend's Exa routes.
///
/// The backend's search route accepts only `{objective, searchQueries}` and
/// fans the queries out itself; the other routes forward Exa's own request
/// body, so they reuse the direct mapping.
fn exa_request(request: &ExecuteToolRequest) -> Result<(String, Value)> {
    let args = &request.arguments;
    let (path, body) = match request.name.as_str() {
        "exa_search" => {
            let query = required_string(args, "query")?;
            ("search", json!({"objective":query,"searchQueries":[query]}))
        }
        "exa_get_contents" | "exa_find_similar" | "exa_answer" => {
            let (path, body) = direct::exa_body(request)?;
            (path.trim_start_matches('/'), body)
        }
        _ => return Err(Error::UnavailableTool(request.name.clone())),
    };
    Ok((format!("/agent-integrations/exa/{path}"), body))
}

mod direct;
mod gemini_search;
mod http;
mod normalize;
use normalize::normalize;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;

#[cfg(test)]
#[path = "gemini_search_tests.rs"]
mod gemini_search_tests;
