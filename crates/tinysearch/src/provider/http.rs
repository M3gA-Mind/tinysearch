//! Bounded response reading and HTTP failure classification shared by every
//! provider route.
//!
//! Classification maps an upstream failure onto the stable bus codes in
//! [`tinysearch_bus::errors`]: 402 (and Tavily's 432) or a backend
//! insufficient-credits rejection is an insufficient balance, 429 is a rate
//! limit, 408/5xx/transport failures are an unavailable provider, and other
//! 400/422 rejections are invalid arguments. Error bodies are inspected only
//! for the backend's machine-readable fields and are never echoed.
use super::MAX_BODY_BYTES;
use crate::{Error, Result};
use serde_json::Value;

/// Largest error body read for classification.
const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;

/// Maps a request that never produced a response.
pub(super) fn transport_error(_: reqwest::Error) -> Error {
    Error::ProviderUnavailable("provider transport failed".into())
}

/// Reads a JSON response, classifying non-success statuses.
pub(super) async fn read_json(mut response: reqwest::Response) -> Result<Value> {
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = read_error_body(&mut response).await;
        return Err(classify_status(status, &body));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES)
    {
        return Err(Error::ProviderUnavailable(
            "provider response too large".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::ProviderUnavailable("provider response read failed".into()))?
    {
        if bytes.len().saturating_add(chunk.len()) as u64 > MAX_BODY_BYTES {
            return Err(Error::ProviderUnavailable(
                "provider response too large".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| Error::ProviderUnavailable("provider returned invalid JSON".into()))
}

async fn read_error_body(response: &mut reqwest::Response) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        let room = MAX_ERROR_BODY_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if bytes.len() >= MAX_ERROR_BODY_BYTES {
            break;
        }
    }
    bytes
}

/// Classifies a non-success HTTP status and its (possibly empty) body.
pub(super) fn classify_status(status: u16, body: &[u8]) -> Error {
    let detail = format!("provider returned HTTP {status}");
    if let Some(error) = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| classify_body(&value))
    {
        return error;
    }
    match status {
        401 | 403 => Error::Unauthorized(status),
        402 | 432 => Error::InsufficientBalance,
        429 => Error::RateLimited,
        400 | 422 => Error::RejectedArguments(detail),
        408 | 500..=599 => Error::ProviderUnavailable(detail),
        _ => Error::Provider(detail),
    }
}

/// Classifies a backend `{success: false}` envelope delivered with HTTP 200.
pub(super) fn classify_envelope(value: &Value) -> Error {
    classify_body(value)
        .unwrap_or_else(|| Error::Provider("backend rejected provider request".into()))
}

/// Recognizes the backend's error code field and its insufficient-balance
/// messages. Returns `None` when the body carries no recognizable signal.
fn classify_body(value: &Value) -> Option<Error> {
    let field = |pointer: &str| value.pointer(pointer).and_then(Value::as_str);
    let code = field("/errorCode")
        .or_else(|| field("/error/code"))
        .or_else(|| field("/code"));
    match code {
        Some("USER_INSUFFICIENT_CREDITS" | "SPEND_CAP_EXCEEDED" | "insufficient_balance") => {
            return Some(Error::InsufficientBalance);
        }
        Some("RATE_LIMITED" | "rate_limited") => return Some(Error::RateLimited),
        Some("UPSTREAM_UNAVAILABLE" | "MODEL_UNAVAILABLE") => {
            return Some(Error::ProviderUnavailable(
                "backend upstream unavailable".into(),
            ));
        }
        _ => {}
    }
    let message = field("/error")
        .or_else(|| field("/error/message"))
        .or_else(|| field("/message"))?
        .to_ascii_lowercase();
    [
        "insufficient balance",
        "insufficient budget",
        "insufficient credits",
    ]
    .iter()
    .any(|phrase| message.contains(phrase))
    .then_some(Error::InsufficientBalance)
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
