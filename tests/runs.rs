//! How a Run actually executes: the environment it sees, where it starts,
//! what it is allowed to spend, and how it is stopped.
//!
//! Driven at the clock seam with a stub Agent that records what it received.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::sync::Arc;
use support::{AgentCall, TestEnv, at};

/// Fires one hourly Task and returns what the Agent saw.
async fn fire(env: &TestEnv) -> AgentCall {
    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build");
    daemon.reload().await.expect("reload should succeed");
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    env.calls()
        .into_iter()
        .next()
        .expect("the Agent should have run")
}

fn task(frontmatter: &str) -> String {
    format!(
        "---\ndescription: Under test\ncron: \"@hourly\"\nagent: stub\njitter: 0\n{frontmatter}---\n\nping\n"
    )
}

// --- Environment --------------------------------------------------------

#[tokio::test]
async fn the_agent_receives_a_usable_environment() {
    let env = TestEnv::new();
    env.write_task("envy", &task(""));
    env.write_config();

    let invocation = fire(&env).await;

    // A login shell always exports these; a stripped service environment —
    // the launchd failure this project exists to fix — would not.
    assert!(
        invocation.var("HOME").is_some_and(|home| !home.is_empty()),
        "expected HOME from the login shell, got {:?}",
        invocation.env
    );
    assert!(
        invocation
            .var("PATH")
            .is_some_and(|path| path.contains('/')),
        "expected a real PATH, got {:?}",
        invocation.var("PATH")
    );
}

/// `list` predicts what a Run will find by sampling the login shell seeded
/// with a service manager's `PATH`. That prediction is only worth anything
/// if a Run is seeded the same way — otherwise a Task passes `list` and runs
/// green under `serve` in a terminal, then fails the first night it runs
/// installed, which is the whole failure this project exists to prevent.
#[tokio::test]
async fn a_run_gets_the_path_the_reachability_check_samples() {
    let env = TestEnv::new();
    env.write_task("envy", &task(""));
    env.write_config();

    let invocation = fire(&env).await;

    // The same question `login_path` asks, asked the same way.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let sampled = std::process::Command::new(shell)
        .args(["-l", "-c", "printf %s \"$PATH\""])
        .env("PATH", openroutine::runner::SERVICE_PATH)
        .output()
        .expect("the login shell should answer");
    let sampled = String::from_utf8_lossy(&sampled.stdout).trim().to_string();

    assert_eq!(
        invocation.var("PATH"),
        Some(sampled.as_str()),
        "the Run's PATH must be the one `program_reach` warns about, not the \
         caller's — this test process's PATH is {:?}",
        std::env::var("PATH").unwrap_or_default()
    );
}

#[tokio::test]
async fn config_env_reaches_the_agent() {
    let env = TestEnv::new();
    env.write_task("envy", &task(""));
    env.write_config_with_extra("[env]\nOPENROUTINE_TEST_FROM = \"config\"\n");

    let invocation = fire(&env).await;

    assert_eq!(invocation.var("OPENROUTINE_TEST_FROM"), Some("config"));
}

#[tokio::test]
async fn task_env_overrides_config_env() {
    let env = TestEnv::new();
    env.write_task(
        "envy",
        &task("env:\n  OPENROUTINE_TEST_FROM: task\n  OPENROUTINE_TEST_ONLY: yes\n"),
    );
    env.write_config_with_extra("[env]\nOPENROUTINE_TEST_FROM = \"config\"\n");

    let invocation = fire(&env).await;

    assert_eq!(
        invocation.var("OPENROUTINE_TEST_FROM"),
        Some("task"),
        "the task file is closer to the work, so it wins"
    );
    assert_eq!(invocation.var("OPENROUTINE_TEST_ONLY"), Some("yes"));
}

#[tokio::test]
async fn an_override_beats_whatever_the_login_profile_set() {
    let env = TestEnv::new();
    env.write_task("envy", &task("env:\n  HOME: /tmp/openroutine-test-home\n"));
    env.write_config();

    let invocation = fire(&env).await;

    assert_eq!(
        invocation.var("HOME"),
        Some("/tmp/openroutine-test-home"),
        "overrides are applied after the profile runs, not before"
    );
}

// --- Working directory --------------------------------------------------

#[tokio::test]
async fn a_run_starts_in_the_project_directory_by_default() {
    let env = TestEnv::new();
    env.write_task("here", &task(""));
    env.write_config();

    let invocation = fire(&env).await;

    assert_eq!(invocation.cwd, env.project_dir().canonicalize().unwrap());
}

