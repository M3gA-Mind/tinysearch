//! Role tool dispatch: one generic tool per capability, served by the first
//! usable provider in the role's order with fallback past provider-side
//! failures.
use super::{SearchService, validate_arguments};
use crate::{Error, Result};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use tinysearch_bus::{
    ExecuteToolRequest, ExecuteToolResponse, Role, ToolSpec, role_for_tool, role_provider_tool,
    role_providers, role_tool_specs,
};

/// The provider that serves only `depth: "deep"` answers.
const DEEP_RESEARCH: &str = "gemini_deep_research";

/// Answer providers that serve only quick answers and are never used for
/// `depth: "deep"`.
const QUICK_ONLY: &[&str] = &["parallel"];

/// The Parallel chat model used for quick answers.
const PARALLEL_ANSWER_MODEL: &str = "speed";

impl SearchService {
    /// Executes a role tool across the role's providers.
    ///
    /// An explicit `provider` argument pins the call to that provider with no
    /// fallback. Otherwise providers are tried in role order, moving on only
    /// after a fallback-eligible error; `fallback_from` records the providers
    /// that failed before the one that answered.
    pub(super) async fn execute_role(
        &self,
        available: &BTreeMap<String, Vec<ToolSpec>>,
        request: ExecuteToolRequest,
    ) -> Result<ExecuteToolResponse> {
        let role = role_for_tool(&request.name)
            .ok_or_else(|| Error::UnavailableTool(request.name.clone()))?;
        let spec = role_tool_specs(available, &self.config.presentation, role)
            .ok_or_else(|| Error::UnavailableTool(request.name.clone()))?;
        let usable = role_providers(available, &self.config.presentation, role);
        let mut request = request;
        downgrade_unservable_depth(role, &usable, &mut request.arguments);
        validate_arguments(&spec, &request.arguments)?;
        let args = request
            .arguments
            .as_object()
            .ok_or(Error::InvalidArguments)?;
        let explicit = args.get("provider").and_then(Value::as_str);
        if role == Role::Answer {
            let depth = args.get("depth").and_then(Value::as_str);
            if explicit == Some(DEEP_RESEARCH) {
                let only_deep = usable.iter().all(|name| name == DEEP_RESEARCH);
                if depth == Some("quick") || (depth.is_none() && !only_deep) {
                    return Err(Error::InvalidArguments);
                }
            }
            if depth == Some("deep") && explicit.is_some_and(|name| QUICK_ONLY.contains(&name)) {
                return Err(Error::InvalidArguments);
            }
        }
        let candidates = match explicit {
            Some(provider) => vec![provider.to_owned()],
            None if role == Role::Answer => answer_order(usable, args),
            None => usable,
        };
        let mut failed: Vec<String> = Vec::new();
        let mut last_error = None;
        for provider in candidates {
            let tool = role_provider_tool(role, &provider)
                .and_then(|name| {
                    available
                        .get(&provider)?
                        .iter()
                        .find(|tool| tool.name == name)
                })
                .cloned()
                .ok_or_else(|| Error::UnavailableProvider(provider.clone()))?;
            let provider_request = ExecuteToolRequest {
                name: tool.name.clone(),
                arguments: provider_arguments(role, &provider, args, &tool),
            };
            match self
                .dispatch(&provider, &tool, provider_request, Some(role))
                .await
            {
                Ok(mut response) => {
                    if role == Role::Search && provider == "gemini" {
                        let count = result_count(args)
                            .as_u64()
                            .or_else(|| self.config.providers.get(&provider)?.max_results)
                            .unwrap_or(20)
                            .clamp(1, 20);
                        let count = usize::try_from(count).map_err(|_| Error::InvalidArguments)?;
                        response.results.truncate(count);
                        response.citations.truncate(count);
                    }
                    response.role = Some(role);
                    response.fallback_from = failed;
                    return Ok(response);
                }
                Err(error) if explicit.is_none() && error.is_fallback_eligible() => {
                    failed.push(provider);
                    // A rejected credential is the one failure the user can
                    // fix, so a later provider being merely unavailable must
                    // not replace it as the reported error.
                    if !last_error.as_ref().is_some_and(Error::is_unauthorized) {
                        last_error = Some(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(Error::UnavailableTool(request.name)))
    }
}

/// Orders answer providers for the requested depth.
///
/// `deep` puts Deep Research first when it is usable and keeps the grounded
/// providers as fallbacks, skipping the quick-only providers (Parallel).
/// `quick` never uses Deep Research. An absent depth is `quick` unless Deep
/// Research is the only usable provider.
fn answer_order(mut usable: Vec<String>, args: &Map<String, Value>) -> Vec<String> {
    let only_deep = usable.iter().all(|name| name == DEEP_RESEARCH);
    let deep = match args.get("depth").and_then(Value::as_str) {
        Some(depth) => depth == "deep",
        None => only_deep,
    };
    if deep {
        usable.retain(|name| !QUICK_ONLY.contains(&name.as_str()));
        if let Some(index) = usable.iter().position(|name| name == DEEP_RESEARCH) {
            let research = usable.remove(index);
            usable.insert(0, research);
        }
    } else {
        usable.retain(|name| name != DEEP_RESEARCH);
    }
    usable
}

/// The requested result count: `max_results`, else its `limit` alias.
fn result_count(args: &Map<String, Value>) -> Value {
    args.get("max_results")
        .or_else(|| args.get("limit"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Translates generic role arguments into `provider`'s own tool arguments,
/// keeping only the fields the provider tool declares.
pub(super) fn provider_arguments(
    role: Role,
    provider: &str,
    args: &Map<String, Value>,
    tool: &ToolSpec,
) -> Value {
    let get = |key: &str| args.get(key).cloned().unwrap_or(Value::Null);
    let mut mapped = match role {
        // Parallel's search takes an objective plus explicit queries.
        Role::Search if provider == "parallel" => json!({
            "objective":get("query"),
            "search_queries":[get("query")],
            "num_results":result_count(args)
        }),
        Role::Search => {
            let count_field = if provider == "brave" {
                "count"
            } else {
                "max_results"
            };
            json!({"query":get("query"), count_field:result_count(args)})
        }
        // Parallel answers through its chat completions API.
        Role::Answer if provider == "parallel" => json!({
            "model":PARALLEL_ANSWER_MODEL,
            "messages":[{"role":"user","content":get("query")}]
        }),
        Role::Answer => json!({"query":get("query")}),
        // Parallel's extract focuses on an objective and can return the full page.
        Role::Contents if provider == "parallel" => json!({
            "urls":get("urls"),
            "objective":get("query"),
            "full_content":true
        }),
        Role::Contents => {
            let mut mapped = json!({"urls":get("urls"),"query":get("query")});
            if provider == "tinyfish" {
                mapped["format"] = json!("markdown");
            }
            mapped
        }
    };
    let declared = tool.parameters.get("properties").and_then(Value::as_object);
    if let Some(fields) = mapped.as_object_mut() {
        fields.retain(|key, value| {
            !value.is_null() && declared.is_some_and(|declared| declared.contains_key(key))
        });
    }
    mapped
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;

/// Drops `depth: "deep"` when Deep Research is not among the role's usable
/// providers, so the call is answered at quick depth instead of failing.
///
/// The Answer declaration lists `deep` whenever a quick provider is usable,
/// because models send it for research questions and hosts validate against
/// the declaration before the module sees the call. Servability is therefore
/// decided from `usable` (the role's usable providers), never from the
/// advertised enum. Only `deep` is downgraded: an explicit `quick` that Deep
/// Research alone cannot serve, or an unrecognized string, is left for
/// `validate_arguments` to reject, so a caller's incompatible request is never
/// silently promoted to Deep Research.
fn downgrade_unservable_depth(role: Role, usable: &[String], arguments: &mut Value) {
    if role != Role::Answer || usable.iter().any(|name| name == DEEP_RESEARCH) {
        return;
    }
    let Some(args) = arguments.as_object_mut() else {
        return;
    };
    if args.get("depth").and_then(Value::as_str) == Some("deep") {
        args.remove("depth");
    }
}
