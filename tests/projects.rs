//! Several Projects on one machine: distinct identities, honest discovery,
//! and the committable `CRONTAB.md` each one gets.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::sync::Arc;
use support::{TestEnv, at};

/// Scans once through the Daemon — which is what keeps `CRONTAB.md` current.
async fn scan(env: &TestEnv) {
    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock), chrono_tz::UTC)
        .expect("daemon should build");
    daemon.reload().await.expect("reload should succeed");
}

const NIGHTLY: &str = r#"---
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: stub
---

Review all open TODO and FIXME comments.
"#;

#[test]
fn two_projects_may_hold_identically_named_tasks() {
    let env = TestEnv::new();
    let other = env.add_project("other");
    env.write_task("nightly", NIGHTLY);
    std::fs::write(other.join("nightly.cron.md"), NIGHTLY).unwrap();
    env.write_config_with_projects();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/nightly"), "{out}");
    assert!(out.contains("other/nightly"), "{out}");
    assert!(out.contains("2 tasks"), "{out}");
}

#[test]
fn adding_a_project_registers_it_and_listing_finds_its_tasks() {
    let env = TestEnv::new();
    env.write_config_with_no_projects();
    let fresh = env.add_project("fresh");
    std::fs::write(fresh.join("nightly.cron.md"), NIGHTLY).unwrap();

    let added = env.run_ok(&["add", fresh.to_str().unwrap()]);
    assert!(
        added.contains("fresh"),
        "add should name the project: {added}"
    );

    let out = env.run_ok(&["list"]);
    assert!(out.contains("fresh/nightly"), "{out}");
}

#[test]
fn adding_the_same_directory_twice_is_not_an_error() {
    let env = TestEnv::new();
    env.write_config_with_no_projects();
    let fresh = env.add_project("fresh");
    std::fs::write(fresh.join("nightly.cron.md"), NIGHTLY).unwrap();

    env.run_ok(&["add", fresh.to_str().unwrap()]);
    let again = env.run_ok(&["add", fresh.to_str().unwrap()]);

    assert!(
        again.to_lowercase().contains("already"),
        "re-adding should say so plainly, got: {again}"
    );
    assert_eq!(
        env.run_ok(&["list"]).matches("fresh/nightly").count(),
        1,
        "and the task appears once, not twice"
    );
}

#[test]
fn a_name_collision_is_refused_with_a_way_out() {
    let env = TestEnv::new();
    env.write_config_with_no_projects();
    // Two different directories that would both be called `work`.
    let one = env.add_project("a/work");
    let two = env.add_project("b/work");

    env.run_ok(&["add", one.to_str().unwrap()]);
    let output = env.run(&["add", two.to_str().unwrap()]);

    assert!(!output.status.success(), "a duplicate name must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("work"), "{stderr}");
    assert!(
        stderr.contains("--name"),
        "the error should offer the way out: {stderr}"
    );
}

#[test]
fn a_project_can_be_registered_under_a_chosen_name() {
    let env = TestEnv::new();
    env.write_config_with_no_projects();
    let one = env.add_project("a/work");
    let two = env.add_project("b/work");
    std::fs::write(two.join("nightly.cron.md"), NIGHTLY).unwrap();

    env.run_ok(&["add", one.to_str().unwrap()]);
    env.run_ok(&["add", two.to_str().unwrap(), "--name", "work-two"]);

    let out = env.run_ok(&["list"]);
    assert!(out.contains("work-two/nightly"), "{out}");
}

#[test]
fn removing_a_project_takes_its_tasks_with_it() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    assert!(env.run_ok(&["list"]).contains("proj/nightly"));

    env.run_ok(&["remove", env.project_dir().to_str().unwrap()]);

    let out = env.run_ok(&["list"]);
    assert!(!out.contains("proj/nightly"), "{out}");
    assert!(out.to_lowercase().contains("no tasks"), "{out}");
}

#[test]
fn removing_something_that_was_never_registered_says_so() {
    let env = TestEnv::new();
    env.write_config();

    let output = env.run(&["remove", "/tmp/never-registered-anywhere"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .to_lowercase()
            .contains("not registered")
    );
}

// --- Discovery rules ----------------------------------------------------

#[test]
fn ignored_files_are_not_discovered() {
    let env = TestEnv::new();
    env.write_task("real", NIGHTLY);
    std::fs::write(env.project_dir().join(".gitignore"), "scratch/\n").unwrap();
    std::fs::create_dir_all(env.project_dir().join("scratch")).unwrap();
    std::fs::write(env.project_dir().join("scratch/draft.cron.md"), NIGHTLY).unwrap();
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/real"), "{out}");
    assert!(
        !out.contains("proj/draft"),
        "a file git ignores is not a task anyone meant to schedule:\n{out}"
    );
}

#[test]
fn the_git_directory_is_never_searched() {
    let env = TestEnv::new();
    std::fs::create_dir_all(env.project_dir().join(".git/objects")).unwrap();
    std::fs::write(env.project_dir().join(".git/objects/x.cron.md"), NIGHTLY).unwrap();
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("no tasks"), "{out}");
}

#[test]
fn a_symlink_pointing_out_of_the_project_is_not_followed() {
    let env = TestEnv::new();
    let outside = env.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("smuggled.cron.md"), NIGHTLY).unwrap();
    std::os::unix::fs::symlink(&outside, env.project_dir().join("link")).unwrap();
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(
        !out.contains("smuggled"),
        "a link is not a reason to schedule someone else's file:\n{out}"
    );
}

