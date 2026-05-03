use chrono::Utc;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::error::CalDavError;
use crate::ics::timezone::{extract_timezones, format_datetime, parse_datetime};
use crate::ics::types::{EventDetail, EventSummary};

/// Parse ICS data and extract event summaries.
/// Layer 1: icalendar crate. Layer 2: regex fallback.
pub fn parse_events(ics: &str, event_url: &str) -> Result<Vec<EventSummary>, CalDavError> {
    match parse_events_icalendar(ics, event_url) {
        Ok(events) if !events.is_empty() => Ok(events),
        Ok(_) => parse_events_fallback(ics, event_url),
        Err(_) => parse_events_fallback(ics, event_url),
    }
}

/// Parse ICS data and extract full event details.
pub fn parse_event_details(ics: &str, event_url: &str) -> Result<Vec<EventDetail>, CalDavError> {
    match parse_details_icalendar(ics, event_url) {
        Ok(details) if !details.is_empty() => Ok(details),
        Ok(_) => parse_details_fallback(ics, event_url),
        Err(_) => parse_details_fallback(ics, event_url),
    }
}

// --- Layer 1: icalendar crate ---

fn parse_events_icalendar(ics: &str, event_url: &str) -> Result<Vec<EventSummary>, CalDavError> {
    use icalendar::{Calendar, CalendarComponent, Component};

    let calendar: Calendar = ics
        .parse()
        .map_err(|e| CalDavError::IcsParseError(format!("icalendar parse error: {e}")))?;

    let tz_map = extract_timezones(ics);
    let mut events = Vec::new();

    for component in &calendar.components {
        let CalendarComponent::Event(event) = component else {
            continue;
        };

        let uid = event
            .property_value("UID")
            .unwrap_or("unknown")
            .to_string();
        let summary = event
            .property_value("SUMMARY")
            .unwrap_or("(no title)")
            .to_string();

        let dtstart = extract_datetime_from_properties(event.properties(), "DTSTART", &tz_map)?;
        let dtend = extract_datetime_from_properties(event.properties(), "DTEND", &tz_map).ok();

        let is_recurring = event.property_value("RRULE").is_some();

        events.push(EventSummary {
            uid,
            summary,
            dtstart,
            dtend,
            location: event.property_value("LOCATION").map(String::from),
            description: event.property_value("DESCRIPTION").map(String::from),
            is_recurring,
            url: event_url.to_string(),
        });
    }

    Ok(events)
}

fn parse_details_icalendar(ics: &str, event_url: &str) -> Result<Vec<EventDetail>, CalDavError> {
    use icalendar::{Calendar, CalendarComponent, Component};

    let calendar: Calendar = ics
        .parse()
        .map_err(|e| CalDavError::IcsParseError(format!("icalendar parse error: {e}")))?;

    let tz_map = extract_timezones(ics);
    let display_tz = tz_map.values().next().copied();
    let mut details = Vec::new();

    for component in &calendar.components {
        let CalendarComponent::Event(event) = component else {
            continue;
        };

        let uid = event
            .property_value("UID")
            .unwrap_or("unknown")
            .to_string();
        let summary = event
            .property_value("SUMMARY")
            .unwrap_or("(no title)")
            .to_string();

        let dtstart = extract_datetime_from_properties(event.properties(), "DTSTART", &tz_map)?;
        let dtend = extract_datetime_from_properties(event.properties(), "DTEND", &tz_map).ok();

        let (_, local_start) = format_datetime(dtstart, display_tz);
        let local_end = dtend.map(|e| format_datetime(e, display_tz).1);

        let rrule = event.property_value("RRULE").map(String::from);

        // Extract attendees from multi_properties
        let attendees: Vec<String> = event
            .multi_properties()
            .get("ATTENDEE")
            .map(|props| props.iter().map(|p| p.value().to_string()).collect())
            .unwrap_or_default();

        details.push(EventDetail {
            uid,
            summary,
            description: event.property_value("DESCRIPTION").map(String::from),
            dtstart,
            dtend,
            location: event.property_value("LOCATION").map(String::from),
            organizer: event.property_value("ORGANIZER").map(String::from),
            attendees,
            rrule,
            is_recurring: event.property_value("RRULE").is_some(),
            url: event_url.to_string(),
            local_start,
            local_end,
        });
    }

    Ok(details)
}

fn extract_datetime_from_properties(
    props: &std::collections::BTreeMap<String, icalendar::Property>,
    prop_name: &str,
    tz_map: &HashMap<String, chrono_tz::Tz>,
) -> Result<chrono::DateTime<Utc>, CalDavError> {
    let prop = props
        .get(prop_name)
        .ok_or_else(|| CalDavError::IcsParseError(format!("missing {prop_name}")))?;

    let value = prop.value();

    // Check for TZID parameter
    let tzid = prop.params().get("TZID").map(|p| p.value());

    parse_datetime(value, tzid, tz_map)
}

// --- Layer 2: Regex fallback ---

static RE_VEVENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)BEGIN:VEVENT\r?\n(.*?)END:VEVENT").unwrap());
static RE_PROP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^([\w-]+)(?:;[^:]*)?:(.*)$").unwrap());

