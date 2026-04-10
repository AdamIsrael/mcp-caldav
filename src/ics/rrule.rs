use chrono::{DateTime, Duration, Utc};
use rrule::RRuleSet;

use crate::error::CalDavError;
use crate::ics::timezone::format_datetime;
use crate::ics::types::EventInstance;

/// Expand a recurring event's RRULE into concrete instances within a date range.
pub fn expand_rrule(
    rrule_str: &str,
    dtstart: DateTime<Utc>,
    event_duration: Duration,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    summary: &str,
    master_uid: &str,
    location: Option<&str>,
    description: Option<&str>,
    display_tz: Option<chrono_tz::Tz>,
) -> Result<Vec<EventInstance>, CalDavError> {
    // Build the full RRULE string with DTSTART for the rrule crate
    let rrule_input = format!(
        "DTSTART:{}\n{}",
        dtstart.format("%Y%m%dT%H%M%SZ"),
        ensure_rrule_prefix(rrule_str)
    );

    let rruleset: RRuleSet = rrule_input
        .parse()
        .map_err(|e| CalDavError::IcsParseError(format!("RRULE parse error: {e}")))?;

    // Generate occurrences with a reasonable limit
    let result = rruleset.all(500);

    let mut instances = Vec::new();
    for occ in result.dates {
        let occ_utc: DateTime<Utc> = occ.to_utc();
        if occ_utc < range_start {
            continue;
        }
        if occ_utc > range_end {
            break;
        }

        let instance_end = occ_utc + event_duration;
        let (_, local_start) = format_datetime(occ_utc, display_tz);
        let (_, local_end) = format_datetime(instance_end, display_tz);

        instances.push(EventInstance {
            master_uid: master_uid.to_string(),
            summary: summary.to_string(),
            instance_start: occ_utc,
            instance_end,
            local_start,
            local_end,
            location: location.map(String::from),
            description: description.map(String::from),
        });
    }

    Ok(instances)
}

fn ensure_rrule_prefix(s: &str) -> String {
    if s.starts_with("RRULE:") {
        s.to_string()
    } else {
        format!("RRULE:{s}")
    }
}
