# mcp-caldav

An [MCP](https://modelcontextprotocol.io/) server that exposes CalDAV calendars to LLM clients (Claude Desktop, Claude Code, and any other MCP-compatible host). It acts as middleware over raw WebDAV/CalDAV, smoothing out the quirks of different servers (Nextcloud, Google, iCloud, Fastmail, Radicale, …) so that an LLM can reliably browse, search, and read calendar events.

## What it does

The server exposes four tools to the LLM:

| Tool | Description |
| --- | --- |
| `list_calendars` | Discover calendars across all configured accounts. Results are cached for 5 minutes. |
| `list_events` | List events in a calendar between two dates. Recurring events are expanded to concrete instances. |
| `search_events` | Full-text search across a calendar. Tries a server-side `REPORT` query first; falls back to client-side filtering if the server returns nothing. |
| `get_event_details` | Fetch the full details of a single event (description, attendees, organizer, RRULE, …) by UID or URL. |

Under the hood it handles the usual CalDAV landmines:

- **Recurrence expansion** — `RRULE` strings are expanded into discrete instances within the requested window using the `rrule` crate.
- **Timezone normalization** — `VTIMEZONE` blocks are parsed and mapped to `chrono-tz` zones. Event times are presented to the LLM in both UTC and the event's local timezone.
- **Layered ICS parsing** — primary parse via the `icalendar` crate, with a regex/line-based fallback for servers that emit non-compliant ICS.
- **`207 Multi-Status` tolerance** — partial failures in a multi-resource response are logged but do not fail the whole request.

## Installation

Requires a recent stable Rust toolchain (edition 2024).

```bash
git clone <this repo>
cd mcp-caldav
cargo build --release
```

The binary lands at `target/release/mcp-caldav`. It speaks MCP over stdio, so it isn't run directly — an MCP host launches it as a subprocess.

## Configuration

On startup the server reads a TOML config file from:

1. `$MCP_CALDAV_CONFIG` if set, otherwise
2. `$HOME/.config/mcp-caldav/config.toml`

The file defines one or more accounts:

```toml
[[accounts]]
name = "Personal"
url = "https://cloud.example.com/remote.php/dav/calendars/alice/"
auth_type = "AppPassword"
credentials = { user = "alice", pass = "xxxx-xxxx-xxxx-xxxx" }

[[accounts]]
name = "Work"
url = "https://caldav.work.com/dav/"
auth_type = "Basic"
credentials = { user = "alice@work.com", pass = "hunter2" }

[[accounts]]
name = "Google"
url = "https://apidata.googleusercontent.com/caldav/v2/alice@gmail.com/user/"
auth_type = "OAuth2"
credentials = { token = "ya29....", refresh_token = "1//0..." }
```

### Fields

- **`name`** — an arbitrary label. Used by `list_calendars` and to tag results.
- **`url`** — the CalDAV base URL for the account. For Nextcloud this is typically `.../remote.php/dav/calendars/<user>/`; for iCloud/Fastmail it's the principal URL returned by their discovery process.
- **`auth_type`** — one of:
  - `Basic` — HTTP Basic auth with `user` + `pass`.
  - `AppPassword` — same wire format as `Basic`, kept as a separate label for clarity when the provider requires an app-specific password (iCloud, Google with 2FA, Fastmail).
  - `OAuth2` — sends `token` as a `Bearer` header. **Note:** automatic token refresh is not yet implemented, so the token must be valid when the server starts.
- **`credentials`** — an inline table. Required keys depend on `auth_type`: `user`+`pass` for `Basic`/`AppPassword`, `token` (and optionally `refresh_token`) for `OAuth2`. The server refuses to start if required fields are missing.

## Using it with an MCP host

### Claude Desktop / Claude Code

Add an entry to the host's MCP server config (e.g. `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "caldav": {
      "command": "/absolute/path/to/target/release/mcp-caldav",
      "env": {
        "MCP_CALDAV_CONFIG": "/home/alice/.config/mcp-caldav/config.toml",
        "RUST_LOG": "mcp_caldav=info"
      }
    }
  }
}
```

Restart the host and the four tools above should appear. A typical LLM session looks like:

1. Call `list_calendars` to discover what's available.
2. Call `list_events` with a `calendar_url` from step 1 and a date window.
3. Call `search_events` or `get_event_details` for follow-up questions.

### Logging

The server logs to **stderr** via `tracing` (stdout is reserved for the MCP JSON-RPC transport). Control verbosity with the standard `RUST_LOG` / `EnvFilter` syntax, e.g. `RUST_LOG=mcp_caldav=debug,rmcp=info`.

## Development

```bash
cargo check        # fast type-check
cargo clippy       # lint
cargo build        # debug build
cargo run          # launch the server on stdio (useful for manual JSON-RPC poking)
cargo test         # run tests
```

The full design spec — including the rationale behind the layered parser, RRULE handling, and the multi-status strategy — lives in [`docs/00-spec.md`](docs/00-spec.md).

## License

See [LICENSE](LICENSE).
