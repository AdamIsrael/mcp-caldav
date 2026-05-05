use chrono::{DateTime, Duration, Utc};
use rrule::{RRuleSet, Tz as RRuleTz};

use crate::error::CalDavError;
use crate::ics::timezone::format_datetime;
use crate::ics::types::EventInstance;

/// Expand a recurring event's RRULE into concrete instances within a date range.
///
/// `extra_lines` carries EXDATE/RDATE strings (and synthetic EXDATEs derived from
/// override VEVENTs' RECURRENCE-ID values) verbatim in the format the rrule crate
/// accepts — see RFC 5545 §3.8.5.
#[allow(clippy::too_many_arguments)] // tracked by mcp-caldav-953
pub fn expand_rrule(
    rrule_str: &str,
    dtstart: DateTime<Utc>,
    dtstart_tz: Option<chrono_tz::Tz>,
    extra_lines: &[String],
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
    let dtstart_line = if let Some(tz) = dtstart_tz {
        let local = dtstart.with_timezone(&tz);
        format!(
            "DTSTART;TZID={}:{}",
            tz.name(),
            local.format("%Y%m%dT%H%M%S")
        )
    } else {
        format!("DTSTART:{}", dtstart.format("%Y%m%dT%H%M%SZ"))
    };

    let mut rrule_input = String::with_capacity(
        dtstart_line.len() + rrule_str.len() + 16 + extra_lines.iter().map(|l| l.len() + 1).sum::<usize>(),
    );
    rrule_input.push_str(&dtstart_line);
    rrule_input.push('\n');
    rrule_input.push_str(&ensure_rrule_prefix(rrule_str));
    for l in extra_lines {
        rrule_input.push('\n');
        rrule_input.push_str(l);
    }

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
            &[],
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
            &[],
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
            &[],
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
            &[],
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
            &[],
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

    #[test]
    fn exdate_excludes_a_specific_instance() {
        // Daily event since 2026-04-01 at 14:00 UTC. EXDATE on 2026-05-03 should
        // remove that day's instance from the expansion.
        let dtstart = Utc.with_ymd_and_hms(2026, 4, 1, 14, 0, 0).unwrap();
        let range_start = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 5, 7, 23, 59, 59).unwrap();

        let extra = vec!["EXDATE:20260503T140000Z".to_string()];
        let instances = expand_rrule(
            "FREQ=DAILY",
            dtstart,
            None,
            &extra,
            Duration::hours(1),
            range_start,
            range_end,
            "Daily",
            "uid-exdate",
            None,
            None,
            None,
        )
        .unwrap();

        // May 1..=7 = 7 days, minus the EXDATE on May 3 = 6 instances.
        assert_eq!(instances.len(), 6, "EXDATE on 2026-05-03 must drop that day");
        assert!(
            !instances
                .iter()
                .any(|i| i.instance_start.format("%Y-%m-%d").to_string() == "2026-05-03"),
            "expansion must not contain the excluded date"
        );
    }

    #[test]
    fn rdate_adds_a_date_outside_the_master_pattern() {
        // Master is "weekly on Monday". 2026-05-03 is a Sunday — NOT in the
        // master pattern. An RDATE for 2026-05-03 must add it anyway. This is
        // the suspected cause of the user's two missing recurring events.
        let dtstart = Utc.with_ymd_and_hms(2026, 1, 5, 14, 0, 0).unwrap(); // Mon Jan 5
        let range_start = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 5, 3, 23, 59, 59).unwrap();

        // First confirm: without RDATE, today (Sunday) has zero instances.
        let baseline = expand_rrule(
            "FREQ=WEEKLY;BYDAY=MO",
            dtstart,
            None,
            &[],
            Duration::hours(1),
            range_start,
            range_end,
            "Weekly Mon",
            "uid-rdate-base",
            None,
            None,
            None,
        )
        .unwrap();
        assert!(baseline.is_empty(), "Sunday is not in BYDAY=MO");

        // Now with the RDATE, today shows up.
        let extra = vec!["RDATE:20260503T140000Z".to_string()];
        let instances = expand_rrule(
            "FREQ=WEEKLY;BYDAY=MO",
            dtstart,
            None,
            &extra,
            Duration::hours(1),
            range_start,
            range_end,
            "Weekly Mon",
            "uid-rdate",
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(instances.len(), 1, "RDATE must inject today's instance");
        assert_eq!(
            instances[0].instance_start,
            Utc.with_ymd_and_hms(2026, 5, 3, 14, 0, 0).unwrap()
        );
    }

    #[test]
    fn weekly_toronto_event_from_2024_expands_into_2026_window() {
        // Mirror of the user's "Ozempic" event: weekly at 22:00 Toronto, started
        // on 2024-06-02 (a Sunday). Today is a Sunday in 2026. The DTSTART_TZ
        // path must produce an instance whose UTC time is on the *next* UTC day
        // (22:00 EDT = 02:00 UTC next day), so the window has to be wide enough
        // to include it — this is the user's observed behavior.
        let toronto: chrono_tz::Tz = "America/Toronto".parse().unwrap();
        let dtstart = toronto
            .with_ymd_and_hms(2024, 6, 2, 22, 0, 0)
            .unwrap()
            .with_timezone(&Utc);

        let range_start = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 5, 5, 23, 59, 59).unwrap();

        let instances = expand_rrule(
            "FREQ=WEEKLY",
            dtstart,
            Some(toronto),
            &[],
            Duration::minutes(15),
            range_start,
            range_end,
            "Ozempic",
            "uid-ozempic",
            None,
            None,
            Some(toronto),
        )
        .unwrap();

        // 22:00 EDT on 2026-05-03 = 02:00 UTC on 2026-05-04.
        let target = Utc.with_ymd_and_hms(2026, 5, 4, 2, 0, 0).unwrap();
        assert!(
            instances.iter().any(|i| i.instance_start == target),
            "expected Sunday 2026-05-03 22:00 Toronto (= 2026-05-04 02:00 UTC); got {:?}",
            instances
                .iter()
                .map(|i| i.instance_start)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn synthetic_exdate_from_recurrence_id_dedups_overrides() {
        // Simulates the upstream of an override: a master recurring weekly on
        // Monday at 14:00 UTC, and a RECURRENCE-ID:20260504T140000Z that the
        // caller has converted to an `EXDATE:...` line so the override doesn't
        // double-emit. The expansion should skip 2026-05-04 entirely.
        let dtstart = Utc.with_ymd_and_hms(2026, 5, 4, 14, 0, 0).unwrap(); // Mon
        let range_start = Utc.with_ymd_and_hms(2026, 5, 4, 0, 0, 0).unwrap();
        let range_end = Utc.with_ymd_and_hms(2026, 5, 11, 23, 59, 59).unwrap();

        let extra = vec!["EXDATE:20260504T140000Z".to_string()];
        let instances = expand_rrule(
            "FREQ=WEEKLY;BYDAY=MO",
            dtstart,
            None,
            &extra,
            Duration::hours(1),
            range_start,
            range_end,
            "Weekly Mon",
            "uid-override",
            None,
            None,
            None,
        )
        .unwrap();

        // May 4 is excluded; May 11 is the next Monday in range.
        assert_eq!(instances.len(), 1);
        assert_eq!(
            instances[0].instance_start.format("%Y-%m-%d").to_string(),
            "2026-05-11"
        );
    }
}
