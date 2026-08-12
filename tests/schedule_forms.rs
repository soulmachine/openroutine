//! The two schedules that aren't cron: a One-shot with a timestamp, and a
//! Manual Task with none at all.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::sync::Arc;
use support::{TestEnv, at};

fn one_shot(when: &str) -> String {
    format!("---\ndescription: Once\nat: \"{when}\"\nagent: stub\njitter: 0\n---\n\nping\n")
}

const MANUAL: &str = "---\ndescription: On demand only\nagent: stub\n---\n\nping\n";

async fn daemon_at(env: &TestEnv, now: &str) -> (Daemon, ManualClock) {
    let clock = ManualClock::new(at(now));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build")
        .watching_config(env.config_path());
    daemon.reload().await.expect("reload should succeed");
    (daemon, clock)
}

// --- One-shots ----------------------------------------------------------

#[tokio::test]
async fn a_one_shot_fires_at_its_moment_and_then_is_done() {
    let env = TestEnv::new();
    env.write_task("once", &one_shot("2026-08-11T01:00:00Z"));
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1);
    assert_eq!(
        env.read_state()["scheduledTasks"][0]["completedFor"],
        "2026-08-11T01:00:00Z",
        "state remembers which moment it answered"
    );
}

#[tokio::test]
async fn a_one_shot_fires_once_however_often_the_daemon_restarts() {
    let env = TestEnv::new();
    env.write_task("once", &one_shot("2026-08-11T01:00:00Z"));
    env.write_config();

    // Restarts either side of, and exactly on, its moment.
    for now in [
        "2026-08-11T00:59:00Z",
        "2026-08-11T01:00:00Z",
        "2026-08-11T01:00:00Z",
        "2026-08-11T02:00:00Z",
        "2026-08-12T00:00:00Z",
    ] {
        let (mut daemon, _clock) = daemon_at(&env, now).await;
        daemon.tick().await.unwrap();
        daemon.wait_for_running().await;
    }

    assert_eq!(
        env.calls().len(),
        1,
        "exactly one Run, no matter how the daemon came and went"
    );
}

#[tokio::test]
async fn editing_the_timestamp_re_arms_a_one_shot() {
    let env = TestEnv::new();
    env.write_task("once", &one_shot("2026-08-11T01:00:00Z"));
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(env.calls().len(), 1);

    // Given a new moment, it is a Task with something left to do again.
    env.write_task("once", &one_shot("2026-08-11T03:00:00Z"));
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T03:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 2, "the new moment is a new obligation");
}

#[tokio::test]
async fn a_one_shot_whose_moment_passed_while_the_daemon_was_down_does_not_run() {
    let env = TestEnv::new();
    env.write_task("once", &one_shot("2026-08-11T01:00:00Z"));
    env.write_config();

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T06:00:00Z").await;
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert!(
        env.calls().is_empty(),
        "no catch-up: a moment that passed unattended is recorded, not run"
    );
    let skips = env.read_state()["recordedSkips"]["once"].clone();
    assert_eq!(skips[0]["reason"], "daemon-down", "{skips}");
}

#[tokio::test]
async fn catch_up_runs_a_one_shot_whose_moment_slipped_by() {
    let env = TestEnv::new();
    env.write_task(
        "once",
        "---\ndescription: Once\nat: \"2026-08-11T01:00:00Z\"\nagent: stub\ncatch_up: true\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T06:00:00Z").await;
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1, "asked for, so caught up");
}

#[tokio::test]
async fn catch_up_gives_up_after_a_week() {
    let env = TestEnv::new();
    env.write_task(
        "once",
        "---\ndescription: Once\nat: \"2026-08-01T01:00:00Z\"\nagent: stub\ncatch_up: true\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T06:00:00Z").await;
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert!(
        env.calls().is_empty(),
        "ten days late is too late to be what anyone wanted"
    );
}

#[tokio::test]
async fn catch_up_runs_the_most_recent_missed_cron_tick_only_once() {
    let env = TestEnv::new();
    env.write_task(
        "hourly",
        "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\ncatch_up: true\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    // Ran at 01:00, then the daemon was away until 05:30.
    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    drop(daemon);

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T05:30:00Z").await;
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        2,
        "one Run for the most recent miss, not one per missed Tick"
    );
    let skips = env.read_state()["recordedSkips"]["hourly"].clone();
    assert_eq!(skips[0]["reason"], "daemon-down", "the rest are recorded");
}

// --- One-shot with no stated moment -------------------------------------

#[tokio::test]
async fn a_task_with_no_schedule_runs_once_and_is_then_done() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:00:00Z").await;
    for hour in 1..=12 {
        clock.set(at(&format!("2026-08-11T{hour:02}:00:00Z")));
        daemon.tick().await.unwrap();
    }
    daemon.wait_for_running().await;

    assert_eq!(daemon.task_count(), 1, "it is a Task, not a mistake");
    assert_eq!(
        env.calls().len(),
        1,
        "a task with no cron runs once, as soon as the daemon takes it in — \
         and then never again on its own"
    );
}

#[tokio::test]
async fn editing_a_completed_one_shot_gives_it_something_to_do_again() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:00:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(env.calls().len(), 1);

    // Reloading an unchanged definition must not run it a second time.
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    assert_eq!(
        env.calls().len(),
        1,
        "an unchanged one-shot stays completed"
    );

    // An edit is what re-arms it.
    env.write_task(
        "ondemand",
        "---\ndescription: On demand only\nagent: stub\n---\n\npong\n",
    );
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T03:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        2,
        "editing the definition is what asks for another run"
    );
}

#[test]
fn a_task_with_no_schedule_lists_as_running_once() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("ondemand"), "{out}");
    assert!(
        !out.to_lowercase().contains("broken"),
        "no cron is a choice, not a fault:\n{out}"
    );
    assert!(
        out.to_lowercase().contains("once"),
        "and it should say so plainly:\n{out}"
    );
}

#[test]
fn a_one_shot_lists_with_its_moment() {
    let env = TestEnv::new();
    env.write_task("once", &one_shot("2026-12-25T09:00:00Z"));
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("2026-12-25"), "{out}");
    assert!(!out.to_lowercase().contains("broken"), "{out}");
}

#[test]
fn naming_both_a_cron_and_a_moment_is_broken() {
    let env = TestEnv::new();
    env.write_task(
        "confused",
        "---\ndescription: Both\ncron: \"@daily\"\nat: \"2026-08-11T01:00:00Z\"\nagent: stub\n---\n\nping\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(
        out.contains("cron") && out.contains("at"),
        "the error should name both, got:\n{out}"
    );
}

#[test]
fn a_timestamp_that_is_not_a_timestamp_is_broken() {
    let env = TestEnv::new();
    env.write_task(
        "vague",
        "---\ndescription: Soon\nat: \"next tuesday\"\nagent: stub\n---\n\nping\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(out.contains("next tuesday"), "{out}");
}
