//! The walking skeleton: a due Tick becomes a Run, the Agent gets the prompt,
//! and the result lands on disk. Driven at the clock seam — no sleeps.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::sync::Arc;
use support::{TestEnv, at};

const NIGHTLY: &str = r#"---
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: stub
---

Review all open TODO and FIXME comments.
"#;

/// Loads a daemon over the env's config with time parked at `now`.
///
/// Ticks are evaluated in UTC so these cases assert the same thing on every
/// machine; real zones and their DST edges belong to the scheduling-semantics
/// work, which is why the zone is injected rather than ambient.
async fn daemon_at(env: &TestEnv, now: &str) -> (Daemon, ManualClock) {
    let clock = ManualClock::new(at(now));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock.clone()), chrono_tz::UTC)
        .expect("daemon should build");
    daemon.reload().await.expect("reload should succeed");
    (daemon, clock)
}

#[tokio::test]
async fn a_due_tick_runs_the_agent_with_the_prompt_as_one_argument() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let invocations = env.invocations();
    assert_eq!(invocations.len(), 1, "exactly one Run should have happened");
    assert_eq!(
        invocations[0].args,
        vec!["--run", "Review all open TODO and FIXME comments."],
        "the prompt arrives as a single argv element"
    );
    assert_eq!(
        invocations[0].cwd,
        env.project_dir().canonicalize().unwrap()
    );
}

#[tokio::test]
async fn a_tick_that_is_not_due_yet_runs_nothing() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T01:59:59Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert!(env.invocations().is_empty(), "nothing was due");
}

#[tokio::test]
async fn the_run_is_recorded_with_its_outcome_and_timings() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let run = env.read_run("proj/todo-digest", 0);
    assert_eq!(run["taskId"], "proj/todo-digest");
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["exitCode"], 0);
    assert_eq!(run["trigger"], "schedule");
    assert_eq!(run["agent"], "stub");
    assert_eq!(run["scheduledFor"], "2026-08-11T02:00:00Z");
    assert_eq!(run["startedAt"], "2026-08-11T02:00:00Z");
    assert!(run["finishedAt"].is_string());
    assert!(
        run["runId"]
            .as_str()
            .unwrap()
            .starts_with("20260811T020000Z"),
        "run id is derived from the UTC start instant, got {}",
        run["runId"]
    );
}

#[tokio::test]
async fn the_log_carries_a_header_then_the_agents_merged_output() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let log = env.read_run_log("proj/todo-digest", 0);
    let (header, output) = log.split_once("\n---\n").expect("header then output");

    assert!(header.contains("proj/todo-digest"), "header names the task");
    assert!(
        header.contains("2026-08-11T02:00:00Z"),
        "header carries times"
    );
    assert!(header.contains("stub"), "header names the agent");
    assert!(output.contains("stub agent ran"), "stdout is captured");
    assert!(output.contains("a line on stderr"), "stderr is merged in");
}

#[tokio::test]
async fn state_records_the_task_and_its_latest_run() {
    let env = TestEnv::new();
    let task_path = env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let state = env.read_state();
    let entry = &state["scheduledTasks"][0];
    assert_eq!(entry["id"], "proj/todo-digest");
    assert_eq!(
        entry["filePath"],
        task_path.canonicalize().unwrap().display().to_string(),
        "state keys back to the task file by absolute path"
    );
    assert_eq!(entry["enabled"], true);
    assert_eq!(entry["lastScheduledFor"], "2026-08-11T02:00:00Z");
    assert_eq!(entry["lastRunAt"], "2026-08-11T02:00:00Z");
}

#[tokio::test]
async fn a_run_is_recorded_before_the_agent_finishes_and_never_blocks_the_scheduler() {
    let env = TestEnv::new();
    env.write_gated_stub_agent();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();

    // The Agent is still working, yet the scheduler already returned — a long
    // Run must never stall the Daemon or hide from anyone reading the disk.
    assert_eq!(daemon.running_count(), 1);
    assert_eq!(
        env.read_run("proj/todo-digest", 0)["status"],
        "running",
        "an in-flight Run is on disk with its status, not invisible until it ends"
    );

    env.open_gate();
    daemon.wait_for_running().await;

    assert_eq!(env.read_run("proj/todo-digest", 0)["status"], "succeeded");
    assert_eq!(daemon.running_count(), 0);
}

#[tokio::test]
async fn an_agent_without_a_prompt_placeholder_receives_it_on_stdin() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config_with_agent(&format!("{} --run", env.stub_path().display()));

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let invocation = &env.invocations()[0];
    assert_eq!(invocation.args, vec!["--run"], "no prompt in argv");
    assert_eq!(
        invocation.stdin.trim(),
        "Review all open TODO and FIXME comments.",
        "the prompt is piped instead"
    );
}

#[tokio::test]
async fn a_failing_agent_is_recorded_as_failed_with_its_exit_code() {
    let env = TestEnv::new();
    env.write_stub_agent(3);
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let run = env.read_run("proj/todo-digest", 0);
    assert_eq!(run["status"], "failed");
    assert_eq!(run["exitCode"], 3);
    assert!(
        env.read_run_log("proj/todo-digest", 0)
            .contains("stub agent ran"),
        "a failed Run still keeps its log"
    );
}

