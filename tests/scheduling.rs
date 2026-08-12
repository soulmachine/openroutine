//! Scheduling core — tested at the clock seam: pure functions over an
//! explicit `now`, no wall-clock reads anywhere in this file.

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use chrono_tz::America::Los_Angeles;
use openroutine::jitter;
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
        .next_tick_after(at("2026-08-11T00:00:00Z"), &utc_zone())
        .unwrap();

    assert_eq!(next, at("2026-08-11T02:00:00Z"));
}

#[test]
fn a_tick_already_past_rolls_to_tomorrow() {
    let schedule = Schedule::parse("0 2 * * *").unwrap();

    let next = schedule
        .next_tick_after(at("2026-08-11T02:00:00Z"), &utc_zone())
        .unwrap();

    assert_eq!(next, at("2026-08-12T02:00:00Z"));
}

#[test]
fn cron_is_evaluated_in_the_given_zone_not_utc() {
    let schedule = Schedule::parse("0 2 * * *").unwrap();
    let minus_five = FixedOffset::west_opt(5 * 3600).unwrap();

    let next = schedule
        .next_tick_after(at("2026-08-11T00:00:00Z"), &minus_five)
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
        .next_tick_after(at("2026-08-11T09:02:00Z"), &utc_zone())
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

// --- Daylight saving ---------------------------------------------------
//
// The doc criticises cron's timezone story, so ours has to be pinned: a wall
// time that spring-forward skips fires at the next valid instant, and one
// that fall-back repeats fires once.

/// Daylight-saving cases, as a table: expression, the instant we ask from,
/// and the UTC instant the next Tick must land on in Los Angeles.
#[test]
fn daylight_saving_transitions_behave_as_documented() {
    let cases = [
        (
            "spring forward: 02:00 never happens, so the Tick lands at 03:00 PDT",
            "0 2 * * *",
            "2026-03-08T08:00:00Z",
            "2026-03-08T10:00:00Z",
        ),
        (
            "the day before spring forward is an ordinary 02:00 PST",
            "0 2 * * *",
            "2026-03-07T00:00:00Z",
            "2026-03-07T10:00:00Z",
        ),
        (
            "fall back: 01:00 happens twice; the Tick is taken once, at 01:00 PDT",
            "0 1 * * *",
            "2026-11-01T00:00:00Z",
            "2026-11-01T08:00:00Z",
        ),
        (
            "after fall back the wall time holds and the UTC instant shifts",
            "0 9 * * *",
            "2026-11-01T00:00:00Z",
            "2026-11-01T17:00:00Z",
        ),
        (
            "before fall back, the same wall time is an hour earlier in UTC",
            "0 9 * * *",
            "2026-10-31T00:00:00Z",
            "2026-10-31T16:00:00Z",
        ),
    ];

    for (what, expression, from, expected) in cases {
        let schedule = Schedule::parse(expression).unwrap();
        let next = schedule.next_tick_after(at(from), &Los_Angeles).unwrap();
        assert_eq!(next, at(expected), "{what}");
    }
}

#[test]
fn a_wall_time_repeated_by_fall_back_fires_only_once() {
    let schedule = Schedule::parse("0 1 * * *").unwrap();

    // 01:00 occurs twice on 2026-11-01 in Los Angeles: 08:00 UTC in PDT and
    // 09:00 UTC in PST. Exactly one of them is a Tick.
    let first = schedule
        .next_tick_after(at("2026-11-01T00:00:00Z"), &Los_Angeles)
        .unwrap();
    let following = schedule.next_tick_after(first, &Los_Angeles).unwrap();

    assert!(
        first < at("2026-11-01T10:00:00Z"),
        "the repeated hour yields one Tick, got {first}"
    );
    assert!(
        following >= at("2026-11-02T00:00:00Z"),
        "and the next is the following day, not the repeat: {following}"
    );
}

// --- Jitter ------------------------------------------------------------
//
// Deterministic per Task, so a machine full of midnight tasks spreads out
// without anything becoming unpredictable.

#[test]
fn the_same_task_always_gets_the_same_offset() {
    let window = Duration::minutes(5);

    let first = jitter::offset("myrepo/todo-digest", window);
    let again = jitter::offset("myrepo/todo-digest", window);

    assert_eq!(first, again);
}

#[test]
fn different_tasks_get_spread_across_the_window() {
    let window = Duration::minutes(5);

    let offsets: Vec<i64> = (0..40)
        .map(|n| jitter::offset(&format!("proj/task-{n}"), window).num_seconds())
        .collect();

    assert!(
        offsets.iter().all(|seconds| (0..300).contains(seconds)),
        "every offset lands inside the window: {offsets:?}"
    );
    let distinct: std::collections::BTreeSet<_> = offsets.iter().collect();
    assert!(
        distinct.len() > 30,
        "offsets should spread, not cluster: {distinct:?}"
    );
}

#[test]
fn a_zero_window_means_exactly_on_the_tick() {
    assert_eq!(
        jitter::offset("myrepo/todo-digest", Duration::zero()),
        Duration::zero()
    );
}

#[test]
fn the_window_never_exceeds_half_the_interval() {
    // Hourly has room for the full default window.
    assert_eq!(
        jitter::window(Some(Duration::hours(1)), None),
        Duration::minutes(5)
    );
    // Every two minutes does not: half of 120s is 60s.
    assert_eq!(
        jitter::window(Some(Duration::minutes(2)), None),
        Duration::minutes(1)
    );
    // An explicit setting is still capped by the same rule.
    assert_eq!(
        jitter::window(Some(Duration::minutes(2)), Some(Duration::minutes(30))),
        Duration::minutes(1)
    );
    // Opting out is honoured exactly.
    assert_eq!(
        jitter::window(Some(Duration::hours(1)), Some(Duration::zero())),
        Duration::zero()
    );
}

// --- Boundary ----------------------------------------------------------

#[test]
fn a_tick_exactly_now_is_available_to_an_inclusive_search() {
    let schedule = Schedule::parse("0 2 * * *").unwrap();
    let exactly = at("2026-08-11T02:00:00Z");

    assert_eq!(
        schedule.next_tick_at_or_after(exactly, &utc_zone()),
        Some(exactly),
        "a reload landing on the tick instant must not drop it"
    );
    assert_eq!(
        schedule.next_tick_after(exactly, &utc_zone()),
        Some(at("2026-08-12T02:00:00Z")),
        "while the exclusive search moves past it"
    );
}
