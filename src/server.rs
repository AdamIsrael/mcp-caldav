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
use crate::ics::timezone::extract_timezones;
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

        let range_start = start.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let range_end = end.and_hms_opt(23, 59, 59).unwrap().and_utc();

        let mut all_events: Vec<EventSummary> = Vec::new();
        let mut instances_output: Vec<String> = Vec::new();

        for resource in &resources {
            let Some(ics) = &resource.calendar_data else {
                continue;
            };

            let events = match parser::parse_events(ics, &resource.href) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("failed to parse ICS at {}: {e}", resource.href);
                    continue;
                }
            };

            for event in &events {
                if event.is_recurring {
                    if let Some(rrule_str) = extract_rrule_from_ics(ics) {
                        let duration = event
                            .dtend
                            .map(|e| e - event.dtstart)
                            .unwrap_or(Duration::hours(1));
                        let tz_map = extract_timezones(ics);
                        let display_tz = tz_map.values().next().copied();

                        match expand_rrule(
                            &rrule_str,
                            event.dtstart,
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
                        all_events.push(event.clone());
                    }
                } else {
                    all_events.push(event.clone());
                }
            }
        }

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
}

fn extract_rrule_from_ics(ics: &str) -> Option<String> {
    for line in ics.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("RRULE:") {
            return Some(line.trim_start_matches("RRULE:").to_string());
        }
    }
    None
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
