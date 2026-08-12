//! The process boundary: the real binary, a sandboxed temp environment, and
//! only public surfaces observed — exit codes, output, and the files written.

mod support;

use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use support::TestEnv;

const BIN: &str = env!("CARGO_BIN_EXE_openroutine");

const NIGHTLY: &str = r#"---
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: stub
---

Review all open TODO and FIXME comments.
"#;

/// Waits for a condition the daemon reaches on startup. This is process
/// startup latency, not a scheduling assertion — schedules are asserted at
/// the clock seam, never by waiting.
fn wait_until(what: &str, ready: impl FnMut() -> bool) {
    wait_up_to(Duration::from_secs(10), what, ready)
}

/// Waits for something whose guaranteed path is slower than its fast path —
/// a file change is noticed by the watcher within moments, but the promise
/// the daemon actually makes is the periodic rescan behind it.
fn wait_up_to(budget: Duration, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn terminate(child: &mut Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
}

/// Reaps the daemon, failing rather than hanging if it ignores the signal.
fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("daemon did not exit after SIGTERM");
}

#[test]
fn version_is_reported() {
    let output = Command::new(BIN).arg("--version").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected the crate version, got: {stdout}"
    );
}

#[test]
fn a_missing_config_fails_with_a_message_naming_the_path() {
    let env = TestEnv::new();
    let missing = env.path().join("nope.toml");

    let output = Command::new(BIN)
        .args(["serve", "--config"])
        .arg(&missing)
        .output()
        .unwrap();

    assert!(!output.status.success(), "should not start without config");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("nope.toml"),
        "error should name the path, got: {stderr}"
    );
}

#[test]
fn serving_discovers_tasks_writes_state_and_stops_cleanly_on_sigterm() {
    let env = TestEnv::new();
    let task_path = env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let mut child = Command::new(BIN)
        .args(["serve", "--config"])
        .arg(env.config_path())
        .env("XDG_STATE_HOME", env.xdg_state_home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_until("the daemon to write its state file", || {
        env.state_file().exists()
    });

    let state = env.read_state();
    let entry = &state["scheduledTasks"][0];
    assert_eq!(entry["id"], "proj/todo-digest");
    assert_eq!(
        entry["filePath"],
        task_path.canonicalize().unwrap().display().to_string()
    );

    // Supervised processes are stopped by signal; exiting cleanly is the
    // contract with launchd and systemd.
    terminate(&mut child);
    let status = wait_for_exit(&mut child);
    assert!(
        status.success(),
        "SIGTERM should be a clean shutdown, got {status:?}"
    );
}

#[test]
fn nothing_is_written_outside_the_sandbox() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let mut child = Command::new(BIN)
        .args(["serve", "--config"])
        .arg(env.config_path())
        .env("XDG_STATE_HOME", env.xdg_state_home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_until("the daemon to write its state file", || {
        env.state_file().exists()
    });
    terminate(&mut child);
    wait_for_exit(&mut child);

    // Everything the daemon created lives under the temp root: the state dir
    // it was told to use, and — inside the Project — only the CRONTAB.md it
    // is documented to keep there.
    let mut project_entries: Vec<String> = std::fs::read_dir(env.project_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    project_entries.sort();
    assert_eq!(
        project_entries,
        vec!["CRONTAB.md".to_string(), "todo-digest.cron.md".to_string()],
        "the daemon writes nothing else into the Project"
    );
}

#[test]
fn a_task_added_while_serving_is_picked_up_without_a_restart() {
    let env = TestEnv::new();
    env.write_config();

    let mut child = Command::new(BIN)
        .args(["serve", "--config"])
        .arg(env.config_path())
        .env("XDG_STATE_HOME", env.xdg_state_home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_until("the daemon to write its state file", || {
        env.state_file().exists()
    });

    // Dropped in after the daemon is already up, as a `git pull` would.
    env.write_task(
        "arrived",
        "---\ndescription: Arrived later\ncron: \"@hourly\"\nagent: stub\n---\n\nping\n",
    );

    wait_up_to(
        Duration::from_secs(45),
        "the new task to be noticed",
        || {
            env.try_read_state().is_some_and(|state| {
                state["scheduledTasks"]
                    .as_array()
                    .is_some_and(|tasks| tasks.iter().any(|task| task["id"] == "proj/arrived"))
            })
        },
    );

    terminate(&mut child);
    assert!(wait_for_exit(&mut child).success());
}
