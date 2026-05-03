use chrono::{DateTime, Duration, Utc};
use rrule::{RRuleSet, Tz as RRuleTz};

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

    // Constrain expansion to the requested window. `.after`/`.before` are inclusive
    // bounds when used with `.all`, and the iterator short-circuits past `before`,
    // so the 500-cap counts in-window matches rather than occurrences from DTSTART.
    let after = range_start.with_timezone(&RRuleTz::UTC);
    let before = range_end.with_timezone(&RRuleTz::UTC);
    let result = rruleset.after(after).before(before).all(500);

    let mut instances = Vec::new();
    for occ in result.dates {
        let occ_utc: DateTime<Utc> = occ.to_utc();
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn expands_long_running_daily_into_recent_window() {
        // Daily event since 2018: ~2900 occurrences before 2026, well past the
        // old 500-cap from DTSTART. The window is one day in 2026 — must still
        // produce exactly one instance.
        let dtstart = Utc.with_ymd_and_hms(2018, 1, 1, 9, 0, 0).unwrap();
        let range_start = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 5, 3, 23, 59, 59).unwrap();

        let instances = expand_rrule(
            "FREQ=DAILY",
            dtstart,
            Duration::hours(1),
            range_start,
            range_end,
            "Standup",
            "uid-1",
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(instances.len(), 1, "expected today's instance to be expanded");
        assert_eq!(
            instances[0].instance_start,
            Utc.with_ymd_and_hms(2026, 5, 3, 9, 0, 0).unwrap()
        );
    }

    #[test]
    fn returns_empty_when_window_is_before_dtstart() {
        let dtstart = Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap();
        let range_start = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 5, 31, 23, 59, 59).unwrap();

        let instances = expand_rrule(
            "FREQ=DAILY",
            dtstart,
            Duration::hours(1),
            range_start,
            range_end,
            "Future",
            "uid-2",
            None,
            None,
            None,
        )
        .unwrap();

        assert!(instances.is_empty());
    }

    #[test]
    fn weekly_on_today_is_included() {
        // Weekly Sunday event since 2020. 2026-05-03 is a Sunday.
        let dtstart = Utc.with_ymd_and_hms(2020, 1, 5, 14, 0, 0).unwrap(); // Sun
        let range_start = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 5, 3, 23, 59, 59).unwrap();

        let instances = expand_rrule(
            "FREQ=WEEKLY;BYDAY=SU",
            dtstart,
            Duration::hours(1),
            range_start,
            range_end,
            "Sunday Sync",
            "uid-3",
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(instances.len(), 1);
    }
}
