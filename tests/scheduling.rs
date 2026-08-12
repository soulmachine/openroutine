//! Scheduling core — tested at the clock seam: pure functions over an
//! explicit `now`, no wall-clock reads anywhere in this file.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use openroutine::schedule::Schedule;

/// A fixed +00:00 zone keeps these cases independent of the host's timezone.
/// DST behaviour gets its own real-timezone cases in the scheduling-semantics work.
fn utc_zone() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn at(iso: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(iso)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn daily_cron_fires_at_the_next_matching_wall_time() {
    let schedule = Schedule::parse("0 2 * * *").unwrap();

    let next = schedule
        .next_fire_after(at("2026-08-11T00:00:00Z"), &utc_zone())
        .unwrap();

    assert_eq!(next, at("2026-08-11T02:00:00Z"));
}

#[test]
fn a_tick_already_past_rolls_to_tomorrow() {
    let schedule = Schedule::parse("0 2 * * *").unwrap();

    let next = schedule
        .next_fire_after(at("2026-08-11T02:00:00Z"), &utc_zone())
        .unwrap();

    assert_eq!(next, at("2026-08-12T02:00:00Z"));
}

#[test]
fn cron_is_evaluated_in_the_given_zone_not_utc() {
    let schedule = Schedule::parse("0 2 * * *").unwrap();
    let minus_five = FixedOffset::west_opt(5 * 3600).unwrap();

    let next = schedule
        .next_fire_after(at("2026-08-11T00:00:00Z"), &minus_five)
        .unwrap();

    // 02:00 at UTC-5 is 07:00 UTC.
    assert_eq!(next, at("2026-08-11T07:00:00Z"));
    assert_eq!(
        minus_five.from_utc_datetime(&next.naive_utc()).to_rfc3339(),
        "2026-08-11T02:00:00-05:00"
    );
}

#[test]
fn step_and_range_syntax_parses() {
    let schedule = Schedule::parse("*/15 9-17 * * MON-FRI").unwrap();

    let next = schedule
        .next_fire_after(at("2026-08-11T09:02:00Z"), &utc_zone())
        .unwrap();

    assert_eq!(next, at("2026-08-11T09:15:00Z"));
}

#[test]
fn shorthand_aliases_parse() {
    for alias in ["@hourly", "@daily", "@weekly", "@monthly"] {
        assert!(
            Schedule::parse(alias).is_ok(),
            "expected {alias} to be accepted"
        );
    }
}

#[test]
fn an_invalid_expression_is_rejected_with_its_reason() {
    let err = Schedule::parse("not a cron").unwrap_err();

    assert!(
        err.to_string().to_lowercase().contains("cron"),
        "error should name the problem, got: {err}"
    );
}

#[test]
fn seconds_resolution_is_not_accepted() {
    // Six fields would be a seconds-resolution dialect; v1 is deliberately 5-field.
    assert!(Schedule::parse("0 0 2 * * *").is_err());
}
