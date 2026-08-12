//! `disabled:` in the file — turning a Task off as a reviewable commit.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::sync::Arc;
use support::{TestEnv, at};

const OFF: &str = "---\ndescription: Switched off\ncron: \"@hourly\"\nagent: stub\njitter: 0\ndisabled: true\n---\n\nping\n";
const ON: &str =
    "---\ndescription: Switched on\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nping\n";

async fn daemon_at(env: &TestEnv, now: &str) -> (Daemon, ManualClock) {
    let clock = ManualClock::new(at(now));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build");
    daemon.reload().await.expect("reload should succeed");
    (daemon, clock)
}

#[tokio::test]
async fn a_disabled_task_never_fires() {
    let env = TestEnv::new();
    env.write_task("off", OFF);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    for hour in 1..=6 {
        clock.set(at(&format!("2026-08-11T{hour:02}:00:00Z")));
        daemon.tick().await.unwrap();
    }
    daemon.wait_for_running().await;

    assert!(
        env.calls().is_empty(),
        "a task turned off in its own file must not run"
    );
}

#[tokio::test]
async fn a_disabled_task_accumulates_no_skips() {
    let env = TestEnv::new();
    env.write_task("off", OFF);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    for hour in 1..=6 {
        clock.set(at(&format!("2026-08-11T{hour:02}:00:00Z")));
        daemon.tick().await.unwrap();
    }

    let skips = env.read_state()["recordedSkips"]["proj/off"].clone();
    assert!(
        skips.is_null(),
        "off is not the same as held: a task switched off in its file is not \
         scheduled at all, so there are no Ticks to record — {skips}"
    );
}

#[test]
fn a_disabled_task_is_listed_as_such_rather_than_ready() {
    let env = TestEnv::new();
    env.write_task("off", OFF);
    env.write_config();

    let out = env.run_ok(&["list"]);

    let row = out
        .lines()
        .find(|line| line.contains("proj/off"))
        .unwrap_or_else(|| panic!("it should still be listed:\n{out}"));
    assert!(
        row.starts_with("disabled"),
        "the row must not read as something that will run: {row}"
    );
    assert!(out.contains("1 disabled"), "and be counted as such:\n{out}");
    assert!(
        !out.to_lowercase().contains("broken"),
        "being switched off is a choice, not a fault:\n{out}"
    );
}

#[tokio::test]
async fn removing_the_flag_brings_the_task_back() {
    let env = TestEnv::new();
    env.write_task("off", OFF);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    assert!(env.calls().is_empty());

    env.write_task("off", ON);
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        1,
        "the commit that removes it turns it on"
    );
}

#[tokio::test]
async fn a_disabled_task_cannot_be_fired_either() {
    let env = TestEnv::new();
    env.write_task("off", OFF);
    env.write_config();

    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    let outcome = daemon.fire("proj/off", None);

    assert!(
        matches!(outcome, openroutine::daemon::FireOutcome::Disabled),
        "the file says no, and firing does not override the file: {outcome:?}"
    );
}
