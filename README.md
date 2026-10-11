# TinySearch

TinySearch is a loadable TinyBus module for web search. `tinysearch-bus` is the
transport-free wire contract; `tinysearch` is the module and provider dispatch
layer. Hosts pass credentials in private module initialization configuration,
which TinyBus can refresh through reinitialization.

The bus serves `ai.tinyhumans.tinysearch.Search` at
`/ai/tinyhumans/tinysearch/Search`. `ListTools` returns currently available
model-facing declarations; `ExecuteTool` invokes a declared tool and returns
normalized results, citations, an optional answer, status, and optional provider
data. The contract version is 2.0.

## Presentation

The default presentation, `roles`, exposes one generic tool per capability
role that has at least one usable provider:

| Role | Tool | Arguments | Providers (default order) |
| --- | --- | --- | --- |
| `search` | `web_search_tool` | `query`, `max_results?` (1-20), `provider?` | exa, brave, tavily, parallel, querit, seltz, searxng, tinyfish, keenable, gemini |
| `answer` | `web_answer_tool` | `query`, `depth?` (`quick` or `deep`), `provider?` | gemini, gemini_deep_research, exa, parallel |
| `contents` | `web_contents_tool` | `urls` (1-10), `query?`, `provider?` | exa, tavily, parallel, tinyfish, keenable |

`presentation.roles` sets an ordered provider list per role; an absent or empty
list uses the default order above. The first usable provider answers. When it
fails with `insufficient_balance`, `rate_limited`, or `provider_unavailable`,
the next one is tried; `invalid_arguments` and unclassified failures stop the
call. The response carries `role` and `fallback_from`, the providers that
failed before the one that answered. An explicit `provider` argument pins the
call to that provider with no fallback; its schema enum lists the usable
providers. `depth: "deep"` prefers Gemini Deep Research, which serves only deep
answers, and falls back to the grounded answer providers; `quick` never uses it.
Parallel serves only quick answers, so a `deep` call skips it, and pinning
`provider: "parallel"` with `depth: "deep"` is rejected as invalid arguments.
When Deep Research is not usable, `deep` is still declared and is answered at
quick depth by the quick providers (Parallel included), so callers that ask for
`deep` are never rejected for it.

`all_tools` exposes every available provider tool, `one_provider` exposes one
provider's tools, and `router` exposes one `search` tool that runs the chosen
provider's first tool with Search role normalization, defaulting to the search
role's first usable provider.

## Errors

Failed `ExecuteTool` calls return a TinyBus method error. When the module can
classify the failure, the message starts with `tinysearch.<code>: `, and
`tinysearch_bus::errors::code_of` extracts the code:

| Code | Cause |
| --- | --- |
| `insufficient_balance` | HTTP 402 (or Tavily 432), or a backend insufficient-credits rejection |
| `rate_limited` | HTTP 429 |
| `provider_unavailable` | HTTP 408 or 5xx, transport failure, timeout, or an unreadable response |
| `invalid_arguments` | Arguments rejected by the schema or by the provider (HTTP 400/422) |

