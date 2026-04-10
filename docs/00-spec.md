# Specification: CalDAV MCP Server (`mcp-caldav`) v2.0

## 1. Overview
The `caldav-mcp` server provides an LLM with the ability to interact with calendar data via WebDAV/CalDAV. It acts as a robust middleware that handles the idiosyncrasies of different CalDAV implementations (Nextcloud, Google, iCloud), expanding recurring events and normalizing timezones into a format the LLM can reliably reason about.

### Core Goals
- **Browse**: Discover calendars and events within specific time windows.
- **Search**: Find events via server-side queries with a mandatory client-side fallback.
- **Read**: Retrieve detailed event data, including expanded instances of recurring events.

---

## 2. Technical Stack
- **Language**: Rust (Edition 2021)
- **MCP Framework**: `rmcp`
- **HTTP Client**: `reqwest` (Async)
- **XML Parsing**: `quick-xml` (with `serde` feature) for handling complex namespaces.
- **ICS Parsing**: `icalendar` as primary, with a **custom regex/line-based fallback** for non-compliant servers.
- **Time/Date**: `chrono` and `chrono-tz` (essential for `VTIMEZONE` and offset handling).
- **Recurrence**: `rrule` crate for expanding RRULE strings into discrete timestamps.
- **Caching**: `moka` or a simple `tokio::time` based TTL cache.

---

## 3. Configuration & Authentication
To support multi-account environments and varying security models, configuration is moved to a per-account basis.

### Account Schema
```toml
[[accounts]]
name = "Work"
url = "https://dav.work.com/..."
auth_type = "OAuth2" # Options: Basic, AppPassword, OAuth2
credentials = { token = "...", refresh_token = "..." }

[[accounts]]
name = "Personal"
url = "https://icloud.com/..."
auth_type = "AppPassword"
credentials = { user = "...", pass = "..." }
```

---

## 4. Tool Definitions

### `list_calendars`
- **Logic**: `PROPFIND` on the principal calendar home.
- **Caching**: Results cached for 5 minutes to reduce latency.
- **Output**: List of `{ calendar_name, calendar_url, account_name }`.

### `list_events`
- **Logic**: 
    1. Request events via `REPORT` (calendar-query).
    2. Retrieve ICS bodies for all returned URLs.
    3. **Expand RRULEs**: If an event is recurring, generate instances that fall within the requested `start_date` and `end_date`.
    4. **Normalize Time**: Convert all times to UTC or the user's preferred local timezone.
- **Input**: `calendar_url`, `start_date`, `end_date`.

### `search_events`
- **Logic**:
    1. **Attempt Server-Side**: Use `REPORT` with a search filter.
    2. **Fallback to Client-Side**: If the server returns an error or an empty list (but `list_events` for that period is not empty), fetch all events for a reasonable window (e.g., $\pm 30$ days) and perform a case-insensitive string match on `SUMMARY` and `DESCRIPTION`.
- **Input**: `query`, `calendar_url`.

### `get_event_details`
- **Logic**: 
    1. If `event_uid` is provided: Perform a `REPORT` to find the resource URL associated with that UID.
    2. If `event_url` is provided: Direct `GET` request.
    3. Parse and return full details.

---

## 5. Technical Challenges & Strategies

### 5.1 The "Multi-Status" Problem (HTTP 207)
WebDAV often returns `207 Multi-Status`, meaning the request succeeded but individual resources within the response may have failed.
- **Strategy**: Implement a `CalDavResponse` parser that iterates through every `<response>` element in the XML. Log partial failures but return the successful resources to the LLM rather than failing the entire request.

### 5.2 Recurrence Expansion (RRULE)
Many servers return a single "Master" event with an `RRULE` rather than individual instances.
- **Strategy**: Use the `rrule` crate. When `list_events` is called, the server must:
    - Parse the `RRULE` string.
    - Calculate all occurrences between `start_date` and `end_date`.
    - Create "virtual" event objects for each occurrence to present to the LLM.

### 5.3 Timezone Landmines
`DTSTART` can be UTC (`Z`), Local (no suffix), or Floating.
- **Strategy**: 
    - Explicitly parse `VTIMEZONE` components from the ICS file.
    - Map these to `chrono-tz` identifiers.
    - Return all event times to the LLM in a standardized format: `YYYY-MM-DD HH:mm:ss UTC` and `YYYY-MM-DD HH:mm:ss [Local Timezone]`.

### 5.4 Robust ICS Parsing
- **Strategy**: Implement a "layered" parser.
    - **Layer 1**: Full `icalendar` crate parsing.
    - **Layer 2 (Fallback)**: If Layer 1 fails, use a line-by-line scanner searching for `SUMMARY:`, `DTSTART:`, etc., to extract the most critical data.

---

## 6. Error Handling
A structured `CalDavError` enum will be used to differentiate between retryable and fatal errors.

```rust
enum CalDavError {
    AuthFailure(String),       // 401/403
    ResourceNotFound(String),  // 404
    ServerOverloaded,          // 429/503
    XmlParseError(String),     // Malformed XML
    IcsParseError(String),     // Malformed ICS
    PartialFailure(Vec<String>), // 207 Multi-Status partial errors
}
```

---

## 7. Implementation Roadmap

### Phase 0: The Mock Environment
- Build a `mock_dav` module that simulates a WebDAV server using a local directory of `.ics` files.
- Hardcode specific "edge case" files: one with a complex `RRULE`, one with a weird `VTIMEZONE`, and one with non-compliant ICS formatting.

### Phase 1: Core Client & Parsing
- Implement `WebDavClient` with `reqwest`.
- Implement the layered ICS parser and RRULE expansion logic.
- Implement the `CalDavError` handling and 207 Multi-Status logic.

### Phase 2: MCP Integration (`rmcp`)
- Map the `WebDavClient` methods to `rmcp` tool handlers.
- Implement the `search_events` fallback logic (Server $\rightarrow$ Client).

### Phase 3: Performance & Polish
- Integrate the `moka` cache for calendar discovery.
- Implement OAuth2 token refresh logic for Google/Microsoft accounts.
- Final integration testing against live Nextcloud/iCloud instances.
