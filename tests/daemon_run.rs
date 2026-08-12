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
jitter: 0
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

    let invocations = env.calls();
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

    assert!(env.calls().is_empty(), "nothing was due");
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

    let invocation = &env.calls()[0];
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
        "---\ndescription: Multi-paragraph prompt\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nFirst para.\n\n  indented line\n\nLast.\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls()[0].args,
        vec!["--run", "First para.\n\n  indented line\n\nLast."],
        "blank lines and indentation survive from file to Agent"
    );
}

/// Writes a Task whose prompt body is `prompt`, firing hourly.
fn hourly_task(env: &TestEnv, name: &str, prompt: &str) {
    env.write_task(
        name,
        &format!(
            "---\ndescription: {name}\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\n{prompt}\n"
        ),
    );
}

/// Runs one hourly Task through a Tick and returns what the Agent received.
async fn fire_once(env: &TestEnv) -> Vec<String> {
    let (mut daemon, clock) = daemon_at(env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    env.calls()
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
        env.calls().is_empty(),
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
    assert!(env.calls().is_empty(), "still broken, still silent");

    env.write_task(
        "hourly",
        "---\ndescription: Fixed\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );
    daemon.reload().await.unwrap();

    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        1,
        "a repaired Task recovers without restarting the Daemon"
    );
}

#[tokio::test]
async fn a_task_with_an_unknown_key_still_fires() {
    let env = TestEnv::new();
    env.write_task(
        "odd",
        "---\ndescription: Key from the future\ncron: \"@hourly\"\nagent: stub\njitter: 0\nretries: 3\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;

    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1);
}

#[tokio::test]
async fn each_tick_produces_its_own_run() {
    let env = TestEnv::new();
    env.write_task(
        "hourly",
        "---\ndescription: d\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    for hour in ["2026-08-11T01:00:00Z", "2026-08-11T02:00:00Z"] {
        clock.set(at(hour));
        daemon.tick().await.unwrap();
        // Each Run finishes before the next Tick, so neither overlaps.
        daemon.wait_for_running().await;
    }

    assert_eq!(env.calls().len(), 2);
    assert_eq!(env.run_dirs("proj/hourly").len(), 2, "one run dir per Run");
    assert_eq!(
        env.read_run("proj/hourly", 1)["scheduledFor"],
        "2026-08-11T02:00:00Z"
    );
}

// --- Jitter -------------------------------------------------------------

const HOURLY_EXACT: &str = "---\ndescription: Exactly on the tick\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nping\n";

const HOURLY_JITTERED: &str =
    "---\ndescription: Spread out\ncron: \"@hourly\"\nagent: stub\n---\n\nping\n";

#[tokio::test]
async fn a_jittered_task_waits_past_its_tick_before_firing() {
    let env = TestEnv::new();
    env.write_task("spread", HOURLY_JITTERED);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert!(
        env.calls().is_empty(),
        "the default jitter window holds the Run back from the exact tick"
    );

    // Past the widest the window can be, it has certainly fired.
    clock.set(at("2026-08-11T01:05:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1);
}

#[tokio::test]
async fn the_run_records_the_tick_it_answers_and_the_moment_it_actually_started() {
    let env = TestEnv::new();
    env.write_task("spread", HOURLY_JITTERED);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:05:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let run = env.read_run("proj/spread", 0);
    assert_eq!(
        run["scheduledFor"], "2026-08-11T01:00:00Z",
        "the Tick is the schedule's instant, unjittered"
    );
    assert_eq!(
        run["startedAt"], "2026-08-11T01:05:00Z",
        "the start is when it really began"
    );
}

#[tokio::test]
async fn jitter_zero_fires_exactly_on_the_tick() {
    let env = TestEnv::new();
    env.write_task("punctual", HOURLY_EXACT);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1, "opted out of jitter, so exact");
}

// --- Skips --------------------------------------------------------------

#[tokio::test]
async fn a_tick_arriving_mid_run_is_skipped_and_recorded_as_overlap() {
    let env = TestEnv::new();
    env.write_gated_stub_agent();
    env.write_task("slow", HOURLY_EXACT);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();

    // Still running when the next Tick comes due.
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap();

    env.open_gate();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        1,
        "a Task never runs concurrently with itself"
    );
    let skips = env.read_state()["recordedSkips"]["proj/slow"].clone();
    assert_eq!(skips[0]["reason"], "overlap");
    assert_eq!(skips[0]["scheduledFor"], "2026-08-11T02:00:00Z");
}

#[tokio::test]
async fn downtime_collapses_into_one_skip_carrying_the_window_and_the_count() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY_EXACT);
    env.write_config();

    // Runs once, then the Daemon goes away.
    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;
    drop(daemon);

    // A fresh Daemon starts hours later over the same state.
    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T05:30:00Z").await;
    clock.set(at("2026-08-11T06:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let skips = env.read_state()["recordedSkips"]["proj/hourly"].clone();
    assert_eq!(
        skips.as_array().map(Vec::len),
        Some(1),
        "one honest entry for the outage, not one per missed Tick: {skips}"
    );
    assert_eq!(skips[0]["reason"], "daemon-down");
    assert_eq!(skips[0]["from"], "2026-08-11T02:00:00Z");
    assert_eq!(skips[0]["to"], "2026-08-11T05:00:00Z");
    assert_eq!(skips[0]["count"], 4);
    assert_eq!(
        env.calls().len(),
        2,
        "no catch-up: the missed Ticks are recorded, not run"
    );
}

#[tokio::test]
async fn skip_records_are_capped_and_the_newest_survive() {
    let env = TestEnv::new();
    env.write_gated_stub_agent();
    env.write_task(
        "minutely",
        "---\ndescription: Every minute\ncron: \"* * * * *\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:00:30Z").await;
    // The first Tick starts a Run that never finishes; every later Tick
    // overlaps it.
    for minute in 1..=59 {
        clock.set(at(&format!("2026-08-11T00:{minute:02}:00Z")));
        daemon.tick().await.unwrap();
    }
    env.open_gate();
    daemon.wait_for_running().await;

    let state = env.read_state();
    let skips = state["recordedSkips"]["proj/minutely"].as_array().unwrap();
    assert_eq!(skips.len(), 50, "capped");
    assert_eq!(
        skips.last().unwrap()["scheduledFor"],
        "2026-08-11T00:59:00Z",
        "the newest Skip is kept"
    );
}

// --- Boundary -----------------------------------------------------------

#[tokio::test]
async fn a_tick_due_at_the_reload_instant_still_fires() {
    let env = TestEnv::new();
    env.write_task("punctual", HOURLY_EXACT);
    env.write_config();

    // Loading exactly as the Tick comes due must not step over it.
    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T01:00:00Z").await;
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1);
    assert_eq!(
        env.read_run("proj/punctual", 0)["scheduledFor"],
        "2026-08-11T01:00:00Z"
    );
}

