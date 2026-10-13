//! Stable error codes carried in `TinySearch` bus error messages.
//!
//! A failed `ExecuteTool` call reaches the host as a `TinyBus` method error
//! whose message starts with `tinysearch.<code>: ` when the module can
//! classify the failure. Hosts classify with [`code_of`] instead of matching
//! prose. Messages without a code are unclassified failures.

/// Prefix that introduces a code in a bus error message.
pub const PREFIX: &str = "tinysearch.";

/// The account paying for the call (the managed backend balance or the
/// provider's own plan) cannot cover it. Role tools fall back past it.
pub const INSUFFICIENT_BALANCE: &str = "insufficient_balance";
/// The provider or backend rate limit was reached. Role tools fall back past it.
pub const RATE_LIMITED: &str = "rate_limited";
/// The provider is unreachable, timed out, or failed on its side (HTTP 5xx).
/// Role tools fall back past it.
pub const UNAVAILABLE: &str = "provider_unavailable";
/// The request arguments were rejected. Role tools do not fall back past it,
/// because another provider would reject the same request.
pub const INVALID_ARGUMENTS: &str = "invalid_arguments";

/// The managed backend rejected the host's `TinyHumans` credential (HTTP 401):
/// the session is no longer valid, or the API key was revoked. The host
/// decides whether that means signing in again. Role tools fall back past it,
/// since a provider keyed by the user's own credential can still answer.
pub const BACKEND_UNAUTHORIZED: &str = "backend_unauthorized";
/// A provider rejected the user's own API key (HTTP 401/403). The message
/// names the provider. Never used for a provider that was sent no key, nor
/// for a backend 403. Role tools fall back past it.
pub const PROVIDER_UNAUTHORIZED: &str = "provider_unauthorized";

/// Every code, for exhaustive host-side matching.
pub const ALL: &[&str] = &[
    INSUFFICIENT_BALANCE,
    RATE_LIMITED,
    UNAVAILABLE,
    INVALID_ARGUMENTS,
    BACKEND_UNAUTHORIZED,
    PROVIDER_UNAUTHORIZED,
];

/// Returns whether a role tool tries the next provider after `code`.
#[must_use]
pub fn is_fallback_eligible(code: &str) -> bool {
    matches!(
        code,
        INSUFFICIENT_BALANCE
            | RATE_LIMITED
            | UNAVAILABLE
            | BACKEND_UNAUTHORIZED
            | PROVIDER_UNAUTHORIZED
    )
}

/// Returns whether `code` reports a rejected credential, which a host must
/// surface even when a later fallback failed for another reason.
#[must_use]
pub fn is_unauthorized(code: &str) -> bool {
    matches!(code, BACKEND_UNAUTHORIZED | PROVIDER_UNAUTHORIZED)
}

/// What a failure means to a host, independent of its exact code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Every provider failed or is unreachable; nothing more can answer now.
    Exhausted,
    /// The account behind the call cannot pay for it.
    InsufficientBalance,
    /// A rate limit was reached; it clears on its own.
    RateLimited,
    /// A credential was rejected (backend or provider).
    Unauthorized,
    /// The request itself was rejected; retrying it unchanged cannot work.
    InvalidArguments,
    /// No recognised code in the message.
    Unclassified,
}

impl Category {
    /// Maps a code from [`ALL`] to its category; unknown codes are unclassified.
    #[must_use]
    pub fn of_code(code: &str) -> Self {
        match code {
            UNAVAILABLE => Self::Exhausted,
            INSUFFICIENT_BALANCE => Self::InsufficientBalance,
            RATE_LIMITED => Self::RateLimited,
            BACKEND_UNAUTHORIZED | PROVIDER_UNAUTHORIZED => Self::Unauthorized,
            INVALID_ARGUMENTS => Self::InvalidArguments,
            _ => Self::Unclassified,
        }
    }
}

/// Classifies a bus error message into a [`Category`] via [`code_of`].
///
/// # Examples
///
/// ```
/// # use tinysearch_bus::errors::{self, Category};
/// assert_eq!(
///     errors::classify("tinysearch.provider_unavailable: all failed"),
///     Category::Exhausted
/// );
/// assert_eq!(errors::classify("boom"), Category::Unclassified);
/// ```
#[must_use]
pub fn classify(message: &str) -> Category {
    code_of(message).map_or(Category::Unclassified, Category::of_code)
}

/// Returns whether `message` means no provider can answer for the rest of the
/// session. Only the "every provider is unavailable" verdict qualifies: a rate
/// limit clears on its own, and balance or argument errors say what to change.
#[must_use]
pub fn exhausts_providers(message: &str) -> bool {
    classify(message) == Category::Exhausted
}

/// Formats `message` with the `tinysearch.<code>: ` prefix.
#[must_use]
pub fn with_code(code: &str, message: &str) -> String {
    format!("{PREFIX}{code}: {message}")
}

/// Extracts the code from a bus error message.
///
/// The code may appear after a transport-added prefix, so the first
/// `tinysearch.<code>: ` occurrence anywhere in the message counts.
///
/// # Examples
///
/// ```
/// # use tinysearch_bus::errors;
/// assert_eq!(
///     errors::code_of("tinysearch.rate_limited: provider rate limit reached"),
///     Some(errors::RATE_LIMITED)
/// );
/// assert_eq!(errors::code_of("search is disabled"), None);
/// ```
#[must_use]
pub fn code_of(message: &str) -> Option<&'static str> {
    message.match_indices(PREFIX).find_map(|(index, _)| {
        let rest = &message[index + PREFIX.len()..];
        ALL.iter().copied().find(|code| {
            rest.strip_prefix(code)
                .is_some_and(|tail| tail.starts_with(": "))
        })
    })
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
