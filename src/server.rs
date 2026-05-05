use std::collections::HashMap;

use chrono::{Duration, NaiveDate, Utc};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::schemars;
use rmcp::tool;
use serde::Deserialize;

use crate::cache::CalendarCache;
use crate::config::AppConfig;
use crate::dav::client::WebDavClient;
use crate::dav::xml;
use crate::error::CalDavError;
use crate::ics::parser;
use crate::ics::rrule::expand_rrule;
use crate::ics::types::EventSummary;

#[derive(Clone)]
pub struct CalDavServer {
    tool_router: ToolRouter<Self>,
    clients: HashMap<String, WebDavClient>,
    cache: CalendarCache,
}

// --- Tool argument types ---

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCalendarsArgs {
    /// Filter by account name (optional, lists all if omitted)
    pub account: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListEventsArgs {
    /// CalDAV URL of the calendar
    pub calendar_url: String,
    /// Start date (YYYY-MM-DD)
    pub start_date: String,
    /// End date (YYYY-MM-DD)
    pub end_date: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchEventsArgs {
    /// Search query string
    pub query: String,
    /// CalDAV URL of the calendar
    pub calendar_url: String,
    /// Start date (YYYY-MM-DD, optional — defaults to -30 days)
    pub start_date: Option<String>,
    /// End date (YYYY-MM-DD, optional — defaults to +30 days)
    pub end_date: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetEventDetailsArgs {
    /// Event UID to look up
    pub event_uid: Option<String>,
    /// Direct URL to the event ICS resource
    pub event_url: Option<String>,
    /// Calendar URL (required when using event_uid)
    pub calendar_url: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DumpRawCalendarDataArgs {
    /// CalDAV URL of the calendar
    pub calendar_url: String,
    /// Start date (YYYY-MM-DD)
    pub start_date: String,
    /// End date (YYYY-MM-DD)
    pub end_date: String,
}

// --- Server implementation ---

impl CalDavServer {
    pub fn new(config: &AppConfig) -> Self {
        let mut clients = HashMap::new();
        for account in &config.accounts {
            let client = WebDavClient::new(account.clone());
            clients.insert(account.name.clone(), client);
        }
        Self {
            tool_router: Self::tool_router(),
            clients,
            cache: CalendarCache::new(),
        }
    }

    fn find_client_for_url(&self, calendar_url: &str) -> Result<&WebDavClient, CalDavError> {
        for client in self.clients.values() {
            if calendar_url.starts_with(client.base_url()) {
                return Ok(client);
            }
        }
        let cal_host = extract_host(calendar_url);
        for client in self.clients.values() {
            if extract_host(client.base_url()) == cal_host {
                return Ok(client);
            }
        }
        Err(CalDavError::Config(format!(
            "no account found for calendar URL: {calendar_url}"
        )))
    }
}

fn extract_host(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or(url)
}

fn parse_date(s: &str) -> Result<NaiveDate, CalDavError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| CalDavError::Config(format!("invalid date '{s}': {e}")))
}

fn to_caldav_timestamp(date: NaiveDate) -> String {
    format!("{}T000000Z", date.format("%Y%m%d"))
}

#[rmcp::tool_router]
impl CalDavServer {
    #[tool(description = "List available CalDAV calendars across all configured accounts")]
    async fn list_calendars(
        &self,
        Parameters(args): Parameters<ListCalendarsArgs>,
    ) -> Result<String, CalDavError> {
        let mut all_calendars = Vec::new();

        let target_clients: Vec<&WebDavClient> = match &args.account {
            Some(name) => {
                let client = self.clients.get(name).ok_or_else(|| {
                    CalDavError::Config(format!("unknown account: {name}"))
                })?;
                vec![client]
            }
            None => self.clients.values().collect(),
        };

        for client in target_clients {
            match self.cache.get_or_discover(client).await {
                Ok(cals) => all_calendars.extend(cals),
                Err(e) => {
                    tracing::warn!("failed to discover calendars for {}: {e}", client.account_name());
                    all_calendars.push(crate::ics::types::CalendarInfo {
                        name: format!("(error: {e})"),
                        url: String::new(),
                        account_name: client.account_name().to_string(),
                        color: None,
                    });
                }
            }
        }

        if all_calendars.is_empty() {
            return Ok("No calendars found.".to_string());
        }

        let output: Vec<String> = all_calendars.iter().map(|c| c.to_string()).collect();
        Ok(output.join("\n"))
    }

    #[tool(description = "List events in a calendar within a date range. Expands recurring events.")]
    async fn list_events(
        &self,
        Parameters(args): Parameters<ListEventsArgs>,
    ) -> Result<String, CalDavError> {
        let client = self.find_client_for_url(&args.calendar_url)?;
        let start = parse_date(&args.start_date)?;
        let end = parse_date(&args.end_date)?;

        let body = xml::calendar_query_body(&to_caldav_timestamp(start), &to_caldav_timestamp(end));
        let resources = client.report(&args.calendar_url, &body).await?;
        tracing::debug!(
            "list_events: range={}..{} resources={}",
            args.start_date,
            args.end_date,
            resources.len()
        );

        let range_start = start.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let range_end = end.and_hms_opt(23, 59, 59).unwrap().and_utc();

        let mut all_events: Vec<EventSummary> = Vec::new();
        let mut instances_output: Vec<String> = Vec::new();

        for resource in &resources {
            let Some(ics) = &resource.calendar_data else {
                tracing::debug!("resource {} has no calendar_data, skipping", resource.href);
                continue;
            };

            let events = match parser::parse_events(ics, &resource.href) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("failed to parse ICS at {}: {e}", resource.href);
                    continue;
                }
            };
            let recurring_count = events.iter().filter(|e| e.is_recurring).count();
            tracing::debug!(
                "resource {} parsed: {} events ({} recurring)",
                resource.href,
                events.len(),
                recurring_count
            );

            let extra_lines = extract_recurrence_modifiers(ics);

            for event in &events {
                if event.is_recurring {
                    if let Some(rrule_str) = extract_rrule_from_ics(ics) {
                        let duration = event
                            .dtend
                            .map(|e| e - event.dtstart)
                            .unwrap_or(Duration::hours(1));
                        // Use the event's own DTSTART timezone for both expansion
                        // and display; falls back to None for UTC/floating/all-day.
                        let display_tz = event.dtstart_tz;

                        match expand_rrule(
                            &rrule_str,
                            event.dtstart,
                            event.dtstart_tz,
                            &extra_lines,
                            duration,
                            range_start,
                            range_end,
                            &event.summary,
                            &event.uid,
                            event.location.as_deref(),
                            None,
                            display_tz,
                        ) {
                            Ok(insts) => {
                                tracing::debug!(
                                    "expanded uid={} rrule={:?} -> {} instances in window",
                                    event.uid,
                                    rrule_str,
                                    insts.len()
                                );
                                for inst in &insts {
                                    instances_output.push(inst.to_string());
                                }
                            }
                            Err(e) => {
                                tracing::warn!("RRULE expansion failed for {}: {e}", event.uid);
                                all_events.push(event.clone());
                            }
                        }
                    } else {
                        tracing::debug!(
                            "uid={} marked recurring but no RRULE found in ICS; treating as one-off",
                            event.uid
                        );
                        all_events.push(event.clone());
                    }
                } else {
                    all_events.push(event.clone());
                }
            }
        }
        tracing::debug!(
            "list_events: total {} master/one-off events, {} expanded instances",
            all_events.len(),
            instances_output.len()
        );

        all_events.sort_by_key(|e| e.dtstart);

        let mut output = Vec::new();
        for event in &all_events {
            output.push(event.to_string());
        }
        output.extend(instances_output);

        if output.is_empty() {
            Ok("No events found in the specified date range.".to_string())
        } else {
            Ok(output.join("\n"))
        }
    }

    #[tool(description = "Search for events by text query. Matches case-insensitively against SUMMARY, DESCRIPTION, and LOCATION.")]
    async fn search_events(
        &self,
        Parameters(args): Parameters<SearchEventsArgs>,
    ) -> Result<String, CalDavError> {
        let client = self.find_client_for_url(&args.calendar_url)?;

        let today = Utc::now().date_naive();
        let start = args
            .start_date
            .as_deref()
            .map(parse_date)
            .transpose()?
            .unwrap_or(today - chrono::Duration::days(30));
        let end = args
            .end_date
            .as_deref()
            .map(parse_date)
            .transpose()?
            .unwrap_or(today + chrono::Duration::days(30));

        // Server-side text-match prop-filters only check SUMMARY across the
        // CalDAV servers we target, so they silently drop hits in DESCRIPTION
        // or LOCATION. Fetch the full date window and filter client-side.
        let body = xml::calendar_query_body(&to_caldav_timestamp(start), &to_caldav_timestamp(end));
        let resources = client.report(&args.calendar_url, &body).await?;

        let mut matches: Vec<EventSummary> = Vec::new();
        for resource in &resources {
            let Some(ics) = &resource.calendar_data else {
                continue;
            };

            let events = match parser::parse_events(ics, &resource.href) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for event in events {
                if event.matches_query(&args.query) {
                    matches.push(event);
                }
            }
        }

        matches.sort_by_key(|e| e.dtstart);

        if matches.is_empty() {
            Ok(format!("No events matching '{}' found.", args.query))
        } else {
            let output: Vec<String> = matches.iter().map(|e| e.to_string()).collect();
            Ok(output.join("\n"))
        }
    }

    #[tool(description = "Get detailed information about a specific event by UID or URL")]
    async fn get_event_details(
        &self,
        Parameters(args): Parameters<GetEventDetailsArgs>,
    ) -> Result<String, CalDavError> {
        let ics = if let Some(url) = &args.event_url {
            let client = self.find_client_for_url(url)?;
            client.get(url).await?
        } else if let Some(uid) = &args.event_uid {
            let calendar_url = args.calendar_url.as_deref().ok_or_else(|| {
                CalDavError::Config("calendar_url is required when using event_uid".into())
            })?;
            let client = self.find_client_for_url(calendar_url)?;

            let body = xml::uid_query_body(uid);
            let resources = client.report(calendar_url, &body).await?;

            let resource = resources
                .into_iter()
                .find(|r| r.calendar_data.is_some())
                .ok_or_else(|| {
                    CalDavError::ResourceNotFound(format!("event with UID '{uid}' not found"))
                })?;

            resource.calendar_data.unwrap()
        } else {
            return Err(CalDavError::Config(
                "either event_uid or event_url must be provided".into(),
            ));
        };

        let url = args
            .event_url
            .as_deref()
            .or(args.calendar_url.as_deref())
            .unwrap_or("unknown");

        let details = parser::parse_event_details(&ics, url)?;

        if details.is_empty() {
            Ok("No event details found.".to_string())
        } else {
            let output: Vec<String> = details.iter().map(|d| d.to_string()).collect();
            Ok(output.join("\n---\n"))
        }
    }

    #[tool(description = "DIAGNOSTIC: fetch and return the unparsed ICS bodies in a date range. Use when list_events seems to be missing events to inspect what the server actually returned.")]
    async fn dump_raw_calendar_data(
        &self,
        Parameters(args): Parameters<DumpRawCalendarDataArgs>,
    ) -> Result<String, CalDavError> {
        let client = self.find_client_for_url(&args.calendar_url)?;
        let start = parse_date(&args.start_date)?;
        let end = parse_date(&args.end_date)?;

        let body = xml::calendar_query_body(&to_caldav_timestamp(start), &to_caldav_timestamp(end));
        let resources = client.report(&args.calendar_url, &body).await?;

        let mut out = String::new();
        out.push_str(&format!(
            "{} resource(s) returned for {}..{}\n\n",
            resources.len(),
            args.start_date,
            args.end_date
        ));
        for (i, r) in resources.iter().enumerate() {
            out.push_str(&format!("=== Resource {} : {} ===\n", i + 1, r.href));
            match &r.calendar_data {
                Some(ics) => {
                    out.push_str(ics);
                    if !ics.ends_with('\n') {
                        out.push('\n');
                    }
                }
                None => out.push_str("(no calendar-data field on this resource)\n"),
            }
            out.push('\n');
        }
        Ok(out)
    }
}

