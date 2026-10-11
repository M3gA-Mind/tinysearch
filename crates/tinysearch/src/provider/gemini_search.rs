//! Gemini grounding normalized as bounded ranked web results for Search.
use super::{MAX_GROUNDING_CHUNKS, MAX_RESULTS};
use crate::{Citation, ExecuteToolResponse, SearchResult, SearchStatus};
use serde_json::Value;
use std::collections::HashSet;

pub(super) fn normalize(value: &Value) -> ExecuteToolResponse {
    let mut response = super::normalize("gemini", "gemini_agentic_search", value);
    response.answer = None;
    response.results.clear();
    response.citations.clear();
    add_results(&mut response, value);
    response.status = if response.results.is_empty() {
        SearchStatus::Empty
    } else {
        SearchStatus::Ok
    };
    response
}

fn add_results(response: &mut ExecuteToolResponse, value: &Value) {
    let Some(metadata) = value.pointer("/candidates/0/groundingMetadata") else {
        return;
    };
    let Some(chunks) = metadata.get("groundingChunks").and_then(Value::as_array) else {
        return;
    };
    let chunk_count = chunks.len().min(MAX_GROUNDING_CHUNKS);
    let referenced = metadata
        .get("groundingSupports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_GROUNDING_CHUNKS)
        .flat_map(|support| {
            let snippet = support
                .pointer("/segment/text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty());
            support
                .get("groundingChunkIndices")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(move |index| (index, snippet))
        })
        .take(MAX_GROUNDING_CHUNKS)
        .filter_map(|(index, snippet)| Some((usize::try_from(index.as_u64()?).ok()?, snippet)));
    let mut seen = HashSet::new();
    for (index, snippet) in referenced.chain((0..chunk_count).map(|index| (index, None))) {
        if response.results.len() >= MAX_RESULTS {
            break;
        }
        let Some(web) = chunks
            .get(index)
            .filter(|_| index < chunk_count)
            .and_then(|chunk| chunk.get("web"))
        else {
            continue;
        };
        let Some(url) = web
            .get("uri")
            .and_then(Value::as_str)
            .filter(|url| usable_url(url))
        else {
            continue;
        };
        if !seen.insert(url) {
            continue;
        }
        let title = web.get("title").and_then(Value::as_str).unwrap_or("");
        let title: String = title.chars().take(300).collect();
        response.results.push(SearchResult {
            title: title.clone(),
            url: url.into(),
            snippet: snippet.map(|text| text.chars().take(1200).collect()),
            published: None,
        });
        response.citations.push(Citation {
            url: url.into(),
            title: Some(title),
        });
    }
}

fn usable_url(url: &str) -> bool {
    url.len() <= 2048
        && !url.chars().any(char::is_whitespace)
        && reqwest::Url::parse(url).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

#[cfg(test)]
#[path = "gemini_search_tests.rs"]
mod test;