#[tokio::test]
async fn a_tick_already_run_is_not_repeated_after_a_reload() {
    let env = TestEnv::new();
    env.write_task("punctual", HOURLY_EXACT);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    // A reload at the same instant sees the Tick already answered.
    daemon.reload().await.unwrap();
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1, "the Tick ran once, not twice");
}

#[tokio::test]
async fn ticks_the_scheduler_reached_late_are_recorded_not_dropped() {
    let env = TestEnv::new();
    env.write_task("hourly", HOURLY_EXACT);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    // The Daemon is alive but doesn't get a pass in for hours — a suspended
    // laptop, or a scheduler pass that ran long.
    clock.set(at("2026-08-11T05:30:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(
        env.calls().len(),
        2,
        "one Run for the Tick it reached — no catch-up storm"
    );
    let skips = env.read_state()["recordedSkips"]["proj/hourly"].clone();
    assert_eq!(skips[0]["reason"], "missed");
    assert_eq!(skips[0]["from"], "2026-08-11T03:00:00Z");
    assert_eq!(skips[0]["to"], "2026-08-11T05:00:00Z");
    assert_eq!(
        skips[0]["count"], 3,
        "every Tick is accounted for: one ran, the rest recorded — {skips}"
    );
}

#[tokio::test]
async fn reloading_inside_the_jitter_window_keeps_the_pending_tick() {
    let env = TestEnv::new();
    env.write_task("spread", HOURLY_JITTERED);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;

    // The Tick has passed but its jittered fire time has not arrived, and
    // something reloads — an edit, a rescan. The Tick must survive.
    clock.set(at("2026-08-11T01:00:01Z"));
    daemon.reload().await.unwrap();

    clock.set(at("2026-08-11T01:05:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    assert_eq!(env.calls().len(), 1, "the pending Tick still fired");
    assert_eq!(
        env.read_run("proj/spread", 0)["scheduledFor"],
        "2026-08-11T01:00:00Z"
    );
}

#[tokio::test]
async fn a_tick_answered_by_a_skip_is_not_counted_again_as_downtime() {
    let env = TestEnv::new();
    env.write_gated_stub_agent();
    env.write_task("slow", HOURLY_EXACT);
    env.write_config();

    let (mut daemon, clock) = daemon_at(&env, "2026-08-11T00:30:00Z").await;
    clock.set(at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    clock.set(at("2026-08-11T02:00:00Z"));
    daemon.tick().await.unwrap(); // overlap Skip for 02:00
    env.open_gate();
    daemon.wait_for_running().await;
    drop(daemon);

    // A fresh Daemon must not count 02:00 a second time as downtime.
    let (mut daemon, _clock) = daemon_at(&env, "2026-08-11T03:30:00Z").await;
    daemon.tick().await.unwrap();

    let state = env.read_state();
    let skips = state["recordedSkips"]["proj/slow"].as_array().unwrap();
    let downtime: Vec<_> = skips
        .iter()
        .filter(|skip| skip["reason"] == "daemon-down")
        .collect();
    assert_eq!(downtime.len(), 1, "one outage entry: {skips:?}");
    assert_eq!(
        downtime[0]["from"], "2026-08-11T03:00:00Z",
        "downtime starts after the Tick the overlap Skip already answered"
    );
    assert_eq!(downtime[0]["count"], 1);
}