#[tokio::test]
async fn cwd_moves_the_run_relative_to_the_project() {
    let env = TestEnv::new();
    std::fs::create_dir_all(env.project_dir().join("sub/dir")).unwrap();
    env.write_task("there", &task("cwd: sub/dir\n"));
    env.write_config();

    let invocation = fire(&env).await;

    assert_eq!(
        invocation.cwd,
        env.project_dir().join("sub/dir").canonicalize().unwrap()
    );
}

#[tokio::test]
async fn a_cwd_that_does_not_exist_makes_the_task_broken() {
    let env = TestEnv::new();
    env.write_task("nowhere", &task("cwd: does/not/exist\n"));
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(out.contains("does/not/exist"), "{out}");
}

// --- Agent parameters ---------------------------------------------------

#[tokio::test]
async fn model_and_permission_mode_arrive_as_single_arguments() {
    let env = TestEnv::new();
    env.write_task(
        "tuned",
        &task("model: claude-opus-4\npermission_mode: acceptEdits\n"),
    );
    env.write_config_with_agent(&format!(
        "{} --run {{prompt}} --model {{model}} --permission-mode {{permission_mode}}",
        env.stub_path().display()
    ));

    let invocation = fire(&env).await;

    assert_eq!(
        invocation.args,
        vec![
            "--run",
            "ping",
            "--model",
            "claude-opus-4",
            "--permission-mode",
            "acceptEdits"
        ]
    );
}

#[tokio::test]
async fn an_unset_parameter_takes_its_flag_with_it() {
    let env = TestEnv::new();
    env.write_task("plain", &task(""));
    env.write_config_with_agent(&format!(
        "{} --run {{prompt}} --model {{model}}",
        env.stub_path().display()
    ));

    let invocation = fire(&env).await;

    assert_eq!(
        invocation.args,
        vec!["--run", "ping"],
        "no dangling --model, and no empty argument"
    );
}

#[tokio::test]
async fn an_unset_parameter_inside_a_joined_flag_drops_the_whole_token() {
    let env = TestEnv::new();
    env.write_task("plain", &task(""));
    env.write_config_with_agent(&format!(
        "{} --run {{prompt}} --model={{model}}",
        env.stub_path().display()
    ));

    let invocation = fire(&env).await;

    assert_eq!(invocation.args, vec!["--run", "ping"]);
}

#[tokio::test]
async fn a_parameter_the_template_cannot_carry_warns_and_is_ignored() {
    let env = TestEnv::new();
    env.write_task("tuned", &task("model: claude-opus-4\n"));
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(
        !out.to_lowercase().contains("broken"),
        "an unusable parameter is a warning, not a broken task:\n{out}"
    );
    assert!(out.to_lowercase().contains("warning"), "{out}");
    assert!(out.contains("model"), "{out}");
}

#[tokio::test]
async fn an_unknown_placeholder_in_a_template_is_rejected_at_load() {
    let env = TestEnv::new();
    env.write_task("plain", &task(""));
    env.write_config_with_agent(&format!("{} --run {{promt}}", env.stub_path().display()));

    let output = env.run(&["list"]);

    assert!(!output.status.success(), "a typo'd template must not load");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("promt"),
        "the error should quote the typo, got: {stderr}"
    );
}

// --- Timeouts -----------------------------------------------------------

#[tokio::test]
async fn a_task_that_outlives_its_timeout_is_killed_with_its_children() {
    let env = TestEnv::new();
    let marker = env.path().join("child-still-alive");
    env.write_forking_stub_agent(&marker);
    env.write_task("hang", &task("timeout: 1s\n"));
    env.write_config();

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build");
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let run = env.read_run("proj/hang", 0);
    assert_eq!(run["status"], "timed-out");

    // The stub's child writes the marker a few seconds after being forked.
    // If the whole process group died, it never gets the chance.
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "the agent's children must die with it, not outlive the run"
    );
}

#[tokio::test]
async fn a_task_finishing_inside_its_timeout_is_untouched() {
    let env = TestEnv::new();
    env.write_task("quick", &task("timeout: 30s\n"));
    env.write_config();

    let invocation = fire(&env).await;

    assert_eq!(invocation.args, vec!["--run", "ping"]);
    assert_eq!(env.read_run("proj/quick", 0)["status"], "succeeded");
}