#[test]
fn a_project_directory_that_is_missing_says_so_rather_than_looking_empty() {
    let env = TestEnv::new();
    env.write_config_with_projects();
    std::fs::remove_dir_all(env.path().join("other")).unwrap();

    let out = env.run_ok(&["list"]);

    assert!(
        out.to_lowercase().contains("unreachable") || out.to_lowercase().contains("cannot"),
        "an unreachable project must not read as an empty one:\n{out}"
    );
    assert!(out.contains("other"), "{out}");
}

// --- CRONTAB.md ---------------------------------------------------------

#[tokio::test]
async fn each_project_gets_a_crontab_listing_only_its_own_tasks() {
    let env = TestEnv::new();
    let other = env.add_project("other");
    env.write_task("nightly", NIGHTLY);
    std::fs::write(other.join("weekly.cron.md"), NIGHTLY).unwrap();
    env.write_config_with_projects();

    scan(&env).await;

    let mine = std::fs::read_to_string(env.project_dir().join("CRONTAB.md")).unwrap();
    let theirs = std::fs::read_to_string(other.join("CRONTAB.md")).unwrap();

    assert!(mine.contains("nightly"), "{mine}");
    assert!(!mine.contains("weekly"), "{mine}");
    assert!(theirs.contains("weekly"), "{theirs}");
    assert!(
        mine.contains("Nightly TODO/FIXME triage") && mine.contains("0 2 * * *"),
        "the table carries description, schedule and agent:\n{mine}"
    );
}

#[tokio::test]
async fn the_crontab_is_byte_identical_when_nothing_changed() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    scan(&env).await;
    let first = std::fs::read(env.project_dir().join("CRONTAB.md")).unwrap();
    scan(&env).await;
    let second = std::fs::read(env.project_dir().join("CRONTAB.md")).unwrap();

    assert_eq!(
        first, second,
        "a generated file at a repo root must not churn"
    );
}

#[tokio::test]
async fn the_crontab_carries_no_run_state() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();
    scan(&env).await;

    let crontab = std::fs::read_to_string(env.project_dir().join("CRONTAB.md")).unwrap();

    for volatile in ["last run", "lastRun", "next fire", "20"] {
        assert!(
            !crontab.to_lowercase().contains(&volatile.to_lowercase()),
            "{volatile:?} would make this churn on every run:\n{crontab}"
        );
    }
}

#[tokio::test]
async fn editing_a_definition_regenerates_the_crontab() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();
    scan(&env).await;

    env.write_task(
        "nightly",
        "---\ndescription: Renamed entirely\ncron: \"@weekly\"\nagent: stub\n---\n\nbody\n",
    );
    scan(&env).await;

    let crontab = std::fs::read_to_string(env.project_dir().join("CRONTAB.md")).unwrap();
    assert!(crontab.contains("Renamed entirely"), "{crontab}");
    assert!(crontab.contains("@weekly"), "{crontab}");
}

#[tokio::test]
async fn a_project_can_opt_out_of_having_a_crontab_written() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config_with_extra_project_key("crontab_md = false\n");

    scan(&env).await;

    assert!(
        !env.project_dir().join("CRONTAB.md").exists(),
        "openroutine must never insist on writing into someone's repo"
    );
}

#[tokio::test]
async fn a_symlinked_crontab_target_is_refused() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    let elsewhere = env.path().join("elsewhere.md");
    std::fs::write(&elsewhere, "not ours\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, env.project_dir().join("CRONTAB.md")).unwrap();
    env.write_config();

    scan(&env).await;

    assert_eq!(
        std::fs::read_to_string(&elsewhere).unwrap(),
        "not ours\n",
        "writing through a symlink would put our output somewhere we were never pointed"
    );
    assert!(
        std::fs::symlink_metadata(env.project_dir().join("CRONTAB.md"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is left exactly as it was found, not replaced"
    );
}

#[tokio::test]
async fn registering_a_project_reaches_a_running_daemon() {
    let env = TestEnv::new();
    env.write_config();
    let later = env.add_project("later");
    std::fs::write(later.join("nightly.cron.md"), NIGHTLY).unwrap();

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock), chrono_tz::UTC)
        .expect("daemon should build")
        .watching_config(env.config_path());
    daemon.reload().await.unwrap();
    assert_eq!(daemon.task_count(), 0);

    // Registered from another terminal while the Daemon is up.
    env.run_ok(&["add", later.to_str().unwrap()]);
    daemon.reload().await.unwrap();

    assert_eq!(
        daemon.task_count(),
        1,
        "a project registered while serving is picked up, like a task file is"
    );
}

#[tokio::test]
async fn a_config_that_stops_parsing_does_not_unschedule_everything() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock), chrono_tz::UTC)
        .expect("daemon should build")
        .watching_config(env.config_path());
    daemon.reload().await.unwrap();
    assert_eq!(daemon.task_count(), 1);

    std::fs::write(env.config_path(), "this is not toml at all [[[\n").unwrap();
    daemon.reload().await.unwrap();

    assert_eq!(
        daemon.task_count(),
        1,
        "a typo in the config must not silently stop every task on the machine"
    );
}
