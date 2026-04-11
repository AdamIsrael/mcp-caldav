# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`mcp-caldav` is a Rust MCP (Model Context Protocol) server that exposes CalDAV calendars to an LLM. It acts as middleware over raw WebDAV/CalDAV, normalizing the quirks of different servers (Nextcloud, Google, iCloud) — expanding recurring events, parsing `VTIMEZONE`, and presenting a clean tool interface. The full design spec lives in `docs/00-spec.md`; read it before making non-trivial changes.

## Commands

```bash
cargo build              # build
cargo run                # runs the MCP server on stdio (JSON-RPC)
cargo check              # fast type-check
cargo clippy             # lint
cargo test               # run all tests
cargo test <name>        # run a single test by name substring
```

There is no test suite yet — `cargo test` currently runs zero tests. When adding tests, follow standard Rust conventions (`#[cfg(test)] mod tests` or `tests/` directory).

## Runtime configuration

The binary loads a TOML config at `$MCP_CALDAV_CONFIG` or, by default, `$HOME/.config/mcp-caldav/config.toml`. Schema is per-account (`[[accounts]]`) with `name`, `url`, `auth_type` (`Basic` | `AppPassword` | `OAuth2`), and `credentials`. See `src/config.rs` and `docs/00-spec.md` §3 for the exact shape. The server refuses to start with invalid credentials for the chosen `auth_type`.

**Transport:** MCP speaks JSON-RPC over stdio. `stdout` is reserved for protocol frames — **all logging must go to stderr**. `tracing_subscriber` is already configured this way in `main.rs`; do not change it, and never `println!` from library code.

## Architecture

The codebase is a thin pipeline: **MCP tool call → `CalDavServer` → `WebDavClient` (HTTP) → XML parsing → ICS parsing → normalization → text response**. Understanding that flow is enough to find your way around.

- `src/server.rs` — `CalDavServer` holds a `HashMap<account_name, WebDavClient>` plus a `CalendarCache`. The four MCP tools (`list_calendars`, `list_events`, `search_events`, `get_event_details`) are defined here via `#[rmcp::tool_router]` / `#[rmcp::tool_handler]` macros from `rmcp`. Tool argument structs derive `schemars::JsonSchema` so `rmcp` can publish their schemas. Each tool returns `Result<String, CalDavError>` — the output is a human-readable text block for the LLM, **not** structured JSON.
- `find_client_for_url` routes an arbitrary `calendar_url` back to the right account by prefix match, then by host. Relevant when adding tools that take a URL.
- `src/dav/` — transport layer. `client.rs` wraps `reqwest` with `PROPFIND` / `REPORT` / `GET` helpers; `xml.rs` builds request bodies and parses `207 Multi-Status` responses via `quick-xml`. Per spec §5.1, multi-status parsing must never fail the whole request on partial errors — log and continue.
- `src/ics/` — parsing and normalization.
  - `parser.rs` implements a **two-layer parser** (spec §5.4): Layer 1 uses the `icalendar` crate; if it fails or returns nothing, Layer 2 is a regex/line-based fallback. Both layers must be kept working — servers in the wild emit non-compliant ICS, so do not remove the fallback "because the crate handles it."
  - `timezone.rs` extracts `VTIMEZONE` blocks and maps them to `chrono_tz::Tz`. All event times are normalized and presented in both UTC and a "local" timezone (spec §5.3).
  - `rrule.rs` uses the `rrule` crate to expand `RRULE` strings into discrete instances within the requested window. `list_events` passes recurring events through this expansion; non-recurring events pass through unchanged. `extract_rrule_from_ics` in `server.rs` is a line-scan helper because `icalendar` doesn't expose the raw RRULE cleanly.
  - `types.rs` — `EventSummary`, `EventDetail`, `CalendarInfo`. Their `Display` impls produce the strings the LLM sees; update these (not the tool handlers) when changing output format.
- `src/cache.rs` — `moka`-based TTL cache for calendar discovery (5 min per spec §4). Only `list_calendars` is cached; events are always fetched fresh.
- `src/auth.rs` — applies credentials to a `reqwest::RequestBuilder` based on `AuthType`. OAuth2 token refresh is not yet implemented (spec phase 3).
- `src/error.rs` — `CalDavError` enum differentiating auth / not-found / overload / parse errors, mapped from HTTP status codes in `from_http_status`.

## Things to preserve

- **Keep the two-layer ICS parser.** Removing the fallback will silently break non-compliant servers.
- **Don't fail on 207 partial errors.** Log and return successful resources.
- **stdout is sacred** (MCP transport). Logs go to stderr via `tracing`.
- **Search has a mandatory client-side fallback** (spec §4 `search_events`): if server-side `REPORT` fails or returns empty, re-fetch via `calendar-query` and filter in-process. `server.rs::search_events` implements this — preserve the pattern.