/// Return the RRULE of the master VEVENT, if any.
///
/// "Master" = a VEVENT that carries an RRULE and no RECURRENCE-ID. RRULEs that
/// live inside VTIMEZONE / STANDARD / DAYLIGHT components (used to describe
/// DST transitions of the timezone itself) MUST be skipped — feeding one of
/// those into expand_rrule produces empty windows and the event silently
/// vanishes. Same for RRULEs on RECURRENCE-ID overrides.
fn extract_rrule_from_ics(ics: &str) -> Option<String> {
    let mut in_vevent = false;
    let mut block: Vec<String> = Vec::new();

    for raw in ics.lines() {
        let line = raw.trim_end_matches('\r').to_string();
        if line == "BEGIN:VEVENT" {
            in_vevent = true;
            block.clear();
        } else if line == "END:VEVENT" {
            in_vevent = false;
            let has_rrule = block.iter().any(|l| l.starts_with("RRULE:"));
            let has_recurrence_id = block.iter().any(|l| l.starts_with("RECURRENCE-ID"));
            if has_rrule && !has_recurrence_id {
                for l in &block {
                    if let Some(rest) = l.strip_prefix("RRULE:") {
                        return Some(rest.to_string());
                    }
                }
            }
        } else if in_vevent {
            block.push(line);
        }
    }

    None
}

