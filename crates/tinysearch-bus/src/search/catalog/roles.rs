//! Capability roles: which providers serve each role, and the generic role
//! tool declarations presented in [`PresentationMode::Roles`](super::PresentationMode::Roles).
use super::{PresentationConfig, Role, ToolSpec, tool};
use crate::names::role_tool_name;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The Gemini Deep Research provider serves only `depth: "deep"` answers.
pub(crate) const DEEP_RESEARCH: &str = "gemini_deep_research";

/// Returns the roles `provider` can serve; empty for an unknown provider.
#[must_use]
pub fn provider_roles(provider: &str) -> &'static [Role] {
    match provider {
        "exa" | "parallel" => &[Role::Search, Role::Answer, Role::Contents],
        "gemini" => &[Role::Search, Role::Answer],
        "gemini_deep_research" => &[Role::Answer],
        "tinyfish" | "tavily" | "keenable" => &[Role::Search, Role::Contents],
        "brave" | "querit" | "seltz" | "searxng" => &[Role::Search],
        _ => &[],
    }
}

/// Returns the provider order a role uses when configuration names none.
#[must_use]
pub fn default_role_providers(role: Role) -> &'static [&'static str] {
    match role {
        Role::Search => &[
            "exa", "brave", "tavily", "parallel", "querit", "seltz", "searxng", "tinyfish",
            "keenable", "gemini",
        ],
        Role::Answer => &["gemini", "gemini_deep_research", "exa", "parallel"],
        Role::Contents => &["exa", "tavily", "parallel", "tinyfish", "keenable"],
    }
}

/// Returns the provider tool that serves `role` for `provider`, if any.
#[must_use]
pub fn role_provider_tool(role: Role, provider: &str) -> Option<&'static str> {
    Some(match (role, provider) {
        (Role::Search, "exa") => "exa_search",
        (Role::Search, "gemini") => "gemini_agentic_search",
        (Role::Search, "brave") => "brave_web_search",
        (Role::Search, "tavily") => "tavily_search",
        (Role::Search, "querit") => "querit_search",
        (Role::Search, "seltz") => "seltz_search",
        (Role::Search, "searxng") => "searxng_search",
        (Role::Search, "tinyfish") => "tinyfish_search",
        (Role::Search, "parallel") => "parallel_search",
        (Role::Search, "keenable") => "keenable_search",
        (Role::Answer, "gemini") => "gemini_agentic_search",
        (Role::Answer, "gemini_deep_research") => "gemini_deep_research",
        (Role::Answer, "exa") => "exa_answer",
        (Role::Answer, "parallel") => "parallel_chat",
        (Role::Contents, "exa") => "exa_get_contents",
        (Role::Contents, "tavily") => "tavily_extract",
        (Role::Contents, "tinyfish") => "tinyfish_fetch",
        (Role::Contents, "parallel") => "parallel_extract",
        (Role::Contents, "keenable") => "keenable_fetch",
        _ => return None,
    })
}

/// Returns the usable providers for `role`, in the order they are tried.
///
/// The order is the configured list for the role, or
/// [`default_role_providers`] when that list is absent or empty. A provider is
/// usable when it serves the role and its role tool is among
/// `provider_tools` (the output of
/// [`configured_provider_tools`](crate::configured_provider_tools)).
/// Duplicates keep their first position.
#[must_use]
pub fn role_providers(
    provider_tools: &BTreeMap<String, Vec<ToolSpec>>,
    presentation: &PresentationConfig,
    role: Role,
) -> Vec<String> {
    let configured = presentation
        .roles
        .get(&role)
        .filter(|list| !list.is_empty());
    let order: Vec<&str> = configured.map_or_else(
        || default_role_providers(role).to_vec(),
        |list| list.iter().map(String::as_str).collect(),
    );
    let mut usable: Vec<String> = Vec::new();
    for provider in order {
        let serves = provider_roles(provider).contains(&role)
            && role_provider_tool(role, provider).is_some_and(|name| {
                provider_tools
                    .get(provider)
                    .is_some_and(|tools| tools.iter().any(|tool| tool.name == name))
            });
        if serves && !usable.iter().any(|seen| seen == provider) {
            usable.push(provider.to_owned());
        }
    }
    usable
}

/// Returns the generic tool declaration for `role`, or `None` when no
/// provider can serve it.
#[must_use]
pub fn role_tool_specs(
    provider_tools: &BTreeMap<String, Vec<ToolSpec>>,
    presentation: &PresentationConfig,
    role: Role,
) -> Option<ToolSpec> {
    let providers = role_providers(provider_tools, presentation, role);
    if providers.is_empty() {
        return None;
    }
    let text = json!({"type":"string","minLength":1});
    let provider = json!({
        "type":"string",
        "enum":providers,
        "description":"Force one provider; disables fallback to the others."
    });
    let backed_by = providers.join(", ");
    let (description, properties, required): (String, Value, &[&str]) = match role {
        Role::Search => (
            format!(
                "Search the web and get ranked result links with short snippets. Use it to find \
                 sources, current information, or pages to read next. Backed by: {backed_by} \
                 (tried in that order; later providers are fallbacks)."
            ),
            // `limit` is accepted as an alias: models carry it over from
            // the other list-style tools beside this one, and rejecting it
            // cost the search (production traces, Oct 2026).
            json!({
                "query":text,
                "max_results":{"type":"integer","minimum":1,"maximum":20},
                "limit":{
                    "type":"integer",
                    "minimum":1,
                    "maximum":20,
                    "description":"Same as max_results."
                },
                "provider":provider
            }),
            &["query"],
        ),
        Role::Answer => {
            let quick = providers.iter().any(|name| name != DEEP_RESEARCH);
            let deep = providers.iter().any(|name| name == DEEP_RESEARCH);
            // With a quick provider, `deep` is always declared: models send it
            // for research questions regardless, hosts validate against this
            // schema before the module sees the call, and the module answers
            // it at quick depth when Deep Research is not usable. Declaring
            // only `quick` there rejected those calls (production traces, Oct
            // 2026). Deep Research alone cannot serve `quick`, so that case
            // declares only `deep`.
            let depths: &[&str] = if quick { &["quick", "deep"] } else { &["deep"] };
            let default_depth = if quick { "quick" } else { "deep" };
            let deep_note = if deep {
                " depth=\"deep\" runs a longer multi-step research report and may return an \
                 in-progress status with an interaction id."
            } else {
                " Deep research is not available: depth=\"deep\" is answered at quick depth."
            };
            (
                format!(
                    "Answer a question with a synthesized reply grounded in live web search, \
                     with citations. Use it when you need an answer rather than a list of \
                     links.{deep_note} Backed by: {backed_by} (tried in that order; later \
                     providers are fallbacks)."
                ),
                json!({
                    "query":text,
                    "depth":{"type":"string","enum":depths,"default":default_depth},
                    "provider":provider
                }),
                &["query"],
            )
        }
        Role::Contents => (
            format!(
                "Fetch the readable contents of specific web pages by URL. Use it after search \
                 to read a page in full. An optional query focuses extraction where the provider \
                 supports it. Backed by: {backed_by} (tried in that order; later providers are \
                 fallbacks)."
            ),
            json!({
                "urls":{"type":"array","items":text,"minItems":1,"maxItems":10},
                "query":text,
                "provider":provider
            }),
            &["urls"],
        ),
    };
    Some(tool(
        role_tool_name(role),
        &description,
        properties,
        required,
    ))
}
