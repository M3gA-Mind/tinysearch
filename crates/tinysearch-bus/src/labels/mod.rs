//! Human-readable provider names.
//!
//! Pure and I/O-free: a table from provider id (the wire form in
//! [`crate::ExecuteToolResponse::provider`]) to the name shown to people.

use std::borrow::Cow;

/// Display name for a provider id (`exa` → `Exa`). Unknown ids are returned
/// unchanged.
#[must_use]
pub fn provider_label(provider: &str) -> Cow<'_, str> {
    Cow::Borrowed(match provider {
        "exa" => "Exa",
        "gemini" => "Gemini",
        "gemini_deep_research" => "Gemini Deep Research",
        "tinyfish" => "TinyFish",
        "parallel" => "Parallel",
        "brave" => "Brave",
        "querit" => "Querit",
        "tavily" => "Tavily",
        "seltz" => "Seltz",
        "searxng" => "SearXNG",
        other => other,
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