/// Collect recurrence-modifier lines that augment the master RRULE:
///   - EXDATE/RDATE lines from the master VEVENT (the one with RRULE and no
///     RECURRENCE-ID).
///   - A synthetic EXDATE for each override VEVENT's RECURRENCE-ID, so that
///     master expansion does not emit an instance the override is replacing.
///     Each override is still surfaced separately by the parser as a
///     non-recurring VEVENT, so the user sees the override's data on the
///     overridden date instead of the original.
///
/// The lines are returned verbatim in the format the rrule crate expects.
fn extract_recurrence_modifiers(ics: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_vevent = false;
    let mut block: Vec<String> = Vec::new();

    for raw in ics.lines() {
        let line = raw.trim_end_matches('\r').to_string();
        if line == "BEGIN:VEVENT" {
            in_vevent = true;
            block.clear();
        } else if line == "END:VEVENT" {
            in_vevent = false;
            let has_rrule = block.iter().any(|l| l.starts_with("RRULE:"));
            let has_recurrence_id = block.iter().any(|l| l.starts_with("RECURRENCE-ID"));
            if has_rrule && !has_recurrence_id {
                for l in &block {
                    if l.starts_with("EXDATE") || l.starts_with("RDATE") {
                        lines.push(l.clone());
                    }
                }
            } else if has_recurrence_id {
                for l in &block {
                    if let Some(rest) = l.strip_prefix("RECURRENCE-ID") {
                        lines.push(format!("EXDATE{rest}"));
                    }
                }
            }
        } else if in_vevent {
            block.push(line);
        }
    }

    lines
}

