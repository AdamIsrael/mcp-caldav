use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;

use crate::error::CalDavError;

/// Extract VTIMEZONE TZID mappings from an ICS string.
/// Returns a map of TZID -> chrono_tz::Tz.
pub fn extract_timezones(ics: &str) -> HashMap<String, Tz> {
    let mut map = HashMap::new();
    let mut current_tzid: Option<String> = None;
    let mut in_vtimezone = false;

    for line in ics.lines() {
        let line = line.trim_end_matches('\r');
        if line == "BEGIN:VTIMEZONE" {
            in_vtimezone = true;
            current_tzid = None;
        } else if line == "END:VTIMEZONE" {
            in_vtimezone = false;
            current_tzid = None;
        } else if in_vtimezone && line.starts_with("TZID:") {
            let tzid = line.trim_start_matches("TZID:").trim();
            current_tzid = Some(tzid.to_string());
            if let Some(tz) = resolve_timezone(tzid) {
                map.insert(tzid.to_string(), tz);
            }
        }
    }

    let _ = current_tzid; // suppress unused warning
    map
}

/// Resolve a TZID string to a chrono-tz Tz.
/// Handles common aliases and non-standard names.
pub fn resolve_timezone(tzid: &str) -> Option<Tz> {
    // Try direct parse first
    if let Ok(tz) = tzid.parse::<Tz>() {
        return Some(tz);
    }

    // Strip common prefixes that some servers add
    let stripped = tzid
        .trim_start_matches("/mozilla.org/20050126_1/")
        .trim_start_matches("/citadel.org/20190914_1/")
        .trim_start_matches("/softwarestudio.org/Olson_20011030_5/");
    if let Ok(tz) = stripped.parse::<Tz>() {
        return Some(tz);
    }

    // Common Windows timezone aliases
    let alias = match tzid {
        "Eastern Standard Time" | "Eastern Daylight Time" | "US/Eastern" => Some("America/New_York"),
        "Central Standard Time" | "Central Daylight Time" | "US/Central" => Some("America/Chicago"),
        "Mountain Standard Time" | "Mountain Daylight Time" | "US/Mountain" => Some("America/Denver"),
        "Pacific Standard Time" | "Pacific Daylight Time" | "US/Pacific" => Some("America/Los_Angeles"),
        "GMT" | "Greenwich Mean Time" => Some("Etc/GMT"),
        "UTC" => Some("Etc/UTC"),
        "W. Europe Standard Time" => Some("Europe/Berlin"),
        "Romance Standard Time" => Some("Europe/Paris"),
        "GMT Standard Time" => Some("Europe/London"),
        "Tokyo Standard Time" => Some("Asia/Tokyo"),
        "China Standard Time" => Some("Asia/Shanghai"),
        "AUS Eastern Standard Time" => Some("Australia/Sydney"),
        _ => None,
    };

    alias.and_then(|a| a.parse::<Tz>().ok())
}

/// Parse a DTSTART/DTEND value and return a UTC DateTime.
/// Handles formats: 20200101T120000Z (UTC), 20200101T120000 (with TZID), 20200101 (all-day).
pub fn parse_datetime(
    value: &str,
    tzid: Option<&str>,
    tz_map: &HashMap<String, Tz>,
) -> Result<DateTime<Utc>, CalDavError> {
    let value = value.trim();

    // UTC suffix
    if value.ends_with('Z') {
        let v = value.trim_end_matches('Z');
        let naive = parse_naive(v)?;
        return Ok(Utc.from_utc_datetime(&naive));
    }

    // If TZID is provided, use it
    if let Some(tzid) = tzid {
        let resolved = resolve_timezone(tzid);
        if let Some(&tz) = tz_map.get(tzid).or(resolved.as_ref()) {
            let naive = parse_naive(value)?;
            return tz
                .from_local_datetime(&naive)
                .earliest()
                .map(|dt| dt.with_timezone(&Utc))
                .ok_or_else(|| CalDavError::IcsParseError(format!("ambiguous local time: {value} in {tzid}")));
        }
    }

    // Floating time — treat as UTC
    let naive = parse_naive(value)?;
    Ok(Utc.from_utc_datetime(&naive))
}

fn parse_naive(value: &str) -> Result<NaiveDateTime, CalDavError> {
    // Full datetime: 20200101T120000
    if value.len() >= 15 && value.contains('T') {
        NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
            .map_err(|e| CalDavError::IcsParseError(format!("bad datetime '{value}': {e}")))
    }
    // All-day date: 20200101
    else if value.len() == 8 {
        chrono::NaiveDate::parse_from_str(value, "%Y%m%d")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
            .map_err(|e| CalDavError::IcsParseError(format!("bad date '{value}': {e}")))
    } else {
        Err(CalDavError::IcsParseError(format!("unrecognized datetime format: '{value}'")))
    }
}

/// Format a UTC datetime into both UTC and local timezone strings.
pub fn format_datetime(dt: DateTime<Utc>, tz: Option<Tz>) -> (String, String) {
    let utc_str = dt.format("%Y-%m-%d %H:%M UTC").to_string();
    let local_str = match tz {
        Some(tz) => {
            let local = dt.with_timezone(&tz);
            format!("{} {}", local.format("%Y-%m-%d %H:%M"), tz)
        }
        None => utc_str.clone(),
    };
    (utc_str, local_str)
}
