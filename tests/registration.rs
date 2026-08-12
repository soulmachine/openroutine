//! Registering Task files: what `add` accepts, what `remove` undoes, and how
//! identity behaves when two files want the same name.

mod support;

use openroutine::clock::ManualClock;
use openroutine::daemon::Daemon;
use std::sync::Arc;
use support::{TestEnv, at};

const NIGHTLY: &str = r#"---
name: nightly
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: stub
---

Review all open TODO and FIXME comments.
"#;

/// A Task file written but not registered — what `add` is handed.
fn unregistered(env: &TestEnv, file: &str, contents: &str) -> std::path::PathBuf {
    let path = env.path().join(file);
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn adding_a_file_registers_it_and_listing_finds_it() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let file = unregistered(&env, "nightly.md", NIGHTLY);

    let added = env.run_ok(&["add", file.to_str().unwrap()]);
    assert!(added.contains("nightly"), "{added}");

    let listed = env.run_ok(&["list"]);
    assert!(listed.contains("nightly"), "{listed}");
    assert!(listed.contains("1 task"), "{listed}");
}

#[test]
fn a_file_is_registered_wherever_it_lives_with_no_naming_rule() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let elsewhere = env.add_dir("some/deep/place");
    // No `.cron.md` suffix, and nowhere near a registered directory: the
    // registration is the whole rule.
    let file = elsewhere.join("anything.md");
    std::fs::write(&file, NIGHTLY).unwrap();

    env.run_ok(&["add", file.to_str().unwrap()]);

    assert!(env.run_ok(&["list"]).contains("nightly"));
}

#[test]
fn adding_the_same_file_twice_changes_nothing() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let file = unregistered(&env, "nightly.md", NIGHTLY);

    env.run_ok(&["add", file.to_str().unwrap()]);
    let again = env.run_ok(&["add", file.to_str().unwrap()]);

    assert!(again.contains("already registered"), "{again}");
    assert!(
        env.run_ok(&["list"]).contains("1 task"),
        "a second add must not register it twice"
    );
}

#[test]
fn adding_a_file_whose_name_is_taken_is_refused() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let first = unregistered(&env, "nightly.md", NIGHTLY);
    let second = unregistered(&env, "copy.md", NIGHTLY);

    env.run_ok(&["add", first.to_str().unwrap()]);
    let output = env.run(&["add", second.to_str().unwrap()]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nightly"), "{stderr}");
    assert!(
        stderr.contains(&first.display().to_string()),
        "the error should name the file already holding it:\n{stderr}"
    );
}

#[test]
fn adding_a_file_that_does_not_parse_is_refused_up_front() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let file = unregistered(
        &env,
        "broken.md",
        "---\nname: broken\ndescription: Broken\ncron: \"not a cron\"\n---\n\nDo something.\n",
    );

    let output = env.run(&["add", file.to_str().unwrap()]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cron"),
        "the refusal should say what is wrong with the file"
    );
    assert!(
        !std::fs::read_to_string(env.config_path())
            .unwrap()
            .contains("broken.md"),
        "a refused file must not end up in the config"
    );
}

#[test]
fn adding_a_file_with_no_name_is_refused() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let file = unregistered(
        &env,
        "anonymous.md",
        "---\ndescription: No name here\ncron: \"0 2 * * *\"\nagent: stub\n---\n\nDo something.\n",
    );

    let output = env.run(&["add", file.to_str().unwrap()]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("name"),
        "identity is stated in the file, so its absence is the error"
    );
}

#[test]
fn adding_a_file_with_an_empty_body_is_refused() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let file = unregistered(
        &env,
        "silent.md",
        "---\nname: silent\ndescription: Nothing to say\ncron: \"0 2 * * *\"\nagent: stub\n---\n\n   \n",
    );

    let output = env.run(&["add", file.to_str().unwrap()]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("prompt"),
        "the body is the prompt, so an empty one is not a task"
    );
}

#[test]
fn adding_something_that_is_not_there_says_so() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();

    let output = env.run(&["add", env.path().join("ghost.md").to_str().unwrap()]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ghost.md"));
}

#[test]
fn removing_by_name_unregisters_but_leaves_the_file() {
    let env = TestEnv::new();
    let file = env.write_task("nightly", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["remove", "nightly"]);

    assert!(out.contains("Unregistered"), "{out}");
    assert!(
        file.exists(),
        "the file is the user's; removing unregisters"
    );
    assert!(env.run_ok(&["list"]).contains("No tasks"));
}

#[test]
fn removing_by_path_works_when_the_file_is_too_broken_to_name_itself() {
    let env = TestEnv::new();
    let file = env.write_task("wreck", "this file has no frontmatter at all\n");
    env.write_config();

    let out = env.run_ok(&["remove", file.to_str().unwrap()]);

    assert!(out.contains("Unregistered"), "{out}");
    assert!(env.run_ok(&["list"]).contains("No tasks"));
}

