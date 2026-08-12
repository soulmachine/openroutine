//! What survives a crash, a corrupted file, or a second daemon.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{TestEnv, at};

const BIN: &str = env!("CARGO_BIN_EXE_openroutine");

const HOURLY: &str =
    "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nping\n";

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn spawn_daemon(env: &TestEnv) -> std::process::Child {
    Command::new(BIN)
        .args(["serve", "--config"])
        .arg(env.config_path())
        .env("XDG_STATE_HOME", env.xdg_state_home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn stop(child: &mut std::process::Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let _ = child.wait();
}

// --- One daemon at a time -----------------------------------------------

#[test]
fn a_second_daemon_refuses_to_start_and_says_where_the_first_is() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config();

    let mut first = spawn_daemon(&env);
    wait_until("the first daemon to take the lock", || {
        env.state_file().exists()
    });

    let second = Command::new(BIN)
        .args(["serve", "--config"])
        .arg(env.config_path())
        .env("XDG_STATE_HOME", env.xdg_state_home())
        .output()
        .unwrap();

    stop(&mut first);

    assert!(
        !second.status.success(),
        "two daemons would race over one state file"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.to_lowercase().contains("already running"),
        "the error should explain, got: {stderr}"
    );
    assert!(
        stderr.contains(&env.state_dir().display().to_string()),
        "and point at the lock: {stderr}"
    );
}

#[test]
fn the_lock_is_released_when_the_daemon_stops() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config();

    let mut first = spawn_daemon(&env);
    wait_until("the first daemon", || env.state_file().exists());
    stop(&mut first);

    let mut second = spawn_daemon(&env);
    let started = second.try_wait().unwrap();
    stop(&mut second);

    assert!(
        started.is_none(),
        "the next daemon should start once the first has gone"
    );
}

// --- State that cannot be trusted ---------------------------------------

#[tokio::test]
async fn a_corrupt_state_file_is_set_aside_and_the_tasks_still_run() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config();
    std::fs::create_dir_all(env.state_dir()).unwrap();
    std::fs::write(env.state_file(), "{ this is not json at all").unwrap();

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build");
    daemon
        .reload()
        .await
        .expect("a bad state file is not fatal");

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1, "losing history does not lose tasks");
    assert_eq!(env.read_state()["scheduledTasks"][0]["id"], "proj/hourly");

    let set_aside: Vec<_> = std::fs::read_dir(env.state_dir())
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            name.contains("corrupt").then_some(name)
        })
        .collect();
    assert_eq!(
        set_aside.len(),
        1,
        "the unreadable file is kept for inspection, not deleted: {set_aside:?}"
    );
}

#[test]
fn killing_the_daemon_never_leaves_an_unreadable_state_file() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config();

    // Kill it repeatedly at whatever moment it happens to be in.
    for _ in 0..6 {
        let mut child = spawn_daemon(&env);
        wait_until("the daemon to write state", || env.state_file().exists());
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();

        let raw = std::fs::read_to_string(env.state_file()).unwrap();
        serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|error| panic!("state was torn: {error}\n{raw}"));
    }

    assert!(
        !env.state_dir().join("scheduled-tasks.json.tmp").exists(),
        "no half-written leftovers"
    );
}

#[tokio::test]
async fn a_run_interrupted_by_a_crash_is_recorded_on_the_next_start() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config();

    // What a killed Daemon leaves behind: a record still marked running,
    // with nobody left to finish it.
    let abandoned = env.state_dir().join("runs/proj/hourly/20260811T010000Z");
    std::fs::create_dir_all(&abandoned).unwrap();
    std::fs::write(
        abandoned.join("run.json"),
        r#"{"runId":"20260811T010000Z","taskId":"proj/hourly","status":"running",
            "trigger":"schedule","agent":"stub","startedAt":"2026-08-11T01:00:00Z"}"#,
    )
    .unwrap();

    let clock = ManualClock::new(at("2026-08-11T02:00:00Z"));
    let daemon = Daemon::with_zone(env.load_config(), Arc::new(clock), chrono_tz::UTC).unwrap();
    daemon.recover_interrupted_runs();

    let run = env.read_run("proj/hourly", 0);
    assert_eq!(
        run["status"], "interrupted",
        "a Run nobody is waiting for must not stay 'running' forever"
    );
    assert_eq!(run["finishedAt"], "2026-08-11T02:00:00Z");
}

#[tokio::test]
async fn recovery_leaves_finished_runs_alone() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config();

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon =
        Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC).unwrap();
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    daemon.recover_interrupted_runs();

    assert_eq!(
        env.read_run("proj/hourly", 0)["status"],
        "succeeded",
        "a Run that finished properly is not rewritten"
    );
}

// --- Keeping the disk honest --------------------------------------------

#[tokio::test]
async fn old_runs_are_pruned_once_the_cap_is_reached() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config_with_extra("max_runs_per_task = 3\n");

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon =
        Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC).unwrap();
    daemon.reload().await.unwrap();

    for hour in 1..=5 {
        clock.set(at(&format!("2026-08-11T{hour:02}:00:00Z")));
        daemon.tick().await.unwrap();
        daemon.wait_for_running().await;
    }

    let dirs = env.run_dirs("proj/hourly");
    assert_eq!(dirs.len(), 3, "the cap holds: {dirs:?}");
    assert_eq!(
        env.read_run("proj/hourly", 2)["scheduledFor"],
        "2026-08-11T05:00:00Z",
        "and it is the newest that survive"
    );
}

#[test]
fn the_daemon_keeps_its_own_log_beside_its_state() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY);
    env.write_config();

    let mut child = spawn_daemon(&env);
    wait_until("the daemon log", || {
        env.state_dir().join("daemon.log").exists()
    });
    stop(&mut child);

    let log = std::fs::read_to_string(env.state_dir().join("daemon.log")).unwrap();
    assert!(
        log.contains("serving"),
        "the log should record what the daemon did: {log}"
    );
}
