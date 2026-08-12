//! Reload: the one moment definitions change. The schedule follows disk when
//! — and only when — someone asks, and says so when a Task is new or edited.

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

#[tokio::test]
async fn a_task_registered_later_is_scheduled_without_a_restart() {
    let env = TestEnv::new();
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    assert_eq!(daemon.task_count(), 0);

    env.write_task("late", HOURLY);
    daemon.reload().await.unwrap();

    assert_eq!(daemon.task_count(), 1);
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(env.calls().len(), 1);
}

#[tokio::test]
async fn an_edit_does_nothing_until_a_reload() {
    let env = TestEnv::new();
    env.write_task(
        "shifting",
        "---\ndescription: Daily\ncron: \"0 2 * * *\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;

    // Re-aimed at every hour, on disk, while the daemon runs.
    env.write_task("shifting", HOURLY);

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert!(
        env.calls().is_empty(),
        "nothing is watched: the daemon still holds the definition it was given"
    );

    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        1,
        "and asking for a reload is what makes the edit real"
    );
}

#[tokio::test]
async fn editing_the_schedule_takes_effect_from_the_next_tick() {
    let env = TestEnv::new();
    env.write_task(
        "shifting",
        "---\ndescription: Hourly\ncron: \"0 2 * * *\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;

    // Re-aimed at every hour instead of 02:00.
    env.write_task("shifting", HOURLY);
    daemon.reload().await.unwrap();

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1, "the new schedule is in force");
}

#[tokio::test]
async fn an_edit_mid_run_leaves_the_active_run_alone() {
    let env = TestEnv::new();
    env.write_gated_stub_agent();
    env.write_task("busy", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();

    // Rewrite the prompt while the Agent is still working.
    env.write_task(
        "busy",
        "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\npong\n",
    );
    daemon.reload().await.unwrap();
    env.open_gate();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls()[0].args,
        vec!["--run", "ping"],
        "the Run in flight kept the definition it started with"
    );

    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(
        env.calls()[1].args,
        vec!["--run", "pong"],
        "and the next Run picks up the edit"
    );
}

#[tokio::test]
async fn unregistering_mid_run_lets_it_finish_then_forgets_the_task() {
    let env = TestEnv::new();
    env.write_gated_stub_agent();
    let path = env.write_task("doomed", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();

    // Unregistered — the deliberate act, not merely a file going missing.
    env.unregister(&path);
    daemon.reload().await.unwrap();
    env.open_gate();
    daemon.wait_for_running().await;

    assert_eq!(daemon.task_count(), 0, "unscheduled");
    assert_eq!(
        env.read_run("doomed", 0)["status"],
        "succeeded",
        "the Run that was already going still finished and was recorded"
    );
    assert!(!env.read_run_log("doomed", 0).is_empty(), "its log is kept");

    let state = env.read_state();
    assert!(
        state["scheduledTasks"].as_array().unwrap().is_empty(),
        "the state entry is pruned once the task is unregistered: {state}"
    );
}

#[tokio::test]
async fn a_registered_file_that_is_deleted_is_broken_and_keeps_its_state() {
    let env = TestEnv::new();
    let path = env.write_task("kept", HOURLY);
    env.write_config();

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    std::fs::remove_file(&path).unwrap();
    daemon.reload().await.unwrap();

    assert_eq!(daemon.task_count(), 0, "it cannot run without its file");
    assert_eq!(daemon.broken_count(), 1, "but it is broken, not forgotten");
    let state = env.read_state();
    assert_eq!(
        state["scheduledTasks"][0]["id"], "kept",
        "a missing file is not proof anyone unregistered it: {state}"
    );
}

#[tokio::test]
async fn a_reload_converges_on_whatever_is_registered() {
    let env = TestEnv::new();
    let one = env.write_task("one", HOURLY);
    env.write_config();

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    assert_eq!(daemon.task_count(), 1);

    // Several changes at once, as a session of edits would make them.
    env.write_task("two", HOURLY);
    env.write_task("three", HOURLY);
    env.unregister(&one);

    daemon.reload().await.unwrap();

    assert_eq!(
        daemon.task_count(),
        2,
        "one reload takes in everything the config now names"
    );
}

// --- Trust flagging -----------------------------------------------------

#[test]
fn a_task_that_has_never_run_is_flagged_as_new() {
    let env = TestEnv::new();
    env.write_task("fresh", HOURLY);
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("fresh"), "{out}");
    assert!(
        out.to_lowercase().contains("new"),
        "a task nobody has run yet should announce itself:\n{out}"
    );
}

#[tokio::test]
async fn the_flag_clears_once_the_task_has_run() {
    let env = TestEnv::new();
    env.write_task("fresh", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let out = env.run_ok(&["list"]);

    assert!(
        !out.to_lowercase().contains("never run"),
        "no permanent noise once it is part of the furniture:\n{out}"
    );
}

#[tokio::test]
async fn editing_a_task_flags_it_again() {
    let env = TestEnv::new();
    env.write_task("fresh", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    // The kind of change a `git pull` brings.
    env.write_task(
        "fresh",
        "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nsomething else entirely\n",
    );

    let out = env.run_ok(&["list"]);

    assert!(
        out.to_lowercase().contains("changed"),
        "a definition that changed since its last Run is worth noticing:\n{out}"
    );
}

#[tokio::test]
async fn a_rescan_between_ticks_does_not_swallow_the_pending_tick() {
    let env = TestEnv::new();
    env.write_task("steady", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    // Rescans happen every half minute now. One landing between Ticks must
    // leave the schedule exactly where it was.
    clock.set(at("2026-08-11T01:30:00Z"));
    daemon.reload().await.unwrap();

    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        2,
        "the 02:00 Tick still fired after the rescan"
    );
}

#[tokio::test]
async fn ticks_passed_over_while_the_daemon_lagged_survive_a_rescan() {
    let env = TestEnv::new();
    env.write_task("steady", HOURLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    // Hours pass with no scheduler pass, then a rescan arrives first.
    clock.set(at("2026-08-11T05:00:00Z"));
    daemon.reload().await.unwrap();
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let skips = env.read_state()["recordedSkips"]["steady"].clone();
    assert_eq!(
        skips[0]["reason"], "missed",
        "a rescan must not quietly absorb the Ticks nobody reached: {skips}"
    );
    assert_eq!(skips[0]["count"], 3, "03:00, 04:00 and 05:00 — {skips}");
}