#[test]
fn removing_with_delete_takes_the_file_too() {
    let env = TestEnv::new();
    let file = env.write_task("nightly", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["remove", "nightly", "--delete"]);

    assert!(out.contains("Deleted"), "{out}");
    assert!(!file.exists(), "--delete is the way to ask for that");
}

#[test]
fn removing_something_that_was_never_registered_says_so() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();

    let output = env.run(&["remove", "ghost"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ghost"));
}

#[test]
fn a_registered_file_that_disappears_stays_registered_and_broken() {
    let env = TestEnv::new();
    let file = env.write_task("nightly", NIGHTLY);
    env.write_config();
    std::fs::remove_file(&file).unwrap();

    let out = env.run_ok(&["list"]);

    assert!(
        out.contains("broken") && out.contains("nightly"),
        "a missing file is broken, not quietly forgotten:\n{out}"
    );
    assert!(
        std::fs::read_to_string(env.config_path())
            .unwrap()
            .contains("nightly.md"),
        "only the user unregisters a task"
    );
}

#[test]
fn a_config_from_before_file_registration_explains_itself() {
    let env = TestEnv::new();
    env.write_legacy_projects_config();

    let output = env.run(&["list"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("projects"), "{stderr}");
    assert!(
        stderr.contains("openroutine add"),
        "the error should say what to do instead:\n{stderr}"
    );
}

#[test]
fn a_relative_path_in_the_config_is_refused() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let config = std::fs::read_to_string(env.config_path()).unwrap();
    std::fs::write(
        env.config_path(),
        config.replace("tasks = []", "tasks = [\"nightly.md\"]"),
    )
    .unwrap();

    let output = env.run(&["list"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("absolute"),
        "the daemon's working directory is not the user's"
    );
}

#[tokio::test]
async fn registering_reaches_a_running_daemon_when_it_reloads() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let file = unregistered(&env, "nightly.md", NIGHTLY);

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock), chrono_tz::UTC)
        .expect("daemon should build")
        .watching_config(env.config_path())
        .watching_config(env.config_path());
    daemon.reload().await.unwrap();
    assert_eq!(daemon.task_count(), 0);

    // Registered from another terminal while the Daemon is up.
    env.run_ok(&["add", file.to_str().unwrap()]);
    daemon.reload().await.unwrap();

    assert_eq!(
        daemon.task_count(),
        1,
        "a task registered while serving is picked up at the next reload"
    );
}

#[tokio::test]
async fn the_daemon_refuses_a_second_claim_on_a_name_and_keeps_the_winner_s_state() {
    let env = TestEnv::new();
    let winner = env.write_task("nightly", NIGHTLY);
    // Written straight into the config, past `add`, which refuses a taken
    // name outright — this is the case where two files reach the Daemon
    // anyway: a name edited into a file that was already registered. It
    // sorts after the winner, so an unguarded upsert would be the one to
    // land in the state file.
    let loser = env.write_unregistered_task("zz-copy", NIGHTLY);
    env.register(&loser);
    env.write_config();
    // The registry resolves every path it reads, so compare against resolved
    // ones — a temp dir on macOS is reached through a symlink.
    let resolved = |path: &std::path::Path| {
        std::fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string()
    };
    let (winner, loser) = (resolved(&winner), resolved(&loser));

    let clock = ManualClock::new(at("2026-08-11T00:30:00Z"));
    let mut daemon = Daemon::with_zone(env.load_config(), Arc::new(clock), chrono_tz::UTC)
        .expect("daemon should build")
        .watching_config(env.config_path());
    let report = daemon.reload().await.unwrap();

    // The Daemon decides this itself: `add`'s check is a courtesy, not the
    // enforcement — a name can be edited into a file long after it is added.
    let second = report
        .iter()
        .find(|entry| entry.path == loser)
        .expect("the second file should still be reported");
    assert_eq!(second.status, openroutine::daemon::ReloadStatus::Broken);
    let error = second.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("already taken") && error.contains(&winner),
        "the loser should say whose name it wanted:\n{error}"
    );
    assert_eq!(
        daemon.task_count(),
        1,
        "one name, one schedule — the winner's"
    );

    let state = env.read_state();
    let rows = state["scheduledTasks"].as_array().expect("scheduled tasks");
    assert_eq!(
        rows.len(),
        1,
        "one row per name, not per claimant:\n{state}"
    );
    assert_eq!(rows[0]["id"], "nightly");
    assert_eq!(
        rows[0]["filePath"], winner,
        "the run history under a name belongs to the file that claimed it \
         first; the loser must not file the winner's past under its own path"
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
        .watching_config(env.config_path())
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
    assert!(
        daemon.config_error().is_some(),
        "and the daemon should be able to say why it kept the old one"
    );
}

