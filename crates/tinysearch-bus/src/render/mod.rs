//! Canonical text and markdown rendering of an [`ExecuteToolResponse`].
//!
//! Pure and I/O-free. The text form is what a model reads: a heading with a
//! `(via <Provider>)` marker that UIs may parse, then the answer, numbered
//! results and a `Sources:` list. Hosts layer their own structured metadata on
//! top; the wording here is part of the contract.

use std::fmt::Write as _;

use serde_json::Value;

use crate::labels::provider_label;
use crate::{ExecuteToolResponse, Role, SearchStatus};

/// Snippets in rendered output are capped at this many characters.
pub const TEXT_EXCERPT_CHARS: usize = 500;

/// What the call was about: the query, the URLs of a contents call, or the
/// objective, taken from the tool arguments.
#[must_use]
pub fn subject(arguments: &Value) -> String {
    if let Some(query) = arguments.get("query").and_then(Value::as_str) {
        return query.trim().to_string();
    }
    if let Some(urls) = arguments.get("urls").and_then(Value::as_array) {
        return urls
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
    }
    arguments
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The one-line heading, e.g. `Search results for: q (via Exa)`.
#[must_use]
pub fn heading(response: &ExecuteToolResponse, subject: &str) -> String {
    let provider = provider_label(&response.provider);
    let via = if response.fallback_from.is_empty() {
        format!("via {provider}")
    } else {
        let skipped: Vec<_> = response
            .fallback_from
            .iter()
            .map(|p| provider_label(p))
            .collect();
        format!("via {provider}, after {}", skipped.join(", "))
    };
    match (response.role, response.status) {
        (_, SearchStatus::InProgress) => format!("Research still running for: {subject} ({via})"),
        (Some(Role::Answer), _) => format!("Answer for: {subject} ({via})"),
        (Some(Role::Contents), _) => format!("Page contents for: {subject} ({via})"),
        _ if response.results.is_empty() && response.answer.is_none() => {
            format!("No results found for: {subject} ({via})")
        }
        _ => format!("Search results for: {subject} ({via})"),
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// Truncate to `max_chars` characters, trimming trailing whitespace and
/// appending `suffix` only when something was cut.
fn truncate(s: &str, max_chars: usize, suffix: &str) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}{suffix}", s[..idx].trim_end()),
        None => s.to_string(),
    }
}

/// Plain-text rendering, showing at most `max_results` results.
#[must_use]
pub fn render_text(response: &ExecuteToolResponse, subject: &str, max_results: usize) -> String {
    let mut lines = vec![heading(response, subject)];
    if let Some(answer) = non_empty(response.answer.as_deref()) {
        lines.push(String::new());
        lines.push(answer.to_string());
    }
    for (index, item) in response.results.iter().take(max_results).enumerate() {
        let title = if item.title.trim().is_empty() {
            "No title"
        } else {
            item.title.trim()
        };
        lines.push(format!("{}. {}", index + 1, title));
        lines.push(format!("   {}", item.url.trim()));
        if let Some(date) = non_empty(item.published.as_deref()) {
            lines.push(format!("   Published: {date}"));
        }
        if let Some(snippet) = non_empty(item.snippet.as_deref()) {
            lines.push(format!("   {}", truncate(snippet, TEXT_EXCERPT_CHARS, "…")));
        }
    }
    if !response.citations.is_empty() {
        lines.push(String::new());
        lines.push("Sources:".into());
        for (index, citation) in response.citations.iter().enumerate() {
            match non_empty(citation.title.as_deref()) {
                Some(title) => lines.push(format!("[{}] {} — {}", index + 1, title, citation.url)),
                None => lines.push(format!("[{}] {}", index + 1, citation.url)),
            }
        }
    }
    lines.join("\n")
}

/// Markdown rendering, showing at most `max_results` results.
#[must_use]
pub fn render_markdown(
    response: &ExecuteToolResponse,
    subject: &str,
    max_results: usize,
) -> String {
    let mut out = format!("# {}\n", heading(response, subject));
    if let Some(answer) = non_empty(response.answer.as_deref()) {
        let _ = write!(out, "\n{answer}\n");
    }
    for item in response.results.iter().take(max_results) {
        let title = if item.title.trim().is_empty() {
            "Untitled"
        } else {
            item.title.trim()
        };
        let _ = write!(out, "\n## [{title}]({})\n", item.url.trim());
        if let Some(date) = non_empty(item.published.as_deref()) {
            let _ = write!(out, "_Published: {date}_\n\n");
        }
        if let Some(snippet) = non_empty(item.snippet.as_deref()) {
            let _ = writeln!(out, "> {}", truncate(snippet, TEXT_EXCERPT_CHARS, "…"));
        }
    }
    if !response.citations.is_empty() {
        out.push_str("\n### Sources\n");
        for citation in &response.citations {
            let title = non_empty(citation.title.as_deref()).unwrap_or(citation.url.as_str());
            let _ = writeln!(out, "- [{title}]({})", citation.url);
        }
    }
    out
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
