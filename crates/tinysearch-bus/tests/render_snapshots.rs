//! Byte-identical snapshots of the canonical renderings. A change here is a
//! change to what models and UIs receive; review it as a contract change.

use tinysearch_bus::render::{render_markdown, render_text};
use tinysearch_bus::{Citation, ExecuteToolResponse, Role, SearchResult, SearchStatus};

fn fixture() -> ExecuteToolResponse {
    ExecuteToolResponse {
        provider: "gemini".into(),
        results: vec![
            SearchResult {
                title: "Tokio select".into(),
                url: "https://tokio.rs/select".into(),
                snippet: Some("Waits on multiple futures".into()),
                published: Some("2026-09-01".into()),
            },
            SearchResult {
                title: "  ".into(),
                url: "https://e.example/2".into(),
                snippet: None,
                published: None,
            },
        ],
        citations: vec![
            Citation {
                url: "https://docs.rs/tokio".into(),
                title: Some("tokio docs".into()),
            },
            Citation {
                url: "https://tokio.rs".into(),
                title: None,
            },
        ],
        answer: Some("It returns the first future to complete.".into()),
        status: SearchStatus::Ok,
        provider_data: None,
        role: Some(Role::Answer),
        fallback_from: vec!["exa".into()],
    }
}

#[test]
fn text_rendering_is_byte_identical() {
    let expected = "Answer for: what does select do (via Gemini, after Exa)\n\
\n\
It returns the first future to complete.\n\
1. Tokio select\n\
\x20  https://tokio.rs/select\n\
\x20  Published: 2026-09-01\n\
\x20  Waits on multiple futures\n\
2. No title\n\
\x20  https://e.example/2\n\
\n\
Sources:\n\
[1] tokio docs — https://docs.rs/tokio\n\
[2] https://tokio.rs";
    assert_eq!(render_text(&fixture(), "what does select do", 5), expected);
}

#[test]
fn markdown_rendering_is_byte_identical() {
    let expected = "# Answer for: what does select do (via Gemini, after Exa)\n\
\n\
It returns the first future to complete.\n\
\n\
## [Tokio select](https://tokio.rs/select)\n\
_Published: 2026-09-01_\n\
\n\
> Waits on multiple futures\n\
\n\
## [Untitled](https://e.example/2)\n\
\n\
### Sources\n\
- [tokio docs](https://docs.rs/tokio)\n\
- [https://tokio.rs](https://tokio.rs)\n";
    assert_eq!(
        render_markdown(&fixture(), "what does select do", 5),
        expected
    );
}