#[tokio::test]
async fn timeout_none_is_accepted_and_leaves_the_run_unbounded() {
    let env = TestEnv::new();
    env.write_task("forever", &task("timeout: none\n"));
    env.write_config();

    let invocation = fire(&env).await;

    assert_eq!(invocation.args, vec!["--run", "ping"]);
    assert_eq!(env.read_run("proj/forever", 0)["status"], "succeeded");
}

#[tokio::test]
async fn a_timeout_that_is_not_a_duration_makes_the_task_broken() {
    let env = TestEnv::new();
    env.write_task("bad", &task("timeout: soonish\n"));
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(out.contains("soonish"), "{out}");
}

// --- Output cap ---------------------------------------------------------

#[tokio::test]
async fn a_chatty_agent_has_its_log_capped_and_says_so() {
    let env = TestEnv::new();
    env.write_noisy_stub_agent(200_000);
    env.write_task("noisy", &task(""));
    env.write_config_with_extra("max_log_bytes = 4096\n");

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build");
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let log = env.read_run_log("proj/noisy", 0);
    assert!(
        log.len() < 20_000,
        "the log should stop near the cap, got {} bytes",
        log.len()
    );
    assert!(
        log.to_lowercase().contains("truncated"),
        "the log must admit it was cut short:\n{}",
        &log[log.len().saturating_sub(400)..]
    );
    assert_eq!(
        env.read_run("proj/noisy", 0)["status"],
        "succeeded",
        "hitting the cap does not fail the Run"
    );
}

#[tokio::test]
async fn an_env_name_that_would_be_read_as_an_option_cannot_change_the_command() {
    let env = TestEnv::new();
    let breach = env.path().join("breached");
    env.write_task(
        "hostile",
        &task(&format!(
            "env:\n  \"--split-string\": \"/bin/sh -c 'touch {}'\"\n  KEEP: fine\n",
            breach.display()
        )),
    );
    env.write_config();

    let invocation = fire(&env).await;

    assert!(
        !breach.exists(),
        "a task file must never be able to run a command of its own choosing"
    );
    assert_eq!(
        invocation.args,
        vec!["--run", "ping"],
        "the configured template is what ran"
    );
    assert_eq!(
        invocation.var("KEEP"),
        Some("fine"),
        "sane names still work"
    );
    assert_eq!(invocation.var("--split-string"), None);
}

#[tokio::test]
async fn a_task_without_a_timeout_inherits_the_configured_default() {
    let env = TestEnv::new();
    env.write_forking_stub_agent(&env.path().join("unused"));
    env.write_task("hang", &task(""));
    env.write_config_with_extra("default_timeout = \"1s\"\n");

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build");
    daemon.reload().await.unwrap();
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.read_run("proj/hang", 0)["status"],
        "timed-out",
        "the default applies to a task that names no timeout"
    );
}

#[test]
fn a_default_timeout_that_is_not_a_duration_is_refused_at_load() {
    let env = TestEnv::new();
    env.write_task("plain", &task(""));
    env.write_config_with_extra("default_timeout = \"soonish\"\n");

    let output = env.run(&["list"]);

    assert!(!output.status.success(), "a bad default must not load");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("soonish"), "got: {stderr}");
}

#[test]
fn a_cwd_that_climbs_out_of_the_project_is_broken() {
    let env = TestEnv::new();
    env.write_task("escapee", &task("cwd: ../..\n"));
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
}

#[test]
fn an_absolute_cwd_is_broken() {
    let env = TestEnv::new();
    env.write_task("escapee", &task("cwd: /etc\n"));
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
}

#[test]
fn a_hyphenated_placeholder_typo_is_refused_at_load() {
    let env = TestEnv::new();
    env.write_task("plain", &task(""));
    env.write_config_with_agent(&format!(
        "{} --run {{prompt}} --mode {{permission-mode}}",
        env.stub_path().display()
    ));

    let output = env.run(&["list"]);

    assert!(!output.status.success(), "a typo'd template must not load");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("permission-mode"),
        "the error should quote the typo"
    );
}

#[tokio::test]
async fn an_out_of_range_duration_breaks_only_its_own_task() {
    let env = TestEnv::new();
    env.write_task("absurd", &task("timeout: 999999999999d\n"));
    env.write_task("fine", &task(""));
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/absurd"), "{out}");
    assert!(out.contains("1 broken"), "one task, not the daemon:\n{out}");
}