// --- rmcp ServerHandler ---

#[rmcp::tool_handler]
impl rmcp::ServerHandler for CalDavServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder().enable_tools().build(),
        )
        .with_instructions("Browse, search, and read calendar events via CalDAV. Use list_calendars to discover available calendars, then list_events or search_events to find events.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_modifiers_from_master_only_returns_exdate_and_rdate() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
DTSTART:20260101T140000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
EXDATE:20260202T140000Z\r\n\
RDATE:20260503T140000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        let lines = extract_recurrence_modifiers(ics);
        assert_eq!(
            lines,
            vec![
                "EXDATE:20260202T140000Z".to_string(),
                "RDATE:20260503T140000Z".to_string(),
            ]
        );
    }

    #[test]
    fn extract_modifiers_converts_recurrence_id_to_synthetic_exdate() {
        // Master + override: the override's RECURRENCE-ID becomes a synthetic
        // EXDATE so master expansion skips that date. The override itself is
        // still emitted by the parser as a separate non-recurring VEVENT.
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
DTSTART:20260101T140000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
RECURRENCE-ID:20260504T140000Z\r\n\
DTSTART:20260504T160000Z\r\n\
SUMMARY:Moved to 16:00\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        let lines = extract_recurrence_modifiers(ics);
        assert_eq!(lines, vec!["EXDATE:20260504T140000Z".to_string()]);
    }

    #[test]
    fn extract_modifiers_handles_tzid_parameters() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