// --- names --------------------------------------------------------------

/// A task file stating an exact `name`, written but not registered.
fn named(env: &TestEnv, index: usize, name: &str) -> std::path::PathBuf {
    unregistered(
        env,
        &format!("probe{index}.md"),
        &format!(
            "---\nname: {name}\ndescription: probe\ncron: \"0 2 * * *\"\nagent: stub\n---\n\nping\n"
        ),
    )
}

#[test]
fn a_name_is_converted_to_an_id_rather_than_being_refused() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();

    // Written name -> the id everything else keys by. The conversion is the
    // one Claude Desktop applies to a name typed in its UI.
    for (index, (name, id)) in [
        ("nightly-triage", "nightly-triage"), // already an id: unchanged
        ("X timeline domain hunter", "x-timeline-domain-hunter"),
        ("Nightly TODO/FIXME triage", "nightly-todofixme-triage"),
        ("C++ build check", "c-build-check"),
        ("  spaced  out  ", "spaced-out"),
        ("2FA check", "2fa-check"),
    ]
    .iter()
    .enumerate()
    {
        let file = named(&env, index, name);
        let output = env.run(&["add", file.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "{name:?} should register: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(id),
            "and `add` should say the id it derived, {id:?}, for {name:?}"
        );
    }

    // The ids, not the names, are what the rest of the tool answers to.
    let listed = env.run_ok(&["list"]);
    for id in [
        "x-timeline-domain-hunter",
        "nightly-todofixme-triage",
        "c-build-check",
    ] {
        assert!(listed.contains(id), "{id} should be listed:\n{listed}");
    }
}

#[test]
fn a_name_with_no_id_in_it_is_refused() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();

    let long = "x".repeat(51);
    for (index, (name, why)) in [
        ("..", "nothing but punctuation leaves no id at all"),
        ("!!!", "same"),
        (
            "a",
            "a one-character id is a typo more often than an intention",
        ),
        (long.as_str(), "an id longer than fifty characters"),
    ]
    .iter()
    .enumerate()
    {
        let file = named(&env, 100 + index, name);
        let output = env.run(&["add", file.to_str().unwrap()]);
        assert!(
            !output.status.success(),
            "{name:?} should be refused — {why}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("name"),
            "and the error should say which field is wrong, for {name:?}"
        );
    }

    assert!(
        env.run_ok(&["list"]).contains("No tasks"),
        "nothing refused should have reached the config"
    );
}

#[test]
fn two_names_that_derive_the_same_id_cannot_both_register() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    // Different prose, one id: `nightly-triage` both times.
    let first = named(&env, 300, "Nightly triage");
    let second = named(&env, 301, "NIGHTLY  TRIAGE!");

    env.run_ok(&["add", first.to_str().unwrap()]);
    let output = env.run(&["add", second.to_str().unwrap()]);

    assert!(!output.status.success(), "the second must not register");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nightly-triage"), "{stderr}");
    assert!(
        stderr.contains("already registered"),
        "and it should say why: {stderr}"
    );
}

#[test]
fn a_task_can_be_addressed_by_its_written_name_as_well_as_its_id() {
    let env = TestEnv::new();
    let file = named(&env, 400, "X timeline domain hunter");
    env.register(&file);
    env.write_config();

    let by_id = env.run_ok(&["run", "x-timeline-domain-hunter", "--dry-run"]);
    let by_name = env.run_ok(&["run", "X timeline domain hunter", "--dry-run"]);

    assert!(by_id.contains("x-timeline-domain-hunter"), "{by_id}");
    assert_eq!(
        by_id, by_name,
        "typing the name should reach the same task as typing the id"
    );
}

#[test]
fn two_names_differing_only_in_case_can_no_longer_both_exist() {
    let env = TestEnv::new();
    env.write_config_with_no_tasks();
    let lower = named(&env, 200, "nightly");
    let upper = named(&env, 201, "Nightly");

    env.run_ok(&["add", lower.to_str().unwrap()]);
    let output = env.run(&["add", upper.to_str().unwrap()]);

    // On a case-insensitive filesystem these two would have shared one
    // `runs/` directory, interleaving their histories in silence.
    assert!(!output.status.success(), "the second must not register");
}

#[test]
fn a_file_too_broken_to_name_itself_still_gets_a_usable_id() {
    let env = TestEnv::new();
    let file = unregistered(&env, "My Weird File.md", "no frontmatter here at all\n");
    env.register(&file);
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("broken"), "{out}");
    assert!(
        out.contains("my-weird-file"),
        "the salvaged id follows the same rule as a stated name:\n{out}"
    );
}
