use super::*;

#[test]
fn known_ids_map_to_display_names() {
    for (id, label) in [
        ("exa", "Exa"),
        ("gemini", "Gemini"),
        ("gemini_deep_research", "Gemini Deep Research"),
        ("tinyfish", "TinyFish"),
        ("parallel", "Parallel"),
        ("brave", "Brave"),
        ("querit", "Querit"),
        ("tavily", "Tavily"),
        ("seltz", "Seltz"),
        ("searxng", "SearXNG"),
        ("keenable", "Keenable"),
    ] {
        assert_eq!(provider_label(id), label);
    }
}

#[test]
fn unknown_ids_pass_through() {
    assert_eq!(provider_label("custom"), "custom");
}
