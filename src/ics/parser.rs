use chrono::Utc;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::error::CalDavError;
use crate::ics::timezone::{extract_timezones, format_datetime, parse_datetime, resolve_timezone};
use crate::ics::types::{EventDetail, EventSummary};

fn resolve_dtstart_tz(
    tzid: Option<&str>,
    tz_map: &HashMap<String, chrono_tz::Tz>,
) -> Option<chrono_tz::Tz> {
    let tzid = tzid?;
    tz_map.get(tzid).copied().or_else(|| resolve_timezone(tzid))
}

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

        let dtstart =
            match extract_datetime_from_properties(event.properties(), "DTSTART", &tz_map) {
                Ok(dt) => dt,
                Err(e) => {
                    tracing::warn!(
                        "skipping VEVENT (uid={uid}) at {event_url}: DTSTART parse failed: {e}"
                    );
                    continue;
                }
            };
        let dtend = extract_datetime_from_properties(event.properties(), "DTEND", &tz_map).ok();

        let dtstart_tzid = event
            .properties()
            .get("DTSTART")
            .and_then(|p| p.params().get("TZID").map(|t| t.value().to_string()));
        let dtstart_tz = resolve_dtstart_tz(dtstart_tzid.as_deref(), &tz_map);

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
            dtstart_tz,
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

        let dtstart =
            match extract_datetime_from_properties(event.properties(), "DTSTART", &tz_map) {
                Ok(dt) => dt,
                Err(e) => {
                    tracing::warn!(
                        "skipping VEVENT (uid={uid}) at {event_url}: DTSTART parse failed: {e}"
                    );
                    continue;
                }
            };
        let dtend = extract_datetime_from_properties(event.properties(), "DTEND", &tz_map).ok();

        let dtstart_tzid = event
            .properties()
            .get("DTSTART")
            .and_then(|p| p.params().get("TZID").map(|t| t.value().to_string()));
        let display_tz = resolve_dtstart_tz(dtstart_tzid.as_deref(), &tz_map);

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

        let Some(dtstart_raw) = props.get("DTSTART") else {
            tracing::warn!("skipping fallback VEVENT (uid={uid}) at {event_url}: missing DTSTART");
            continue;
        };

        let tzid = extract_tzid_param(block, "DTSTART");
        let dtstart = match parse_datetime(dtstart_raw, tzid.as_deref(), &tz_map) {
            Ok(dt) => dt,
            Err(e) => {
                tracing::warn!(
                    "skipping fallback VEVENT (uid={uid}) at {event_url}: DTSTART parse failed: {e}"
                );
                continue;
            }
        };
        let dtstart_tz = resolve_dtstart_tz(tzid.as_deref(), &tz_map);

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
            dtstart_tz,
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
    let mut details = Vec::new();

    for vevent in RE_VEVENT.captures_iter(ics) {
        let block = &vevent[1];
        let props = extract_properties(block);

        let uid = props.get("UID").cloned().unwrap_or_default();
        let summary = props
            .get("SUMMARY")
            .cloned()
            .unwrap_or_else(|| "(no title)".to_string());

        let Some(dtstart_raw) = props.get("DTSTART") else {
            tracing::warn!("skipping fallback details VEVENT (uid={uid}) at {event_url}: missing DTSTART");
            continue;
        };

        let tzid = extract_tzid_param(block, "DTSTART");
        let dtstart = match parse_datetime(dtstart_raw, tzid.as_deref(), &tz_map) {
            Ok(dt) => dt,
            Err(e) => {
                tracing::warn!(
                    "skipping fallback details VEVENT (uid={uid}) at {event_url}: DTSTART parse failed: {e}"
                );
                continue;
            }
        };
        let display_tz = resolve_dtstart_tz(tzid.as_deref(), &tz_map);
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
    fn icalendar_path_resolves_dtstart_tz_from_tzid() {
        // Event with explicit TZID — parser must surface the resolved zone so
        // downstream RRULE expansion can iterate in local time across DST.
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc-123\r\n\
SUMMARY:NY Sync\r\n\
DTSTART;TZID=America/New_York:20260105T090000\r\n\
DTEND;TZID=America/New_York:20260105T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
        let events = parse_events(ics, "https://example/cal/abc.ics").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].dtstart_tz.map(|tz| tz.name().to_string()),
            Some("America/New_York".to_string())
        );
    }

    #[test]
    fn icalendar_path_leaves_dtstart_tz_none_for_utc_events() {
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc-123\r\n\
SUMMARY:UTC event\r\n\
DTSTART:20260105T140000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
        let events = parse_events(ics, "https://example/cal/abc.ics").unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].dtstart_tz.is_none());
    }

    #[test]
    fn event_detail_local_time_uses_dtstart_tzid_when_multiple_vtimezones_present() {
        // Two VTIMEZONE blocks; the event's DTSTART references New York. The
        // displayed local time must always render in New York, not Berlin —
        // i.e., independent of HashMap iteration order across runs. This pins
        // the fix for ypp.
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:America/New_York\r\n\
END:VTIMEZONE\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Europe/Berlin\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
UID:abc-123\r\n\
SUMMARY:NY meeting\r\n\
DTSTART;TZID=America/New_York:20260105T090000\r\n\
DTEND;TZID=America/New_York:20260105T100000\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        // Run the parse repeatedly: HashMap iteration order is randomized
        // per-process, but the same process keeps the same seed, so a single
        // run isn't enough to demonstrate determinism. Instead we assert the
        // structural invariant: the rendered timezone is NY, never Berlin.
        let details = parse_event_details(ics, "https://example/x.ics").unwrap();
        assert_eq!(details.len(), 1);
        let local = &details[0].local_start;
        assert!(
            local.contains("America/New_York") || local.contains("EST") || local.contains("EDT"),
            "expected NY in local_start, got {local:?}"
        );
        assert!(
            !local.contains("Europe/Berlin") && !local.contains("Berlin"),
            "expected NOT to render in Berlin, got {local:?}"
        );
    }

    #[test]
    fn icalendar_path_skips_unparseable_vevent_and_keeps_master() {
        // Master VEVENT is well-formed and recurring. The override VEVENT has
        // a DTSTART our parser can't make sense of (unknown TZID with a value
        // shape we don't recognize). The whole resource must NOT be dropped —
        // just the unparseable override.
        let ics = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
SUMMARY:Master series\r\n\
DTSTART;TZID=America/New_York:20260105T090000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
RECURRENCE-ID;TZID=America/New_York:20260420T090000\r\n\
DTSTART:notadate\r\n\
SUMMARY:Override\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

        let events = parse_events(ics, "https://example/cal/abc.ics").unwrap();
        let masters: Vec<_> = events.iter().filter(|e| e.is_recurring).collect();
        assert_eq!(masters.len(), 1, "master must survive override parse failure");
        assert_eq!(masters[0].summary, "Master series");
    }

    #[test]
    fn fallback_path_skips_unparseable_vevent_and_keeps_master() {
        // Force the regex fallback by stripping the VCALENDAR wrapper. Same
        // shape as above: master + bad override; master must survive.
        let ics = "BEGIN:VEVENT\r\n\
UID:abc\r\n\
SUMMARY:Master series\r\n\
DTSTART:20260105T140000Z\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:abc\r\n\
RECURRENCE-ID:20260420T140000Z\r\n\
DTSTART:notadate\r\n\
SUMMARY:Override\r\n\
END:VEVENT\r\n";

        let events = parse_events(ics, "https://example/cal/abc.ics").unwrap();
        let masters: Vec<_> = events.iter().filter(|e| e.is_recurring).collect();
        assert_eq!(masters.len(), 1, "master must survive override parse failure");
        assert_eq!(masters[0].summary, "Master series");
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