fn parse_events_fallback(ics: &str, event_url: &str) -> Result<Vec<EventSummary>, CalDavError> {
    let tz_map = extract_timezones(ics);
    let mut events = Vec::new();

    for vevent in RE_VEVENT.captures_iter(ics) {
        let block = &vevent[1];
        let props = extract_properties(block);

        let uid = props.get("UID").cloned().unwrap_or_default();
        let summary = props
            .get("SUMMARY")
            .cloned()
            .unwrap_or_else(|| "(no title)".to_string());

        let dtstart_raw = props
            .get("DTSTART")
            .ok_or_else(|| CalDavError::IcsParseError("fallback: missing DTSTART".into()))?;

        let tzid = extract_tzid_param(block, "DTSTART");
        let dtstart = parse_datetime(dtstart_raw, tzid.as_deref(), &tz_map)?;

        let dtend = props.get("DTEND").and_then(|v| {
            let tzid = extract_tzid_param(block, "DTEND");
            parse_datetime(v, tzid.as_deref(), &tz_map).ok()
        });

        let is_recurring = props.contains_key("RRULE");

        events.push(EventSummary {
            uid,
            summary,
            dtstart,
            dtend,
            location: props.get("LOCATION").cloned(),
            description: props.get("DESCRIPTION").cloned(),
            is_recurring,
            url: event_url.to_string(),
        });
    }

    if events.is_empty() {
        Err(CalDavError::IcsParseError("no VEVENT found".into()))
    } else {
        Ok(events)
    }
}

fn parse_details_fallback(ics: &str, event_url: &str) -> Result<Vec<EventDetail>, CalDavError> {
    let tz_map = extract_timezones(ics);
    let display_tz = tz_map.values().next().copied();
    let mut details = Vec::new();

    for vevent in RE_VEVENT.captures_iter(ics) {
        let block = &vevent[1];
        let props = extract_properties(block);

        let uid = props.get("UID").cloned().unwrap_or_default();
        let summary = props
            .get("SUMMARY")
            .cloned()
            .unwrap_or_else(|| "(no title)".to_string());

        let dtstart_raw = props
            .get("DTSTART")
            .ok_or_else(|| CalDavError::IcsParseError("fallback: missing DTSTART".into()))?;

        let tzid = extract_tzid_param(block, "DTSTART");
        let dtstart = parse_datetime(dtstart_raw, tzid.as_deref(), &tz_map)?;
        let dtend = props.get("DTEND").and_then(|v| {
            let tzid = extract_tzid_param(block, "DTEND");
            parse_datetime(v, tzid.as_deref(), &tz_map).ok()
        });

        let (_, local_start) = format_datetime(dtstart, display_tz);
        let local_end = dtend.map(|e| format_datetime(e, display_tz).1);

        details.push(EventDetail {
            uid,
            summary,
            description: props.get("DESCRIPTION").cloned(),
            dtstart,
            dtend,
            location: props.get("LOCATION").cloned(),
            organizer: props.get("ORGANIZER").cloned(),
            attendees: vec![],
            rrule: props.get("RRULE").cloned(),
            is_recurring: props.contains_key("RRULE"),
            url: event_url.to_string(),
            local_start,
            local_end,
        });
    }

    if details.is_empty() {
        Err(CalDavError::IcsParseError("no VEVENT found".into()))
    } else {
        Ok(details)
    }
}

fn extract_properties(block: &str) -> HashMap<String, String> {
    let mut props = HashMap::new();
    for cap in RE_PROP.captures_iter(block) {
        let key = cap[1].to_string();
        let value = cap[2].trim().to_string();
        props.entry(key).or_insert(value);
    }
    props
}

static RE_TZID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(\w+);.*?TZID=([^;:]+)").unwrap());

fn extract_tzid_param(block: &str, prop_name: &str) -> Option<String> {
    for cap in RE_TZID.captures_iter(block) {
        if &cap[1] == prop_name {
            return Some(cap[2].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICS_FULL_FIELDS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc-123\r\n\
SUMMARY:Weekly Sync\r\n\
DESCRIPTION:Discuss roadmap and dragonfruit plans\r\n\
LOCATION:Cafe Aurelius\r\n\
DTSTART:20260503T140000Z\r\n\
DTEND:20260503T150000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn icalendar_path_captures_description_and_location() {
        let events = parse_events(ICS_FULL_FIELDS, "https://example/cal/abc.ics").unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.summary, "Weekly Sync");
        assert_eq!(e.location.as_deref(), Some("Cafe Aurelius"));
        assert_eq!(
            e.description.as_deref(),
            Some("Discuss roadmap and dragonfruit plans")
        );
        // Whole point of fixing 9gu: search by a description-only word resolves.
        assert!(e.matches_query("dragonfruit"));
        assert!(e.matches_query("aurelius"));
    }

    #[test]
    fn fallback_path_captures_description_and_location() {
        // Force the regex fallback by feeding a malformed-but-recoverable ICS
        // (missing BEGIN:VCALENDAR/VERSION/END:VCALENDAR — icalendar parser will fail).
        let malformed = "BEGIN:VEVENT\r\n\
UID:abc-123\r\n\
SUMMARY:Weekly Sync\r\n\
DESCRIPTION:Discuss roadmap and dragonfruit plans\r\n\
LOCATION:Cafe Aurelius\r\n\
DTSTART:20260503T140000Z\r\n\
END:VEVENT\r\n";
        let events = parse_events(malformed, "https://example/cal/abc.ics").unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.location.as_deref(), Some("Cafe Aurelius"));
        assert_eq!(
            e.description.as_deref(),
            Some("Discuss roadmap and dragonfruit plans")
        );
    }
}
