//! Error code wire tests.
use super::*;

#[test]
fn codes_are_stable_wire_strings() {
    assert_eq!(INSUFFICIENT_BALANCE, "insufficient_balance");
    assert_eq!(RATE_LIMITED, "rate_limited");
    assert_eq!(UNAVAILABLE, "provider_unavailable");
    assert_eq!(INVALID_ARGUMENTS, "invalid_arguments");
    assert_eq!(BACKEND_UNAUTHORIZED, "backend_unauthorized");
    assert_eq!(PROVIDER_UNAUTHORIZED, "provider_unauthorized");
}

#[test]
fn prefixed_messages_round_trip_through_code_of() {
    for code in ALL {
        let message = with_code(code, "details");
        assert!(message.starts_with(&format!("tinysearch.{code}: ")));
        assert_eq!(code_of(&message), Some(*code));
        assert_eq!(code_of(&format!("method failed: {message}")), Some(*code));
    }
    assert_eq!(code_of("tinysearch.unknown: nope"), None);
    assert_eq!(code_of("tinysearch.rate_limitedx: nope"), None);
    assert_eq!(code_of("plain failure"), None);
}

#[test]
fn only_provider_side_conditions_fall_back() {
    assert!(is_fallback_eligible(INSUFFICIENT_BALANCE));
    assert!(is_fallback_eligible(RATE_LIMITED));
    assert!(is_fallback_eligible(UNAVAILABLE));
    assert!(!is_fallback_eligible(INVALID_ARGUMENTS));
}

#[test]
fn credential_codes_are_classified_and_fall_back() {
    for code in [BACKEND_UNAUTHORIZED, PROVIDER_UNAUTHORIZED] {
        assert!(ALL.contains(&code));
        assert!(is_fallback_eligible(code));
        assert!(is_unauthorized(code));
        assert_eq!(code_of(&with_code(code, "detail")), Some(code));
    }
    assert!(!is_unauthorized(UNAVAILABLE));
}

#[test]
fn classify_maps_every_code_to_a_category() {
    let expected = [
        (INSUFFICIENT_BALANCE, Category::InsufficientBalance),
        (RATE_LIMITED, Category::RateLimited),
        (UNAVAILABLE, Category::Exhausted),
        (INVALID_ARGUMENTS, Category::InvalidArguments),
        (BACKEND_UNAUTHORIZED, Category::Unauthorized),
        (PROVIDER_UNAUTHORIZED, Category::Unauthorized),
    ];
    assert_eq!(expected.len(), ALL.len());
    for (code, category) in expected {
        assert_eq!(classify(&with_code(code, "x")), category, "{code}");
    }
    assert_eq!(classify("plain failure"), Category::Unclassified);
    assert_eq!(classify("tinysearch.nope: x"), Category::Unclassified);
}

#[test]
fn only_the_unavailable_verdict_exhausts_providers() {
    assert!(exhausts_providers(&with_code(UNAVAILABLE, "all failed")));
    for code in ALL.iter().filter(|c| **c != UNAVAILABLE) {
        assert!(!exhausts_providers(&with_code(code, "x")), "{code}");
    }
    assert!(!exhausts_providers("unclassified"));
}
