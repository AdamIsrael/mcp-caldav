use chrono::{DateTime, Duration, Utc};
use rrule::{RRuleSet, Tz as RRuleTz};

use crate::error::CalDavError;
use crate::ics::timezone::format_datetime;
use crate::ics::types::EventInstance;

/// Expand a recurring event's RRULE into concrete instances within a date range.
#[allow(clippy::too_many_arguments)] // tracked by mcp-caldav-953
pub fn expand_rrule(
    rrule_str: &str,
    dtstart: DateTime<Utc>,
    dtstart_tz: Option<chrono_tz::Tz>,
    event_duration: Duration,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    summary: &str,
    master_uid: &str,
    location: Option<&str>,
    description: Option<&str>,
    display_tz: Option<chrono_tz::Tz>,
) -> Result<Vec<EventInstance>, CalDavError> {
    // When the original event was anchored to a real timezone (TZID=America/New_York
    // etc.), feed the rrule crate a TZID-qualified DTSTART so iteration happens in
    // local time and DST transitions land correctly. Falling back to a UTC DTSTART
    // is only correct for floating/UTC events.
    let rrule_input = if let Some(tz) = dtstart_tz {
        let local = dtstart.with_timezone(&tz);
        format!(
            "DTSTART;TZID={}:{}\n{}",
            tz.name(),
            local.format("%Y%m%dT%H%M%S"),
            ensure_rrule_prefix(rrule_str)
        )
    } else {
        format!(
            "DTSTART:{}\n{}",
            dtstart.format("%Y%m%dT%H%M%SZ"),
            ensure_rrule_prefix(rrule_str)
        )
    };

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
            None,
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
            None,
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
            None,
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

    #[test]
    fn weekly_event_at_local_time_holds_steady_across_dst() {
        // Weekly Monday meeting at 09:00 America/New_York, started before 2026 DST.
        // US spring-forward 2026 is 2026-03-08 (Sunday), so the Monday before falls
        // in EST (UTC-5) and the Monday after falls in EDT (UTC-4). The meeting
        // should remain at 09:00 NY local on both sides — i.e., 14:00 UTC pre-DST
        // and 13:00 UTC post-DST.
        //
        // dtstart is `DateTime<Utc>` because that's how the parser stores it; the
        // round-trip through dtstart_tz is what restores the original local time.
        let ny: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let dtstart = ny
            .with_ymd_and_hms(2026, 1, 5, 9, 0, 0) // Mon Jan 5, 2026, 09:00 EST
            .unwrap()
            .with_timezone(&Utc);

        let range_start = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 3, 31, 23, 59, 59).unwrap();

        let instances = expand_rrule(
            "FREQ=WEEKLY;BYDAY=MO",
            dtstart,
            Some(ny),
            Duration::hours(1),
            range_start,
            range_end,
            "NY Monday Sync",
            "uid-dst",
            None,
            None,
            Some(ny),
        )
        .unwrap();

        // Expect Mondays of March 2026: 2, 9, 16, 23, 30
        assert_eq!(instances.len(), 5, "expected 5 Mondays in March 2026");

        // Pre-DST Monday (Mar 2): 09:00 EST = 14:00 UTC
        let pre_dst = instances
            .iter()
            .find(|i| i.instance_start.format("%Y-%m-%d").to_string() == "2026-03-02")
            .expect("missing 2026-03-02 instance");
        assert_eq!(
            pre_dst.instance_start,
            Utc.with_ymd_and_hms(2026, 3, 2, 14, 0, 0).unwrap(),
            "pre-DST instance should be 14:00 UTC (09:00 EST)"
        );

        // Post-DST Monday (Mar 9): 09:00 EDT = 13:00 UTC
        let post_dst = instances
            .iter()
            .find(|i| i.instance_start.format("%Y-%m-%d").to_string() == "2026-03-09")
            .expect("missing 2026-03-09 instance");
        assert_eq!(
            post_dst.instance_start,
            Utc.with_ymd_and_hms(2026, 3, 9, 13, 0, 0).unwrap(),
            "post-DST instance should be 13:00 UTC (09:00 EDT)"
        );

        // Display strings must show the same local time on both sides.
        assert!(
            pre_dst.local_start.contains("09:00"),
            "pre-DST local_start should show 09:00, got {:?}",
            pre_dst.local_start
        );
        assert!(
            post_dst.local_start.contains("09:00"),
            "post-DST local_start should show 09:00, got {:?}",
            post_dst.local_start
        );
    }

    #[test]
    fn utc_dtstart_without_tz_keeps_old_behavior() {
        // Floating/UTC events (no TZID): the rrule input falls back to a UTC
        // DTSTART, and instances stay at fixed UTC offsets — i.e., they DO
        // shift one hour relative to wall-clock time across DST. This is the
        // documented behavior for events without a timezone anchor; the test
        // pins it so future refactors don't accidentally regress.
        let dtstart = Utc.with_ymd_and_hms(2026, 1, 5, 14, 0, 0).unwrap();
        let range_start = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 3, 31, 23, 59, 59).unwrap();

        let instances = expand_rrule(
            "FREQ=WEEKLY;BYDAY=MO",
            dtstart,
            None, // no tz → UTC-anchored
            Duration::hours(1),
            range_start,
            range_end,
            "Floating",
            "uid-float",
            None,
            None,
            None,
        )
        .unwrap();

        for inst in &instances {
            assert_eq!(
                inst.instance_start.format("%H:%M").to_string(),
                "14:00",
                "UTC-anchored instances must stay at 14:00 UTC"
            );
        }
    }
}