DTSTART;TZID=America/New_York:20260101T090000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
EXDATE;TZID=America/New_York:20260202T090000\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
RECURRENCE-ID;TZID=America/New_York:20260504T090000\r\n\
DTSTART;TZID=America/New_York:20260504T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        let lines = extract_recurrence_modifiers(ics);
        assert_eq!(
            lines,
            vec![
                "EXDATE;TZID=America/New_York:20260202T090000".to_string(),
                "EXDATE;TZID=America/New_York:20260504T090000".to_string(),
            ]
        );
    }

    #[test]
    fn extract_rrule_skips_vtimezone_standard_and_daylight() {
        // Real-shape ICS from Fastmail: VTIMEZONE STANDARD/DAYLIGHT each carry
        // their own RRULE describing DST transitions. Those must NOT be
        // returned — the consumer wants the VEVENT's RRULE only. This was the
        // root cause of two recurring events silently disappearing from
        // list_events for a user on Fastmail (Toronto).
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:America/Toronto\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19700101T000000\r\n\
RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\r\n\
TZOFFSETFROM:-0400\r\n\
TZOFFSETTO:-0500\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:19700101T000000\r\n\
RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\r\n\
TZOFFSETFROM:-0500\r\n\
TZOFFSETTO:-0400\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
DTSTART;TZID=America/Toronto:20240602T220000\r\n\
RRULE:FREQ=WEEKLY\r\n\
SUMMARY:Ozempic\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        assert_eq!(
            extract_rrule_from_ics(ics).as_deref(),
            Some("FREQ=WEEKLY"),
            "must return the VEVENT's RRULE, not VTIMEZONE STANDARD's"
        );
    }

    #[test]
    fn extract_rrule_returns_master_when_override_is_present() {
        // Master + RECURRENCE-ID override: master's RRULE wins, override is
        // skipped (its RRULE — if any — would be a per-instance modification).
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
DTSTART:20260101T140000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
RECURRENCE-ID:20260504T140000Z\r\n\
DTSTART:20260504T160000Z\r\n\
RRULE:FREQ=DAILY\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        assert_eq!(
            extract_rrule_from_ics(ics).as_deref(),
            Some("FREQ=WEEKLY;BYDAY=MO"),
            "override RRULE must not shadow the master's"
        );
    }

    #[test]
    fn extract_rrule_returns_none_for_non_recurring() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:America/Toronto\r\n\
BEGIN:STANDARD\r\n\
RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
DTSTART:20260503T140000Z\r\n\
SUMMARY:One-off\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        assert!(extract_rrule_from_ics(ics).is_none());
    }

    #[test]
    fn extract_modifiers_returns_empty_when_no_modifiers_present() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
DTSTART:20260101T140000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        let lines = extract_recurrence_modifiers(ics);
        assert!(lines.is_empty());
    }
}