A direct provider requires its own credential unless it is explicitly keyless.
A backend route requires a backend credential. Search can be disabled globally.
Provider configuration includes a route, optional base URL, credential,
`max_results`, `timeout_secs`, and a SearXNG `default_language`.
Debug output redacts credentials.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
```

The module crate is `crates/tinysearch`; its compiled `cdylib` can be loaded by
TinyBus. The contract crate is `crates/tinysearch-bus` and has no transport or
HTTP dependencies. `vendor/tinybus` is a pinned git submodule.

## Built-in providers

A provider appears only when the host configures it explicitly and enables it.
Exa and Gemini (`BACKEND_PROVIDERS`) can use `route: "backend"`,
which requires a backend credential; a backend credential alone enables
nothing. `backend.auth_mode` is `session` (Authorization bearer) or `api_key`
(`x-api-key`); only backend requests receive `x-sdk-name`. Backend responses
are unwrapped from the `{success, data}` envelope.

Exa offers `exa_search`, `exa_find_similar`, `exa_get_contents`, and
`exa_answer`. Directly, they call Exa's `/search`, `/findSimilar`, `/contents`,
and `/answer` with `x-api-key`. On the backend route they call
`/agent-integrations/exa/{search,findSimilar,contents,answer}`. The backend's
search takes only `{objective, searchQueries}`, so the backend `exa_search`
schema accepts only `query`, sent as both; the other routes forward Exa's own
request bodies.

Gemini's `gemini_agentic_search` calls `generateContent` with Google Search
grounding: directly with `x-goog-api-key`, or through
`/agent-integrations/gemini/models/{model}/generate-content`, whose schema
limits `model` to the backend's allowlist (default `gemini-3.8-flash`). The
answer joins the candidate's text parts, and citations list the grounding
chunks that `groundingSupports` reference first, then the rest.
For the Search role, the same grounded call returns deduplicated HTTP(S)
result links in grounding-support order, with the supporting text as snippets
when available. Unreferenced web chunks follow in their original order. Results
and matching citations honor `max_results` (or `limit`), then the provider's
configured count, up to 20. Search returns `empty` when grounding contains no
usable web links, even if Gemini generated answer text. Answer and the raw
`gemini_agentic_search` tool retain their synthesized answer and citations.
`gemini_deep_research` remains Answer-only and calls the direct asynchronous
Interactions API; it can be resumed with `interaction_id` when the bounded poll returns `in_progress`.
Direct Google calls never receive backend attribution or credentials.

TinyFish is bring-your-own-key only (the managed backend does not proxy it).
Every TinyFish call carries the user's key as `X-API-Key`: `tinyfish_search`
is `GET https://api.search.tinyfish.ai` (`query`, `location?`, `language?`,
`page?`), `tinyfish_fetch` is `POST https://api.fetch.tinyfish.ai` (`urls`
1-10, `format?`), and `tinyfish_agent_run` is
`POST https://agent.tinyfish.ai/v1/automation/run` (`url`, `goal`, optional
browser/proxy/vault fields), with its own 300 s timeout because a browser run
takes minutes; a `FAILED` run is a provider error. A `base_url` override
replaces the host for all three.

Parallel is bring-your-own-key only: there is no managed Parallel, so it is not
in `BACKEND_PROVIDERS`, and a `route: "backend"` entry leaves it unavailable
even with a backend credential. With `route: "direct"`, its own credential and
an optional `base_url`, it offers `parallel_search`, `parallel_extract`,
`parallel_chat`, `parallel_research`, `parallel_enrich`, and `parallel_dataset`.
Requests send `x-api-key` to Parallel's `/v1/search`, `/v1/extract`,
`/v1beta/chat/completions`, `/v1/tasks/runs`, and `/v1beta/findall/runs`.
Research and enrichment create Task runs and dataset creates a FindAll run;
they return `in_progress` with `run_id` or `findall_id` in `provider_data`.
`parallel_research_status`, `parallel_enrich_status`, and
`parallel_dataset_status` take that ID, check the run, and fetch completed
results. Every call has a bounded timeout. The search schema lists Parallel's
current modes (`turbo`, `fast`, `basic`, `advanced`). In roles mode Parallel
serves search (`query` becomes the `objective` and the single entry of
`search_queries`; `max_results` becomes `num_results`), quick answers
(`parallel_chat` with the `speed` model and the query as the user message),
and contents (`urls`, with `query` as the `objective` and `full_content` on).

Brave web, news, image, and video search, Querit search, and Tavily search and
extract use `route: "direct"` with a provider credential. Their `base_url` can
target a controlled endpoint for testing. These providers do not have managed
backend routes; unsupported routes and tool arguments are rejected.

Seltz (`seltz_search`) also requires a direct credential. It posts to
`https://api.seltz.ai/v1/search` with `x-api-key`, supports domain and date
filters plus news scope, and returns up to 20 results. SearXNG
(`searxng_search`) is keyless and appears only when the host explicitly enables
it with a `base_url` and a direct route. It requests `/search?format=json`,
maps `web` to the `general` category, uses the configured default language,
and returns up to 50 results with their source names in `provider_data.sources`.

Keenable (`keenable_search`, `keenable_fetch`) needs no credential: once the
host enables it with a direct route, it calls Keenable's keyless
`/v1/search/public` and `/v1/fetch/public`, which are rate limited per IP. A
configured credential switches both tools to `/v1/search` and `/v1/fetch`,
sent as `X-API-Key`, for higher limits. Every request names the caller with
`X-Keenable-Title: tinysearch`, which the keyless endpoints require; it
carries no user or host identifier. Search posts the query with optional
`site`, `published_after` and `published_before` filters, returns up to 20
results with publish dates, and asks for 1,200-character snippets.
`keenable_fetch` reads each URL (1-10) as markdown from Keenable's index and
fetches a page live from the source when it is not indexed (a 404). Pages that
fail are counted in `provider_data.failed_count`; when every page fails, the
first failure's code is returned so the contents role can fall back.

Every response bounds results, citations, snippets, answers, and retained
provider metadata. Upstream error bodies are not returned or logged. Provider
base URL overrides are intended for local testing and controlled deployments.