#[tokio::test]
async fn the_prompt_reaches_the_agent_with_its_shape_intact() {
    let env = TestEnv::new();
    env.write_task(
        "shaped",
        "---\ndescription: Multi-paragraph prompt\ncron: \"@hourly\"\nagent: stub\n---\n\nFirst para.\n\n  indented line\n\nLast.\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.invocations()[0].args,
        vec!["--run", "First para.\n\n  indented line\n\nLast."],
        "blank lines and indentation survive from file to Agent"
    );
}

/// Writes a Task whose prompt body is `prompt`, firing hourly.
fn hourly_task(env: &TestEnv, name: &str, prompt: &str) {
    env.write_task(
        name,
        &format!("---\ndescription: {name}\ncron: \"@hourly\"\nagent: stub\n---\n\n{prompt}\n"),
    );
}

/// Runs one hourly Task through a Tick and returns what the Agent received.
async fn fire_once(env: &TestEnv) -> Vec<String> {
    let (mut daemon, clock) = daemon_at(env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    env.invocations()
        .first()
        .expect("the Agent should have run")
        .args
        .clone()
}

#[tokio::test]
async fn a_prompt_full_of_shell_syntax_reaches_the_agent_inert() {
    let env = TestEnv::new();
    let marker = env.path().join("pwned");
    let nasty = format!(
        "; touch {} && echo $(whoami) `id` | tee {}",
        marker.display(),
        marker.display()
    );
    hourly_task(&env, "nasty", &nasty);
    env.write_config();

    let args = fire_once(&env).await;

    assert_eq!(
        args,
        vec!["--run".to_string(), nasty.clone()],
        "the whole prompt is one argument"
    );
    assert!(
        !marker.exists(),
        "no shell ever interpreted the prompt, so nothing it asked for happened"
    );
}

#[tokio::test]
async fn a_prompt_that_looks_like_a_flag_is_still_just_the_prompt() {
    let env = TestEnv::new();
    hourly_task(&env, "flaggy", "--dangerously-skip-permissions");
    env.write_config();

    let args = fire_once(&env).await;

    assert_eq!(args, vec!["--run", "--dangerously-skip-permissions"]);
}

#[tokio::test]
async fn a_placeholder_inside_a_flag_stays_one_argument() {
    let env = TestEnv::new();
    hourly_task(&env, "embedded", "hello");
    env.write_config_with_agent(&format!(
        "{} --prompt={{prompt}}",
        env.stub_path().display()
    ));

    let args = fire_once(&env).await;

    assert_eq!(args, vec!["--prompt=hello"]);
}

#[tokio::test]
async fn a_quoted_program_path_with_spaces_is_one_token() {
    let env = TestEnv::new();
    hourly_task(&env, "spaced", "hello");
    let spaced = env.write_stub_agent_named("stub agent.sh");
    env.write_config_with_agent(&format!("\"{}\" --run {{prompt}}", spaced.display()));

    let args = fire_once(&env).await;

    assert_eq!(args, vec!["--run", "hello"]);
}

#[tokio::test]
async fn a_broken_task_is_counted_but_never_fires() {
    let env = TestEnv::new();
    env.write_task(
        "typo",
        "---\ndescription: Typo'd schedule\ncron: \"0 25 * * *\"\nagent: stub\n---\n\nbody\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T01:59:00Z").await;

    // Every minute of the day it might plausibly have fired.
    for minute in 0..60 {
        clock.set(at(&format!("2026-08-11T02:{minute:02}:00Z")));
        daemon.tick().await.unwrap();
    }
    daemon.wait_for_running().await;

    assert!(
        env.invocations().is_empty(),
        "a Broken Task must never reach the Agent"
    );
}

#[tokio::test]
async fn fixing_a_broken_task_lets_it_fire_on_the_next_scan() {
    let env = TestEnv::new();
    env.write_task(
        "hourly",
        "---\ndescription: Broken for now\ncron: \"every hour please\"\nagent: stub\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    assert!(env.invocations().is_empty(), "still broken, still silent");

    env.write_task(
        "hourly",
        "---\ndescription: Fixed\ncron: \"@hourly\"\nagent: stub\n---\n\nping\n",
    );
    daemon.reload().await.unwrap();

    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.invocations().len(),
        1,
        "a repaired Task recovers without restarting the Daemon"
    );
}

#[tokio::test]
async fn a_task_with_an_unknown_key_still_fires() {
    let env = TestEnv::new();
    env.write_task(
        "odd",
        "---\ndescription: Key from the future\ncron: \"@hourly\"\nagent: stub\nretries: 3\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.invocations().len(), 1);
}

#[tokio::test]
async fn each_tick_produces_its_own_run() {
    let env = TestEnv::new();
    env.write_task(
        "hourly",
        "---\ndescription: d\ncron: \"@hourly\"\nagent: stub\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    for hour in ["2026-08-11T01:00:00Z", "2026-08-11T02:00:00Z"] {
        clock.set(at(hour));
        daemon.tick().await.unwrap();
    }
    daemon.wait_for_running().await;

    assert_eq!(env.invocations().len(), 2);
    assert_eq!(env.run_dirs("proj/hourly").len(), 2, "one run dir per Run");
    assert_eq!(
        env.read_run("proj/hourly", 1)["scheduledFor"],
        "2026-08-11T02:00:00Z"
    );
}
