//! Refresh: a Task re-reading its own file at the moment it is about to Run.
//!
//! The only place a definition changes outside a Reload, and the narrowest
//! one — this file, on this path, for this Run.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::sync::Arc;
use support::{TestEnv, at};

const HOURLY: &str =
    "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nping\n";

async fn daemon_at(env: &TestEnv, now: &str) -> (Daemon, ManualClock) {
    let clock = ManualClock::new(at(now));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build")
        .watching_config(env.config_path());
    daemon.reload().await.expect("reload should succeed");
    (daemon, clock)
}

/// The whole point: the prompt you just fixed is the one that runs.
#[tokio::test]
async fn an_edited_prompt_runs_without_a_reload() {
    let env = TestEnv::new();
    env.write_task("edited", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;

    // Changed on disk, with no reload of any kind.
    env.write_task(
        "edited",
        "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\npong\n",
    );

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let prompt = env.calls()[0].args.last().unwrap().clone();
    assert_eq!(prompt, "pong", "the run should carry the edited prompt");
}

#[tokio::test]
async fn a_file_touched_without_being_edited_is_not_a_change() {
    let env = TestEnv::new();
    // A cron-less One-shot: it runs once, and re-arms only on a real edit.
    env.write_task("once", "---\ndescription: Once\nagent: stub\n---\n\nping\n");
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:00:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(env.calls().len(), 1);

    // Rewritten byte-for-byte, as `git checkout` or `touch` would: the mtime
    // moves, the content does not.
    env.write_task("once", "---\ndescription: Once\nagent: stub\n---\n\nping\n");
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        1,
        "a moved mtime with identical content must not re-arm anything"
    );
}

#[tokio::test]
async fn a_definition_that_stopped_loading_withdraws_its_tick() {
    let env = TestEnv::new();
    env.write_task("wreck", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    env.write_task(
        "wreck",
        "---\ndescription: Broken\ncron: \"nope\"\n---\n\nping\n",
    );

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert!(
        env.calls().is_empty(),
        "a definition that will not load must not run"
    );
    assert_eq!(
        daemon.task_count(),
        0,
        "and it holds no schedule until a reload"
    );
    let skips = env.read_state()["recordedSkips"]["wreck"].clone();
    assert_eq!(skips[0]["reason"], "definition-changed", "{skips}");
    assert!(
        skips[0]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("no longer loads")),
        "the skip should say what happened: {skips}"
    );
}

#[tokio::test]
async fn a_definition_disabled_since_the_tick_was_planned_withdraws_it() {
    let env = TestEnv::new();
    env.write_task("offswitch", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    env.write_task(
        "offswitch",
        "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\ndisabled: true\n---\n\nping\n",
    );

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert!(
        env.calls().is_empty(),
        "the file says no, so it does not run"
    );
    let skips = env.read_state()["recordedSkips"]["offswitch"].clone();
    assert_eq!(skips[0]["reason"], "definition-changed", "{skips}");
}

#[tokio::test]
async fn a_task_that_renamed_itself_waits_for_a_reload() {
    let env = TestEnv::new();
    env.write_task("original", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    env.write_task(
        "original",
        "---\nname: renamed\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\n---\n\nping\n",
    );

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert!(
        env.calls().is_empty(),
        "identity is a registry-level change; the fire path must not settle it"
    );
    let skips = env.read_state()["recordedSkips"]["original"].clone();
    assert!(
        skips[0]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("renamed")),
        "{skips}"
    );
}

#[tokio::test]
async fn a_one_shot_whose_moment_moved_is_re_armed_rather_than_fired() {
    let env = TestEnv::new();
    env.write_task(
        "once",
        "---\ndescription: Once\nat: \"2026-08-11T01:00:00Z\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    // Moved before the old moment arrives.
    env.write_task(
        "once",
        "---\ndescription: Once\nat: \"2026-08-11T03:00:00Z\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert!(
        env.calls().is_empty(),
        "firing at an abandoned moment would be plainly wrong"
    );

    // Re-armed at the moment the file now names, with no reload in between.
    clock.set(at("2026-08-11T03:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(env.calls().len(), 1, "it runs at the new moment instead");
}

#[tokio::test]
async fn a_cron_tick_still_runs_when_the_new_schedule_has_moved_on() {
    let env = TestEnv::new();
    env.write_task("shifting", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    // Re-aimed at 02:00 daily; the 01:00 tick is no longer in the schedule.
    env.write_task(
        "shifting",
        "---\ndescription: Daily\ncron: \"0 2 * * *\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        1,
        "the tick was legitimately due when it was planned — every tick becomes \
         exactly one run or one skip"
    );
}

#[tokio::test]
async fn a_fired_run_refreshes_on_the_same_path() {
    let env = TestEnv::new();
    env.write_task(
        "ondemand",
        "---\ndescription: On demand\ncron: \"0 4 1 1 *\"\nagent: stub\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    env.write_task(
        "ondemand",
        "---\ndescription: On demand\ncron: \"0 4 1 1 *\"\nagent: stub\n---\n\npong\n",
    );

    daemon.fire("ondemand", None);
    daemon.wait_for_running().await;

    let prompt = env.calls()[0].args.last().unwrap().clone();
    assert_eq!(prompt, "pong", "I edited it, then fired it");
}

/// The accepted boundary: Refresh rides the run path, so a Task that will
/// never run can never take it.
#[tokio::test]
async fn a_task_that_never_fires_never_refreshes() {
    let env = TestEnv::new();
    env.write_task(
        "off",
        "---\ndescription: Off\ncron: \"@hourly\"\nagent: stub\ndisabled: true\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    // Switched back on in the file — which nothing will notice on its own.
    env.write_task("off", HOURLY);

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert!(
        env.calls().is_empty(),
        "a disabled task is not scheduled, so it has no tick to refresh on"
    );

    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(env.calls().len(), 1, "a reload is what reaches it");
}
